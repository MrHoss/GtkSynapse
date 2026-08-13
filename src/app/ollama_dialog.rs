//! Ollama management UI — first-run install prompt and a manager dialog
//! (install, start/stop the server, pull new models) with a live console.

use gtk4::prelude::*;
use gtk4::{self as gtk, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

use crate::app::downloads::DownloadsIndicator;
use crate::app::input_bar::InputBar;
use crate::app::window::load_models_for_provider;
use crate::ollama::{self, CliEvent};
use crate::providers::manager::ProviderManager;

// ─── Shared helpers ──────────────────────────────────────────────

/// Reload the app's model dropdown if the Ollama provider is selected.
fn reload_app_models(
    manager: &Arc<ProviderManager>,
    input_bar: &Arc<Mutex<InputBar>>,
    runtime: &tokio::runtime::Handle,
) {
    let is_ollama = input_bar
        .lock()
        .unwrap()
        .selected_provider_id()
        .as_deref()
        == Some("ollama");
    if is_ollama {
        load_models_for_provider(input_bar, manager, runtime, None);
    }
}

/// Start the Ollama server in the background, then toast the result.
fn start_server_async(
    overlay: &adw::ToastOverlay,
    manager: &Arc<ProviderManager>,
    input_bar: &Arc<Mutex<InputBar>>,
    runtime: &tokio::runtime::Handle,
) {
    let overlay = overlay.clone();
    let manager = manager.clone();
    let input_bar = input_bar.clone();
    let runtime = runtime.clone();
    let (tx, mut rx) = mpsc::channel::<Result<bool, String>>(1);
    runtime.spawn(async move {
        let _ = tx
            .send(ollama::ensure_server_running().await.map_err(|e| e.to_string()))
            .await;
    });
    glib::timeout_add_local(Duration::from_millis(15), move || {
        match rx.try_recv() {
            Ok(Ok(_)) => {
                overlay.add_toast(adw::Toast::new("Local Ollama server is running"));
                reload_app_models(&manager, &input_bar, &runtime);
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                overlay.add_toast(adw::Toast::new(&format!(
                    "Could not start the Ollama server: {}",
                    e
                )));
                glib::ControlFlow::Break
            }
            Err(mpsc::error::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::error::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

/// A modal console that streams CLI output until the operation finishes.
fn show_op_console<F, Fut>(
    parent: &adw::ApplicationWindow,
    runtime: &tokio::runtime::Handle,
    title: &str,
    running_text: &str,
    task: F,
    on_done: Option<Arc<dyn Fn()>>,
) where
    F: FnOnce(mpsc::Sender<CliEvent>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let dialog = adw::AlertDialog::new(Some(title), Some(running_text));

    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 6);
    box_.set_size_request(460, 240);

    let text_view = gtk::TextView::new();
    text_view.set_editable(false);
    text_view.set_cursor_visible(false);
    text_view.set_monospace(true);
    text_view.set_wrap_mode(gtk::WrapMode::WordChar);
    let buffer = text_view.buffer();

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scrolled.set_vexpand(true);
    scrolled.set_child(Some(&text_view));

    let tip = gtk::Label::new(Some(
        "This can take a few minutes. If the installer asks for your password \
         and cannot prompt you in the terminal, the app will show an error \
         and you can run the command manually afterwards.",
    ));
    tip.add_css_class("dim-label");
    tip.set_wrap(true);

    box_.append(&tip);
    box_.append(&scrolled);
    dialog.set_extra_child(Some(&box_));

    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.present(Some(parent));

    let on_done: Arc<dyn Fn()> = match on_done {
        Some(cb) => cb,
        None => Arc::new(|| {}),
    };

    let (tx, mut rx) = mpsc::channel::<CliEvent>(64);
    let rt = runtime.clone();
    rt.spawn(async move {
        let future = task(tx);
        future.await;
    });

    let dialog_c = dialog.clone();
    let text_view_c = text_view.clone();
    glib::timeout_add_local(Duration::from_millis(15), move || {
        match rx.try_recv() {
            Ok(CliEvent::Output(line)) => {
                buffer.insert_at_cursor(&format!("{}\n", line));
                let mut buffer_end = buffer.end_iter();
                text_view_c.scroll_to_iter(&mut buffer_end, 80.0, true, 0.0, 1.0);
                glib::ControlFlow::Continue
            }
            Ok(CliEvent::Success) => {
                dialog_c.set_body("Finished.");
                on_done();
                glib::ControlFlow::Break
            }
            Ok(CliEvent::Error(e)) => {
                dialog_c.set_body(&e);
                on_done();
                glib::ControlFlow::Break
            }
            Err(mpsc::error::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                dialog_c.set_body("Operation interrupted.");
                glib::ControlFlow::Break
            }
        }
    });
}

/// Kick off the Ollama installer in a console dialog, then start the server.
fn run_install(
    window: &adw::ApplicationWindow,
    overlay: &adw::ToastOverlay,
    manager: Arc<ProviderManager>,
    input_bar: Arc<Mutex<InputBar>>,
    runtime: &tokio::runtime::Handle,
    after_install: Option<Arc<dyn Fn()>>,
) {
    let overlay = overlay.clone();
    let runtime_owned = runtime.clone();
    let done = Arc::new(move || {
        overlay.add_toast(adw::Toast::new("Ollama installed"));
        start_server_async(&overlay, &manager, &input_bar, &runtime_owned);
        if let Some(cb) = &after_install {
            cb();
        }
    });
    show_op_console(
        window,
        runtime,
        "Install Ollama",
        "Downloading and installing Ollama…",
        |tx| async move {
            ollama::install_ollama(tx).await;
        },
        Some(done),
    );
}

// ─── First-run install prompt ────────────────────────────────────

/// Ask the user whether they want Ollama installed, then run the installer
/// (and start the server) if they accept.
pub fn prompt_install_ollama(
    window: &adw::ApplicationWindow,
    manager: Arc<ProviderManager>,
    input_bar: Arc<Mutex<InputBar>>,
    overlay: &adw::ToastOverlay,
    runtime: tokio::runtime::Handle,
) {
    let body = format!(
        "To run AI models locally — free and with no API key — this app uses \
         Ollama, which is not installed on this system ({}).\n\n\
         Install it now so you can chat with local models?",
        ollama::os_description()
    );
    let dialog = adw::AlertDialog::new(Some("Install Ollama?"), Some(&body));

    dialog.add_response("later", "Not now");
    dialog.add_response("install", "Install");
    dialog.set_response_appearance("install", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("install"));

    let win = window.clone();
    let overlay = overlay.clone();
    let manager = manager.clone();
    let input_bar = input_bar.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "install" {
            run_install(
                &win,
                &overlay,
                manager.clone(),
                input_bar.clone(),
                &runtime,
                None,
            );
        }
    });
    dialog.present(Some(window));
}

// ─── Manager dialog ──────────────────────────────────────────────

/// Rebuild the installed-models list.
///
/// Rows are cleared by removing the exact widget references we passed to
/// `group.add`. `AdwPreferencesGroup` is a plain `GtkWidget` that wraps each
/// added child in its own internal row, so `first_child`/`next_sibling`
/// traversal and `GtkListBox` downcasts do not reliably reflect the contents;
/// keeping references to what we added is the robust way to clear it.
fn populate_models(
    group: &adw::PreferencesGroup,
    model_rows: &Arc<Mutex<Vec<gtk::Widget>>>,
    models: Vec<ollama::InstalledModel>,
) {
    for widget in model_rows.lock().unwrap().drain(..) {
        group.remove(&widget);
    }

    if models.is_empty() {
        let row = adw::ActionRow::new();
        row.set_title("No local models");
        row.set_subtitle("Download a model below to get started.");
        let widget: gtk::Widget = row.clone().upcast();
        group.add(&widget);
        model_rows.lock().unwrap().push(widget);
        return;
    }

    for model in models {
        let row = adw::ActionRow::new();
        row.set_title(&model.name);
        let mut subtitle = String::new();
        if let Some(ps) = &model.parameter_size {
            subtitle.push_str(ps);
            subtitle.push_str(" · ");
        }
        subtitle.push_str(&model.size_human);
        row.set_subtitle(&subtitle);
        let widget: gtk::Widget = row.clone().upcast();
        group.add(&widget);
        model_rows.lock().unwrap().push(widget);
    }
}

/// Full manager: install, start/stop the server, download and list models.
pub fn show_ollama_manager(
    parent: &adw::ApplicationWindow,
    overlay: &adw::ToastOverlay,
    manager: Arc<ProviderManager>,
    input_bar: Arc<Mutex<InputBar>>,
    runtime: tokio::runtime::Handle,
    downloads: DownloadsIndicator,
) {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Ollama Management");
    dialog.set_search_enabled(true);

    // ── Server page ──────────────────────────────────────────────
    let server_page = adw::PreferencesPage::new();
    server_page.set_title("Server");
    server_page.set_icon_name(Some("network-server-symbolic"));

    let status_group = adw::PreferencesGroup::new();
    status_group.set_title("Local Server");
    status_group.set_description(Some(&ollama::os_description()));

    let install_row = adw::ActionRow::new();
    install_row.set_title("Install Ollama");
    install_row.set_subtitle(
        "Ollama is not installed. The official installer for your system will be used.",
    );
    let install_btn = gtk::Button::with_label("Install");
    install_btn.set_valign(gtk::Align::Center);
    install_btn.add_css_class("suggested-action");
    install_row.add_suffix(&install_btn);

    let server_row = adw::ActionRow::new();
    server_row.set_title("Server");
    let server_label = gtk::Label::new(Some("Checking…"));
    server_label.set_margin_end(8);
    server_row.add_suffix(&server_label);
    let stop_btn = gtk::Button::with_label("Stop");
    stop_btn.set_valign(gtk::Align::Center);
    server_row.add_suffix(&stop_btn);
    let start_btn = gtk::Button::with_label("Start");
    start_btn.set_valign(gtk::Align::Center);
    server_row.add_suffix(&start_btn);

    status_group.add(&install_row);
    status_group.add(&server_row);
    server_page.add(&status_group);
    dialog.add(&server_page);

    // ── Models page ──────────────────────────────────────────────
    let models_page = adw::PreferencesPage::new();
    models_page.set_title("Models");
    models_page.set_icon_name(Some("folder-download-symbolic"));

    let installed_group = adw::PreferencesGroup::new();
    installed_group.set_title("Installed Models");
    installed_group.set_description(Some("Models stored on this machine."));

    let refresh_row = adw::ActionRow::new();
    refresh_row.set_title("Refresh");
    let refresh_models_btn = gtk::Button::with_label("Refresh");
    refresh_models_btn.set_valign(gtk::Align::Center);
    refresh_row.add_suffix(&refresh_models_btn);
    installed_group.add(&refresh_row);

    let model_rows: Arc<Mutex<Vec<gtk::Widget>>> = Arc::new(Mutex::new(Vec::new()));
    let models_list = adw::PreferencesGroup::new();

    let download_group = adw::PreferencesGroup::new();
    download_group.set_title("Download a Model");
    download_group.set_description(Some(
        "Pick a popular model or type any tag, then pull it locally. \
         Larger models give better answers but use more disk and RAM.",
    ));

    let catalog = gtk::DropDown::from_strings(ollama::CATALOG);
    let catalog_row = adw::ActionRow::new();
    catalog_row.set_title("Preset");
    catalog_row.add_suffix(&catalog);

    let model_entry = adw::EntryRow::new();
    model_entry.set_title("Model tag");
    model_entry.set_text(ollama::CATALOG[0]);
    model_entry.set_show_apply_button(true);

    let download_btn = gtk::Button::with_label("Download");
    download_btn.add_css_class("suggested-action");
    let download_row = adw::ActionRow::new();
    download_row.set_title("Status");
    let download_status = gtk::Label::new(Some("Idle"));
    download_status.add_css_class("provider-status-disconnected");
    download_status.set_margin_end(8);
    download_row.add_suffix(&download_status);
    download_row.add_suffix(&download_btn);

    download_group.add(&catalog_row);
    download_group.add(&model_entry);
    download_group.add(&download_row);

    models_page.add(&installed_group);
    models_page.add(&models_list);
    models_page.add(&download_group);
    dialog.add(&models_page);

    dialog.present(Some(parent));

    // ── Shared status refresh ────────────────────────────────────
    // A guard prevents overlapping refreshes (e.g. fast clicks on Refresh)
    // from stacking up and appending the model list more than once.
    let refreshing = Arc::new(Mutex::new(false));
    let refresh_all: Arc<dyn Fn()> = {
        let server_label = server_label.clone();
        let install_row = install_row.clone();
        let models_list = models_list.clone();
        let model_rows = model_rows.clone();
        let runtime = runtime.clone();
        let refreshing = refreshing.clone();
        Arc::new(move || {
            {
                let mut guard = refreshing.lock().unwrap();
                if *guard {
                    return;
                }
                *guard = true;
            }
            let server_label = server_label.clone();
            let install_row = install_row.clone();
            let models_list = models_list.clone();
            let model_rows = model_rows.clone();
            let runtime = runtime.clone();
            let refreshing = refreshing.clone();

            let (tx, mut rx) = mpsc::channel::<ollama::OllamaStatus>(1);
            runtime.clone().spawn(async move {
                let _ = tx.send(ollama::status().await).await;
            });

            glib::timeout_add_local(Duration::from_millis(15), move || {
                match rx.try_recv() {
                    Ok(st) => {
                        if st.installed {
                            install_row.set_visible(false);
                            if st.server_running {
                                server_label.set_text("Running ✓");
                                server_label.remove_css_class("provider-status-error");
                                server_label.add_css_class("provider-status-connected");
                            } else {
                                server_label.set_text("Stopped ✗");
                                server_label.remove_css_class("provider-status-connected");
                                server_label.add_css_class("provider-status-error");
                            }
                        } else {
                            install_row.set_visible(true);
                            server_label.set_text("Not installed");
                            server_label.remove_css_class("provider-status-connected");
                            server_label.add_css_class("provider-status-error");
                        }

                        // Then refresh the installed-models list.
                        let models_list = models_list.clone();
                        let model_rows = model_rows.clone();
                        let runtime = runtime.clone();
                        let refreshing = refreshing.clone();
                        let (tx2, mut rx2) =
                            mpsc::channel::<Result<Vec<ollama::InstalledModel>, String>>(1);
                        runtime.spawn(async move {
                            let _ = tx2
                                .send(
                                    ollama::list_installed_models()
                                        .await
                                        .map_err(|e| e.to_string()),
                                )
                                .await;
                        });
                        glib::timeout_add_local(Duration::from_millis(15), move || {
                            match rx2.try_recv() {
                                Ok(Ok(models)) => {
                                    populate_models(&models_list, &model_rows, models);
                                    *refreshing.lock().unwrap() = false;
                                    glib::ControlFlow::Break
                                }
                                Ok(Err(e)) => {
                                    populate_models(&models_list, &model_rows, Vec::new());
                                    let row = adw::ActionRow::new();
                                    row.set_title("Server unreachable");
                                    row.set_subtitle(&e);
                                    let widget: gtk::Widget = row.clone().upcast();
                                    models_list.add(&widget);
                                    model_rows.lock().unwrap().push(widget);
                                    *refreshing.lock().unwrap() = false;
                                    glib::ControlFlow::Break
                                }
                                Err(mpsc::error::TryRecvError::Empty) => {
                                    glib::ControlFlow::Continue
                                }
                                Err(mpsc::error::TryRecvError::Disconnected) => {
                                    *refreshing.lock().unwrap() = false;
                                    glib::ControlFlow::Break
                                }
                            }
                        });
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::error::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        *refreshing.lock().unwrap() = false;
                        glib::ControlFlow::Break
                    }
                }
            });
        })
    };

    // ── Install ──────────────────────────────────────────────────
    install_btn.connect_clicked({
        let parent = parent.clone();
        let overlay = overlay.clone();
        let manager = manager.clone();
        let input_bar = input_bar.clone();
        let runtime = runtime.clone();
        let refresh_all = refresh_all.clone();
        move |_| {
            let refresh_all = refresh_all.clone();
            run_install(
                &parent,
                &overlay,
                manager.clone(),
                input_bar.clone(),
                &runtime,
                Some(refresh_all),
            );
        }
    });

    // ── Start ────────────────────────────────────────────────────
    start_btn.connect_clicked({
        let overlay = overlay.clone();
        let manager = manager.clone();
        let input_bar = input_bar.clone();
        let runtime = runtime.clone();
        let server_label = server_label.clone();
        let refresh_all = refresh_all.clone();
        move |_| {
            server_label.set_text("Starting…");
            server_label.remove_css_class("provider-status-error");
            server_label.remove_css_class("provider-status-connected");
            server_label.add_css_class("provider-status-disconnected");
            start_server_async(&overlay, &manager, &input_bar, &runtime);
            let refresh_all = refresh_all.clone();
            glib::timeout_add_local(Duration::from_millis(2500), move || {
                refresh_all();
                glib::ControlFlow::Break
            });
        }
    });

    // ── Stop ─────────────────────────────────────────────────────
    stop_btn.connect_clicked({
        let runtime = runtime.clone();
        let server_label = server_label.clone();
        let refresh_all = refresh_all.clone();
        move |_| {
            server_label.set_text("Stopping…");
            server_label.remove_css_class("provider-status-error");
            server_label.add_css_class("provider-status-disconnected");
            runtime.spawn(async move {
                let _ = ollama::stop_server().await;
            });
            let refresh_all = refresh_all.clone();
            glib::timeout_add_local(Duration::from_millis(1200), move || {
                refresh_all();
                glib::ControlFlow::Break
            });
        }
    });

    // ── Catalog selection fills the tag entry ────────────────────
    {
        let catalog = catalog.clone();
        let model_entry = model_entry.clone();
        let catalog_for_close = catalog.clone();
        catalog.connect_selected_notify(move |_| {
            let idx = catalog_for_close.selected() as usize;
            if let Some(name) = ollama::CATALOG.get(idx) {
                model_entry.set_text(name);
            }
        });
    }

    // ── Download model ───────────────────────────────────────────
    download_btn.connect_clicked({
        let overlay = overlay.clone();
        let manager = manager.clone();
        let input_bar = input_bar.clone();
        let runtime = runtime.clone();
        let model_entry = model_entry.clone();
        let download_status = download_status.clone();
        let refresh_all = refresh_all.clone();
        let downloads = downloads.clone();
        move |_| {
            let model = model_entry.text().trim().to_string();
            if model.is_empty() {
                return;
            }
            download_status.set_text("Downloading…");
            download_status.remove_css_class("provider-status-error");
            download_status.remove_css_class("provider-status-connected");
            download_status.add_css_class("provider-status-disconnected");

            let download_status = download_status.clone();
            let overlay = overlay.clone();
            let manager = manager.clone();
            let input_bar = input_bar.clone();
            let runtime = runtime.clone();
            let refresh_all = refresh_all.clone();
            let on_finish: Arc<dyn Fn(&str, bool) + 'static> = Arc::new(
                move |model: &str, ok: bool| {
                    download_status.set_text("Idle");
                    download_status.remove_css_class("provider-status-connected");
                    download_status.add_css_class("provider-status-disconnected");
                    if ok {
                        overlay
                            .add_toast(adw::Toast::new(&format!("Model {} downloaded", model)));
                    } else {
                        overlay
                            .add_toast(adw::Toast::new(&format!("Failed to download {}", model)));
                    }
                    reload_app_models(&manager, &input_bar, &runtime);
                    refresh_all();
                },
            );
            crate::app::downloads::start(&downloads, &model, Some(on_finish));
        }
    });

    // ── Refresh button ───────────────────────────────────────────
    refresh_models_btn.connect_clicked({
        let refresh_all = refresh_all.clone();
        move |_| {
            refresh_all();
        }
    });

    refresh_all();
}