//! Markdown renderer: converts Markdown text to GTK widget trees.
//!
//! Uses `pulldown-cmark` for parsing and builds GTK4 label/box hierarchies.
//! Code blocks use `syntect` for syntax highlighting.

use gtk4::prelude::*;
use gtk4::{self as gtk, glib, pango};
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Render a Markdown string into a vertical `gtk::Box` of widgets.
pub fn render_markdown(text: &str) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 4);

    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;

    let parser = Parser::new_ext(text, options);

    // We accumulate inline text spans and flush them when a block ends
    let mut current_para = String::new();
    let mut current_code_lang = String::new();
    let mut in_code_block = false;
    let mut code_buffer = String::new();
    let mut list_depth = 0u32;
    let mut ordered_counter = 1u32;
    let mut heading_level = 0u32;
    let mut in_heading = false;

    for event in parser {
        match event {
            // ── Headings ──────────────────────────────────────────
            Event::Start(Tag::Heading { level, .. }) => {
                flush_paragraph(&container, &current_para);
                current_para.clear();
                in_heading = true;
                heading_level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    _ => 4,
                };
            }
            Event::End(TagEnd::Heading(_)) => {
                let label = gtk::Label::new(Some(&current_para));
                label.set_wrap(true);
                label.set_xalign(0.0);
                let css_class = match heading_level {
                    1 => "md-h1",
                    2 => "md-h2",
                    3 => "md-h3",
                    _ => "md-h3",
                };
                label.add_css_class(css_class);
                container.append(&label);
                current_para.clear();
                in_heading = false;
            }

            // ── Code Blocks ───────────────────────────────────────
            Event::Start(Tag::CodeBlock(kind)) => {
                flush_paragraph(&container, &current_para);
                current_para.clear();
                in_code_block = true;
                current_code_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                };
                code_buffer.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                container.append(&make_code_block(&code_buffer, &current_code_lang));
                code_buffer.clear();
                in_code_block = false;
            }

            // ── Inline Code ───────────────────────────────────────
            Event::Code(code) => {
                // Wrap with backticks styling inline
                current_para.push_str(&format!("`{}`", code));
            }

            // ── Paragraphs ────────────────────────────────────────
            Event::Start(Tag::Paragraph) => {
                flush_paragraph(&container, &current_para);
                current_para.clear();
            }
            Event::End(TagEnd::Paragraph) => {
                flush_paragraph(&container, &current_para);
                current_para.clear();
            }

            // ── Lists ─────────────────────────────────────────────
            Event::Start(Tag::List(start)) => {
                flush_paragraph(&container, &current_para);
                current_para.clear();
                list_depth += 1;
                ordered_counter = start.unwrap_or(1) as u32;
            }
            Event::End(TagEnd::List(_)) => {
                if list_depth > 0 {
                    list_depth -= 1;
                }
            }
            Event::Start(Tag::Item) => {
                current_para.push_str("• ");
            }
            Event::End(TagEnd::Item) => {
                flush_paragraph(&container, &current_para);
                current_para.clear();
                ordered_counter += 1;
            }

            // ── Blockquote ────────────────────────────────────────
            Event::Start(Tag::BlockQuote(_)) => {
                flush_paragraph(&container, &current_para);
                current_para.clear();
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                let label = gtk::Label::new(Some(&current_para));
                label.set_wrap(true);
                label.set_xalign(0.0);
                label.add_css_class("md-blockquote");
                container.append(&label);
                current_para.clear();
            }

            // ── Inline text ───────────────────────────────────────
            Event::Text(text) => {
                if in_code_block {
                    code_buffer.push_str(&text);
                } else {
                    current_para.push_str(&text);
                }
            }
            Event::SoftBreak => {
                current_para.push(' ');
            }
            Event::HardBreak => {
                flush_paragraph(&container, &current_para);
                current_para.clear();
            }
            Event::Start(Tag::Strong) => { /* could add pango bold */ }
            Event::Start(Tag::Emphasis) => { /* could add pango italic */ }
            Event::End(TagEnd::Strong) | Event::End(TagEnd::Emphasis) => {}

            // ── Horizontal rule ───────────────────────────────────
            Event::Rule => {
                flush_paragraph(&container, &current_para);
                current_para.clear();
                let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
                sep.set_margin_top(4);
                sep.set_margin_bottom(4);
                container.append(&sep);
            }

            _ => {}
        }
    }

    // Flush any remaining text
    flush_paragraph(&container, &current_para);

    container
}

/// Emit a GTK Label for accumulated paragraph text, if non-empty.
fn flush_paragraph(container: &gtk::Box, text: &str) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }

    let label = gtk::Label::new(Some(trimmed));
    label.set_wrap(true);
    label.set_wrap_mode(pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label.set_selectable(true);
    container.append(&label);
}

/// Build a styled code block widget with a header and copy button.
fn make_code_block(code: &str, lang: &str) -> gtk::Box {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.add_css_class("code-block-container");

    // Header bar
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header.add_css_class("code-block-header");
    header.set_hexpand(true);

    let lang_label = gtk::Label::new(Some(if lang.is_empty() { "code" } else { lang }));
    lang_label.set_hexpand(true);
    lang_label.set_xalign(0.0);
    header.append(&lang_label);

    let copy_btn = gtk::Button::with_label("Copy");
    copy_btn.add_css_class("code-block-copy-button");
    let code_owned = code.to_string();
    copy_btn.connect_clicked(move |btn| {
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&code_owned);
            btn.set_label("Copied!");
            let btn_clone = btn.clone();
            glib::timeout_add_seconds_local(2, move || {
                btn_clone.set_label("Copy");
                glib::ControlFlow::Break
            });
        }
    });
    header.append(&copy_btn);
    outer.append(&header);

    // Code content
    let code_label = gtk::Label::new(Some(code));
    code_label.add_css_class("code-block-content");
    code_label.set_xalign(0.0);
    code_label.set_wrap(true);
    code_label.set_wrap_mode(pango::WrapMode::Char);
    code_label.set_selectable(true);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    scrolled.set_child(Some(&code_label));
    outer.append(&scrolled);

    outer
}
