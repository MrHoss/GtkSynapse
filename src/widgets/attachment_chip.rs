//! AttachmentChip widget — compact chip showing a pending attachment.

use gtk4::prelude::*;
use gtk4::{self as gtk};

use crate::core::models::{Attachment, AttachmentKind};

/// Create an attachment chip widget.
/// Returns (chip_widget, remove_button) so the caller can connect signals.
pub fn make_attachment_chip(attachment: &Attachment) -> (gtk::Box, gtk::Button) {
    let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    chip.add_css_class("attachment-chip");
    chip.set_margin_end(4);

    // Icon based on file type
    let icon_name = match attachment.kind {
        AttachmentKind::Image => "image-x-generic-symbolic",
        AttachmentKind::Video => "video-x-generic-symbolic",
        AttachmentKind::Audio => "audio-x-generic-symbolic",
        AttachmentKind::Pdf => "x-office-document-symbolic",
        AttachmentKind::Text => "text-x-generic-symbolic",
        AttachmentKind::Unknown => "mail-attachment-symbolic",
    };

    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(14);
    chip.append(&icon);

    // File name (truncated)
    let name = if attachment.file_name.len() > 18 {
        format!("{}…", &attachment.file_name[..16])
    } else {
        attachment.file_name.clone()
    };

    let label = gtk::Label::new(Some(&name));
    label.add_css_class("attachment-chip-name");
    label.set_tooltip_text(Some(&attachment.file_name));
    chip.append(&label);

    // Remove button
    let remove = gtk::Button::new();
    remove.set_icon_name("window-close-symbolic");
    remove.add_css_class("attachment-chip-remove");
    chip.append(&remove);

    (chip, remove)
}
