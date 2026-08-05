//! Provider registry and factory.
//!
//! All providers are registered here. The rest of the application uses
//! `ProviderRegistry` to look up providers by ID, never importing concrete types.

pub mod capabilities;
pub mod gemini;
pub mod groq;
pub mod manager;
pub mod ollama;
pub mod openrouter;
pub mod pixverse;

pub use capabilities::Capabilities;
pub use manager::{ProviderInfo, ProviderManager};

use std::collections::HashMap;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use keyring::Entry;

use crate::core::models::ModelInfo;
use crate::core::provider::AiProvider;

// ─── ProviderRegistry ─────────────────────────────────────────

/// Central registry of all available AI providers.
///
/// Providers are lazily instantiated when first accessed.
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn AiProvider>>,
}

impl ProviderRegistry {
    /// Create a new registry with all built-in providers pre-registered.
    ///
    /// API keys are loaded from the system keyring. Providers without a key
    /// are still registered but will return `ProviderError::NotConfigured`
    /// on API calls.
    pub fn new() -> Self {
        let mut registry = Self {
            providers: HashMap::new(),
        };
        registry.register_builtin_providers();
        registry
    }

    /// Register all built-in providers.
    fn register_builtin_providers(&mut self) {
        // Ollama — no API key needed
        let ollama_url = Self::load_setting("ollama_base_url")
            .unwrap_or_else(|_| "http://localhost:11434".to_string());
        self.providers.insert(
            "ollama".to_string(),
            Arc::new(ollama::OllamaProvider::new(ollama_url)),
        );

        // Gemini
        if let Ok(key) = Self::load_api_key("gemini") {
            self.providers.insert(
                "gemini".to_string(),
                Arc::new(gemini::GeminiProvider::new(key)),
            );
        } else {
            // Register with empty key — will fail on use
            self.providers.insert(
                "gemini".to_string(),
                Arc::new(gemini::GeminiProvider::new("")),
            );
        }

        // Groq
        if let Ok(key) = Self::load_api_key("groq") {
            self.providers.insert(
                "groq".to_string(),
                Arc::new(groq::GroqProvider::new(key)),
            );
        } else {
            self.providers.insert(
                "groq".to_string(),
                Arc::new(groq::GroqProvider::new("")),
            );
        }

        // OpenRouter
        if let Ok(key) = Self::load_api_key("openrouter") {
            self.providers.insert(
                "openrouter".to_string(),
                Arc::new(openrouter::OpenRouterProvider::new(key)),
            );
        } else {
            self.providers.insert(
                "openrouter".to_string(),
                Arc::new(openrouter::OpenRouterProvider::new("")),
            );
        }

        // PixVerse
        if let Ok(key) = Self::load_api_key("pixverse") {
            self.providers.insert(
                "pixverse".to_string(),
                Arc::new(pixverse::PixVerseProvider::new(key)),
            );
        } else {
            self.providers.insert(
                "pixverse".to_string(),
                Arc::new(pixverse::PixVerseProvider::new("")),
            );
        }
    }

    /// Get a provider by ID.
    pub fn get(&self, id: &str) -> Option<Arc<dyn AiProvider>> {
        self.providers.get(id).cloned()
    }

    /// List all registered providers.
    pub fn all(&self) -> Vec<Arc<dyn AiProvider>> {
        self.providers.values().cloned().collect()
    }

    /// List all provider IDs in a stable order.
    pub fn provider_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.providers.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Reload a provider with a new API key (after the user updates settings).
    pub fn reload_provider(&mut self, provider_id: &str) -> Result<()> {
        match provider_id {
            "gemini" => {
                let key = Self::load_api_key("gemini").unwrap_or_default();
                self.providers.insert(
                    "gemini".to_string(),
                    Arc::new(gemini::GeminiProvider::new(key)),
                );
            }
            "groq" => {
                let key = Self::load_api_key("groq").unwrap_or_default();
                self.providers.insert(
                    "groq".to_string(),
                    Arc::new(groq::GroqProvider::new(key)),
                );
            }
            "openrouter" => {
                let key = Self::load_api_key("openrouter").unwrap_or_default();
                self.providers.insert(
                    "openrouter".to_string(),
                    Arc::new(openrouter::OpenRouterProvider::new(key)),
                );
            }
            "pixverse" => {
                let key = Self::load_api_key("pixverse").unwrap_or_default();
                self.providers.insert(
                    "pixverse".to_string(),
                    Arc::new(pixverse::PixVerseProvider::new(key)),
                );
            }
            "ollama" => {
                let url = Self::load_setting("ollama_base_url")
                    .unwrap_or_else(|_| "http://localhost:11434".to_string());
                self.providers.insert(
                    "ollama".to_string(),
                    Arc::new(ollama::OllamaProvider::new(url)),
                );
            }
            _ => bail!("Unknown provider: {}", provider_id),
        }
        Ok(())
    }

    // ── Keyring helpers ───────────────────────────────────────────

    /// The keyring service name for the application.
    const SERVICE: &'static str = "io.github.daniel.aichat";

    /// Load an API key for a provider.
    ///
    /// The local fallback file is authoritative (it is written on every
    /// save), so keys survive restarts even if the system keyring is
    /// unavailable. The keyring is consulted for keys saved before the file
    /// store existed.
    pub fn load_api_key(provider_id: &str) -> Result<String, keyring::Error> {
        let from_file = load_keys_file()
            .get(provider_id)
            .cloned()
            .unwrap_or_default();
        if !from_file.is_empty() {
            return Ok(from_file);
        }
        let entry = Entry::new(Self::SERVICE, &format!("{}_api_key", provider_id))?;
        entry.get_password()
    }

    /// Save an API key for a provider.
    ///
    /// Always persists to the local fallback file first, then best-effort
    /// syncs to the system keyring.
    pub fn save_api_key(provider_id: &str, key: &str) -> Result<(), keyring::Error> {
        let mut keys = load_keys_file();
        keys.insert(provider_id.to_string(), key.to_string());
        save_keys_file(&keys);

        if let Err(e) = (|| -> Result<(), keyring::Error> {
            let entry = Entry::new(Self::SERVICE, &format!("{}_api_key", provider_id))?;
            entry.set_password(key)
        })() {
            tracing::warn!("Keyring save failed for {} (local copy kept): {}", provider_id, e);
        }
        Ok(())
    }

    /// Delete an API key for a provider from both the local file and keyring.
    pub fn delete_api_key(provider_id: &str) -> Result<(), keyring::Error> {
        let mut keys = load_keys_file();
        keys.remove(provider_id);
        save_keys_file(&keys);

        let result = (|| -> Result<(), keyring::Error> {
            let entry = Entry::new(Self::SERVICE, &format!("{}_api_key", provider_id))?;
            entry.delete_credential()
        })();
        match result {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Check whether an API key is configured for a provider.
    pub fn has_api_key(provider_id: &str) -> bool {
        Self::load_api_key(provider_id)
            .map(|k| !k.is_empty())
            .unwrap_or(false)
    }

    /// Load a non-secret setting from keyring (e.g., Ollama URL).
    fn load_setting(key: &str) -> Result<String, keyring::Error> {
        let entry = Entry::new(Self::SERVICE, key)?;
        entry.get_password()
    }

    /// Save a non-secret setting to keyring.
    pub fn save_setting(key: &str, value: &str) -> Result<(), keyring::Error> {
        let entry = Entry::new(Self::SERVICE, key)?;
        entry.set_password(value)
    }
}

/// Path to the local fallback API key store.
fn keys_file_path() -> Option<PathBuf> {
    let dir = dirs::config_dir()?.join("aichat");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("api_keys.json"))
}

/// Load the API keys from the local fallback file.
fn load_keys_file() -> HashMap<String, String> {
    let Some(path) = keys_file_path() else {
        return HashMap::new();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the API keys to the local fallback file with owner-only access.
fn save_keys_file(keys: &HashMap<String, String>) {
    let Some(path) = keys_file_path() else { return };
    if let Ok(json) = serde_json::to_string_pretty(keys) {
        if let Ok(mut f) = std::fs::File::create(&path) {
            let _ = f.write_all(json.as_bytes());
            let _ = f.flush();
        }
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_file_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("aichat-key-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &tmp);

        let keys = HashMap::from([("pixverse".to_string(), "test-key-123".to_string())]);
        save_keys_file(&keys);
        assert_eq!(
            load_keys_file().get("pixverse").map(String::as_str),
            Some("test-key-123")
        );

        let mut keys = load_keys_file();
        keys.remove("pixverse");
        save_keys_file(&keys);
        assert!(load_keys_file().get("pixverse").is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Curated model lists ─────────────────────────────────────

/// Return a curated list of well-known models for a provider.
///
/// Used by the UI as a fallback when the live model list cannot be fetched
/// (provider offline, no API key, network error). The list is provider
/// specific so the model selector never ends up empty.
pub fn curated_models(provider_id: &str) -> Vec<ModelInfo> {
    match provider_id {
        "ollama" => ollama::curated_models(),
        "gemini" => gemini::curated_models(),
        "groq" => groq::curated_models(),
        "openrouter" => openrouter::curated_models(),
        "pixverse" => pixverse::curated_models(),
        _ => Vec::new(),
    }
}
