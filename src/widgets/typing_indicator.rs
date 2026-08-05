//! TypingIndicator widget — animated three-dot "AI is thinking" indicator.

use gtk4::prelude::*;
use gtk4::{self as gtk, glib};

/// The animated three-dot "thinking" box. Used standalone or embedded inside
/// the streaming bubble so the dots read as part of the assistant reply
/// instead of a separate row.
pub fn make_typing_dots() -> gtk::Box {
    let dots_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    dots_box.add_css_class("typing-dots");

    for _ in 0..3 {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("typing-dot");
        dot.set_size_request(8, 8);
        dots_box.append(&dot);
    }

    dots_box
}

/// Returns a widget showing an animated typing indicator (avatar + dots),
/// used when there is no streaming bubble to attach the dots to.
pub fn make_typing_indicator() -> gtk::Box {
    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    outer.add_css_class("typing-indicator");
    outer.set_margin_start(24);
    outer.set_margin_top(4);
    outer.set_margin_bottom(4);

    // Avatar placeholder
    let avatar = gtk::Label::new(Some("✦"));
    avatar.set_margin_end(10);
    outer.append(&avatar);

    outer.append(&make_typing_dots());
    outer
}
