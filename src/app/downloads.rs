//! Header "model downloads" indicator.
//!
//! Shows a Nautilus-style transfer button in the header bar while models are
//! being pulled. Clicking it opens a popover listing every active download
//! with its own progress bar, so multiple simultaneous pulls stay visible.
//!
//! Each download owns a single Tokio task streaming `ollama pull` output; the
//! lines are parsed for percentage/bytes and fed to the matching row on the
//! GTK main loop.

use gtk4::prelude::*;
use gtk4::{self as gtk, glib};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

use crate::ollama::{self, CliEvent};

/// One active download: the pull task, its row widgets, and live state.
struct Row {
    widget: gtk::Box,
    progress: gtk::ProgressBar,
    status: gtk::Label,
}

struct Inner {
    menu_button: gtk::MenuButton,
    count_label: gtk::Label,
    popover: gtk::Popover,
    list_box: gtk::ListBox,
    empty_label: gtk::Label,
    /// Model tag -> open row. Only touched on the GTK main thread (every
    /// poller runs there); a mutex makes the sharing safe and simple.
    rows: Mutex<HashMap<String, Row>>,
    runtime: tokio::runtime::Handle,
}

/// Clonable handle to the header indicator (each clone shares the same UI).
#[derive(Clone)]
pub struct DownloadsIndicator {
    inner: Arc<Inner>,
}

/// Start pulling `model` in the background, showing live progress in the
/// header indicator. `on_finish(model_tag, success)` runs on the GTK main
/// thread when the download completes or fails.
pub fn start(
    downloads: &DownloadsIndicator,
    model: &str,
    on_finish: Option<Arc<dyn Fn(&str, bool) + 'static>>,
) {
    let inner = downloads.inner.clone();
    let model = model.trim().to_string();
    if model.is_empty() {
        if let Some(cb) = &on_finish {
            cb(&model, false);
        }
        return;
    }

    // Skip if this model is already being downloaded.
    {
        let mut rows = inner.rows.lock().unwrap();
        if rows.contains_key(&model) {
            if let Some(cb) = &on_finish {
                cb(&model, false);
            }
            return;
        }
        rows.insert(model.clone(), inner.add_row(&model));
    }
    inner.refresh_button();

    let (tx, mut rx) = mpsc::channel::<CliEvent>(64);
    let model_task = model.clone();
    let rt = inner.runtime.clone();
    rt.spawn(async move {
        ollama::pull_model(&model_task, tx).await;
    });

    let on_finish = match on_finish {
        Some(cb) => cb,
        None => {
            let noop: Arc<dyn Fn(&str, bool) + 'static> = Arc::new(|_: &str, _: bool| {});
            noop
        }
    };
    let model_c = model.clone();
    glib::timeout_add_local(Duration::from_millis(50), move || {
        loop {
            match rx.try_recv() {
                Ok(CliEvent::Output(line)) => {
                    if let Some((fraction, text)) = parse_progress(&line) {
                        inner.update_row(&model_c, fraction, &text);
                    }
                }
                Ok(CliEvent::Success) => {
                    inner.finish(&model_c);
                    on_finish(&model_c, true);
                    return glib::ControlFlow::Break;
                }
                Ok(CliEvent::Error(e)) => {
                    inner.update_row(&model_c, -1.0, &e);
                    inner.finish(&model_c);
                    on_finish(&model_c, false);
                    return glib::ControlFlow::Break;
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    inner.finish(&model_c);
                    on_finish(&model_c, false);
                    return glib::ControlFlow::Break;
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

impl DownloadsIndicator {
    /// Build the header widget. The download button stays hidden until the
    /// first download starts.
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        let menu_button = gtk::MenuButton::new();
        menu_button.add_css_class("flat");

        let count_label = gtk::Label::new(Some("0"));
        count_label.add_css_class("download-count");
        let icon = gtk::Image::from_icon_name("folder-download-symbolic");
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        hbox.set_margin_end(4);
        hbox.append(&icon);
        hbox.append(&count_label);
        menu_button.set_child(Some(&hbox));
        menu_button.set_visible(false);

        let popover = gtk::Popover::new();
        menu_button.set_popover(Some(&popover));

        let pop_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        pop_box.set_width_request(300);
        let title = gtk::Label::new(Some("Model Downloads"));
        title.add_css_class("heading");
        title.set_halign(gtk::Align::Start);
        title.set_margin_start(8);
        title.set_margin_end(8);
        title.set_margin_top(8);
        pop_box.append(&title);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_max_content_height(260);
        scrolled.set_max_content_width(320);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::None);
        scrolled.set_child(Some(&list_box));

        let empty_label = gtk::Label::new(Some("No downloads in progress"));
        empty_label.add_css_class("dim-label");
        empty_label.set_margin_top(12);
        empty_label.set_margin_bottom(12);

        pop_box.append(&scrolled);
        pop_box.append(&empty_label);
        popover.set_child(Some(&pop_box));

        let inner = Arc::new(Inner {
            menu_button,
            count_label,
            popover,
            list_box,
            empty_label,
            rows: Mutex::new(HashMap::new()),
            runtime,
        });

        Self { inner }
    }

    /// The widget to place in the header bar.
    pub fn header_button(&self) -> gtk::MenuButton {
        self.inner.menu_button.clone()
    }
}

impl Inner {
    /// Build and append a new row for `model`.
    fn add_row(&self, model: &str) -> Row {
        let box_ = gtk::Box::new(gtk::Orientation::Vertical, 3);
        box_.set_margin_start(8);
        box_.set_margin_end(8);
        box_.set_margin_top(6);
        box_.set_margin_bottom(6);

        let title = gtk::Label::new(Some(model));
        title.set_halign(gtk::Align::Start);
        title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        box_.append(&title);

        let progress = gtk::ProgressBar::new();
        progress.set_fraction(-1.0);
        box_.append(&progress);

        let status = gtk::Label::new(Some("Downloading…"));
        status.set_halign(gtk::Align::Start);
        status.add_css_class("dim-label");
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        box_.append(&status);

        self.list_box.append(&box_);
        Row {
            widget: box_,
            progress,
            status,
        }
    }

    /// Refresh the percentage/status of an active download row.
    fn update_row(&self, model: &str, fraction: f64, text: &str) {
        let mut rows = self.rows.lock().unwrap();
        if let Some(row) = rows.get_mut(model) {
            row.progress.set_fraction(fraction);
            row.status.set_label(text);
        }
    }

    /// Remove a finished download and refresh the button state.
    fn finish(&self, model: &str) {
        if let Some(row) = self.rows.lock().unwrap().remove(model) {
            self.list_box.remove(&row.widget);
        }
        self.refresh_button();
    }

    /// Keep the button/count/popover in sync with the number of active rows.
    fn refresh_button(&self) {
        let n = self.rows.lock().unwrap().len();
        self.count_label.set_text(&n.to_string());
        self.menu_button.set_visible(n > 0);
        let tooltip = match n {
            1 => "1 model download in progress".to_string(),
            _ => format!("{} model downloads in progress", n),
        };
        self.menu_button.set_tooltip_text(Some(&tooltip));
        let idle = n == 0;
        self.list_box.set_visible(!idle);
        self.empty_label.set_visible(idle);
        if idle {
            self.popover.popdown();
        }
    }
}

// ─── Progress parsing (best effort) ─────────────────────────────

/// Interpret a `ollama pull` output line as `(fraction, status_text)`.
///
/// Fragile by design: the exact wording of the CLI is not a stable API, so any
/// line we do not recognize is ignored and the bar simply keeps its current
/// state. Returns a negative fraction for indeterminate phases.
fn parse_progress(line: &str) -> Option<(f64, String)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    if let Some(pct) = percent_of(line) {
        let text = match bytes_pair(line) {
            Some((done, total)) => format!("{}% · {} / {}", pct, done, total),
            None => format!("{}%", pct),
        };
        return Some((pct as f64 / 100.0, text));
    }

    let status = match line {
        l if l.starts_with("pulling manifest") => "Resolving layers…",
        l if l.starts_with("verifying") => "Verifying checksum…",
        l if l.starts_with("writing manifest") => "Finalizing…",
        l if l.starts_with("removing any") => "Cleaning up…",
        l if l.starts_with("success") => "Finished",
        _ => return None,
    };
    Some((-1.0, status.to_string()))
}

/// Extract `NN` from a `NN%` progress token.
fn percent_of(line: &str) -> Option<u8> {
    let left = line.split('%').next()?.trim_end();
    let token = left.split_whitespace().next_back()?;
    token
        .parse::<f32>()
        .ok()
        .filter(|f| (0.0..=100.0).contains(f))
        .map(|f| f as u8)
}

/// Extract the `done/total` byte counts printed like `24 MB/805 MB`.
fn bytes_pair(line: &str) -> Option<(String, String)> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    for (i, tok) in tokens.iter().enumerate() {
        if let Some((left, right)) = tok.split_once('/') {
            let done = tokens.get(i.wrapping_sub(1)).map(|n| (*n, left));
            let total = tokens.get(i + 1).map(|n| (right, *n));
            return match (done, total) {
                (Some((dn, du)), Some((tn, tu))) => {
                    Some((format!("{} {}", dn, du), format!("{} {}", tn, tu)))
                }
                _ => None,
            };
        }
    }
    None
}