//! MediaView — chat-like media generation interface.
//!
//! Mirrors the chat layout: results accumulate in a scrollable area above
//! (a grid for images/videos, a list for audio) and the prompt is typed in
//! an input bar at the bottom. Media conversations are persisted like chat
//! conversations and can be reopened to reload their generated results.

use base64::Engine;
use gtk4::prelude::*;
use gtk4::{self as gtk, gdk, pango};
use libadwaita as adw;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::core::models::{Attachment, GeneratedMedia, VideoProgress, VideoStatus};
use crate::providers::pixverse::models::{clamp_duration, model_duration_range};
use crate::widgets::video_view::{make_video_progress_card, VideoProgressWidgets};

use super::generate::{
    dropdown_text, estimate_video_credits, format_num, GenKind, GenerateRequest, ProviderCap,
    PIXVERSE_MODELS,
};

pub struct MediaView {
    pub container: gtk::Box,
    title_label: gtk::Label,
    provider_selector: gtk::DropDown,
    balance_label: gtk::Label,
    prompt_view: gtk::TextView,
    image_options: gtk::Box,
    num_images: gtk::SpinButton,
    video_options: gtk::Box,
    duration: gtk::SpinButton,
    model_selector: gtk::DropDown,
    quality: gtk::DropDown,
    aspect_ratio: gtk::DropDown,
    estimate_label: gtk::Label,
    attach_btn: gtk::Button,
    image_path: Arc<Mutex<Option<PathBuf>>>,
    audio_options: gtk::Box,
    generate_btn: gtk::Button,
    status_label: gtk::Label,
    flow_box: gtk::FlowBox,
    audio_list: gtk::Box,
    results_scroll: gtk::ScrolledWindow,
    empty_state: gtk::Box,
    empty_title: gtk::Label,
    empty_subtitle: gtk::Label,
    error_banner: gtk::Box,
    error_title: gtk::Label,
    error_message: gtk::Label,
    providers: Vec<ProviderCap>,
    balance: Option<(i64, i64)>,
    video_cards: HashMap<String, VideoProgressWidgets>,
    on_generate: Arc<Mutex<Option<Box<dyn Fn(GenerateRequest) + 'static>>>>,
    on_settings: Arc<Mutex<Option<Box<dyn Fn() + 'static>>>>,
    on_about: Arc<Mutex<Option<Box<dyn Fn() + 'static>>>>,
    /// Reference image preview box (in video_options) showing thumbnail or placeholder.
    reference_preview: gtk::Box,
}

impl MediaView {
    pub fn new() -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.add_css_class("media-view");

        // ── Header with three-dot menu (Settings / About) ──
        let header_bar = adw::HeaderBar::new();

        let title_label = gtk::Label::new(Some("Generate Media"));
        title_label.add_css_class("title");
        title_label.set_halign(gtk::Align::Center);
        header_bar.set_title_widget(Some(&title_label));

        let (menu_btn, on_settings, on_about) = build_header_menu();
        header_bar.pack_end(&menu_btn);
        container.append(&header_bar);

        // ── Results area (scrollable, above the input) ──
        let results_scroll = gtk::ScrolledWindow::new();
        results_scroll.set_vexpand(true);
        results_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);

        let results_container = gtk::Box::new(gtk::Orientation::Vertical, 6);
        results_container.set_margin_start(24);
        results_container.set_margin_end(24);
        results_container.set_margin_top(8);
        results_container.set_margin_bottom(8);

        // Grid for images and videos.
        let flow_box = gtk::FlowBox::new();
        flow_box.set_selection_mode(gtk::SelectionMode::None);
        flow_box.set_max_children_per_line(2);
        flow_box.set_min_children_per_line(1);
        flow_box.set_halign(gtk::Align::Fill);
        results_container.append(&flow_box);

        // List for audio.
        let audio_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
        audio_list.set_visible(false);
        results_container.append(&audio_list);

        results_scroll.set_child(Some(&results_container));
        container.append(&results_scroll);

        // Empty state placeholder, shown while there are no results yet.
        let empty_state = gtk::Box::new(gtk::Orientation::Vertical, 16);
        empty_state.add_css_class("media-empty-state");
        empty_state.set_valign(gtk::Align::Center);
        empty_state.set_halign(gtk::Align::Center);
        empty_state.set_vexpand(true);
        empty_state.set_visible(false);
        container.append(&empty_state);

        let empty_icon = gtk::Label::new(Some("✦"));
        empty_icon.add_css_class("media-empty-icon");
        empty_state.append(&empty_icon);

        let empty_title = gtk::Label::new(Some("Generate media"));
        empty_title.add_css_class("media-empty-title");
        empty_state.append(&empty_title);

        let empty_subtitle = gtk::Label::new(Some("Describe what you want and press Generate."));
        empty_subtitle.add_css_class("media-empty-subtitle");
        empty_state.append(&empty_subtitle);

        // Error banner, shown above the input bar when a generation fails.
        let error_banner = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        error_banner.set_margin_start(20);
        error_banner.set_margin_end(20);
        error_banner.set_margin_top(8);
        error_banner.set_visible(false);

        let error_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        error_box.add_css_class("chat-bubble");
        error_box.add_css_class("chat-error-bubble");
        error_box.set_hexpand(true);
        error_banner.append(&error_box);

        let error_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        error_header.set_halign(gtk::Align::Start);
        error_box.append(&error_header);

        let error_icon = gtk::Image::from_icon_name("dialog-error-symbolic");
        error_icon.add_css_class("chat-error-icon");
        error_header.append(&error_icon);

        let error_title = gtk::Label::new(Some("Generation failed"));
        error_title.add_css_class("chat-error-title");
        error_title.set_xalign(0.0);
        error_title.set_halign(gtk::Align::Start);
        error_header.append(&error_title);

        let error_message = gtk::Label::new(None);
        error_message.add_css_class("chat-error-text");
        error_message.set_wrap(true);
        error_message.set_wrap_mode(pango::WrapMode::WordChar);
        error_message.set_xalign(0.0);
        error_message.set_halign(gtk::Align::Start);
        error_message.set_selectable(true);
        error_box.append(&error_message);

        container.append(&error_banner);

        // ── Input bar (chat-like card) ──
        let input_area = gtk::Box::new(gtk::Orientation::Vertical, 6);
        input_area.set_margin_top(8);
        input_area.set_margin_bottom(12);
        input_area.set_margin_start(20);
        input_area.set_margin_end(20);

        // Reference image preview (hidden by default, shown when an image is selected)
        let reference_preview = gtk::Box::new(gtk::Orientation::Vertical, 6);
        reference_preview.set_visible(false);
        input_area.append(&reference_preview);

        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("card");
        card.add_css_class("input-bar-inner");

        let prompt_scroll = gtk::ScrolledWindow::new();
        prompt_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        prompt_scroll.set_hexpand(true);
        prompt_scroll.set_min_content_height(52);

        let prompt_view = gtk::TextView::new();
        prompt_view.add_css_class("input-text-view");
        prompt_view.set_wrap_mode(gtk::WrapMode::WordChar);
        prompt_view.set_pixels_above_lines(2);
        prompt_view.set_pixels_below_lines(2);
        prompt_view.set_accepts_tab(false);
        prompt_scroll.set_child(Some(&prompt_view));
        card.append(&prompt_scroll);

        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        separator.set_margin_top(6);
        separator.set_margin_bottom(6);
        card.append(&separator);

        // Footer row: provider, options, generate button.
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        footer.set_valign(gtk::Align::Center);

        let provider_selector = gtk::DropDown::from_strings(&[]);
        provider_selector.set_tooltip_text(Some("Provider"));
        footer.append(&provider_selector);

        // Image options (number of images).
        let image_options = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let num_label = gtk::Label::new(Some("Images:"));
        num_label.add_css_class("dim-label");
        image_options.append(&num_label);
        let num_images = gtk::SpinButton::with_range(1.0, 4.0, 1.0);
        image_options.append(&num_images);
        footer.append(&image_options);

        // Video options (model, duration, quality, aspect, reference image).
        let video_options = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        video_options.set_visible(false);

        let model_selector = gtk::DropDown::from_strings(PIXVERSE_MODELS);
        model_selector.set_tooltip_text(Some("Model"));
        video_options.append(&model_selector);

        let dur_label = gtk::Label::new(Some("Duration:"));
        dur_label.add_css_class("dim-label");
        video_options.append(&dur_label);
        let duration = gtk::SpinButton::with_range(1.0, 10.0, 1.0);
        duration.set_value(5.0);
        video_options.append(&duration);

        let quality = gtk::DropDown::from_strings(&["360p", "540p", "720p", "1080p"]);
        quality.set_tooltip_text(Some("Quality"));
        video_options.append(&quality);

        let aspect_ratio = gtk::DropDown::from_strings(&["16:9", "9:16", "1:1"]);
        aspect_ratio.set_tooltip_text(Some("Aspect ratio"));
        video_options.append(&aspect_ratio);

        let attach_btn = gtk::Button::new();
        attach_btn.set_icon_name("image-x-generic-symbolic");
        attach_btn.add_css_class("flat");
        attach_btn.set_tooltip_text(Some("Attach reference image"));
        video_options.append(&attach_btn);

        // Reference image preview area (shows thumbnail or placeholder)
        let reference_preview = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        reference_preview.set_halign(gtk::Align::Start);
        let initial_label = gtk::Label::new(Some("No image"));
        initial_label.add_css_class("dim-label");
        initial_label.set_ellipsize(pango::EllipsizeMode::End);
        initial_label.set_max_width_chars(14);
        reference_preview.append(&initial_label);
        video_options.append(&reference_preview);

        footer.append(&video_options);

        // Audio note.
        let audio_options = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        audio_options.set_visible(false);
        let audio_note = gtk::Label::new(Some("No provider supports audio generation yet."));
        audio_note.add_css_class("dim-label");
        audio_options.append(&audio_note);
        footer.append(&audio_options);

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);

        let generate_btn = gtk::Button::with_label("Generate");
        generate_btn.set_icon_name("media-record-symbolic");
        generate_btn.add_css_class("suggested-action");
        footer.append(&generate_btn);

        card.append(&footer);
        input_area.append(&card);

        // Status / balance row below the card.
        let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        status_row.set_margin_start(16);
        status_row.set_margin_end(16);

        let status_label = gtk::Label::new(None);
        status_label.add_css_class("dim-label");
        status_label.set_xalign(0.0);
        status_label.set_hexpand(true);
        status_label.set_wrap(true);
        status_row.append(&status_label);

        let estimate_label = gtk::Label::new(None);
        estimate_label.add_css_class("dim-label");
        estimate_label.set_xalign(0.0);
        estimate_label.set_wrap(true);
        status_row.append(&estimate_label);

        let balance_label = gtk::Label::new(None);
        balance_label.add_css_class("dim-label");
        balance_label.set_xalign(1.0);
        status_row.append(&balance_label);

        input_area.append(&status_row);
        container.append(&input_area);

        let on_generate: Arc<Mutex<Option<Box<dyn Fn(GenerateRequest) + 'static>>>> =
            Arc::new(Mutex::new(None));

let media = Self {
            container,
            title_label,
            provider_selector,
            balance_label,
            prompt_view,
            image_options,
            num_images,
            video_options,
            duration,
            model_selector,
            quality,
            aspect_ratio,
            estimate_label,
            attach_btn,
            image_path: Arc::new(Mutex::new(None)),
            audio_options,
            generate_btn,
            status_label,
            flow_box,
            audio_list,
            results_scroll,
            empty_state,
            empty_title,
            empty_subtitle,
            error_banner,
            error_title,
            error_message,
            providers: Vec::new(),
            balance: None,
            video_cards: HashMap::new(),
            on_generate,
            on_settings,
            on_about,
            /// Reference image preview box (in video_options) showing thumbnail or placeholder.
            reference_preview,
        };

        media.setup_static_signals();
        media
    }

fn setup_static_signals(&self) {
        let image_path = self.image_path.clone();
        let reference_preview = self.reference_preview.clone();
        let container_root = self.container.clone();

        self.attach_btn.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::new();
            dialog.set_title("Choose reference image");
            let filters = gtk::FileFilter::new();
            filters.set_name(Some("Images"));
            filters.add_mime_type("image/*");
            dialog.set_default_filter(Some(&filters));

            if let Some(root) = container_root.root() {
                if let Ok(win) = root.downcast::<gtk::Window>() {
                    let img_path = image_path.clone();
                    let ref_preview = reference_preview.clone();
                    dialog.open(Some(&win), gtk::gio::Cancellable::NONE, move |res| {
                        if let Ok(file) = res {
                            if let Some(path) = file.path() {
                                if let Ok(att) = Attachment::from_path(path.clone()) {
                                    *img_path.lock().unwrap() = Some(path);
                                    // Show preview
                                    let (preview_widget, remove_btn) =
                                        crate::widgets::attachment_chip::make_attachment_preview(&att);
                                    // Make the preview widget more compact for the footer
                                    preview_widget.set_hexpand(false);
                                    preview_widget.set_halign(gtk::Align::Start);
                                    // Clear previous content
                                    while let Some(child) = ref_preview.first_child() {
                                        ref_preview.remove(&child);
                                    }
                                    // Add the preview widget with some styling to make it compact
                                    let preview_container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                                    preview_container.set_halign(gtk::Align::Start);
                                    preview_container.append(&preview_widget);
                                    ref_preview.append(&preview_container);
                                    // Wire remove button to clear the image
                                    let img_path_clone = img_path.clone();
                                    let ref_preview_clone = ref_preview.clone();
                                    remove_btn.connect_clicked(move |_| {
                                        *img_path_clone.lock().unwrap() = None;
                                        // Reset to "No image" label
                                        while let Some(child) = ref_preview.first_child() {
                                            ref_preview.remove(&child);
                                        }
                                        let label = gtk::Label::new(Some("No image"));
                                        label.add_css_class("dim-label");
                                        label.set_ellipsize(pango::EllipsizeMode::End);
                                        label.set_max_width_chars(14);
                                        ref_preview.append(&label);
                                    });
                                }
                            }
                        }
                    });
                }
            }
        });
    }

    // ── Callbacks ─────────────────────────────────────────────

    pub fn set_on_generate<F: Fn(GenerateRequest) + 'static>(&mut self, callback: F) {
        *self.on_generate.lock().unwrap() = Some(Box::new(callback));
    }

    pub fn set_on_settings<F: Fn() + 'static>(&mut self, callback: F) {
        *self.on_settings.lock().unwrap() = Some(Box::new(callback));
    }

    pub fn set_on_about<F: Fn() + 'static>(&mut self, callback: F) {
        *self.on_about.lock().unwrap() = Some(Box::new(callback));
    }

    /// Connect the generate button. The callback is invoked WITHOUT holding
    /// the media-view mutex to avoid re-entrant deadlocks.
    pub fn connect_generate_button(&self, media_arc: Arc<Mutex<Self>>) {
        let gen_cb = self.on_generate.clone();
        self.generate_btn.connect_clicked(move |_| {
            let req = {
                let m = media_arc.lock().unwrap();
                m.build_request()
            };
            if let Some(req) = req {
                if let Some(cb) = gen_cb.lock().unwrap().as_ref() {
                    cb(req);
                }
            } else {
                let m = media_arc.lock().unwrap();
                m.show_status("Enter a prompt to generate media.");
            }
        });
    }

    // ── Kind & providers ──────────────────────────────────────

    /// Switch the view to a generation kind (Image / Video / Audio).
    pub fn set_kind(&mut self, kind: GenKind) {
        self.title_label
            .set_text(&format!("Generate {}", kind.label()));
        self.image_options.set_visible(kind == GenKind::Image);
        self.video_options.set_visible(kind == GenKind::Video);
        self.audio_options.set_visible(kind == GenKind::Audio);
        self.flow_box.set_visible(kind != GenKind::Audio);
        self.audio_list.set_visible(kind == GenKind::Audio);
        let (title, subtitle) = match kind {
            GenKind::Image => (
                "Generate images",
                "Describe what you want and press Generate.",
            ),
            GenKind::Video => ("Generate videos", "Describe the scene and press Generate."),
            GenKind::Audio => (
                "Generate audio",
                "Describe the sound you want and press Generate.",
            ),
        };
        self.empty_title.set_text(title);
        self.empty_subtitle.set_text(subtitle);
        self.refresh_provider_list();
    }

    pub fn kind(&self) -> GenKind {
        // Kind is set externally; the field mirrors the active conversation.
        if self.image_options.is_visible() {
            GenKind::Image
        } else if self.video_options.is_visible() {
            GenKind::Video
        } else {
            GenKind::Audio
        }
    }

    /// Register the providers available for generation with their capabilities.
    pub fn set_providers(&mut self, providers: Vec<ProviderCap>) {
        self.providers = providers;
        self.refresh_provider_list();
    }

    /// Rebuild the provider dropdown for the current kind.
    fn refresh_provider_list(&self) {
        let kind = self.kind();
        let mut names: Vec<String> = Vec::new();
        for p in &self.providers {
            let ok = match kind {
                GenKind::Image => p.supports_image,
                GenKind::Video => p.supports_video,
                GenKind::Audio => false,
            };
            if ok {
                names.push(p.name.clone());
            }
        }
        let name_refs: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
        self.provider_selector
            .set_model(Some(&gtk::StringList::new(&name_refs)));

        let can_generate = match kind {
            GenKind::Image => self.providers.iter().any(|p| p.supports_image),
            GenKind::Video => self.providers.iter().any(|p| p.supports_video),
            GenKind::Audio => false,
        };
        self.generate_btn.set_sensitive(can_generate);
    }

    pub fn provider_selector(&self) -> gtk::DropDown {
        self.provider_selector.clone()
    }

    /// The currently selected provider ID for the active kind.
    pub fn selected_provider_id(&self) -> Option<String> {
        self.selected_provider().map(|p| p.id)
    }

    fn selected_provider(&self) -> Option<ProviderCap> {
        let kind = self.kind();
        let idx = self.provider_selector.selected() as usize;
        let mut visible = Vec::new();
        for p in &self.providers {
            let ok = match kind {
                GenKind::Image => p.supports_image,
                GenKind::Video => p.supports_video,
                GenKind::Audio => false,
            };
            if ok {
                visible.push(p.clone());
            }
        }
        visible.get(idx).cloned()
    }

    // ── Request building ──────────────────────────────────────

    pub fn build_request(&self) -> Option<GenerateRequest> {
        let kind = self.kind();
        let provider = self.selected_provider()?;

        let buffer = self.prompt_view.buffer();
        let start = buffer.start_iter();
        let end = buffer.end_iter();
        let prompt = buffer
            .text(&start, &end, false)
            .to_string()
            .trim()
            .to_string();

        if prompt.is_empty() {
            return None;
        }

        Some(GenerateRequest {
            provider_id: provider.id.clone(),
            kind,
            prompt,
            num_images: self.num_images.value() as u32,
            model: dropdown_text(&self.model_selector).unwrap_or_else(|| "v6".to_string()),
            duration_seconds: self.duration.value() as u8,
            quality: dropdown_text(&self.quality).unwrap_or_else(|| "720p".to_string()),
            aspect_ratio: dropdown_text(&self.aspect_ratio).unwrap_or_else(|| "16:9".to_string()),
            image_path: self.image_path.lock().unwrap().clone(),
        })
    }

    // ── Status / balance ──────────────────────────────────────

    pub fn set_generating(&self, generating: bool) {
        self.generate_btn.set_sensitive(!generating);
        if generating {
            self.status_label.set_text("Generating…");
            self.clear_error();
        }
    }

    pub fn show_status(&self, message: &str) {
        self.status_label.set_text(message);
    }

    /// Show the error banner with a short title and an actionable message.
    pub fn show_error(&self, title: &str, message: &str) {
        self.error_title.set_text(title);
        self.error_message.set_text(message);
        self.error_banner.set_visible(true);
    }

    pub fn clear_error(&self) {
        self.error_banner.set_visible(false);
    }

    pub fn set_balance(&mut self, monthly: Option<i64>, package: Option<i64>) {
        self.balance = monthly.map(|m| (m, package.unwrap_or(0)));
        let text = match self.balance {
            Some((m, p)) => format!(
                "Credits: {} monthly \u{00b7} {} package",
                format_num(m),
                format_num(p)
            ),
            None => String::new(),
        };
        self.balance_label.set_text(&text);
        self.update_video_estimate();
    }

    pub fn set_balance_error(&mut self, message: &str) {
        self.balance = None;
        self.balance_label
            .set_text(&format!("Credits unavailable: {}", message));
        self.update_video_estimate();
    }

    /// Recompute and display the estimated credit cost for the current video
    /// settings (PixVerse billing, no audio).
    pub fn update_video_estimate(&self) {
        let duration = self.duration.value() as u64;
        let quality = dropdown_text(&self.quality).unwrap_or_else(|| "720p".to_string());
        let model = dropdown_text(&self.model_selector).unwrap_or_else(|| "v6".to_string());
        let cost = estimate_video_credits(&model, &quality, duration);

        let mut text = format!(
            "Estimated cost: ~{} credits ({} \u{00b7} {} \u{00b7} {}s)",
            format_num(cost as i64),
            model,
            quality,
            duration
        );
        if let Some((monthly, package)) = self.balance {
            let total = monthly.saturating_add(package);
            if total >= cost as i64 {
                text.push_str(&format!(
                    " \u{00b7} Credits after: {}",
                    format_num(total - cost as i64)
                ));
            } else {
                text.push_str(" \u{00b7} Insufficient credits!");
            }
        }
        self.estimate_label.set_text(&text);
    }

    /// Constrain the duration spin button to the values the selected model
    /// supports, snapping the current value to the closest allowed duration.
    pub fn update_duration_for_model(&self) {
        let model = dropdown_text(&self.model_selector).unwrap_or_else(|| "v6".to_string());
        let (min, max) = model_duration_range(&model);
        self.duration.set_range(min as f64, max as f64);
        self.duration.set_increments(1.0, 5.0);

        let current = self.duration.value() as u8;
        let snapped = clamp_duration(&model, current);
        if snapped != current {
            self.duration.set_value(snapped as f64);
        }
        self.update_video_estimate();
    }

    /// Connect duration/quality/model widgets so the cost estimate updates live.
    pub fn connect_live_estimate(&self, media_arc: Arc<Mutex<Self>>) {
        let duration = self.duration.clone();
        {
            let media = media_arc.clone();
            duration.connect_value_changed(move |_| {
                media.lock().unwrap().update_video_estimate();
            });
        }
        let quality = self.quality.clone();
        {
            let media = media_arc.clone();
            quality.connect_selected_notify(move |_| {
                media.lock().unwrap().update_video_estimate();
            });
        }
        let model_selector = self.model_selector.clone();
        {
            let media = media_arc.clone();
            model_selector.connect_selected_notify(move |_| {
                media.lock().unwrap().update_duration_for_model();
            });
        }
    }

    // ── Results ───────────────────────────────────────────────

    /// Toggle between the placeholder and the results area based on whether
    /// any results are present.
    fn update_empty_state(&self) {
        let has_results =
            self.flow_box.first_child().is_some() || self.audio_list.first_child().is_some();
        self.empty_state.set_visible(!has_results);
        self.results_scroll.set_visible(has_results);
    }

    pub fn clear_results(&mut self) {
        while let Some(child) = self.flow_box.first_child() {
            self.flow_box.remove(&child);
        }
        while let Some(child) = self.audio_list.first_child() {
            self.audio_list.remove(&child);
        }
        self.video_cards.clear();
        self.clear_error();
        self.update_empty_state();
    }

    /// Display a generated image in the results grid.
    pub fn add_image_result(&self, data: &[u8], mime: &str) {
        let mime_owned = mime.to_string();
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("gen-result-card");
        card.set_margin_top(6);
        card.set_margin_bottom(6);

        if let Ok(texture) = gdk::Texture::from_bytes(&gtk::glib::Bytes::from(data)) {
            let pic = gtk::Picture::new();
            pic.set_paintable(Some(&texture));
            pic.set_can_shrink(true);
            pic.set_content_fit(gtk::ContentFit::Contain);
            pic.set_size_request(-1, 240);
            card.append(&pic);

            let save_btn = gtk::Button::with_label("Save image…");
            save_btn.set_icon_name("document-save-symbolic");
            save_btn.add_css_class("flat");
            card.append(&save_btn);

            let data_for_save = data.to_vec();
            let container_root = self.container.clone();
            save_btn.connect_clicked(move |_| {
                let dialog = gtk::FileDialog::new();
                dialog.set_title("Save generated image");
                let ext = if mime_owned.contains("png") {
                    "png"
                } else if mime_owned.contains("jpeg") {
                    "jpg"
                } else {
                    "webp"
                };
                dialog.set_initial_name(Some(&format!("gtksynapse-image.{}", ext)));
                if let Some(root) = container_root.root() {
                    if let Ok(win) = root.downcast::<gtk::Window>() {
                        let data = data_for_save.clone();
                        dialog.save(Some(&win), gtk::gio::Cancellable::NONE, move |res| {
                            if let Ok(file) = res {
                                if let Some(path) = file.path() {
                                    let _ = std::fs::write(path, &data);
                                }
                            }
                        });
                    }
                }
            });
        } else {
            let label = gtk::Label::new(Some("Image generated but could not be displayed."));
            label.set_halign(gtk::Align::Start);
            card.append(&label);
        }

        self.flow_box.append(&card);
        self.update_empty_state();
    }

    pub fn has_video_card(&self, task_id: &str) -> bool {
        self.video_cards.contains_key(task_id)
    }

    /// Create a video progress card and remember it for later updates.
    pub fn add_video_card(&mut self, task_id: &str) -> VideoProgressWidgets {
        let (card, widgets) = make_video_progress_card(task_id);
        self.flow_box.append(&card);
        self.video_cards
            .insert(task_id.to_string(), widgets.clone());
        self.update_empty_state();
        widgets
    }

    /// Update an existing video progress card.
    pub fn update_video(&mut self, task_id: &str, progress: &VideoProgress) {
        if let Some(widgets) = self.video_cards.get(task_id) {
            widgets.update(progress);
        }
    }

    /// Add an audio item to the list.
    pub fn add_audio_item(&self, name: &str, url: &str) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("gen-result-card");
        row.set_margin_top(4);
        row.set_margin_bottom(4);

        let icon = gtk::Image::from_icon_name("audio-x-generic-symbolic");
        row.append(&icon);

        let label = gtk::Label::new(Some(name));
        label.set_hexpand(true);
        label.set_xalign(0.0);
        label.set_ellipsize(pango::EllipsizeMode::End);
        row.append(&label);

        let open_btn = gtk::Button::with_label("Open");
        open_btn.add_css_class("flat");
        row.append(&open_btn);

        let url_owned = url.to_string();
        open_btn.connect_clicked(move |_| {
            let _ = std::process::Command::new("xdg-open")
                .arg(&url_owned)
                .spawn();
        });

        self.audio_list.append(&row);
        self.update_empty_state();
    }

    /// Render a persisted batch of generated media (image grid / audio list).
    pub fn render_media(&mut self, items: &[GeneratedMedia]) {
        for item in items {
            match item.kind.as_str() {
                "image" => {
                    if let Some(b64) = &item.base64 {
                        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                            self.add_image_result(&bytes, &item.mime);
                        }
                    }
                }
                "video" => {
                    let task_id = format!("v-{}", uuid::Uuid::new_v4());
                    let (card, widgets) = make_video_progress_card(&task_id);
                    self.flow_box.append(&card);
                    self.video_cards.insert(task_id.clone(), widgets.clone());
                    let status = match item.video_status.as_deref() {
                        Some("completed") => VideoStatus::Completed,
                        Some("failed") => VideoStatus::Failed,
                        Some("processing") => VideoStatus::Processing,
                        _ => VideoStatus::Queued,
                    };
                    widgets.update(&VideoProgress {
                        task_id: task_id.clone(),
                        status: status.clone(),
                        percent: if status == VideoStatus::Completed {
                            100
                        } else {
                            10
                        },
                        video_url: item.url.clone(),
                        message: item.message.clone(),
                    });
                }
                "audio" => {
                    if let Some(url) = &item.url {
                        self.add_audio_item(&item.prompt, url);
                    }
                }
                _ => {}
            }
        }
        self.update_empty_state();
    }
}

/// Build the three-dot header menu with Settings / About entries. Returns
/// the button and the two callback slots so callers can wire their handlers.
pub(crate) fn build_header_menu() -> (
    gtk::MenuButton,
    Arc<Mutex<Option<Box<dyn Fn() + 'static>>>>,
    Arc<Mutex<Option<Box<dyn Fn() + 'static>>>>,
) {
    let menu_btn = gtk::MenuButton::new();
    menu_btn.set_icon_name("open-menu-symbolic");
    menu_btn.add_css_class("flat");

    let on_settings: Arc<Mutex<Option<Box<dyn Fn() + 'static>>>> = Arc::new(Mutex::new(None));
    let on_about: Arc<Mutex<Option<Box<dyn Fn() + 'static>>>> = Arc::new(Mutex::new(None));

    let popover = gtk::Popover::new();
    menu_btn.set_popover(Some(&popover));

    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu_box.set_width_request(180);
    popover.set_child(Some(&menu_box));

    let settings_btn = make_menu_item("Settings", "preferences-system-symbolic");
    {
        let pop = popover.clone();
        let cb = on_settings.clone();
        settings_btn.connect_clicked(move |_| {
            pop.popdown();
            if let Some(ref cb) = *cb.lock().unwrap() {
                cb();
            }
        });
    }
    menu_box.append(&settings_btn);

    let about_btn = make_menu_item("About", "help-about-symbolic");
    {
        let pop = popover.clone();
        let cb = on_about.clone();
        about_btn.connect_clicked(move |_| {
            pop.popdown();
            if let Some(ref cb) = *cb.lock().unwrap() {
                cb();
            }
        });
    }
    menu_box.append(&about_btn);

    (menu_btn, on_settings, on_about)
}

/// Build a flat menu item button with an icon + label, sized to fill the row.
fn make_menu_item(label: &str, icon: &str) -> gtk::Button {
    let btn = gtk::Button::new();
    btn.set_halign(gtk::Align::Fill);
    btn.add_css_class("flat");

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let icon_img = gtk::Image::from_icon_name(icon);
    hbox.append(&icon_img);

    let lbl = gtk::Label::new(Some(label));
    lbl.set_xalign(0.0);
    lbl.set_hexpand(true);
    hbox.append(&lbl);

    btn.set_child(Some(&hbox));
    btn
}
