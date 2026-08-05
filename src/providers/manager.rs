//! `ProviderManager` — the single entry point the UI uses to talk to
//! AI providers.
//!
//! It wraps a [`ProviderRegistry`](super::ProviderRegistry) (which owns the
//! provider instances) and adds the notion of an *active* provider. All
//! provider access goes through this manager, so the UI never depends on
//! concrete provider types.

use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};

use crate::core::models::ModelInfo;
use crate::core::provider::AiProvider;

use super::capabilities::Capabilities;
use super::ProviderRegistry;

/// A lightweight, UI-facing description of a registered provider.
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    /// Machine-readable provider ID (e.g. "ollama", "openrouter").
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Capabilities the provider supports.
    pub capabilities: Capabilities,
}

/// Central provider manager.
///
/// Thread-safe: all methods take `&self` and lock internally, so it can be
/// shared behind an `Arc` without the caller worrying about lock ordering.
pub struct ProviderManager {
    registry: Mutex<ProviderRegistry>,
    active_provider: Mutex<Option<String>>,
}

impl ProviderManager {
    /// Create a manager with all built-in providers registered.
    ///
    /// `default_provider` selects the initially active provider; if it is not
    /// registered (or `None`), the first chat-capable provider is used.
    pub fn new(default_provider: Option<String>) -> Self {
        let registry = ProviderRegistry::new();

        let mut active = default_provider.filter(|id| registry.get(id).is_some());
        if active.is_none() {
            active = registry
                .all()
                .into_iter()
                .find(|p| p.supports(Capabilities::CHAT))
                .map(|p| p.id().to_string());
        }

        Self {
            registry: Mutex::new(registry),
            active_provider: Mutex::new(active),
        }
    }

    /// Get a provider by ID.
    pub fn get(&self, id: &str) -> Option<Arc<dyn AiProvider>> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
    }

    /// All registered providers.
    pub fn all(&self) -> Vec<Arc<dyn AiProvider>> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .all()
    }

    /// All provider IDs in stable order.
    pub fn provider_ids(&self) -> Vec<String> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .provider_ids()
    }

    /// UI-facing descriptions of all registered providers.
    pub fn providers(&self) -> Vec<ProviderInfo> {
        self.all()
            .into_iter()
            .map(|p| ProviderInfo {
                id: p.id().to_string(),
                name: p.name().to_string(),
                capabilities: p.capabilities(),
            })
            .collect()
    }

    /// UI-facing descriptions of the providers that can run chat sessions.
    pub fn chat_providers(&self) -> Vec<ProviderInfo> {
        self.all()
            .into_iter()
            .filter(|p| p.supports(Capabilities::CHAT))
            .map(|p| ProviderInfo {
                id: p.id().to_string(),
                name: p.name().to_string(),
                capabilities: p.capabilities(),
            })
            .collect()
    }

    /// The currently active provider ID, if any.
    pub fn active_provider_id(&self) -> Option<String> {
        self.active_provider
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The currently active provider, if any.
    pub fn active_provider(&self) -> Option<Arc<dyn AiProvider>> {
        let pid = self.active_provider_id()?;
        self.get(&pid)
    }

    /// Switch the active provider.
    pub fn set_active_provider(&self, id: &str) -> Result<()> {
        let registered = self
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .is_some();
        if !registered {
            bail!("Unknown provider: {}", id);
        }
        *self.active_provider.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.to_string());
        Ok(())
    }

    /// Reload a provider (e.g. after the user changed its API key in
    /// settings) so the new configuration takes effect immediately.
    pub fn reload_provider(&self, id: &str) -> Result<()> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reload_provider(id)
    }

    /// List the models available from a provider.
    ///
    /// Providers that fail (not reachable, bad key) return an error; callers
    /// are expected to fall back to [`super::curated_models`] for display.
    pub async fn list_models(&self, provider_id: &str) -> Result<Vec<ModelInfo>> {
        let Some(provider) = self.get(provider_id) else {
            bail!("Unknown provider: {}", provider_id);
        };
        provider.list_models().await
    }
}
