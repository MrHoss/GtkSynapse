//! Core domain models shared across the entire application.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

// ─── Identifiers ────────────────────────────────────────────

/// Unique identifier for a conversation.
pub type ConversationId = String;
/// Unique identifier for a message.
pub type MessageId = String;
/// Unique identifier for an attachment.
pub type AttachmentId = String;

// ─── Conversation ───────────────────────────────────────────

/// The kind of a conversation: a text chat or a media generation session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationKind {
    Chat,
    Image,
    Video,
    Audio,
}

impl ConversationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConversationKind::Chat => "chat",
            ConversationKind::Image => "image",
            ConversationKind::Video => "video",
            ConversationKind::Audio => "audio",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "image" => Self::Image,
            "video" => Self::Video,
            "audio" => Self::Audio,
            _ => Self::Chat,
        }
    }
}

/// A complete conversation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: ConversationId,
    pub title: String,
    pub provider_id: String,
    pub model_id: String,
    pub kind: ConversationKind,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub is_favorite: bool,
    pub system_prompt: Option<String>,
    pub message_count: usize,
}

impl Conversation {
    pub fn new(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            title: "New Chat".to_string(),
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            kind: ConversationKind::Chat,
            created_at: now,
            updated_at: now,
            is_favorite: false,
            system_prompt: None,
            message_count: 0,
        }
    }

    /// Create a new conversation for a media generation session.
    pub fn new_media(kind: ConversationKind) -> Self {
        let now = Utc::now();
        let title = match kind {
            ConversationKind::Image => "Image Generation",
            ConversationKind::Video => "Video Generation",
            ConversationKind::Audio => "Audio Generation",
            ConversationKind::Chat => "New Chat",
        }
        .to_string();
        Self {
            id: Uuid::new_v4().to_string(),
            title,
            provider_id: String::new(),
            model_id: String::new(),
            kind,
            created_at: now,
            updated_at: now,
            is_favorite: false,
            system_prompt: None,
            message_count: 0,
        }
    }
}

// ─── Messages ───────────────────────────────────────────────

/// The role of a message participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        }
    }
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub role: MessageRole,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub attachments: Vec<Attachment>,
    pub metadata: Option<MessageMetadata>,
}

impl Message {
    pub fn user(conversation_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.into(),
            role: MessageRole::User,
            content: content.into(),
            created_at: Utc::now(),
            attachments: Vec::new(),
            metadata: None,
        }
    }

    pub fn assistant(conversation_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.into(),
            role: MessageRole::Assistant,
            content: content.into(),
            created_at: Utc::now(),
            attachments: Vec::new(),
            metadata: None,
        }
    }

    pub fn system(conversation_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.into(),
            role: MessageRole::System,
            content: content.into(),
            created_at: Utc::now(),
            attachments: Vec::new(),
            metadata: None,
        }
    }
}

/// Metadata associated with a message (token counts, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageMetadata {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub model_used: Option<String>,
    pub finish_reason: Option<String>,
    pub duration_ms: Option<u64>,
}

// ─── Attachments ────────────────────────────────────────────

/// Supported attachment types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentKind {
    Image,
    Video,
    Audio,
    Pdf,
    Text,
    Unknown,
}

impl AttachmentKind {
    pub fn from_mime(mime: &str) -> Self {
        if mime.starts_with("image/") {
            return Self::Image;
        }
        if mime.starts_with("video/") {
            return Self::Video;
        }
        if mime.starts_with("audio/") {
            return Self::Audio;
        }
        if mime == "application/pdf" {
            return Self::Pdf;
        }
        if mime.starts_with("text/") {
            return Self::Text;
        }
        Self::Unknown
    }
}

/// A file attachment linked to a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: AttachmentId,
    pub message_id: Option<MessageId>,
    pub file_path: PathBuf,
    pub file_name: String,
    pub mime_type: String,
    pub kind: AttachmentKind,
    pub size_bytes: u64,
    /// Remote URL after uploading to a provider (when applicable).
    pub remote_url: Option<String>,
    /// Remote file ID after uploading to a provider (when applicable).
    pub remote_id: Option<String>,
}

impl Attachment {
    pub fn from_path(path: PathBuf) -> anyhow::Result<Self> {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let mime = mime_guess::from_path(&path)
            .first_or_octet_stream()
            .to_string();
        let size = std::fs::metadata(&path)?.len();
        let kind = AttachmentKind::from_mime(&mime);
        Ok(Self {
            id: Uuid::new_v4().to_string(),
            message_id: None,
            file_path: path,
            file_name,
            mime_type: mime,
            kind,
            size_bytes: size,
            remote_url: None,
            remote_id: None,
        })
    }
}

// ─── Provider Models ─────────────────────────────────────────

/// Information about an available AI model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    /// Unique model identifier (e.g. "llama3.1:8b", "gemini-2.0-flash").
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Provider ID this model belongs to.
    pub provider_id: String,
    /// Optional description.
    pub description: Option<String>,
    /// Context window size in tokens.
    pub context_length: Option<u32>,
    /// Whether the model supports multimodal (image) input.
    pub supports_vision: bool,
    /// Whether the model supports streaming.
    pub supports_streaming: bool,
}

// ─── Chat Request / Response ─────────────────────────────────

/// A chat request to send to a provider.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model_id: String,
    pub messages: Vec<Message>,
    pub stream: bool,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub system_prompt: Option<String>,
}

/// A complete (non-streaming) chat response.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub metadata: MessageMetadata,
}

/// A single chunk delivered during streaming.
#[derive(Debug, Clone)]
pub struct StreamChunk {
    /// Incremental text content.
    pub delta: String,
    /// Whether this is the final chunk.
    pub is_done: bool,
    /// Optional metadata in the final chunk.
    pub metadata: Option<MessageMetadata>,
}

// ─── Image Generation ────────────────────────────────────────

/// Options for image generation requests.
#[derive(Debug, Clone, Default)]
pub struct ImageGenOptions {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub num_images: Option<u32>,
    pub style: Option<String>,
    pub negative_prompt: Option<String>,
}

/// Result of an image generation request.
#[derive(Debug, Clone)]
pub struct GeneratedImage {
    pub url: Option<String>,
    pub base64_data: Option<String>,
    pub mime_type: String,
    pub local_path: Option<PathBuf>,
}

// ─── Generated Media (persisted) ─────────────────────────────

/// A single generated media item, persisted inside a media conversation as
/// JSON so it can be reloaded when the conversation is reopened.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedMedia {
    /// "image" | "video" | "audio".
    pub kind: String,
    /// MIME type of the generated item.
    pub mime: String,
    /// The prompt used to generate it.
    pub prompt: String,
    /// The provider model used.
    pub model: String,
    /// Base64-encoded bytes (images).
    pub base64: Option<String>,
    /// Remote URL (videos/audio).
    pub url: Option<String>,
    /// Last known video status ("queued"/"processing"/"completed"/"failed").
    pub video_status: Option<String>,
    /// Human-readable status/message.
    pub message: Option<String>,
}

// ─── Credit Balance ───────────────────────────────────────────

/// Credit balance reported by a provider (for usage-aware UIs).
#[derive(Debug, Clone)]
pub struct CreditBalance {
    /// Provider this balance belongs to.
    pub provider_id: String,
    /// Recurring monthly credits, when reported.
    pub monthly_credits: Option<i64>,
    /// Purchased/one-off credits, when reported.
    pub package_credits: Option<i64>,
}

// ─── Video Generation ────────────────────────────────────────/// A video generation request.
#[derive(Debug, Clone)]
pub struct VideoRequest {
    pub prompt: String,
    pub source_image_path: Option<PathBuf>,
    /// Model identifier (provider-specific, e.g. "v6" for PixVerse).
    pub model: Option<String>,
    pub duration_seconds: Option<u8>,
    pub aspect_ratio: Option<String>,
    pub quality: Option<String>,
    pub motion_strength: Option<f32>,
}

/// Progress update during video generation.
#[derive(Debug, Clone)]
pub struct VideoProgress {
    /// Task ID on the provider side.
    pub task_id: String,
    /// Current status string.
    pub status: VideoStatus,
    /// Progress percentage (0-100).
    pub percent: u8,
    /// URL to the final video (when complete).
    pub video_url: Option<String>,
    /// Human-readable status message.
    pub message: Option<String>,
}

/// Video generation status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoStatus {
    Queued,
    Processing,
    Completed,
    Failed,
}

// ─── File Upload ─────────────────────────────────────────────

/// Result of uploading a file to a provider.
#[derive(Debug, Clone)]
pub struct UploadedFile {
    /// Remote file ID (provider-specific).
    pub file_id: String,
    /// Remote URI/URL for this file.
    pub uri: Option<String>,
    /// MIME type of the uploaded file.
    pub mime_type: String,
    /// Display name of the file.
    pub display_name: String,
}

// ─── App Settings ────────────────────────────────────────────

/// Application-wide settings (stored in SQLite).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub default_provider_id: String,
    pub default_model_id: String,
    pub theme: ThemePreference,
    pub language: String,
    pub max_context_messages: u32,
    pub request_timeout_secs: u64,
    pub download_folder: PathBuf,
    pub proxy_url: Option<String>,
    pub enable_logging: bool,
    pub stream_by_default: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_provider_id: "ollama".to_string(),
            default_model_id: "llama3.2:3b".to_string(),
            theme: ThemePreference::System,
            language: "en".to_string(),
            max_context_messages: 20,
            request_timeout_secs: 120,
            download_folder: dirs::download_dir().unwrap_or_else(|| PathBuf::from(".")),
            proxy_url: None,
            enable_logging: false,
            stream_by_default: true,
        }
    }
}

/// Preferred color theme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemePreference {
    System,
    Light,
    Dark,
}
