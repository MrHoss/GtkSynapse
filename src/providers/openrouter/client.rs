//! OpenRouter provider — OpenAI-compatible gateway to 400+ models.
//!
//! API: <https://openrouter.ai/docs/api-reference>
//! Auth: `Authorization: Bearer <key>` (plus optional `HTTP-Referer` /
//! `X-Title` headers). Vision models accept images as inline base64
//! data URLs in the chat content.

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

use crate::core::attachment;
use crate::core::models::{
    ChatRequest, ChatResponse, GeneratedImage, ImageGenOptions, Message, MessageMetadata,
    MessageRole, ModelInfo, StreamChunk, UploadedFile, VideoProgress, VideoRequest,
};
use crate::core::provider::{AiProvider, ProviderError, TextStream, VideoStream};
use crate::not_supported;

const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";

// ─── Wire types (OpenAI-compatible) ───────────────────────────

#[derive(Debug, Serialize)]
struct OpenRouterChatRequest {
    model: String,
    messages: Vec<OpenRouterMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenRouterMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<OpenRouterContent>,
}

/// Content is either a plain string or a list of typed parts (for vision).
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
enum OpenRouterContent {
    Text(String),
    Parts(Vec<OpenRouterPart>),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
enum OpenRouterPart {
    Text { r#type: String, text: String },
    Image { r#type: String, image_url: OpenRouterImage },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenRouterImage {
    url: String,
}

#[derive(Debug, Deserialize)]
struct OpenRouterChatResponse {
    choices: Vec<OpenRouterChoice>,
    #[serde(default)]
    usage: Option<OpenRouterUsage>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterChoice {
    message: Option<OpenRouterMessage>,
    delta: Option<OpenRouterDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterUsage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
    #[serde(default)]
    total_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterModelsResponse {
    data: Vec<OpenRouterModel>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterModel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context_length: Option<u32>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    modalities: Option<OpenRouterModalities>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenRouterModalities {
    #[serde(default)]
    input: Vec<String>,
    #[serde(default)]
    output: Vec<String>,
}

// ─── Provider ────────────────────────────────────────────────

pub struct OpenRouterProvider {
    client: Client,
    api_key: String,
}

impl OpenRouterProvider {
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

    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", self.api_key)) {
            headers.insert(reqwest::header::AUTHORIZATION, v);
        }
        // Identify the app to OpenRouter for usage analytics (optional but
        // recommended by their API docs).
        if let Ok(v) = reqwest::header::HeaderValue::from_str("AIChat") {
            headers.insert("X-Title", v);
        }
        if let Ok(v) = reqwest::header::HeaderValue::from_str("https://github.com/") {
            headers.insert("HTTP-Referer", v);
        }
        headers
    }

    /// Convert core messages to the OpenAI-compatible wire format.
    ///
    /// Embedded system messages are folded into a leading system message;
    /// image attachments are serialized as inline base64 data URLs.
    fn messages_to_openai(
        messages: &[Message],
        system_prompt: Option<&str>,
    ) -> Vec<OpenRouterMessage> {
        let mut result = Vec::new();

        if let Some(system) = system_prompt {
            result.push(OpenRouterMessage {
                role: "system".to_string(),
                content: Some(OpenRouterContent::Text(system.to_string())),
            });
        }

        for msg in messages {
            if msg.role == MessageRole::System {
                continue; // handled above
            }

            let images: Vec<String> = msg
                .attachments
                .iter()
                .filter(|att| {
                    matches!(att.kind, crate::core::models::AttachmentKind::Image)
                })
                .filter_map(|att| {
                    attachment::file_to_base64(&att.file_path)
                        .ok()
                        .map(|b64| format!("data:{};base64,{}", att.mime_type, b64))
                })
                .collect();

            let content = if images.is_empty() {
                OpenRouterContent::Text(msg.content.clone())
            } else {
                let mut parts = Vec::new();
                if !msg.content.trim().is_empty() {
                    parts.push(OpenRouterPart::Text {
                        r#type: "text".to_string(),
                        text: msg.content.clone(),
                    });
                }
                for url in images {
                    parts.push(OpenRouterPart::Image {
                        r#type: "image_url".to_string(),
                        image_url: OpenRouterImage { url },
                    });
                }
                OpenRouterContent::Parts(parts)
            };

            result.push(OpenRouterMessage {
                role: msg.role.as_str().to_string(),
                content: Some(content),
            });
        }

        result
    }

    fn extract_text(response: &OpenRouterChatResponse) -> String {
        response
            .choices
            .first()
            .and_then(|c| c.message.as_ref())
            .and_then(|m| m.content.as_ref())
            .map(|content| match content {
                OpenRouterContent::Text(t) => t.clone(),
                OpenRouterContent::Parts(parts) => parts
                    .iter()
                    .filter_map(|p| match p {
                        OpenRouterPart::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect(),
            })
            .unwrap_or_default()
    }
}

/// A curated list of well-known OpenRouter models, used when the live
/// model list cannot be fetched (no network / no API key).
pub fn curated_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "openai/gpt-4o-mini".to_string(),                name: "OpenAI GPT-4o Mini".to_string(),
                provider_id: "openrouter".to_string(),
                description: Some("Fast and affordable, good default".to_string()),
                context_length: Some(128_000),
                supports_vision: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "openai/gpt-4o".to_string(),
                name: "OpenAI GPT-4o".to_string(),
                provider_id: "openrouter".to_string(),
                description: Some("High-quality multimodal model".to_string()),
                context_length: Some(128_000),
                supports_vision: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "anthropic/claude-3.5-haiku".to_string(),
                name: "Anthropic Claude 3.5 Haiku".to_string(),
                provider_id: "openrouter".to_string(),
                description: Some("Fast and capable".to_string()),
                context_length: Some(200_000),
                supports_vision: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "anthropic/claude-3.7-sonnet".to_string(),
                name: "Anthropic Claude 3.7 Sonnet".to_string(),
                provider_id: "openrouter".to_string(),
                description: Some("Balanced reasoning and speed".to_string()),
                context_length: Some(200_000),
                supports_vision: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "google/gemini-2.0-flash-001".to_string(),
                name: "Google Gemini 2.0 Flash".to_string(),
                provider_id: "openrouter".to_string(),
                description: Some("Fast Google model".to_string()),
                context_length: Some(1_000_000),
                supports_vision: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "google/gemini-2.5-pro-exp-03-25:free".to_string(),
                name: "Google Gemini 2.5 Pro (free)".to_string(),
                provider_id: "openrouter".to_string(),
                description: Some("Free tier Gemini 2.5 Pro".to_string()),
                context_length: Some(1_000_000),
                supports_vision: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: "meta-llama/llama-3.3-70b-instruct".to_string(),
                name: "Meta Llama 3.3 70B Instruct".to_string(),
                provider_id: "openrouter".to_string(),
                description: Some("Strong open-weights model".to_string()),
                context_length: Some(128_000),
                supports_vision: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: "mistralai/mistral-small-3.1-24b-instruct".to_string(),
                name: "Mistral Small 3.1 24B".to_string(),
                provider_id: "openrouter".to_string(),
                description: Some("Compact, efficient Mistral model".to_string()),
                context_length: Some(128_000),
                supports_vision: false,
                supports_streaming: true,
            },
        ]
}

#[async_trait]
impl AiProvider for OpenRouterProvider {
    fn id(&self) -> &str { "openrouter" }
    fn name(&self) -> &str { "OpenRouter" }

    fn capabilities(&self) -> crate::providers::capabilities::Capabilities {
        crate::providers::capabilities::Capabilities::CHAT
            | crate::providers::capabilities::Capabilities::VISION
            | crate::providers::capabilities::Capabilities::STREAMING
    }

    fn supports_streaming(&self) -> bool { true }
    fn supports_image_generation(&self) -> bool { false }
    fn supports_video_generation(&self) -> bool { false }
    fn supports_multimodal_input(&self) -> bool { true }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let url = format!("{}/models", OPENROUTER_BASE);
        let resp = self
            .client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await?;

        if !resp.status().is_success() {
            // Without a valid key OpenRouter still returns the model list on
            // the public endpoint, but if it fails fall back to curated.
            return Ok(curated_models());
        }

        let data: OpenRouterModelsResponse = resp.json().await.unwrap_or_else(|_| {
            OpenRouterModelsResponse { data: Vec::new() }
        });

        if data.data.is_empty() {
            return Ok(curated_models());
        }

        Ok(data.data.into_iter().map(|m| {
            let supports_vision = m
                .modalities
                .as_ref()
                .map(|mods| mods.input.iter().any(|i| i == "image"))
                .unwrap_or(false);
            ModelInfo {
                id: m.id.clone(),
                name: m.name.unwrap_or_else(|| m.id.clone()),
                provider_id: "openrouter".to_string(),
                description: m.description,
                context_length: m.context_length,
                supports_vision,
                supports_streaming: true,
            }
        }).collect())
    }

    async fn send_message(&self, request: ChatRequest) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", OPENROUTER_BASE);
        let messages = Self::messages_to_openai(&request.messages, request.system_prompt.as_deref());

        let body = OpenRouterChatRequest {
            model: request.model_id.clone(),
            messages,
            stream: false,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        let resp = self
            .client
            .post(&url)
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            if status == 401 {
                return Err(ProviderError::AuthError("Invalid OpenRouter API key".to_string()).into());
            }
            return Err(ProviderError::ApiError { status, message: text }.into());
        }

        let data: OpenRouterChatResponse = resp.json().await?;
        let content = Self::extract_text(&data);

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
        let url = format!("{}/chat/completions", OPENROUTER_BASE);
        let model_id = request.model_id.clone();
        let messages = Self::messages_to_openai(&request.messages, request.system_prompt.as_deref());

        let body = OpenRouterChatRequest {
            model: request.model_id,
            messages,
            stream: true,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        let resp = self
            .client
            .post(&url)
            .headers(self.auth_headers())
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
                        yield StreamChunk {
                            delta: String::new(),
                            is_done: true,
                            metadata: None,
                        };
                        return;
                    }

                    if let Ok(data) = serde_json::from_str::<OpenRouterChatResponse>(json_str) {
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
        not_supported!("OpenRouter", "image generation")
    }

    async fn generate_video(&self, _request: VideoRequest) -> Result<VideoStream> {
        not_supported!("OpenRouter", "video generation")
    }

    async fn upload_file(&self, _path: &Path) -> Result<UploadedFile> {
        not_supported!("OpenRouter", "file upload")
    }

    async fn health_check(&self) -> Result<()> {
        let url = format!("{}/auth/key", OPENROUTER_BASE);
        let resp = self
            .client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await?;

        if resp.status() == 401 {
            return Err(ProviderError::AuthError("Invalid OpenRouter API key".to_string()).into());
        }
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::ApiError { status, message: text }.into());
        }
        Ok(())
    }
}
