//! Google Gemini provider — chat, multimodal input, image generation, streaming.

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

use crate::core::attachment;
use crate::core::models::{
    AttachmentKind, ChatRequest, ChatResponse, GeneratedImage, ImageGenOptions,
    Message, MessageMetadata, MessageRole, ModelInfo, StreamChunk, UploadedFile,
    VideoProgress, VideoRequest,
};
use crate::core::provider::{AiProvider, ProviderError, TextStream, VideoStream};
use crate::not_supported;
use crate::providers::capabilities::Capabilities;

const GEMINI_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

// ─── Gemini wire types ────────────────────────────────────────

#[derive(Debug, Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
enum GeminiPart {
    Text { text: String },
    InlineData { inline_data: GeminiInlineData },
    FileData { file_data: GeminiFileData },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GeminiInlineData {
    mime_type: String,
    data: String, // base64
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GeminiFileData {
    mime_type: String,
    file_uri: String,
}

#[derive(Debug, Serialize)]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiUsage {
    #[serde(default)]
    prompt_token_count: Option<u32>,
    #[serde(default)]
    candidates_token_count: Option<u32>,
    #[serde(default)]
    total_token_count: Option<u32>,
}

// Image generation
#[derive(Debug, Deserialize)]
struct ImageGenResponse {
    predictions: Vec<ImagePrediction>,
}

#[derive(Debug, Deserialize)]
struct ImagePrediction {
    #[serde(rename = "bytesBase64Encoded")]
    bytes_base64_encoded: Option<String>,
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
}

// ─── Provider ────────────────────────────────────────────────

pub struct GeminiProvider {
    client: Client,
    api_key: String,
}

impl GeminiProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            api_key: api_key.into(),
        }
    }

    fn chat_url(&self, model: &str, stream: bool) -> String {
        let method = if stream { "streamGenerateContent" } else { "generateContent" };
        format!("{}/models/{}:{}?key={}", GEMINI_BASE, model, method, self.api_key)
    }

    fn messages_to_gemini(messages: &[Message]) -> (Vec<GeminiContent>, Option<GeminiContent>) {
        let mut contents = Vec::new();
        let mut system_instruction = None;

        for msg in messages {
            if msg.role == MessageRole::System {
                system_instruction = Some(GeminiContent {
                    role: "user".to_string(),
                    parts: vec![GeminiPart::Text { text: msg.content.clone() }],
                });
                continue;
            }

            let role = match msg.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "model",
                MessageRole::System => "user",
            };

            let mut parts = vec![GeminiPart::Text { text: msg.content.clone() }];

            for att in &msg.attachments {
                if matches!(att.kind, AttachmentKind::Image) {
                    if let Some(uri) = &att.remote_url {
                        parts.push(GeminiPart::FileData {
                            file_data: GeminiFileData {
                                mime_type: att.mime_type.clone(),
                                file_uri: uri.clone(),
                            },
                        });
                    } else if let Ok(b64) = attachment::file_to_base64(&att.file_path) {
                        parts.push(GeminiPart::InlineData {
                            inline_data: GeminiInlineData {
                                mime_type: att.mime_type.clone(),
                                data: b64,
                            },
                        });
                    }
                }
            }

            contents.push(GeminiContent {
                role: role.to_string(),
                parts,
            });
        }

        (contents, system_instruction)
    }

    fn extract_text(response: &GeminiResponse) -> String {
        response
            .candidates
            .first()
            .and_then(|c| {
                c.content.parts.iter().find_map(|p| {
                    if let GeminiPart::Text { text } = p {
                        Some(text.clone())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default()
    }
}

/// Curated list of Gemini models (used by `list_models` and as a fallback
/// by the UI when the live list is unavailable).
pub fn curated_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "gemini-2.0-flash".to_string(),
            name: "Gemini 2.0 Flash".to_string(),
            provider_id: "gemini".to_string(),
            description: Some("Fast and efficient, great for most tasks".to_string()),
            context_length: Some(1_000_000),
            supports_vision: true,
            supports_streaming: true,
        },
        ModelInfo {
            id: "gemini-2.5-pro".to_string(),
            name: "Gemini 2.5 Pro".to_string(),
            provider_id: "gemini".to_string(),
            description: Some("Most capable Gemini model for complex reasoning".to_string()),
            context_length: Some(2_000_000),
            supports_vision: true,
            supports_streaming: true,
        },
        ModelInfo {
            id: "gemini-2.0-flash-exp".to_string(),
            name: "Gemini 2.0 Flash Experimental".to_string(),
            provider_id: "gemini".to_string(),
            description: Some("Experimental features including image generation".to_string()),
            context_length: Some(1_000_000),
            supports_vision: true,
            supports_streaming: true,
        },
        ModelInfo {
            id: "gemini-1.5-flash".to_string(),
            name: "Gemini 1.5 Flash".to_string(),
            provider_id: "gemini".to_string(),
            description: Some("Efficient model with 1M context window".to_string()),
            context_length: Some(1_000_000),
            supports_vision: true,
            supports_streaming: true,
        },
    ]
}

#[async_trait]
impl AiProvider for GeminiProvider {
    fn id(&self) -> &str { "gemini" }
    fn name(&self) -> &str { "Google Gemini" }

    fn capabilities(&self) -> Capabilities {
        Capabilities::CHAT
            | Capabilities::STREAMING
            | Capabilities::VISION
            | Capabilities::IMAGE_GENERATION
            | Capabilities::FILE_UPLOAD
    }

    fn supports_streaming(&self) -> bool { true }
    fn supports_image_generation(&self) -> bool { true }
    fn supports_video_generation(&self) -> bool { false }
    fn supports_multimodal_input(&self) -> bool { true }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(curated_models())
    }

    async fn send_message(&self, request: ChatRequest) -> Result<ChatResponse> {
        let url = self.chat_url(&request.model_id, false);
        let (contents, system_instruction) = Self::messages_to_gemini(&request.messages);

        let body = GeminiRequest {
            contents,
            system_instruction,
            generation_config: Some(GeminiGenerationConfig {
                temperature: request.temperature,
                max_output_tokens: request.max_tokens,
            }),
        };

        let resp = self.client.post(&url).json(&body).send().await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError { status, message: text }.into());
        }

        let data: GeminiResponse = resp.json().await?;
        let content = Self::extract_text(&data);

        Ok(ChatResponse {
            content,
            metadata: MessageMetadata {
                prompt_tokens: data.usage_metadata.as_ref().and_then(|u| u.prompt_token_count),
                completion_tokens: data.usage_metadata.as_ref().and_then(|u| u.candidates_token_count),
                total_tokens: data.usage_metadata.as_ref().and_then(|u| u.total_token_count),
                model_used: Some(request.model_id),
                finish_reason: data.candidates.first().and_then(|c| c.finish_reason.clone()),
                duration_ms: None,
            },
        })
    }

    async fn send_message_stream(&self, request: ChatRequest) -> Result<TextStream> {
        let url = format!("{}&alt=sse", self.chat_url(&request.model_id, true));
        let model_id = request.model_id.clone();
        let (contents, system_instruction) = Self::messages_to_gemini(&request.messages);

        let body = GeminiRequest {
            contents,
            system_instruction,
            generation_config: Some(GeminiGenerationConfig {
                temperature: request.temperature,
                max_output_tokens: request.max_tokens,
            }),
        };

        let resp = self.client.post(&url).json(&body).send().await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError { status, message: text }.into());
        }

        let byte_stream = resp.bytes_stream();

        let stream = async_stream::try_stream! {
            let mut bytes_stream = byte_stream;
            let mut buffer = String::new();

            while let Some(chunk) = bytes_stream.next().await {
                let chunk = chunk.map_err(|e| anyhow::anyhow!(e))?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // SSE: lines starting with "data: "
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim().to_string();
                    buffer = buffer[newline_pos + 1..].to_string();

                    if line.is_empty() || !line.starts_with("data: ") { continue; }

                    let json_str = &line["data: ".len()..];
                    if json_str == "[DONE]" {
                        yield StreamChunk { delta: String::new(), is_done: true, metadata: None };
                        return;
                    }

                    if let Ok(data) = serde_json::from_str::<GeminiResponse>(json_str) {
                        let text = Self::extract_text(&data);
                        if !text.is_empty() {
                            yield StreamChunk {
                                delta: text,
                                is_done: false,
                                metadata: None,
                            };
                        }
                    }
                }
            }

            yield StreamChunk { delta: String::new(), is_done: true, metadata: None };
        };

        Ok(Box::pin(stream))
    }

    async fn generate_image(&self, prompt: &str, options: ImageGenOptions) -> Result<Vec<GeneratedImage>> {
        // Use Imagen 3 for image generation
        let url = format!(
            "https://us-central1-aiplatform.googleapis.com/v1/projects/{{project}}/locations/us-central1/publishers/google/models/imagen-3.0-generate-001:predict?key={}",
            self.api_key
        );

        // Fallback: use Gemini 2.0 Flash for inline image generation
        let gemini_url = format!(
            "{}/models/gemini-2.0-flash-exp:generateContent?key={}",
            GEMINI_BASE, self.api_key
        );

        let body = json!({
            "contents": [{
                "parts": [{"text": format!("Generate an image: {}", prompt)}]
            }],
            "generationConfig": {
                "response_modalities": ["IMAGE", "TEXT"]
            }
        });

        let resp = self.client.post(&gemini_url).json(&body).send().await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError { status, message: text }.into());
        }

        let data: Value = resp.json().await?;
        let mut images = Vec::new();

        if let Some(candidates) = data["candidates"].as_array() {
            for candidate in candidates {
                if let Some(parts) = candidate["content"]["parts"].as_array() {
                    for part in parts {
                        if let Some(inline) = part.get("inlineData") {
                            images.push(GeneratedImage {
                                url: None,
                                base64_data: inline["data"].as_str().map(String::from),
                                mime_type: inline["mimeType"]
                                    .as_str()
                                    .unwrap_or("image/png")
                                    .to_string(),
                                local_path: None,
                            });
                        }
                    }
                }
            }
        }

        if images.is_empty() {
            anyhow::bail!("No images were generated");
        }

        Ok(images)
    }

    async fn generate_video(&self, _request: VideoRequest) -> Result<VideoStream> {
        not_supported!("Gemini", "video generation")
    }

    async fn upload_file(&self, path: &Path) -> Result<UploadedFile> {
        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let data = std::fs::read(path)?;

        // Gemini File API
        let start_url = format!(
            "https://generativelanguage.googleapis.com/upload/v1beta/files?key={}",
            self.api_key
        );

        let resp = self.client
            .post(&start_url)
            .header("X-Goog-Upload-Command", "start, upload, finalize")
            .header("X-Goog-Upload-Header-Content-Type", &mime)
            .header("X-Goog-Upload-Header-Content-Length", data.len().to_string())
            .header("Content-Type", &mime)
            .body(data)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::UploadFailed(format!("{}: {}", status, text)).into());
        }

        let result: Value = resp.json().await?;
        let file_uri = result["file"]["uri"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let file_id = result["file"]["name"]
            .as_str()
            .unwrap_or("")
            .to_string();

        Ok(UploadedFile {
            file_id,
            uri: Some(file_uri),
            mime_type: mime,
            display_name: file_name,
        })
    }

    async fn health_check(&self) -> Result<()> {
        let url = format!("{}/models?key={}", GEMINI_BASE, self.api_key);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            return Err(ProviderError::AuthError("Invalid Gemini API key".to_string()).into());
        }
        Ok(())
    }
}
