//! GtkSynapse Library - Core, Storage, Providers, Widgets, and Application UI modules.

pub mod core;
pub mod providers;
pub mod storage;
pub mod widgets;
pub mod app;
pub mod ollama;

use std::sync::Arc;
use libadwaita as adw;
use libadwaita::prelude::*;
use gtk4 as gtk;

use crate::storage::StorageManager;
use crate::providers::ProviderManager;
use crate::app::MainWindow;

pub fn run_app() -> gtk::glib::ExitCode {
    // Initialize GResources
    gio::resources_register_include!("gtksynapse.gresource")
        .expect("Failed to register GResources");

    let app = adw::Application::builder()
        .application_id("io.github.daniel.gtksynapse")
        .flags(gio::ApplicationFlags::FLAGS_NONE)
        .build();

    app.connect_activate(|app| {
        // Load custom styles
        let provider = gtk::CssProvider::new();
        provider.load_from_resource("/io/github/daniel/gtksynapse/style.css");
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        // Initialize storage & providers
        let storage = Arc::new(StorageManager::open().expect("Failed to open database"));

        // The default provider for new chats comes from persisted settings.
        let default_provider = storage
            .load_settings()
            .ok()
            .map(|s| s.default_provider_id);
        let manager = Arc::new(ProviderManager::new(default_provider));

        // Create (and keep alive for the app's lifetime) a Tokio runtime.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to start Tokio runtime");
        let runtime_handle = runtime.handle().clone();
        std::mem::forget(runtime);

        let main_win = MainWindow::new(app, storage, manager, runtime_handle);
        main_win.window.present();
    });

    app.run()
}

use gtk4::gio;
