//! ChatView - coordinates conversation bubble lists, typing indicators, empty states, and auto-scrolling.

use gtk4::prelude::*;
use gtk4::{self as gtk, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::sync::{Arc, Mutex};

use crate::core::models::{Conversation, Message};
use crate::widgets::{chat_bubble, typing_indicator};

pub struct ChatView {
    pub container: gtk::Box,
    scrolled: gtk::ScrolledWindow,
    messages_box: gtk::Box,
    empty_state: gtk::Box,
    typing_indicator: Option<gtk::Box>,
    /// Whether the typing dots were embedded inside the streaming bubble
    /// (true) or appended to the message list as a standalone row (false).
    typing_in_bubble: bool,
    current_stream_row: Option<(gtk::Box, gtk::Label, gtk::Box)>,
}

impl ChatView {
    pub fn new() -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.add_css_class("chat-view");

        // Empty state placeholder
        let empty_state = gtk::Box::new(gtk::Orientation::Vertical, 16);
        empty_state.add_css_class("chat-empty-state");
        empty_state.set_valign(gtk::Align::Center);
        empty_state.set_halign(gtk::Align::Center);
        empty_state.set_vexpand(true);

        let icon = gtk::Label::new(Some("✦"));
        icon.add_css_class("chat-empty-icon");
        empty_state.append(&icon);

        let title = gtk::Label::new(Some("Welcome to AIChat"));
        title.add_css_class("chat-empty-title");
        empty_state.append(&title);

        let subtitle = gtk::Label::new(Some(
            "Start a conversation with any local or cloud AI provider.",
        ));
        subtitle.add_css_class("chat-empty-subtitle");
        empty_state.append(&subtitle);

        container.append(&empty_state);

        // Messages list ScrolledWindow
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_vexpand(true);
        scrolled.set_visible(false);

        let messages_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        messages_box.add_css_class("chat-messages-area");
        scrolled.set_child(Some(&messages_box));

        container.append(&scrolled);

        Self {
            container,
            scrolled,
            messages_box,
            empty_state,
            typing_indicator: None,
            typing_in_bubble: false,
            current_stream_row: None,
        }
    }

    pub fn set_conversation(&mut self, conversation: &Conversation, messages: &[Message]) {
        self.clear();
        self.empty_state.set_visible(false);
        self.scrolled.set_visible(true);

        for msg in messages {
            self.append_message(msg);
        }

        self.scroll_to_bottom();
    }

    pub fn set_empty(&mut self) {
        self.clear();
        self.scrolled.set_visible(false);
        self.empty_state.set_visible(true);
    }

    pub fn append_message(&self, message: &Message) {
        self.reveal_messages();
        let bubble = chat_bubble::make_chat_bubble(message);
        self.messages_box.append(&bubble);
        self.scroll_to_bottom();
    }

    pub fn start_stream_bubble(&mut self) {
        self.reveal_messages();
        let (row, label, bubble) = chat_bubble::make_streaming_bubble();
        self.messages_box.append(&row);
        self.current_stream_row = Some((row, label, bubble));
        self.scroll_to_bottom();
    }

    pub fn append_stream_chunk(&self, delta: &str) {
        if let Some((_, ref label, _)) = self.current_stream_row {
            let current_text = label.text().to_string();
            label.set_label(&format!("{}{}", current_text, delta));
            self.scroll_to_bottom();
        }
    }

    pub fn end_stream_bubble(&mut self) {
        self.current_stream_row = None;
    }

    pub fn set_typing(&mut self, is_typing: bool) {
        if is_typing {
            self.reveal_messages();
            if self.typing_indicator.is_none() {
                // Embed the dots inside the streaming bubble when one is
                // active so the "thinking" state is part of the reply,
                // instead of a separate row next to it.
                if let Some((_, _, bubble)) = &self.current_stream_row {
                    let dots = typing_indicator::make_typing_dots();
                    bubble.append(&dots);
                    self.typing_indicator = Some(dots);
                    self.typing_in_bubble = true;
                } else {
                    let indicator = typing_indicator::make_typing_indicator();
                    self.messages_box.append(&indicator);
                    self.typing_indicator = Some(indicator);
                    self.typing_in_bubble = false;
                }
                self.scroll_to_bottom();
            }
        } else if let Some(indicator) = self.typing_indicator.take() {
            if self.typing_in_bubble {
                if let Some((_, _, bubble)) = &self.current_stream_row {
                    bubble.remove(&indicator);
                }
            } else {
                self.messages_box.remove(&indicator);
            }
            self.typing_in_bubble = false;
        }
    }

    /// Render a failure that happened while streaming: discard the empty
    /// streaming bubble and show a formatted error bubble in its place.
    pub fn show_stream_error(&mut self, message: &str) {
        if let Some((row, _, _)) = self.current_stream_row.take() {
            self.messages_box.remove(&row);
        }
        self.show_error(message.to_string());
    }

    /// Show a nicely formatted error bubble in the conversation.
    pub fn show_error(&self, message: String) {
        self.reveal_messages();

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.add_css_class("chat-message-row");
        row.set_hexpand(true);
        row.set_halign(gtk::Align::Start);
        row.set_margin_start(16);
        row.set_margin_end(80);
        row.set_margin_top(2);
        row.set_margin_bottom(2);

        let bubble = gtk::Box::new(gtk::Orientation::Vertical, 4);
        bubble.add_css_class("chat-bubble");
        bubble.add_css_class("chat-error-bubble");
        bubble.set_margin_start(8);
        bubble.set_margin_end(8);

        // Header: warning icon + title
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_halign(gtk::Align::Start);

        let icon = gtk::Image::from_icon_name("dialog-error-symbolic");
        icon.add_css_class("chat-error-icon");
        header.append(&icon);

        let title = gtk::Label::new(Some("Something went wrong"));
        title.add_css_class("chat-error-title");
        title.set_xalign(0.0);
        title.set_halign(gtk::Align::Start);
        header.append(&title);

        bubble.append(&header);

        let label = gtk::Label::new(Some(&message));
        label.add_css_class("chat-error-text");
        label.set_wrap(true);
        label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
        label.set_xalign(0.0);
        label.set_halign(gtk::Align::Start);
        label.set_selectable(true);
        bubble.append(&label);

        row.append(&bubble);
        self.messages_box.append(&row);
        self.scroll_to_bottom();
    }

    pub fn clear(&mut self) {
        while let Some(child) = self.messages_box.first_child() {
            self.messages_box.remove(&child);
        }
        self.typing_indicator = None;
        self.typing_in_bubble = false;
        self.current_stream_row = None;
    }

    /// Show the message list area and hide the welcome placeholder.
    fn reveal_messages(&self) {
        self.empty_state.set_visible(false);
        self.scrolled.set_visible(true);
    }

    fn scroll_to_bottom(&self) {
        // Safe asynchronous page scrolling down
        let adj = self.scrolled.vadjustment();
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            adj.set_value(adj.upper() - adj.page_size());
            glib::ControlFlow::Break
        });
    }
}
