//! InputBar - multiline prompt input, file attachment chips, quick
//! provider/model selectors, capability indicator, and send/cancel buttons.

use gtk4::prelude::*;
use gtk4::{self as gtk, gdk, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::core::models::{Attachment, ModelInfo};
use crate::providers::capabilities::Capabilities;
use crate::providers::manager::ProviderInfo;
use crate::widgets::attachment_chip;

pub struct InputBar {
    pub container: gtk::Box,
    text_view: gtk::TextView,
    send_btn: gtk::Button,
    cancel_btn: gtk::Button,
    attach_btn: gtk::Button,
    provider_selector: gtk::DropDown,
    model_selector: gtk::DropDown,
    capability_label: gtk::Label,
    attachments_box: gtk::Box,
    streaming_indicator: gtk::Label,
    attachments: Arc<Mutex<Vec<Attachment>>>,
    available_providers: Vec<ProviderInfo>,
    available_models: Vec<ModelInfo>,
    /// Set while the widget selects are being updated programmatically, so
    /// window handlers can tell user-driven changes apart from syncing.
    syncing: Arc<Mutex<bool>>,
    /// Rebuilds the attachment previews. Armed with a clone of the input
    /// bar's Arc so add/remove callbacks can refresh the UI without holding
    /// a borrow of `self` at signal time.
    refresh_chips: Arc<Mutex<Option<Box<dyn Fn() + 'static>>>>,
    on_send_message: Option<Box<dyn Fn(String, Vec<Attachment>, ModelInfo) -> bool + 'static>>,
    on_cancel: Option<Box<dyn Fn() + 'static>>,
}

impl InputBar {
    pub fn new() -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
        container.set_margin_top(12);
        container.set_margin_bottom(16);
        container.set_margin_start(20);
        container.set_margin_end(20);

        // Attachments scroll bar area
        let attachments_scroll = gtk::ScrolledWindow::new();
        attachments_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
        attachments_scroll.set_min_content_height(40);
        attachments_scroll.set_visible(false);

        let attachments_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        attachments_scroll.set_child(Some(&attachments_box));
        container.append(&attachments_scroll);

        // Core Input Box (inner): the "card" style class is built into the
        // Adwaita theme, so its surface, border and radius follow the OS
        // theme automatically (light/dark and accent). The model/provider
        // selectors live in a footer row inside the card so they read as
        // part of the input instead of floating beside it.
        let inner_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        inner_box.add_css_class("card");
        inner_box.add_css_class("input-bar-inner");

        // Input text view scrolled wrapper (fills the top of the card).
        let text_scroll = gtk::ScrolledWindow::new();
        text_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        text_scroll.set_hexpand(true);
        text_scroll.set_vexpand(false);
        text_scroll.set_min_content_height(42);

        let text_view = gtk::TextView::new();
        text_view.add_css_class("input-text-view");
        text_view.set_wrap_mode(gtk::WrapMode::WordChar);
        text_view.set_pixels_above_lines(2);
        text_view.set_pixels_below_lines(2);
        text_view.set_accepts_tab(false);
        text_scroll.set_child(Some(&text_view));
        inner_box.append(&text_scroll);

        // Native separator drawn by the theme (respects light/dark mode).
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.set_margin_top(6);
        separator.set_margin_bottom(6);
        inner_box.append(&separator);

        // Footer toolbar row: attachment button, provider/model selectors,
        // and the send/cancel buttons.
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        footer.set_valign(gtk::Align::Center);

        let attach_btn = gtk::Button::new();
        attach_btn.set_icon_name("mail-attachment-symbolic");
        attach_btn.add_css_class("flat");
        attach_btn.set_tooltip_text(Some("Attach files (images, audio, video, PDF, text)"));
        footer.append(&attach_btn);

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);

        // Provider dropdown selector
        let provider_selector = gtk::DropDown::from_strings(&[]);
        provider_selector.set_tooltip_text(Some("AI provider"));
        footer.append(&provider_selector);

        // Model dropdown selector
        let model_selector = gtk::DropDown::from_strings(&[]);
        footer.append(&model_selector);

        // Send and cancel buttons
        let send_btn = gtk::Button::new();
        send_btn.set_icon_name("mail-send-symbolic");
        send_btn.add_css_class("suggested-action");
        send_btn.set_tooltip_text(Some("Send message"));
        footer.append(&send_btn);

        let cancel_btn = gtk::Button::new();
        cancel_btn.set_icon_name("media-playback-stop-symbolic");
        cancel_btn.add_css_class("destructive-action");
        cancel_btn.set_tooltip_text(Some("Cancel response generation"));
        cancel_btn.set_visible(false);
        footer.append(&cancel_btn);

        inner_box.append(&footer);
        container.append(&inner_box);

        // Bottom status row (streaming active + model capabilities)
        let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        status_row.set_margin_start(16);
        status_row.set_margin_end(16);

        let streaming_indicator = gtk::Label::new(Some("Streaming..."));
        streaming_indicator.add_css_class("dim-label");
        streaming_indicator.set_visible(false);
        status_row.append(&streaming_indicator);

        let capability_label = gtk::Label::new(None);
        capability_label.add_css_class("dim-label");
        capability_label.set_xalign(0.0);
        capability_label.set_hexpand(true);
        capability_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status_row.append(&capability_label);

        container.append(&status_row);

        let attachments = Arc::new(Mutex::new(Vec::new()));
        let syncing = Arc::new(Mutex::new(false));
        let refresh_chips: Arc<Mutex<Option<Box<dyn Fn() + 'static>>>> =
            Arc::new(Mutex::new(None));

        let mut input_bar = Self {
            container,
            text_view,
            send_btn,
            cancel_btn,
            attach_btn,
            provider_selector,
            model_selector,
            capability_label,
            attachments_box,
            streaming_indicator,
            attachments,
            available_providers: Vec::new(),
            available_models: Vec::new(),
            syncing,
            refresh_chips,
            on_send_message: None,
            on_cancel: None,
        };

        input_bar
    }

    pub fn set_on_send_message<F: Fn(String, Vec<Attachment>, ModelInfo) -> bool + 'static>(
        &mut self,
        callback: F,
    ) {
        self.on_send_message = Some(Box::new(callback));
    }

    pub fn set_on_cancel<F: Fn() + 'static>(&mut self, callback: F) {
        self.on_cancel = Some(Box::new(callback));
    }

    // ── Provider selector ──────────────────────────────────────

    /// Populate the provider dropdown. Selection resets to the first entry.
    pub fn update_providers(&mut self, providers: Vec<ProviderInfo>) {
        self.available_providers = providers;
        let names: Vec<String> = self
            .available_providers
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let name_refs: Vec<&str> = names.iter().map(|n| n.as_str()).collect();

        // Guard the programmatic reset so window handlers don't treat it as a
        // user-driven change (and don't re-lock the input bar synchronously).
        *self.syncing.lock().unwrap() = true;
        self.provider_selector
            .set_model(Some(&gtk::StringList::new(&name_refs)));
        *self.syncing.lock().unwrap() = false;

        self.refresh_attach_state();
    }

    /// Show or hide the file-attach button depending on whether the currently
    /// selected provider can accept attachments (vision or file upload).
    pub fn refresh_attach_state(&self) {
        let idx = self.provider_selector.selected() as usize;
        let can_attach = self
            .available_providers
            .get(idx)
            .map(|p| {
                p.capabilities
                    .intersects(Capabilities::VISION | Capabilities::FILE_UPLOAD)
            })
            .unwrap_or(false);
        self.attach_btn.set_visible(can_attach);
    }

    /// The ID of the currently selected provider.
    pub fn selected_provider_id(&self) -> Option<String> {
        let idx = self.provider_selector.selected() as usize;
        self.available_providers.get(idx).map(|p| p.id.clone())
    }

    /// The index of a provider by ID (if it is in the dropdown).
    pub fn provider_index(&self, id: &str) -> Option<u32> {
        self.available_providers
            .iter()
            .position(|p| p.id == id)
            .map(|i| i as u32)
    }

    /// The provider ID at a dropdown index.
    pub fn provider_id_at(&self, idx: u32) -> Option<String> {
        self.available_providers
            .get(idx as usize)
            .map(|p| p.id.clone())
    }

    /// Expose the provider dropdown so the window can connect its signal.
    pub fn provider_selector(&self) -> gtk::DropDown {
        self.provider_selector.clone()
    }

    /// The shared flag used to distinguish programmatic syncing from
    /// user-driven selection changes.
    pub fn syncing_flag(&self) -> Arc<Mutex<bool>> {
        self.syncing.clone()
    }

    // ── Model selector ─────────────────────────────────────────

    pub fn update_models(&mut self, models: Vec<ModelInfo>) {
        self.available_models = models;
        let model_names: Vec<String> = self
            .available_models
            .iter()
            .map(|m| m.name.clone())
            .collect();
        let name_refs: Vec<&str> = model_names.iter().map(|n| n.as_str()).collect();
        self.model_selector
            .set_model(Some(&gtk::StringList::new(&name_refs)));
        self.update_capability_label();
    }

    pub fn get_selected_model(&self) -> Option<ModelInfo> {
        let index = self.model_selector.selected() as usize;
        self.available_models.get(index).cloned()
    }

    /// Select a model by ID (no-op if it is not in the current list).
    pub fn select_model_id(&mut self, id: &str) -> bool {
        if let Some(idx) = self.available_models.iter().position(|m| m.id == id) {
            self.model_selector.set_selected(idx as u32);
            self.update_capability_label();
            return true;
        }
        false
    }

    /// Refresh the small capability badge under the input bar for the
    /// currently selected model (vision, streaming, context size).
    fn update_capability_label(&self) {
        let Some(model) = self.get_selected_model() else {
            self.capability_label.set_text("");
            return;
        };
        let mut parts: Vec<String> = Vec::new();
        if model.supports_vision {
            parts.push("Vision".to_string());
        }
        if model.supports_streaming {
            parts.push("Streaming".to_string());
        }
        if let Some(ctx) = model.context_length {
            if ctx >= 1000 {
                parts.push(format!("{}k context", ctx / 1000));
            }
        }
        if parts.is_empty() {
            self.capability_label.set_text("");
        } else {
            self.capability_label.set_text(&parts.join(" · "));
        }
    }

    pub fn set_streaming(&self, is_streaming: bool) {
        self.streaming_indicator.set_visible(is_streaming);
        self.send_btn.set_visible(!is_streaming);
        self.cancel_btn.set_visible(is_streaming);
    }

    pub fn connect_events(&self, input_bar_arc: Arc<Mutex<Self>>) {
        // Enter sends the message; Shift+Enter inserts a new line. The event
        // controller runs in the capture phase (default for
        // `EventControllerKey`), so it sees the key before the text view
        // inserts a newline.
        let controller = gtk::EventControllerKey::new();
        let send_arc = input_bar_arc.clone();
        controller.connect_key_pressed(move |_, key, _keycode, mods| {
            let is_enter = key == gdk::Key::Return
                || key == gdk::Key::KP_Enter
                || key == gdk::Key::ISO_Enter;
            if !is_enter {
                return glib::Propagation::Proceed;
            }
            // Let the default handler insert a newline for Shift+Enter.
            if mods.contains(gdk::ModifierType::SHIFT_MASK) {
                return glib::Propagation::Proceed;
            }
            let ib = send_arc.lock().unwrap();
            // While a response is streaming the send button is hidden; fall
            // back to the default newline behaviour in that state.
            if !ib.send_btn.is_visible() {
                return glib::Propagation::Proceed;
            }
            ib.trigger_send();
            glib::Propagation::Stop
        });
        self.text_view.add_controller(controller);

        // Arm the refresh callback so add/remove can rebuild previews.
        let ib_refresh = input_bar_arc.clone();
        *self.refresh_chips.lock().unwrap() = Some(Box::new(move || {
            ib_refresh.lock().unwrap().refresh_attachment_chips();
        }));

        let input_bar_clone = input_bar_arc.clone();
        self.send_btn.connect_clicked(move |_| {
            let ib = input_bar_clone.lock().unwrap();
            ib.trigger_send();
        });

        let input_bar_clone = input_bar_arc.clone();
        self.cancel_btn.connect_clicked(move |_| {
            let ib = input_bar_clone.lock().unwrap();
            if let Some(ref cb) = ib.on_cancel {
                cb();
            }
        });

        let input_bar_clone = input_bar_arc.clone();
        let refresh_chips = self.refresh_chips.clone();
        self.attach_btn.connect_clicked(move |_| {
            let ib = input_bar_clone.lock().unwrap();
            ib.open_file_chooser(refresh_chips.clone());
        });

        // Refresh the capability badge when the user picks another model.
        // Deferred to the idle loop so the badge can be updated without
        // holding the input-bar lock (the selector may fire synchronously).
        let input_bar_clone = input_bar_arc.clone();
        self.model_selector.connect_selected_notify(move |_| {
            let ib = input_bar_clone.clone();
            glib::idle_add_local(move || {
                ib.lock().unwrap().update_capability_label();
                glib::ControlFlow::Break
            });
        });
    }

    fn trigger_send(&self) {
        let buffer = self.text_view.buffer();
        let start = buffer.start_iter();
        let end = buffer.end_iter();
        let text = buffer.text(&start, &end, false).to_string();

        {
            let atts = self.attachments.lock().unwrap();
            if text.trim().is_empty() && atts.is_empty() {
                return;
            }
        }

        if let Some(model) = self.get_selected_model() {
            let atts = {
                let a = self.attachments.lock().unwrap();
                a.clone()
            };
            let started = if let Some(ref cb) = self.on_send_message {
                cb(text, atts, model)
            } else {
                false
            };
            // Clear input after sending
            buffer.set_text("");
            self.attachments.lock().unwrap().clear();
            self.refresh_attachment_chips();
            if started {
                self.set_streaming(true);
            }
        }
    }

    fn open_file_chooser(&self, refresh_chips: Arc<Mutex<Option<Box<dyn Fn() + 'static>>>>) {
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Choose File Attachment");

        let attachments_clone = self.attachments.clone();
        let container_clone = self.container.clone();

        // Safe query root window
        if let Some(root) = container_clone.root() {
            if let Ok(win) = root.downcast::<gtk::Window>() {
                let attachments_cb = attachments_clone.clone();
                let ib_container = container_clone.clone();
                let refresh = refresh_chips.clone();
                dialog.open(Some(&win), gtk::gio::Cancellable::NONE, move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            if let Ok(att) = Attachment::from_path(path) {
                                attachments_cb.lock().unwrap().push(att);
                                // Trigger UI refresh
                                if let Some(ref cb) = *refresh.lock().unwrap() {
                                    cb();
                                }
                            }
                        }
                    }
                });
            }
        }
    }

    pub fn add_attachment(&self, path: PathBuf) {
        if let Ok(att) = Attachment::from_path(path) {
            self.attachments.lock().unwrap().push(att);
            self.refresh_attachment_chips();
        }
    }

    fn refresh_attachment_chips(&self) {
        // Clear children
        while let Some(child) = self.attachments_box.first_child() {
            self.attachments_box.remove(&child);
        }

        let atts = self.attachments.lock().unwrap();
        if atts.is_empty() {
            self.attachments_box.parent().unwrap().set_visible(false);
            return;
        }

        self.attachments_box.parent().unwrap().set_visible(true);

        let refresh_chips = self.refresh_chips.clone();
        let attachments_clone = self.attachments.clone();

        for (idx, att) in atts.iter().enumerate() {
            let (chip_box, remove_btn) =
                crate::widgets::attachment_chip::make_attachment_preview(att);

            let refresh_chips_cb = refresh_chips.clone();
            let attachments_cb = attachments_clone.clone();
            let parent_box = self.attachments_box.clone();

            remove_btn.connect_clicked(move |_| {
                let mut guard = attachments_cb.lock().unwrap();
                if idx < guard.len() {
                    guard.remove(idx);
                }
                // Trigger redraw
                if let Some(ref cb) = *refresh_chips_cb.lock().unwrap() {
                    cb();
                }
                let _ = &parent_box;
            });
            self.attachments_box.append(&chip_box);
        }
    }
}
