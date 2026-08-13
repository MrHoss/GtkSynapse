//! AttachmentPreview widget — a preview card for a pending attachment.
//!
//! Images get a real thumbnail; other file types get a type icon. Both show
//! the file name, its size, and a remove button so the caller can wire the
//! removal signal.

use gtk4::prelude::*;
use gtk4::{self as gtk};

use crate::core::models::{Attachment, AttachmentKind};

/// Create an attachment preview card widget.
/// Returns (widget, remove_button) so the caller can connect signals.
pub fn make_attachment_preview(attachment: &Attachment) -> (gtk::Box, gtk::Button) {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.add_css_class("attachment-preview");
    card.set_margin_end(8);
    card.set_margin_bottom(4);

    // Media area: real thumbnail for images, a type icon otherwise.
    if matches!(attachment.kind, AttachmentKind::Image) {
        let pic = gtk::Picture::for_filename(&attachment.file_path);
        pic.add_css_class("attachment-preview-thumb");
        pic.set_can_shrink(true);
        pic.set_content_fit(gtk::ContentFit::Contain);
        pic.set_size_request(72, 72);
        card.append(&pic);
    } else {
        let icon_name = match attachment.kind {
            AttachmentKind::Image => "image-x-generic-symbolic",
            AttachmentKind::Video => "video-x-generic-symbolic",
            AttachmentKind::Audio => "audio-x-generic-symbolic",
            AttachmentKind::Pdf => "x-office-document-symbolic",
            AttachmentKind::Text => "text-x-generic-symbolic",
            AttachmentKind::Unknown => "mail-attachment-symbolic",
        };
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.add_css_class("attachment-preview-icon");
        icon.set_pixel_size(40);
        card.append(&icon);
    }

    // Footer: name + size on the left, remove button on the right.
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    footer.set_halign(gtk::Align::Fill);

    let meta = gtk::Box::new(gtk::Orientation::Vertical, 0);
    meta.set_hexpand(true);

    let name = gtk::Label::new(Some(&attachment.file_name));
    name.add_css_class("attachment-preview-name");
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(12);
    name.set_tooltip_text(Some(&attachment.file_name));
    meta.append(&name);

    let size = gtk::Label::new(Some(&human_size(attachment.size_bytes)));
    size.add_css_class("attachment-preview-size");
    size.set_xalign(0.0);
    meta.append(&size);

    footer.append(&meta);

    let remove = gtk::Button::new();
    remove.set_icon_name("window-close-symbolic");
    remove.add_css_class("attachment-preview-remove");
    remove.set_tooltip_text(Some("Remove attachment"));
    footer.append(&remove);

    card.append(&footer);

    (card, remove)
}

/// Format a byte count into a compact human-readable string.
fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{} B", bytes);
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{:.1} {}", value, UNITS[unit])
}
