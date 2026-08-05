//! Settings dialog — AdwPreferencesDialog with provider configuration.

use gtk4::prelude::*;
use gtk4::{self as gtk, glib};
use libadwaita::prelude::*;
use libadwaita as adw;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::providers::manager::ProviderManager;
use crate::providers::ProviderRegistry;
use crate::storage::StorageManager;

/// Show the settings preferences dialog.
pub fn show_settings_dialog(
    parent: &adw::ApplicationWindow,
    storage: Arc<StorageManager>,
    manager: Arc<ProviderManager>,
    runtime: tokio::runtime::Handle,
) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Settings");
    dialog.set_search_enabled(true);

    // ── General Page ─────────────────────────────────────────────
    let general_page = adw::PreferencesPage::new();
    general_page.set_title("General");
    general_page.set_icon_name(Some("preferences-system-symbolic"));

    // Interface group
    let interface_group = adw::PreferencesGroup::new();
    interface_group.set_title("Interface");
    interface_group.set_description(Some("Customize the look and feel"));

    let theme_row = adw::ComboRow::new();
    theme_row.set_title("Color Scheme");
    theme_row.set_subtitle("Choose light, dark, or follow system");
    let theme_model = gtk::StringList::new(&["Follow System", "Light", "Dark"]);
    theme_row.set_model(Some(&theme_model));
    interface_group.add(&theme_row);

    let lang_row = adw::EntryRow::new();
    lang_row.set_title("Language");
    lang_row.set_text("en");
    interface_group.add(&lang_row);

    general_page.add(&interface_group);

    // Chat behavior group
    let chat_group = adw::PreferencesGroup::new();
    chat_group.set_title("Chat Behavior");

    let stream_row = adw::SwitchRow::new();
    stream_row.set_title("Enable Streaming");
    stream_row.set_subtitle("Show responses word-by-word as they are generated");
    stream_row.set_active(true);
    chat_group.add(&stream_row);

    let ctx_row = adw::SpinRow::with_range(1.0, 100.0, 1.0);
    ctx_row.set_title("Max Context Messages");
    ctx_row.set_subtitle("Number of past messages sent to the AI");
    ctx_row.set_value(20.0);
    chat_group.add(&ctx_row);

    let timeout_row = adw::SpinRow::with_range(10.0, 600.0, 10.0);
    timeout_row.set_title("Request Timeout (seconds)");
    timeout_row.set_value(120.0);
    chat_group.add(&timeout_row);

    general_page.add(&chat_group);

    // Downloads group
    let dl_group = adw::PreferencesGroup::new();
    dl_group.set_title("Downloads");

    let dl_row = adw::ActionRow::new();
    dl_row.set_title("Download Folder");
    dl_row.set_subtitle(
        dirs::download_dir()
            .map(|p| p.to_string_lossy().to_string())
            .as_deref()
            .unwrap_or("~/Downloads"),
    );
    let dl_btn = gtk::Button::with_label("Browse…");
    dl_btn.set_valign(gtk::Align::Center);
    dl_row.add_suffix(&dl_btn);
    dl_group.add(&dl_row);

    general_page.add(&dl_group);
    dialog.add(&general_page);

    // ── Providers Page ───────────────────────────────────────────
    let providers_page = adw::PreferencesPage::new();
    providers_page.set_title("Providers");
    providers_page.set_icon_name(Some("network-server-symbolic"));

    // Ollama group
    let ollama_group = adw::PreferencesGroup::new();
    ollama_group.set_title("Ollama (Local)");
    ollama_group.set_description(Some("Run models locally — no API key needed"));

    let ollama_url_row = adw::EntryRow::new();
    ollama_url_row.set_title("Base URL");
    ollama_url_row.set_text("http://localhost:11434");
    ollama_group.add(&ollama_url_row);

    let ollama_test_row = adw::ActionRow::new();
    ollama_test_row.set_title("Connection");
    let ollama_test_btn = gtk::Button::with_label("Test");
    ollama_test_btn.set_valign(gtk::Align::Center);
    let ollama_status = gtk::Label::new(Some("Not tested"));
    ollama_status.add_css_class("provider-status-disconnected");
    ollama_status.set_margin_end(8);
    ollama_test_row.add_suffix(&ollama_status);
    ollama_test_row.add_suffix(&ollama_test_btn);
    ollama_group.add(&ollama_test_row);

    {
        let status_label = ollama_status.clone();
        let url_entry = ollama_url_row.clone();
        let runtime = runtime.clone();
        ollama_test_btn.connect_clicked(move |_| {
            status_label.set_label("Testing…");
            status_label.remove_css_class("provider-status-error");
            status_label.remove_css_class("provider-status-connected");
            status_label.add_css_class("provider-status-disconnected");

            // Run the HTTP probe on the Tokio runtime (reqwest requires a
            // reactor) and push the result back to the main thread.
            let url = url_entry.text().to_string();
            let sl = status_label.clone();
            let rt = runtime.clone();
            let (tx, mut rx) = mpsc::channel::<bool>(1);
            rt.spawn(async move {
                let client = reqwest::Client::new();
                let ok = matches!(
                    client.get(&format!("{}/api/tags", url)).send().await,
                    Ok(r) if r.status().is_success()
                );
                let _ = tx.send(ok).await;
            });
            glib::timeout_add_local(
                std::time::Duration::from_millis(15),
                move || match rx.try_recv() {
                    Ok(ok) => {
                        if ok {
                            sl.set_label("Connected ✓");
                            sl.remove_css_class("provider-status-disconnected");
                            sl.add_css_class("provider-status-connected");
                        } else {
                            sl.set_label("Failed ✗");
                            sl.remove_css_class("provider-status-disconnected");
                            sl.add_css_class("provider-status-error");
                        }
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::error::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        sl.set_label("Failed ✗");
                        sl.remove_css_class("provider-status-disconnected");
                        sl.add_css_class("provider-status-error");
                        glib::ControlFlow::Break
                    }
                },
            );
        });
    }

    providers_page.add(&ollama_group);

    // Gemini group
    let gemini_group = make_api_key_group(
        "Google Gemini",
        "Chat, vision, and image generation",
        "gemini",
        "Get an API key at aistudio.google.com",
        manager.clone(),
        runtime.clone(),
    );
    providers_page.add(&gemini_group);

    // Groq group
    let groq_group = make_api_key_group(
        "Groq",
        "Ultra-fast LLM inference",
        "groq",
        "Get an API key at console.groq.com",
        manager.clone(),
        runtime.clone(),
    );
    providers_page.add(&groq_group);

    // OpenRouter group
    let openrouter_group = make_api_key_group(
        "OpenRouter",
        "One API key for 400+ models (OpenAI, Anthropic, Google, Meta, …)",
        "openrouter",
        "Get an API key at openrouter.ai/keys",
        manager.clone(),
        runtime.clone(),
    );
    providers_page.add(&openrouter_group);

    // PixVerse group
    let pixverse_group = make_api_key_group(
        "PixVerse",
        "AI video generation (text-to-video, image-to-video)",
        "pixverse",
        "Get an API key at app.pixverse.ai",
        manager.clone(),
        runtime.clone(),
    );
    providers_page.add(&pixverse_group);

    dialog.add(&providers_page);

    // ── Advanced Page ────────────────────────────────────────────
    let advanced_page = adw::PreferencesPage::new();
    advanced_page.set_title("Advanced");
    advanced_page.set_icon_name(Some("preferences-other-symbolic"));

    let proxy_group = adw::PreferencesGroup::new();
    proxy_group.set_title("Network");

    let proxy_row = adw::EntryRow::new();
    proxy_row.set_title("HTTP Proxy");
    proxy_row.set_show_apply_button(true);
    proxy_group.add(&proxy_row);

    advanced_page.add(&proxy_group);

    let logging_group = adw::PreferencesGroup::new();
    logging_group.set_title("Developer");

    let logging_row = adw::SwitchRow::new();
    logging_row.set_title("Enable Debug Logging");
    logging_row.set_subtitle("Writes detailed logs to stderr");
    logging_group.add(&logging_row);

    advanced_page.add(&logging_group);
    dialog.add(&advanced_page);

    dialog.present(Some(parent));
}

/// Helper to create an API key preferences group for a provider.
fn make_api_key_group(
    title: &str,
    description: &str,
    provider_id: &str,
    hint: &str,
    manager: Arc<ProviderManager>,
    runtime: tokio::runtime::Handle,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    group.set_title(title);
    group.set_description(Some(description));

    let key_row = adw::PasswordEntryRow::new();
    key_row.set_title("API Key");
    key_row.add_css_class("settings-api-key-row");
    key_row.set_tooltip_text(Some(hint));

    // Pre-fill if key exists
    if let Ok(key) = ProviderRegistry::load_api_key(provider_id) {
        key_row.set_text(&key);
    }

    group.add(&key_row);

    // Save button
    let actions_row = adw::ActionRow::new();
    actions_row.set_title("Status");

    let status_label = gtk::Label::new(Some(if ProviderRegistry::has_api_key(provider_id) {
        "Configured ✓"
    } else {
        "Not configured"
    }));
    if ProviderRegistry::has_api_key(provider_id) {
        status_label.add_css_class("provider-status-connected");
    } else {
        status_label.add_css_class("provider-status-disconnected");
    }
    status_label.set_margin_end(8);
    actions_row.add_suffix(&status_label);

    let save_btn = gtk::Button::with_label("Save Key");
    save_btn.set_valign(gtk::Align::Center);
    save_btn.add_css_class("suggested-action");
    actions_row.add_suffix(&save_btn);

    let test_btn = gtk::Button::with_label("Test");
    test_btn.set_valign(gtk::Align::Center);
    actions_row.add_suffix(&test_btn);

    let pid = provider_id.to_string();
    let pid_test = pid.clone();
    let key_entry = key_row.clone();
    let sl = status_label.clone();
    let sl_test = status_label.clone();
    let manager = manager.clone();
    let manager_test = manager.clone();
    save_btn.connect_clicked(move |_| {
        let key = key_entry.text().trim().to_string();

        // Apply the key to the running provider immediately, so it takes
        // effect even if persisting to the keyring fails.
        if let Some(p) = manager.get(&pid) {
            p.set_api_key(&key);
        }

        if key.is_empty() {
            let _ = ProviderRegistry::delete_api_key(&pid);
            sl.set_label("Not configured");
            sl.remove_css_class("provider-status-connected");
            sl.add_css_class("provider-status-disconnected");
        } else {
            match ProviderRegistry::save_api_key(&pid, &key) {
                Ok(_) => {
                    // Reload from keyring so providers without a live key
                    // update (gemini/groq/openrouter) pick up the new value too.
                    let _ = manager.reload_provider(&pid);
                    sl.set_label("Saved ✓");
                    sl.remove_css_class("provider-status-disconnected");
                    sl.add_css_class("provider-status-connected");
                }
                Err(e) => {
                    // In-memory key still works for this session.
                    sl.set_label("Saved (keyring failed)");
                    sl.remove_css_class("provider-status-disconnected");
                    sl.add_css_class("provider-status-error");
                    tracing::error!("Failed to persist API key for {}: {}", pid, e);
                }
            }
        }
    });

    test_btn.connect_clicked(move |_| {
        let sl = sl_test.clone();
        sl.set_label("Testing…");
        sl.remove_css_class("provider-status-connected");
        sl.remove_css_class("provider-status-error");
        sl.add_css_class("provider-status-disconnected");

        // Clone the provider on the main thread so the manager lock is not
        // held across the network call (and cannot poison the mutex).
        let provider = match manager_test.get(&pid_test) {
            Some(p) => p.clone(),
            None => {
                sl.set_label("Failed ✗ Provider not registered");
                sl.remove_css_class("provider-status-disconnected");
                sl.add_css_class("provider-status-error");
                return;
            }
        };

        let rt = runtime.clone();
        let (tx, mut rx) = mpsc::channel::<Result<(), String>>(1);
        rt.spawn(async move {
            let res = match provider.health_check().await {
                Ok(_) => Ok(()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(res).await;
        });
        glib::timeout_add_local(
            std::time::Duration::from_millis(15),
            move || match rx.try_recv() {
                Ok(Ok(())) => {
                    sl.set_label("Connected ✓");
                    sl.remove_css_class("provider-status-disconnected");
                    sl.add_css_class("provider-status-connected");
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    sl.set_label(&format!("Failed ✗ {}", e));
                    sl.remove_css_class("provider-status-disconnected");
                    sl.add_css_class("provider-status-error");
                    glib::ControlFlow::Break
                }
                Err(mpsc::error::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    sl.set_label("Failed ✗ No response");
                    sl.remove_css_class("provider-status-disconnected");
                    sl.add_css_class("provider-status-error");
                    glib::ControlFlow::Break
                }
            },
        );
    });

    actions_row.add_suffix(&save_btn);
    group.add(&actions_row);

    group
}
