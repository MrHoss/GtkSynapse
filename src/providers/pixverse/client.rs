//! PixVerse provider — AI video generation (text-to-video, image-to-video).
//!
//! API: <https://app-api.pixverse.ai>
//! Video generation is asynchronous; this provider uses polling to track progress.

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{multipart, Client};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::sleep;

use super::models::*;
use crate::core::models::{
    AttachmentKind, ChatRequest, ChatResponse, CreditBalance, GeneratedImage, ImageGenOptions,
    ModelInfo, StreamChunk, UploadedFile, VideoProgress, VideoRequest, VideoStatus,
};
use crate::core::provider::{AiProvider, ProviderError, TextStream, VideoStream};
use crate::not_supported;
use crate::providers::capabilities::Capabilities;

const PIXVERSE_BASE: &str = "https://app-api.pixverse.ai/openapi/v2";
/// Polling interval during video generation.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Maximum polling attempts (5s × 120 = 10 minutes).
const MAX_POLLS: u32 = 120;

// ─── Provider ────────────────────────────────────────────────

/// Curated list of PixVerse models (used by `list_models` and as a
/// fallback by the UI when the live list is unavailable).
pub fn curated_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "text-to-video".to_string(),
            name: "Text → Video".to_string(),
            provider_id: "pixverse".to_string(),
            description: Some("Generate video from text description".to_string()),
            context_length: None,
            supports_vision: false,
            supports_streaming: false,
        },
        ModelInfo {
            id: "image-to-video".to_string(),
            name: "Image → Video".to_string(),
            provider_id: "pixverse".to_string(),
            description: Some("Animate an image into a video".to_string()),
            context_length: None,
            supports_vision: true,
            supports_streaming: false,
        },
    ]
}

pub struct PixVerseProvider {
    client: Client,
    api_key: Arc<Mutex<String>>,
}

impl PixVerseProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            api_key: Arc::new(Mutex::new(api_key.into())),
        }
    }

    /// Update the key in memory so it takes effect immediately.
    pub fn set_api_key(&self, key: &str) {
        *self.api_key.lock().unwrap() = key.trim().to_string();
    }

    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let api_key = self.api_key.lock().unwrap().clone();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "API-KEY",
            reqwest::header::HeaderValue::from_str(&api_key).unwrap(),
        );
        headers.insert(
            "Ai-trace-id",
            reqwest::header::HeaderValue::from_str(&uuid::Uuid::new_v4().to_string()).unwrap(),
        );
        headers
    }

    /// Upload an image to PixVerse and return its `img_id` + URL.
    async fn upload_image(&self, path: &Path) -> Result<PixVerseUploadResponse> {
        let url = format!("{}/image/upload", PIXVERSE_BASE);
        let file_bytes = std::fs::read(path)?;
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image.jpg")
            .to_string();
        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();

        let part = multipart::Part::bytes(file_bytes)
            .file_name(file_name)
            .mime_str(&mime)?;
        let form = multipart::Form::new().part("image", part);

        let resp = self.client
            .post(&url)
            .headers(self.auth_headers())
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::UploadFailed(format!("{}: {}", status, text)).into());
        }

        let api_resp: PixVerseApiResponse<PixVerseUploadResponse> = resp.json().await?;

        if api_resp.err_code != 0 {
            return Err(ProviderError::UploadFailed(
                api_resp.err_msg.unwrap_or("Upload failed".to_string())
            ).into());
        }

        api_resp.resp.context("No upload response")
    }

    /// Create a text-to-video task and return the video_id.
    async fn create_text_to_video(&self, request: &VideoRequest) -> Result<i64> {
        let url = format!("{}/video/text/generate", PIXVERSE_BASE);
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| PIXVERSE_DEFAULT_MODEL.to_string());
        let body = PixVerseTextToVideoRequest {
            prompt: request.prompt.clone(),
            model: model.clone(),
            negative_prompt: None,
            duration: clamp_duration(&model, request.duration_seconds.unwrap_or(5)),
            quality: request.quality.clone().unwrap_or_else(|| "720p".to_string()),
            aspect_ratio: request.aspect_ratio.clone().unwrap_or_else(|| "16:9".to_string()),
            motion_mode: None,
            seed: None,
        };

        let resp = self.client
            .post(&url)
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await?;

        let api_resp: PixVerseApiResponse<PixVerseCreateTaskResp> = resp.json().await?;

        if api_resp.err_code != 0 {
            return Err(ProviderError::VideoFailed(
                api_resp.err_msg.unwrap_or("Failed to create video task".to_string())
            ).into());
        }

        Ok(api_resp.resp.context("No task response")?.video_id)
    }

    /// Create an image-to-video task and return the video_id.
    async fn create_image_to_video(&self, request: &VideoRequest, img_id: i64) -> Result<i64> {
        let url = format!("{}/video/img/generate", PIXVERSE_BASE);
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| PIXVERSE_DEFAULT_MODEL.to_string());
        // NOTE: the image-to-video endpoint does NOT accept `aspect_ratio`;
        // sending it makes the API reject the request with "invalid param".
        let body = PixVerseImageToVideoRequest {
            prompt: request.prompt.clone(),
            model: model.clone(),
            img_id,
            negative_prompt: None,
            duration: clamp_duration(&model, request.duration_seconds.unwrap_or(5)),
            quality: request.quality.clone().unwrap_or_else(|| "720p".to_string()),
            motion_mode: None,
        };

        let resp = self.client
            .post(&url)
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await?;

        let api_resp: PixVerseApiResponse<PixVerseCreateTaskResp> = resp.json().await?;

        if api_resp.err_code != 0 {
            return Err(ProviderError::VideoFailed(
                api_resp.err_msg.unwrap_or("Failed to create video task".to_string())
            ).into());
        }

        Ok(api_resp.resp.context("No task response")?.video_id)
    }

    /// Poll the status of a video generation task.
    async fn poll_status(&self, video_id: i64) -> Result<PixVerseVideoStatus> {
        let url = format!("{}/video/result/{}", PIXVERSE_BASE, video_id);
        let resp = self.client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await?;

        let api_resp: PixVerseApiResponse<PixVerseVideoStatus> = resp.json().await?;

        if api_resp.err_code != 0 {
            return Err(ProviderError::VideoFailed(
                api_resp.err_msg.unwrap_or("Status check failed".to_string())
            ).into());
        }

        api_resp.resp.context("No status response")
    }

    /// Fetch the account's current credit balance.
    pub async fn get_balance(&self) -> Result<PixVerseBalance> {
        let url = format!("{}/account/balance", PIXVERSE_BASE);
        let resp = self.client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            bail!("Balance request failed ({}): {}", status, text);
        }

        let api_resp: PixVerseApiResponse<PixVerseBalance> = resp.json().await?;

        if api_resp.err_code != 0 {
            bail!(api_resp.err_msg.unwrap_or("Failed to fetch credit balance".to_string()));
        }

        api_resp.resp.context("No balance response")
    }
}

#[async_trait]
impl AiProvider for PixVerseProvider {
    fn id(&self) -> &str { "pixverse" }
    fn name(&self) -> &str { "PixVerse" }

    fn capabilities(&self) -> Capabilities {
        Capabilities::VIDEO_GENERATION | Capabilities::FILE_UPLOAD
    }

    fn supports_streaming(&self) -> bool { false }
    fn supports_image_generation(&self) -> bool { false }
    fn supports_video_generation(&self) -> bool { true }
    fn supports_multimodal_input(&self) -> bool { false }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(curated_models())
    }

    async fn send_message(&self, _request: ChatRequest) -> Result<ChatResponse> {
        not_supported!("PixVerse", "chat")
    }

    async fn send_message_stream(&self, _request: ChatRequest) -> Result<TextStream> {
        not_supported!("PixVerse", "streaming chat")
    }

    async fn generate_image(&self, _prompt: &str, _options: ImageGenOptions) -> Result<Vec<GeneratedImage>> {
        not_supported!("PixVerse", "image generation")
    }

    async fn generate_video(&self, request: VideoRequest) -> Result<VideoStream> {
        // Upload image if provided
        let upload = if let Some(ref image_path) = request.source_image_path {
            Some(self.upload_image(image_path).await?)
        } else {
            None
        };

        // Create the video task
        let video_id = if let Some(upload) = &upload {
            self.create_image_to_video(&request, upload.img_id).await?
        } else {
            self.create_text_to_video(&request).await?
        };

        let task_id = video_id.to_string();
        let client = self.client.clone();
        let api_key = self.api_key.clone();

        let stream = async_stream::try_stream! {
            // Emit initial queued event
            yield VideoProgress {
                task_id: task_id.clone(),
                status: VideoStatus::Queued,
                percent: 5,
                video_url: None,
                message: Some("Video queued for generation".to_string()),
            };

            for attempt in 0..MAX_POLLS {
                sleep(POLL_INTERVAL).await;

                // Poll status
                let status_url = format!("{}/video/result/{}", PIXVERSE_BASE, video_id);
                let current_key = api_key.lock().unwrap().clone();
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert(
                    "API-KEY",
                    reqwest::header::HeaderValue::from_str(&current_key).unwrap(),
                );
                headers.insert(
                    "Ai-trace-id",
                    reqwest::header::HeaderValue::from_str(&uuid::Uuid::new_v4().to_string()).unwrap(),
                );

                let resp = client.get(&status_url).headers(headers).send().await
                    .map_err(|e| anyhow::anyhow!(e))?;

                let api_resp: PixVerseApiResponse<PixVerseVideoStatus> = resp.json().await
                    .map_err(|e| anyhow::anyhow!(e))?;

                if api_resp.err_code != 0 {
                    let msg = api_resp.err_msg.unwrap_or("Unknown error".to_string());
                    yield VideoProgress {
                        task_id: task_id.clone(),
                        status: VideoStatus::Failed,
                        percent: 0,
                        video_url: None,
                        message: Some(msg.clone()),
                    };
                    return;
                }

                if let Some(status) = api_resp.resp {
                    if status.is_done() {
                        yield VideoProgress {
                            task_id: task_id.clone(),
                            status: VideoStatus::Completed,
                            percent: 100,
                            video_url: status.url.clone(),
                            message: Some("Video generation complete!".to_string()),
                        };
                        return;
                    } else if status.is_failed() {
                        yield VideoProgress {
                            task_id: task_id.clone(),
                            status: VideoStatus::Failed,
                            percent: 0,
                            video_url: None,
                            message: Some(status.status_label().to_string()),
                        };
                        return;
                    } else {
                        let percent = status.progress_percent();
                        let label = status.status_label();
                        yield VideoProgress {
                            task_id: task_id.clone(),
                            status: VideoStatus::Processing,
                            percent,
                            video_url: None,
                            message: Some(format!("{} ({}%)", label, percent)),
                        };
                    }
                }
            }

            // Timeout
            yield VideoProgress {
                task_id: task_id.clone(),
                status: VideoStatus::Failed,
                percent: 0,
                video_url: None,
                message: Some("Video generation timed out after 10 minutes".to_string()),
            };
        };

        Ok(Box::pin(stream))
    }

    async fn upload_file(&self, path: &Path) -> Result<UploadedFile> {
        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();

        // For PixVerse, we only support image uploads
        if !mime.starts_with("image/") {
            bail!("PixVerse only supports image uploads");
        }

        let upload = self.upload_image(path).await?;

        Ok(UploadedFile {
            file_id: upload.img_id.to_string(),
            uri: upload.img_url,
            mime_type: mime,
            display_name: file_name,
        })
    }

    async fn health_check(&self) -> Result<()> {
        // A missing key must fail loudly here — otherwise the API itself
        // reports "apiKey is empty" only when a real request is made.
        if self.api_key.lock().unwrap().trim().is_empty() {
            return Err(ProviderError::NotConfigured.into());
        }

        // A lightweight authenticated call that surfaces both bad keys (401)
        // and API-level errors (err_code != 0).
        let url = format!("{}/account/balance", PIXVERSE_BASE);
        let resp = self.client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await?;

        if resp.status() == 401 {
            return Err(ProviderError::AuthError("Invalid PixVerse API key".to_string()).into());
        }
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError { status, message: text }.into());
        }

        let api_resp: PixVerseApiResponse<PixVerseBalance> = resp.json().await?;
        if api_resp.err_code != 0 {
            return Err(anyhow::anyhow!(
                api_resp.err_msg.unwrap_or_else(|| "Unknown error".to_string())
            ));
        }
        Ok(())
    }

    async fn account_balance(&self) -> Option<Result<CreditBalance>> {
        match self.get_balance().await {
            Ok(b) => Some(Ok(CreditBalance {
                provider_id: "pixverse".to_string(),
                monthly_credits: Some(b.credit_monthly),
                package_credits: Some(b.credit_package),
            })),
            Err(e) => Some(Err(e)),
        }
    }

    fn set_api_key(&self, key: &str) {
        PixVerseProvider::set_api_key(self, key);
    }
}
