//! ImageView widget — zoomable inline image viewer with download button.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::path::Path;

/// Create a full image viewer overlay dialog for the given image path.
pub fn open_image_viewer(parent: &impl IsA<gtk::Widget>, path: &Path) {
    let dialog = gtk::Window::new();
    dialog.set_title(Some(
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Image"),
    ));
    dialog.set_default_size(800, 600);
    dialog.set_modal(true);

    if let Some(root) = parent.root() {
        if let Ok(win) = root.downcast::<gtk::Window>() {
            dialog.set_transient_for(Some(&win));
        }
    }

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);

    // Toolbar
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.set_margin_start(12);
    toolbar.set_margin_end(12);
    toolbar.set_margin_top(8);
    toolbar.set_margin_bottom(8);

    let download_btn = gtk::Button::with_label("Download");
    download_btn.set_icon_name("document-save-symbolic");
    toolbar.append(&download_btn);

    let copy_btn = gtk::Button::with_label("Copy");
    copy_btn.set_icon_name("edit-copy-symbolic");
    toolbar.append(&copy_btn);

    let close_btn = gtk::Button::with_label("Close");
    close_btn.set_hexpand(true);
    close_btn.set_halign(gtk::Align::End);
    toolbar.append(&close_btn);

    vbox.append(&toolbar);

    // Image
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hexpand(true);

    let picture = gtk::Picture::for_filename(path);
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_vexpand(true);
    picture.set_hexpand(true);
    scrolled.set_child(Some(&picture));
    vbox.append(&scrolled);

    dialog.set_child(Some(&vbox));

    let dialog_clone = dialog.clone();
    close_btn.connect_clicked(move |_| dialog_clone.close());

    dialog.present();
}
