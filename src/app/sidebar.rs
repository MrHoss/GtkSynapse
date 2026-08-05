//! SidebarPanel - quick-access media buttons, chat list, search and actions.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::sync::{Arc, Mutex};

use crate::core::models::{Conversation, ConversationKind};
use crate::storage::StorageManager;

use super::generate::GenKind;

/// An action the user picked from a chat's context menu.
#[derive(Debug, Clone)]
pub enum ChatAction {
    Rename { id: String, current_title: String },
    Delete { id: String, title: String },
    Favorite { id: String, is_favorite: bool },
    Duplicate { id: String },
    Export { id: String, title: String },
}

pub struct SidebarPanel {
    pub container: gtk::Box,
    fixed_list: gtk::ListBox,
    list_box: gtk::ListBox,
    search_entry: gtk::SearchEntry,
    storage: Arc<StorageManager>,
    on_chat_selected: Option<Arc<dyn Fn(String) + 'static>>,
    on_new_chat: Option<Arc<dyn Fn() + 'static>>,
    on_open_settings: Option<Arc<dyn Fn() + 'static>>,
    on_open_media: Option<Arc<dyn Fn(GenKind) + 'static>>,
    on_chat_action: Option<Arc<dyn Fn(ChatAction) + 'static>>,
}

impl SidebarPanel {
    pub fn new(storage: Arc<StorageManager>) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.add_css_class("sidebar-panel");

        // Header with app title.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.add_css_class("sidebar-header");

        let title_label = gtk::Label::new(Some("GtkSynapse"));
        title_label.add_css_class("title");
        title_label.set_hexpand(true);
        title_label.set_xalign(0.0);
        header.append(&title_label);

        container.append(&header);

        // Search Entry (always at the top).
        let search_entry = gtk::SearchEntry::new();
        search_entry.add_css_class("sidebar-search");
        search_entry.set_placeholder_text(Some("Search chats..."));
        container.append(&search_entry);

        // Fixed quick actions, styled like the chat list rows. Kept in their
        // own list so a divider can separate them from the dynamic chats below.
        let fixed_list = gtk::ListBox::new();
        fixed_list.set_selection_mode(gtk::SelectionMode::None);

        let new_chat_row =
            make_fixed_row("New Chat", "document-new-symbolic", "Start a conversation");
        new_chat_row.set_widget_name("new-chat");
        fixed_list.append(&new_chat_row);

        let image_row = make_fixed_row("Image", "image-x-generic-symbolic", "Image generation");
        image_row.set_widget_name("image");
        fixed_list.append(&image_row);

        let video_row = make_fixed_row("Video", "video-x-generic-symbolic", "Video generation");
        video_row.set_widget_name("video");
        fixed_list.append(&video_row);

        let audio_row = make_fixed_row("Audio", "audio-x-generic-symbolic", "Audio generation");
        audio_row.set_widget_name("audio");
        fixed_list.append(&audio_row);

        container.append(&fixed_list);

        // Divider separating the fixed actions from the dynamic chat list.
        // A plain libadwaita separator (1px line, theme-colored) inset to
        // match the sidebar's horizontal rhythm.
        let divider = gtk::Separator::new(gtk::Orientation::Horizontal);
        divider.set_margin_top(4);
        divider.set_margin_bottom(4);
        divider.set_margin_start(16);
        divider.set_margin_end(16);
        container.append(&divider);

        // Scrolled List Container
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::Single);
        scrolled.set_child(Some(&list_box));
        container.append(&scrolled);

        let sidebar = Self {
            container,
            fixed_list,
            list_box,
            search_entry,
            storage,
            on_chat_selected: None,
            on_new_chat: None,
            on_open_settings: None,
            on_open_media: None,
            on_chat_action: None,
        };

        sidebar.reload_conversations();
        sidebar
    }

    pub fn set_on_chat_selected<F: Fn(String) + 'static>(&mut self, callback: F) {
        self.on_chat_selected = Some(Arc::new(callback));
    }

    pub fn set_on_new_chat<F: Fn() + 'static>(&mut self, callback: F) {
        self.on_new_chat = Some(Arc::new(callback));
    }

    pub fn set_on_open_settings<F: Fn() + 'static>(&mut self, callback: F) {
        self.on_open_settings = Some(Arc::new(callback));
    }

    pub fn set_on_open_media<F: Fn(GenKind) + 'static>(&mut self, callback: F) {
        self.on_open_media = Some(Arc::new(callback));
    }

    pub fn set_on_chat_action<F: Fn(ChatAction) + 'static>(&mut self, callback: F) {
        self.on_chat_action = Some(Arc::new(callback));
    }

    pub fn reload_conversations(&self) {
        // Clear current list items
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        // Only chat conversations are listed; media conversations are kept
        // hidden (their generated files still persist in storage).
        if let Ok(convs) = self.storage.list_conversations() {
            for conv in convs
                .into_iter()
                .filter(|c| c.kind == ConversationKind::Chat)
            {
                let row = self.create_row_for_conversation(&conv);
                self.list_box.append(&row);
            }
        }
    }

    /// Highlight the given fixed action row (New Chat / Image / Video / Audio)
    /// and clear any dynamic list selection. Passing a name that matches no
    /// fixed row clears every highlight.
    pub fn set_fixed_active(&self, name: &str) {
        let mut child = self.fixed_list.first_child();
        while let Some(widget) = child {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() {
                if row.widget_name().as_str() == name {
                    row.add_css_class("selected");
                } else {
                    row.remove_css_class("selected");
                }
            }
            child = widget.next_sibling();
        }
        self.list_box.unselect_all();
    }

    /// Remove the highlight from all fixed action rows.
    pub fn clear_fixed_active(&self) {
        let mut child = self.fixed_list.first_child();
        while let Some(widget) = child {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() {
                row.remove_css_class("selected");
            }
            child = widget.next_sibling();
        }
    }

    fn create_row_for_conversation(&self, conv: &Conversation) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("conversation-row");
        row.set_widget_name(&conv.id);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);

        // Favorite icon (star)
        let star_icon = gtk::Image::from_icon_name("non-starred-symbolic");
        star_icon.add_css_class("conversation-favorite");
        if conv.is_favorite {
            star_icon.set_icon_name(Some("starred-symbolic"));
        } else {
            star_icon.set_visible(false);
        }
        hbox.append(&star_icon);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_hexpand(true);

        let title_lbl = gtk::Label::new(Some(&conv.title));
        title_lbl.add_css_class("conversation-title");
        title_lbl.set_xalign(0.0);
        title_lbl.set_ellipsize(pango::EllipsizeMode::End);
        vbox.append(&title_lbl);

        let subtitle_lbl = match conv.kind {
            ConversationKind::Chat => {
                let label =
                    gtk::Label::new(Some(&format!("{} • {}", conv.provider_id, conv.model_id)));
                label
            }
            ConversationKind::Image => {
                let label = gtk::Label::new(Some("Image generation"));
                label
            }
            ConversationKind::Video => {
                let label = gtk::Label::new(Some("Video generation"));
                label
            }
            ConversationKind::Audio => {
                let label = gtk::Label::new(Some("Audio generation"));
                label
            }
        };
        subtitle_lbl.add_css_class("conversation-subtitle");
        subtitle_lbl.set_xalign(0.0);
        subtitle_lbl.set_ellipsize(pango::EllipsizeMode::End);
        vbox.append(&subtitle_lbl);

        hbox.append(&vbox);

        // Context menu with per-chat actions (rename, favorite, branch, export, delete).
        let menu_btn = gtk::MenuButton::new();
        menu_btn.set_icon_name("view-more-symbolic");
        menu_btn.add_css_class("flat");

        let popover = gtk::Popover::new();
        menu_btn.set_popover(Some(&popover));

        let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        menu_box.set_width_request(200);
        popover.set_child(Some(&menu_box));

        let id = conv.id.clone();
        let title = conv.title.clone();
        let is_fav = conv.is_favorite;
        let action_cb = self.on_chat_action.clone();

        let rename_btn = make_menu_item("Rename", "edit-rename-symbolic");
        {
            let pop = popover.clone();
            let cb = action_cb.clone();
            let id = id.clone();
            let title = title.clone();
            rename_btn.connect_clicked(move |_| {
                pop.popdown();
                if let Some(ref cb) = cb {
                    cb(ChatAction::Rename {
                        id: id.clone(),
                        current_title: title.clone(),
                    });
                }
            });
        }
        menu_box.append(&rename_btn);

        let fav_label = if is_fav {
            "Remove from Favorites"
        } else {
            "Add to Favorites"
        };
        let fav_btn = make_menu_item(fav_label, "starred-symbolic");
        {
            let pop = popover.clone();
            let cb = action_cb.clone();
            let id = id.clone();
            fav_btn.connect_clicked(move |_| {
                pop.popdown();
                if let Some(ref cb) = cb {
                    cb(ChatAction::Favorite {
                        id: id.clone(),
                        is_favorite: is_fav,
                    });
                }
            });
        }
        menu_box.append(&fav_btn);

        let branch_btn = make_menu_item("Create Branch", "edit-copy-symbolic");
        {
            let pop = popover.clone();
            let cb = action_cb.clone();
            let id = id.clone();
            branch_btn.connect_clicked(move |_| {
                pop.popdown();
                if let Some(ref cb) = cb {
                    cb(ChatAction::Duplicate { id: id.clone() });
                }
            });
        }
        menu_box.append(&branch_btn);

        let export_btn = make_menu_item("Export…", "document-save-symbolic");
        {
            let pop = popover.clone();
            let cb = action_cb.clone();
            let id = id.clone();
            let title = title.clone();
            export_btn.connect_clicked(move |_| {
                pop.popdown();
                if let Some(ref cb) = cb {
                    cb(ChatAction::Export {
                        id: id.clone(),
                        title: title.clone(),
                    });
                }
            });
        }
        menu_box.append(&export_btn);

        menu_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let delete_btn = make_menu_item("Delete…", "edit-delete-symbolic");
        {
            let pop = popover.clone();
            let cb = action_cb.clone();
            let id = id.clone();
            let title = title.clone();
            delete_btn.connect_clicked(move |_| {
                pop.popdown();
                if let Some(ref cb) = cb {
                    cb(ChatAction::Delete {
                        id: id.clone(),
                        title: title.clone(),
                    });
                }
            });
        }
        menu_box.append(&delete_btn);

        hbox.append(&menu_btn);

        row.set_child(Some(&hbox));
        row
    }

    pub fn connect_events(&self, sidebar_arc: Arc<Mutex<Self>>) {
        let sidebar_clone = sidebar_arc.clone();
        self.list_box.connect_row_selected(move |_, row| {
            if let Some(r) = row {
                let id = r.widget_name().to_string();
                // Use try_lock instead of lock: programmatic selection (e.g.
                // opening a media button) emits row-selected while the sidebar
                // is already locked, and that reentrancy would deadlock a
                // std Mutex. Skipping the callback there is also correct,
                // since it is only needed for real user clicks.
                let cb = match sidebar_clone.try_lock() {
                    Ok(sb) => {
                        // A real click on a chat row: clear the fixed-action
                        // highlight so only the list selection remains.
                        sb.clear_fixed_active();
                        sb.on_chat_selected.clone()
                    }
                    Err(_) => None,
                };
                if let Some(cb) = cb {
                    cb(id);
                }
            }
        });

        let sidebar_clone = sidebar_arc.clone();
        self.search_entry.connect_search_changed(move |entry| {
            let query = entry.text().to_string();
            let sb = sidebar_clone.lock().unwrap();
            sb.filter_conversations(&query);
        });
    }

    pub fn connect_buttons(&self, sidebar_arc: Arc<Mutex<Self>>) {
        let sidebar_clone = sidebar_arc.clone();
        self.fixed_list.connect_row_activated(move |_, row| {
            let name = row.widget_name().to_string();
            match name.as_str() {
                "new-chat" => {
                    let cb = sidebar_clone.lock().unwrap().on_new_chat.clone();
                    if let Some(cb) = cb {
                        cb();
                    }
                }
                "image" | "video" | "audio" => {
                    let cb = sidebar_clone.lock().unwrap().on_open_media.clone();
                    let kind = match name.as_str() {
                        "image" => GenKind::Image,
                        "video" => GenKind::Video,
                        _ => GenKind::Audio,
                    };
                    if let Some(cb) = cb {
                        cb(kind);
                    }
                }
                _ => {}
            }
        });
    }

    fn filter_conversations(&self, query: &str) {
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        let convs = if query.trim().is_empty() {
            self.storage.list_conversations().unwrap_or_default()
        } else {
            self.storage.search_conversations(query).unwrap_or_default()
        };

        for conv in convs
            .into_iter()
            .filter(|c| c.kind == ConversationKind::Chat)
        {
            let row = self.create_row_for_conversation(&conv);
            self.list_box.append(&row);
        }
    }
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

/// Build a fixed sidebar row styled identically to the conversation rows:
/// icon + title + subtitle, two-line, with the same hover/selected look.
fn make_fixed_row(label: &str, icon: &str, subtitle: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("conversation-row");
    row.set_activatable(true);

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);

    let icon_img = gtk::Image::from_icon_name(icon);
    hbox.append(&icon_img);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
    vbox.set_hexpand(true);

    let title_lbl = gtk::Label::new(Some(label));
    title_lbl.add_css_class("conversation-title");
    title_lbl.set_xalign(0.0);
    title_lbl.set_ellipsize(pango::EllipsizeMode::End);
    vbox.append(&title_lbl);

    let subtitle_lbl = gtk::Label::new(Some(subtitle));
    subtitle_lbl.add_css_class("conversation-subtitle");
    subtitle_lbl.set_xalign(0.0);
    subtitle_lbl.set_ellipsize(pango::EllipsizeMode::End);
    vbox.append(&subtitle_lbl);

    hbox.append(&vbox);
    row.set_child(Some(&hbox));
    row
}

use gtk4::pango;
