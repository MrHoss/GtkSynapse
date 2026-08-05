//! Groq provider — OpenAI-compatible API with very fast inference.

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

use crate::core::models::{
    ChatRequest, ChatResponse, GeneratedImage, ImageGenOptions,
    Message, MessageMetadata, MessageRole, ModelInfo, StreamChunk,
    UploadedFile, VideoProgress, VideoRequest,
};
use crate::core::provider::{AiProvider, ProviderError, TextStream, VideoStream};
use crate::not_supported;
use crate::providers::capabilities::Capabilities;

const GROQ_BASE: &str = "https://api.groq.com/openai/v1";

// ─── Wire types (OpenAI-compatible) ───────────────────────────

#[derive(Debug, Serialize)]
struct GroqChatRequest {
    model: String,
    messages: Vec<GroqMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GroqMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct GroqChatResponse {
    choices: Vec<GroqChoice>,
    #[serde(default)]
    usage: Option<GroqUsage>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GroqChoice {
    message: Option<GroqMessage>,
    delta: Option<GroqDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GroqDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GroqUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    total_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct GroqModelsResponse {
    data: Vec<GroqModelData>,
}

#[derive(Debug, Deserialize)]
struct GroqModelData {
    id: String,
    #[serde(default)]
    context_window: Option<u32>,
}

// ─── Provider ────────────────────────────────────────────────

pub struct GroqProvider {
    client: Client,
    api_key: String,
}

impl GroqProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            api_key: api_key.into(),
        }
    }

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_key)
    }

    fn messages_to_groq(messages: &[Message], system_prompt: Option<&str>) -> Vec<GroqMessage> {
        let mut result = Vec::new();

        if let Some(system) = system_prompt {
            result.push(GroqMessage {
                role: "system".to_string(),
                content: system.to_string(),
            });
        }

        for msg in messages {
            if msg.role == MessageRole::System { continue; } // handled above
            result.push(GroqMessage {
                role: msg.role.as_str().to_string(),
                content: msg.content.clone(),
            });
        }
        result
    }
}

#[async_trait]
impl AiProvider for GroqProvider {
    fn id(&self) -> &str { "groq" }
    fn name(&self) -> &str { "Groq" }

    fn capabilities(&self) -> Capabilities {
        Capabilities::CHAT | Capabilities::STREAMING
    }

    fn supports_streaming(&self) -> bool { true }
    fn supports_image_generation(&self) -> bool { false }
    fn supports_video_generation(&self) -> bool { false }
    fn supports_multimodal_input(&self) -> bool { false }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let url = format!("{}/models", GROQ_BASE);
        let resp = self.client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;

        if !resp.status().is_success() {
            // Return curated fallback list if API fails
            return Ok(curated_models());
        }

        let data: GroqModelsResponse = resp.json().await.unwrap_or_else(|_| {
            GroqModelsResponse { data: Vec::new() }
        });

        if data.data.is_empty() {
            return Ok(curated_models());
        }

        Ok(data.data.into_iter().map(|m| ModelInfo {
            id: m.id.clone(),
            name: m.id.clone(),
            provider_id: "groq".to_string(),
            description: None,
            context_length: m.context_window,
            supports_vision: false,
            supports_streaming: true,
        }).collect())
    }

    async fn send_message(&self, request: ChatRequest) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", GROQ_BASE);
        let messages = Self::messages_to_groq(&request.messages, request.system_prompt.as_deref());

        let body = GroqChatRequest {
            model: request.model_id.clone(),
            messages,
            stream: false,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        let resp = self.client
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError { status, message: text }.into());
        }

        let data: GroqChatResponse = resp.json().await?;
        let content = data.choices
            .first()
            .and_then(|c| c.message.as_ref())
            .map(|m| m.content.clone())
            .unwrap_or_default();

        Ok(ChatResponse {
            content,
            metadata: MessageMetadata {
                prompt_tokens: data.usage.as_ref().and_then(|u| u.prompt_tokens),
                completion_tokens: data.usage.as_ref().and_then(|u| u.completion_tokens),
                total_tokens: data.usage.as_ref().and_then(|u| u.total_tokens),
                model_used: data.model.or(Some(request.model_id)),
                finish_reason: data.choices.first().and_then(|c| c.finish_reason.clone()),
                duration_ms: None,
            },
        })
    }

    async fn send_message_stream(&self, request: ChatRequest) -> Result<TextStream> {
        let url = format!("{}/chat/completions", GROQ_BASE);
        let model_id = request.model_id.clone();
        let messages = Self::messages_to_groq(&request.messages, request.system_prompt.as_deref());

        let body = GroqChatRequest {
            model: request.model_id,
            messages,
            stream: true,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        let resp = self.client
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send()
            .await?;

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

                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim().to_string();
                    buffer = buffer[newline_pos + 1..].to_string();

                    if line.is_empty() || !line.starts_with("data: ") { continue; }

                    let json_str = &line["data: ".len()..];
                    if json_str == "[DONE]" {
                        yield StreamChunk { delta: String::new(), is_done: true, metadata: None };
                        return;
                    }

                    if let Ok(data) = serde_json::from_str::<GroqChatResponse>(json_str) {
                        let delta = data.choices
                            .first()
                            .and_then(|c| c.delta.as_ref())
                            .and_then(|d| d.content.clone())
                            .unwrap_or_default();

                        let finish = data.choices
                            .first()
                            .and_then(|c| c.finish_reason.as_deref())
                            .is_some_and(|r| r == "stop");

                        yield StreamChunk {
                            delta,
                            is_done: finish,
                            metadata: None,
                        };
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn generate_image(&self, _prompt: &str, _options: ImageGenOptions) -> Result<Vec<GeneratedImage>> {
        not_supported!("Groq", "image generation")
    }

    async fn generate_video(&self, _request: VideoRequest) -> Result<VideoStream> {
        not_supported!("Groq", "video generation")
    }

    async fn upload_file(&self, _path: &Path) -> Result<UploadedFile> {
        not_supported!("Groq", "file upload")
    }

    async fn health_check(&self) -> Result<()> {
        let url = format!("{}/models", GROQ_BASE);
        let resp = self.client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(ProviderError::AuthError("Invalid Groq API key".to_string()).into());
        }
        Ok(())
    }
}

/// Curated list of Groq models (used as a fallback by `list_models`).
pub fn curated_models() -> Vec<ModelInfo> {
    vec![
            ModelInfo {
                id: "llama-3.3-70b-versatile".to_string(),
                name: "Llama 3.3 70B Versatile".to_string(),
                provider_id: "groq".to_string(),
                description: Some("High-performance, versatile model".to_string()),
                context_length: Some(128_000),
                supports_vision: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: "llama-3.1-8b-instant".to_string(),
                name: "Llama 3.1 8B Instant".to_string(),
                provider_id: "groq".to_string(),
                description: Some("Ultra-fast responses".to_string()),
                context_length: Some(128_000),
                supports_vision: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: "mixtral-8x7b-32768".to_string(),
                name: "Mixtral 8x7B".to_string(),
                provider_id: "groq".to_string(),
                description: Some("Mixture of experts model".to_string()),
                context_length: Some(32_768),
                supports_vision: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: "gemma2-9b-it".to_string(),
                name: "Gemma 2 9B IT".to_string(),
                provider_id: "groq".to_string(),
                description: Some("Google's efficient Gemma 2 model".to_string()),
                context_length: Some(8_192),
                supports_vision: false,
                supports_streaming: true,
            },
        ]
}