//! The central `AiProvider` trait. Every AI backend must implement this.
//!
//! The rest of the application never depends on concrete provider types —
//! all access goes through this interface.

use std::path::Path;
use std::pin::Pin;

use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;

use crate::core::models::{
    ChatRequest, ChatResponse, CreditBalance, GeneratedImage, ImageGenOptions, ModelInfo,
    StreamChunk, UploadedFile, VideoProgress, VideoRequest,
};
use crate::providers::capabilities::Capabilities;

/// The type alias for a streaming text response.
pub type TextStream = Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>;

/// The type alias for a streaming video generation progress.
pub type VideoStream = Pin<Box<dyn Stream<Item = Result<VideoProgress>> + Send>>;

/// Every AI provider must implement this trait.
///
/// Providers that don't support a specific capability should return
/// [`ProviderError::NotSupported`] for those methods.
#[async_trait]
pub trait AiProvider: Send + Sync {
    // ── Identity ──────────────────────────────────────────────────

    /// The unique machine-readable identifier for this provider.
    /// Examples: "ollama", "gemini", "groq", "pixverse"
    fn id(&self) -> &str;

    /// Human-readable provider name.
    fn name(&self) -> &str;

    // ── Capabilities ──────────────────────────────────────────────

    /// Returns the full set of capabilities this provider supports.
    ///
    /// The default implementation derives from the individual `supports_*`
    /// methods. Providers may override it to express finer-grained
    /// capabilities (e.g. file upload, embeddings) or to remove `CHAT` for
    /// non-chat backends such as video-only services.
    fn capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::CHAT;
        if self.supports_streaming() {
            caps.insert(Capabilities::STREAMING);
        }
        if self.supports_image_generation() {
            caps.insert(Capabilities::IMAGE_GENERATION);
        }
        if self.supports_video_generation() {
            caps.insert(Capabilities::VIDEO_GENERATION);
        }
        if self.supports_multimodal_input() {
            caps.insert(Capabilities::VISION);
        }
        caps
    }

    /// Whether this provider supports a given capability.
    fn supports(&self, cap: Capabilities) -> bool {
        self.capabilities().contains(cap)
    }

    /// Returns true if the provider supports streaming text responses.
    fn supports_streaming(&self) -> bool;

    /// Returns true if the provider can generate images.
    fn supports_image_generation(&self) -> bool;

    /// Returns true if the provider can generate videos.
    fn supports_video_generation(&self) -> bool;

    /// Returns true if the provider accepts image/audio input (multimodal).
    fn supports_multimodal_input(&self) -> bool;

    // ── Model Discovery ───────────────────────────────────────────

    /// List all models available from this provider.
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;

    // ── Chat ──────────────────────────────────────────────────────

    /// Send a chat request and wait for the complete response.
    async fn send_message(&self, request: ChatRequest) -> Result<ChatResponse>;

    /// Send a chat request and receive the response as a stream of chunks.
    ///
    /// Providers that don't support streaming should return
    /// [`ProviderError::NotSupported`].
    async fn send_message_stream(&self, request: ChatRequest) -> Result<TextStream>;

    // ── Image Generation ──────────────────────────────────────────

    /// Generate one or more images from a text prompt.
    async fn generate_image(
        &self,
        prompt: &str,
        options: ImageGenOptions,
    ) -> Result<Vec<GeneratedImage>>;

    // ── Video Generation ──────────────────────────────────────────

    /// Start a video generation task and stream back progress updates.
    ///
    /// The last [`VideoProgress`] in the stream will have
    /// [`VideoStatus::Completed`] (or [`VideoStatus::Failed`]) and, on
    /// success, a `video_url` pointing to the finished video.
    async fn generate_video(&self, request: VideoRequest) -> Result<VideoStream>;

    // ── File Upload ───────────────────────────────────────────────

    /// Upload a local file to the provider and return a handle for later use.
    async fn upload_file(&self, path: &Path) -> Result<UploadedFile>;

    // ── Session Management ────────────────────────────────────────

    /// Test the provider's connection / API key validity.
    async fn health_check(&self) -> Result<()>;

    // ── Credit Balance ───────────────────────────────────────────

    /// Fetch the account's credit balance, if the provider exposes one.
    ///
    /// Providers without a concept of credits return `None`.
    async fn account_balance(&self) -> Option<Result<CreditBalance>> {
        None
    }

    // ── Key Management ───────────────────────────────────────────

    /// Update the API key on a running provider so it takes effect
    /// immediately without restarting. No-op by default.
    fn set_api_key(&self, _key: &str) {}
}

// ─── Provider Errors ─────────────────────────────────────────────────────────

use thiserror::Error;

/// Errors specific to provider operations.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("Provider '{provider}' does not support '{capability}'")]
    NotSupported {
        provider: String,
        capability: String,
    },

    #[error("Authentication failed: {0}")]
    AuthError(String),

    #[error("API error {status}: {message}")]
    ApiError { status: u16, message: String },

    #[error("Rate limit exceeded. Retry after {retry_after_secs}s")]
    RateLimit { retry_after_secs: u64 },

    #[error("Request timed out")]
    Timeout,

    #[error("Streaming error: {0}")]
    StreamError(String),

    #[error("Video generation failed: {0}")]
    VideoFailed(String),

    #[error("File upload failed: {0}")]
    UploadFailed(String),

    #[error("Provider not configured. Please set an API key in Settings.")]
    NotConfigured,

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience macro to create a `ProviderError::NotSupported` error.
#[macro_export]
macro_rules! not_supported {
    ($provider:expr, $cap:expr) => {
        Err(crate::core::provider::ProviderError::NotSupported {
            provider: $provider.to_string(),
            capability: $cap.to_string(),
        }
        .into())
    };
}
