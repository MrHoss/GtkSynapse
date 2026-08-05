//! VideoView widget — video progress display and playback link.

use gtk4::prelude::*;
use gtk4::{self as gtk, glib};

use crate::core::models::{VideoProgress, VideoStatus};

/// Create a video generation progress card.
pub fn make_video_progress_card(task_id: &str) -> (gtk::Box, VideoProgressWidgets) {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.set_margin_start(8);
    card.set_margin_end(8);
    card.set_margin_top(4);
    card.set_margin_bottom(4);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let icon = gtk::Label::new(Some("🎬"));
    header.append(&icon);

    let title = gtk::Label::new(Some("Generating Video…"));
    title.set_hexpand(true);
    title.set_xalign(0.0);
    header.append(&title);

    card.append(&header);

    // Progress bar
    let progress = gtk::ProgressBar::new();
    progress.add_css_class("video-progress-bar");
    progress.set_fraction(0.05);
    progress.set_show_text(true);
    progress.set_text(Some("Queued"));
    card.append(&progress);

    // Status label
    let status_label = gtk::Label::new(Some("Waiting to start…"));
    status_label.set_xalign(0.0);
    card.append(&status_label);

    // Action buttons (initially hidden)
    let action_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    action_row.set_visible(false);

    let open_btn = gtk::Button::with_label("Open Video");
    open_btn.set_icon_name("media-playback-start-symbolic");
    action_row.append(&open_btn);

    let download_btn = gtk::Button::with_label("Download");
    download_btn.set_icon_name("document-save-symbolic");
    action_row.append(&download_btn);

    card.append(&action_row);

    let widgets = VideoProgressWidgets {
        card: card.clone(),
        title,
        progress,
        status_label,
        action_row,
        open_btn,
        download_btn,
    };

    (card, widgets)
}

/// Holds references to the mutable parts of a video progress card.
#[derive(Clone)]
pub struct VideoProgressWidgets {
    pub card: gtk::Box,
    pub title: gtk::Label,
    pub progress: gtk::ProgressBar,
    pub status_label: gtk::Label,
    pub action_row: gtk::Box,
    pub open_btn: gtk::Button,
    pub download_btn: gtk::Button,
}

impl VideoProgressWidgets {
    /// Update the card based on a new progress event.
    pub fn update(&self, progress: &VideoProgress) {
        let fraction = progress.percent as f64 / 100.0;
        self.progress.set_fraction(fraction);

        match progress.status {
            VideoStatus::Queued => {
                self.progress.set_text(Some("Queued"));
                self.status_label.set_label("Waiting in queue…");
            }
            VideoStatus::Processing => {
                self.progress
                    .set_text(Some(&format!("{}%", progress.percent)));
                self.status_label
                    .set_label(progress.message.as_deref().unwrap_or("Processing…"));
            }
            VideoStatus::Completed => {
                self.title.set_label("Video Ready!");
                self.progress.set_fraction(1.0);
                self.progress.set_text(Some("Complete"));
                self.status_label
                    .set_label("Your video has been generated.");
                self.action_row.set_visible(true);

                // Set up download button
                if let Some(url) = &progress.video_url {
                    let url_clone = url.clone();
                    self.open_btn.connect_clicked(move |_| {
                        let _ = std::process::Command::new("xdg-open")
                            .arg(&url_clone)
                            .spawn();
                    });

                    let url_dl = url.clone();
                    self.download_btn.connect_clicked(move |_| {
                        let _ = std::process::Command::new("xdg-open").arg(&url_dl).spawn();
                    });
                }
            }
            VideoStatus::Failed => {
                self.title.set_label("Video Generation Failed");
                self.progress.set_text(Some("Failed"));
                self.status_label
                    .set_label(progress.message.as_deref().unwrap_or("Generation failed"));
            }
        }
    }
}
