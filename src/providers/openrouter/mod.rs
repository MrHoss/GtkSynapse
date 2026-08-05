//! OpenRouter provider module.
//!
//! OpenRouter is a gateway that exposes hundreds of models (OpenAI, Anthropic,
//! Google, Meta, Mistral, …) through a single OpenAI-compatible API.

pub mod client;
pub use client::{curated_models, OpenRouterProvider};
