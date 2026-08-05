//! Auxiliary dialogs — rename, delete confirmation, export.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;

/// Show a rename conversation dialog.
/// Calls `on_confirm` with the new title if the user confirms.
pub fn show_rename_dialog(
    parent: &adw::ApplicationWindow,
    current_title: &str,
    on_confirm: impl Fn(String) + 'static,
) {
    let dialog = adw::AlertDialog::new(
        Some("Rename Conversation"),
        Some("Enter a new name for this conversation."),
    );

    let entry = gtk::Entry::new();
    entry.set_text(current_title);
    entry.set_activates_default(true);
    dialog.set_extra_child(Some(&entry));

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("rename", "Rename");
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("rename"));

    let entry_clone = entry.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "rename" {
            let text = entry_clone.text().to_string();
            if !text.is_empty() {
                on_confirm(text);
            }
        }
    });

    dialog.present(Some(parent));
}

/// Show a delete confirmation dialog.
/// Calls `on_confirm` if the user confirms deletion.
pub fn show_delete_dialog(
    parent: &adw::ApplicationWindow,
    conversation_title: &str,
    on_confirm: impl Fn() + 'static,
) {
    let body = format!(
        "Are you sure you want to delete \"{}\"? This action cannot be undone.",
        conversation_title
    );
    let dialog = adw::AlertDialog::new(Some("Delete Conversation"), Some(&body));

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));

    dialog.connect_response(None, move |_, response| {
        if response == "delete" {
            on_confirm();
        }
    });

    dialog.present(Some(parent));
}

/// Show an export dialog to save conversation as Markdown.
pub fn show_export_dialog(
    parent: &adw::ApplicationWindow,
    markdown_content: String,
    default_name: &str,
) {
    let dialog = gtk::FileDialog::new();
    dialog.set_title("Export Conversation");
    dialog.set_initial_name(Some(&format!("{}.md", default_name)));

    let filter = gtk::FileFilter::new();
    filter.add_mime_type("text/markdown");
    filter.add_pattern("*.md");
    filter.set_name(Some("Markdown files"));

    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));

    let parent_clone = parent.clone();
    dialog.save(Some(parent), gtk::gio::Cancellable::NONE, move |result| {
        if let Ok(file) = result {
            if let Some(path) = file.path() {
                match std::fs::write(&path, &markdown_content) {
                    Ok(_) => {
                        let toast = adw::Toast::new("Conversation exported successfully");
                        if let Some(win) = parent_clone
                            .upcast_ref::<gtk::Widget>()
                            .root()
                            .and_then(|r| r.downcast::<adw::ApplicationWindow>().ok())
                        {
                            // would need overlay ref
                        }
                        tracing::info!("Exported conversation to {:?}", path);
                    }
                    Err(e) => {
                        tracing::error!("Failed to export: {}", e);
                    }
                }
            }
        }
    });
}

/// Show the application About dialog.
pub fn show_about_dialog(parent: &adw::ApplicationWindow) {
    let dialog = adw::AboutDialog::new();
    dialog.set_application_name("GtkSynapse");
    dialog.set_developer_name("GtkSynapse");
    dialog.set_copyright("© GtkSynapse");
    dialog.set_license_type(gtk::License::MitX11);
    dialog.present(Some(parent));
}

use gtk4::gio;
