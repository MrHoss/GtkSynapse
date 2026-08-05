//! Core application layer — models, provider trait, chat, history, attachments.

pub mod attachment;
pub mod chat;
pub mod history;
pub mod models;
pub mod provider;

pub use models::*;
pub use provider::{AiProvider, ProviderError, TextStream, VideoStream};
