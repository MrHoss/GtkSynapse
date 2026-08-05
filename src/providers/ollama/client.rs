//! Ollama provider — local LLM inference via the Ollama REST API.
//!
//! Ollama must be running at the configured endpoint (default: http://localhost:11434).
//! No API key required.

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use crate::core::models::{
    ChatRequest, ChatResponse, GeneratedImage, ImageGenOptions, Message, MessageMetadata,
    MessageRole, ModelInfo, StreamChunk, UploadedFile, VideoProgress, VideoRequest,
};
use crate::core::provider::{AiProvider, ProviderError, TextStream, VideoStream};
use crate::core::attachment;
use crate::not_supported;
use crate::providers::capabilities::Capabilities;

// ─── Ollama wire types ────────────────────────────────────────

#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    images: Vec<String>, // base64-encoded images
}

#[derive(Debug, Serialize, Default)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_ctx: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaMessage>,
    done: bool,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
    #[serde(default)]
    total_duration: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelInfo>,
}

#[derive(Debug, Deserialize)]
struct OllamaModelInfo {
    name: String,
    #[serde(default)]
    details: OllamaModelDetails,
}

#[derive(Debug, Deserialize, Default)]
struct OllamaModelDetails {
    #[serde(default)]
    parameter_size: Option<String>,
    #[serde(default)]
    families: Option<Vec<String>>,
}

// ─── Provider Implementation ──────────────────────────────────

/// Curated fallback list used when the local Ollama server is offline.
pub fn curated_models() -> Vec<ModelInfo> {
    let mk = |id: &str, desc: &str, ctx: u32| ModelInfo {
        id: id.to_string(),
        name: id.to_string(),
        provider_id: "ollama".to_string(),
        description: Some(desc.to_string()),
        context_length: Some(ctx),
        supports_vision: false,
        supports_streaming: true,
    };
    vec![
        mk("llama3.1:8b", "Meta Llama 3.1 8B", 8192),
        mk("llama3.2:3b", "Meta Llama 3.2 3B", 8192),
        mk("llama3.2:1b", "Meta Llama 3.2 1B", 8192),
        mk("qwen2.5:7b", "Qwen 2.5 7B", 8192),
        mk("qwen2.5:3b", "Qwen 2.5 3B", 8192),
        mk("mistral:7b", "Mistral 7B", 8192),
        mk("gemma2:9b", "Google Gemma 2 9B", 8192),
        mk("phi3:mini", "Microsoft Phi-3 Mini", 8192),
    ]
}

pub struct OllamaProvider {
    client: Client,
    base_url: String,
}

impl OllamaProvider {
    pub fn new(base_url: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            base_url: base_url.into(),
        }
    }

    pub fn default() -> Self {
        Self::new("http://localhost:11434")
    }

    fn messages_to_ollama(messages: &[Message]) -> Vec<OllamaMessage> {
        messages.iter().map(|msg| {
            let mut images = Vec::new();
            for att in &msg.attachments {
                if att.kind == crate::core::models::AttachmentKind::Image {
                    if let Ok(b64) = attachment::file_to_base64(&att.file_path) {
                        images.push(b64);
                    }
                }
            }
            OllamaMessage {
                role: msg.role.as_str().to_string(),
                content: msg.content.clone(),
                images,
            }
        }).collect()
    }
}

#[async_trait]
impl AiProvider for OllamaProvider {
    fn id(&self) -> &str { "ollama" }
    fn name(&self) -> &str { "Ollama (Local)" }

    fn capabilities(&self) -> Capabilities {
        Capabilities::CHAT | Capabilities::STREAMING | Capabilities::VISION
    }

    fn supports_streaming(&self) -> bool { true }
    fn supports_image_generation(&self) -> bool { false }
    fn supports_video_generation(&self) -> bool { false }
    fn supports_multimodal_input(&self) -> bool { true }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let url = format!("{}/api/tags", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to connect to Ollama. Is it running?")?;

        if !resp.status().is_success() {
            return Err(ProviderError::ApiError {
                status: resp.status().as_u16(),
                message: resp.text().await.unwrap_or_default(),
            }.into());
        }

        let data: OllamaTagsResponse = resp.json().await?;
        let models = data.models.into_iter().map(|m| {
            let is_vision = m
                .details
                .families
                .as_ref()
                .map(|f| f.iter().any(|fam| fam.contains("clip") || fam.contains("vision")))
                .unwrap_or(false);
            ModelInfo {
                id: m.name.clone(),
                name: m.name.clone(),
                provider_id: "ollama".to_string(),
                description: m.details.parameter_size,
                context_length: None,
                supports_vision: is_vision,
                supports_streaming: true,
            }
        }).collect();
        Ok(models)
    }

    async fn send_message(&self, request: ChatRequest) -> Result<ChatResponse> {
        let url = format!("{}/api/chat", self.base_url);
        let body = OllamaChatRequest {
            model: request.model_id.clone(),
            messages: Self::messages_to_ollama(&request.messages),
            stream: false,
            options: Some(OllamaOptions {
                temperature: request.temperature,
                num_ctx: None,
            }),
        };

        let resp = self.client.post(&url).json(&body).send().await?;

        if !resp.status().is_success() {
            return Err(ProviderError::ApiError {
                status: resp.status().as_u16(),
                message: resp.text().await.unwrap_or_default(),
            }.into());
        }

        let data: OllamaChatResponse = resp.json().await?;
        let content = data.message.map(|m| m.content).unwrap_or_default();

        Ok(ChatResponse {
            content,
            metadata: MessageMetadata {
                prompt_tokens: data.prompt_eval_count,
                completion_tokens: data.eval_count,
                total_tokens: data.prompt_eval_count.zip(data.eval_count).map(|(a, b)| a + b),
                model_used: Some(request.model_id),
                finish_reason: Some("stop".into()),
                duration_ms: data.total_duration.map(|ns| ns / 1_000_000),
            },
        })
    }

    async fn send_message_stream(&self, request: ChatRequest) -> Result<TextStream> {
        let url = format!("{}/api/chat", self.base_url);
        let model_id = request.model_id.clone();
        let body = OllamaChatRequest {
            model: request.model_id,
            messages: Self::messages_to_ollama(&request.messages),
            stream: true,
            options: Some(OllamaOptions {
                temperature: request.temperature,
                num_ctx: None,
            }),
        };

        let resp = self.client.post(&url).json(&body).send().await?;

        if !resp.status().is_success() {
            return Err(ProviderError::ApiError {
                status: resp.status().as_u16(),
                message: resp.text().await.unwrap_or_default(),
            }.into());
        }

        let byte_stream = resp.bytes_stream();

        let stream = async_stream::try_stream! {
            let mut bytes_stream = byte_stream;
            let mut buffer = String::new();

            while let Some(chunk) = bytes_stream.next().await {
                let chunk = chunk.map_err(|e| anyhow::anyhow!(e))?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Ollama sends newline-delimited JSON
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim().to_string();
                    buffer = buffer[newline_pos + 1..].to_string();

                    if line.is_empty() { continue; }

                    if let Ok(data) = serde_json::from_str::<OllamaChatResponse>(&line) {
                        let delta = data.message.map(|m| m.content).unwrap_or_default();
                        let is_done = data.done;

                        if is_done {
                            yield StreamChunk {
                                delta,
                                is_done: true,
                                metadata: Some(MessageMetadata {
                                    prompt_tokens: data.prompt_eval_count,
                                    completion_tokens: data.eval_count,
                                    total_tokens: data
                                        .prompt_eval_count
                                        .zip(data.eval_count)
                                        .map(|(a, b)| a + b),
                                    model_used: Some(model_id.clone()),
                                    finish_reason: Some("stop".into()),
                                    duration_ms: data.total_duration.map(|ns| ns / 1_000_000),
                                }),
                            };
                            return;
                        } else {
                            yield StreamChunk {
                                delta,
                                is_done: false,
                                metadata: None,
                            };
                        }
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn generate_image(&self, _prompt: &str, _options: ImageGenOptions) -> Result<Vec<GeneratedImage>> {
        not_supported!("Ollama", "image generation")
    }

    async fn generate_video(&self, _request: VideoRequest) -> Result<VideoStream> {
        not_supported!("Ollama", "video generation")
    }

    async fn upload_file(&self, _path: &Path) -> Result<UploadedFile> {
        not_supported!("Ollama", "file upload")
    }

    async fn health_check(&self) -> Result<()> {
        let url = format!("{}/api/tags", self.base_url);
        self.client.get(&url).send().await
            .context("Cannot reach Ollama. Make sure it's running.")?;
        Ok(())
    }
}
