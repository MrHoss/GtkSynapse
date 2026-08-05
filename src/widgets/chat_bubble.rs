//! ChatBubble widget — displays a single AI or user message.
//!
//! Features:
//! - User (right-aligned) / assistant (left-aligned) layout
//! - Markdown rendering for assistant messages
//! - Image attachment display
//! - Copy, regenerate, and timestamp actions
//! - Animated slide-in on first display

use gtk4::prelude::*;
use gtk4::{self as gtk, gdk, glib, pango};

use crate::core::models::{AttachmentKind, Message, MessageRole};
use crate::widgets::markdown;

/// Construct a chat bubble widget for the given message.
pub fn make_chat_bubble(message: &Message) -> gtk::Box {
    let is_user = message.role == MessageRole::User;

    // Outer row (full width)
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("chat-message-row");
    row.set_hexpand(true);
    row.set_margin_top(2);
    row.set_margin_bottom(2);

    if is_user {
        row.set_halign(gtk::Align::End);
        row.set_margin_end(16);
        row.set_margin_start(80);
    } else {
        row.set_halign(gtk::Align::Start);
        row.set_margin_start(16);
        row.set_margin_end(80);
    }

    // Avatar
    let avatar = make_avatar(is_user, &message.role);
    if !is_user {
        row.append(&avatar);
    }

    // Content column
    let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
    col.set_hexpand(false);
    col.set_halign(if is_user {
        gtk::Align::End
    } else {
        gtk::Align::Start
    });

    // Bubble
    let bubble = gtk::Box::new(gtk::Orientation::Vertical, 6);
    bubble.add_css_class("chat-bubble");
    bubble.add_css_class(if is_user {
        "user-bubble"
    } else {
        "assistant-bubble"
    });
    bubble.set_margin_start(8);
    bubble.set_margin_end(8);

    // Image attachments
    for att in &message.attachments {
        if matches!(att.kind, AttachmentKind::Image) {
            if let Some(img_widget) = make_inline_image(&att.file_path) {
                bubble.append(&img_widget);
            }
        }
    }

    // Message content
    if is_user {
        // User messages are plain text labels
        let label = gtk::Label::new(Some(&message.content));
        label.set_wrap(true);
        label.set_wrap_mode(pango::WrapMode::WordChar);
        label.set_xalign(0.0);
        label.set_selectable(true);
        bubble.append(&label);
    } else {
        // Assistant messages render Markdown
        if !message.content.is_empty() {
            let md_widget = markdown::render_markdown(&message.content);
            bubble.append(&md_widget);
        }
    }

    col.append(&bubble);

    // Timestamp + actions row
    let meta_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    meta_row.set_margin_start(8);
    meta_row.set_margin_end(8);
    meta_row.set_halign(if is_user {
        gtk::Align::End
    } else {
        gtk::Align::Start
    });

    let ts = gtk::Label::new(Some(&message.created_at.format("%H:%M").to_string()));
    ts.add_css_class("bubble-timestamp");

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    actions.add_css_class("bubble-actions");

    // Copy button
    let copy_btn = gtk::Button::new();
    copy_btn.set_icon_name("edit-copy-symbolic");
    copy_btn.set_tooltip_text(Some("Copy message"));
    copy_btn.add_css_class("bubble-action-button");
    let content_clone = message.content.clone();
    copy_btn.connect_clicked(move |_| {
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(&content_clone);
        }
    });
    actions.append(&copy_btn);

    if is_user {
        meta_row.append(&actions);
        meta_row.append(&ts);
    } else {
        meta_row.append(&ts);
        meta_row.append(&actions);
    }

    col.append(&meta_row);
    row.append(&col);

    if is_user {
        row.append(&avatar);
    }

    row
}

/// Build a small circular avatar for the message sender.
fn make_avatar(is_user: bool, role: &MessageRole) -> gtk::Box {
    let av = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    av.add_css_class("bubble-avatar");
    if is_user {
        av.add_css_class("user-avatar");
    } else {
        av.add_css_class("assistant-avatar");
    }
    av.set_size_request(32, 32);
    av.set_valign(gtk::Align::Start);
    av.set_margin_top(4);
    av.set_margin_start(4);
    av.set_margin_end(4);

    let icon = gtk::Label::new(Some(if is_user { "U" } else { "✦" }));
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    av.append(&icon);
    av.set_halign(gtk::Align::Center);
    av.set_valign(gtk::Align::Start);

    av
}

/// Attempt to create an inline image widget from a file path.
fn make_inline_image(path: &std::path::Path) -> Option<gtk::Picture> {
    let pic = gtk::Picture::for_filename(path);
    pic.add_css_class("inline-image");
    pic.set_can_shrink(true);
    pic.set_content_fit(gtk::ContentFit::Contain);
    pic.set_size_request(-1, 200);
    Some(pic)
}

/// Create a simple placeholder bubble for streaming in progress.
/// The caller should update its content label as chunks arrive.
/// Returns the outer row, the content label, and the bubble container.
pub fn make_streaming_bubble() -> (gtk::Box, gtk::Label, gtk::Box) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("chat-message-row");
    row.set_hexpand(true);
    row.set_halign(gtk::Align::Start);
    row.set_margin_start(16);
    row.set_margin_end(80);
    row.set_margin_top(2);
    row.set_margin_bottom(2);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 4);

    let bubble = gtk::Box::new(gtk::Orientation::Vertical, 6);
    bubble.add_css_class("chat-bubble");
    bubble.add_css_class("assistant-bubble");
    bubble.set_margin_start(8);

    let label = gtk::Label::new(Some(""));
    label.set_wrap(true);
    label.set_wrap_mode(pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label.set_selectable(true);
    label.set_hexpand(true);

    bubble.append(&label);
    col.append(&bubble);
    row.append(&col);

    (row, label, bubble)
}
