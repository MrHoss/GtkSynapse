//! MainWindow - links SidebarPanel, ChatView, and InputBar in a responsive Libadwaita layout.

use gtk4::prelude::*;
use gtk4::{self as gtk, glib};
use libadwaita::prelude::*;
use libadwaita as adw;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use base64::Engine;
use futures::StreamExt;

use crate::app::chat::ChatView;
use crate::app::dialogs;
use crate::app::downloads::DownloadsIndicator;
use crate::app::generate::{friendly_error, GenKind, ProviderCap};
use crate::app::input_bar::InputBar;
use crate::app::media_view::MediaView;
use crate::app::ollama_dialog;
use crate::app::settings;
use crate::app::sidebar::{ChatAction, SidebarPanel};
use crate::core::chat::{ChatEvent, ChatSession};
use crate::core::models::{
    Conversation, ConversationKind, GeneratedMedia, ImageGenOptions, Message, MessageRole,
    ModelInfo, VideoProgress, VideoRequest, VideoStatus,
};
use crate::ollama;
use crate::providers::manager::ProviderManager;
use crate::providers::curated_models;
use crate::storage::StorageManager;

/// Events emitted by the generation background task.
enum GenerateEvent {
    Image { data: Vec<u8>, mime: String },
    Video(VideoProgress),
    Error(String),
    Done,
}

/// Fetch the credit balance for the provider currently selected in the
/// generate panel and push the result back to the UI on the main thread.
fn refresh_generate_balance(
    panel: Arc<Mutex<MediaView>>,
    manager: &Arc<ProviderManager>,
    rt: &tokio::runtime::Handle,
) {
    let pid = panel.lock().unwrap().selected_provider_id();
    let Some(pid) = pid else { return };

    let provider = match manager.get(&pid) {
        Some(p) => p,
        None => {
            panel.lock().unwrap().set_balance(None, None);
            return;
        }
    };

    let (tx, mut rx) = mpsc::channel::<Option<Result<(i64, i64), String>>>(1);
    let panel_for_task = panel.clone();
    let rt = rt.clone();
    rt.spawn(async move {
        let balance = match provider.account_balance().await {
            Some(Ok(b)) => Some(Ok((
                b.monthly_credits.unwrap_or(0),
                b.package_credits.unwrap_or(0),
            ))),
            Some(Err(e)) => Some(Err(e.to_string())),
            None => None,
        };
        let _ = tx.send(balance).await;
    });

    glib::timeout_add_local(
        std::time::Duration::from_millis(15),
        move || match rx.try_recv() {
            Ok(bal) => {
                match bal {
                    Some(Ok((m, p))) => {
                        panel_for_task.lock().unwrap().set_balance(Some(m), Some(p))
                    }
                    Some(Err(msg)) => {
                        panel_for_task.lock().unwrap().set_balance_error(&msg)
                    }
                    None => panel_for_task.lock().unwrap().set_balance(None, None),
                }
                glib::ControlFlow::Break
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                glib::ControlFlow::Continue
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                glib::ControlFlow::Break
            }
        },
    );
}

/// Programmatically set the provider dropdown of the input bar without
/// triggering the user-driven "provider changed" handler.
///
/// The shared syncing flag is set (and the input-bar mutex released) before
/// `set_selected`, which fires the notify handler synchronously; the handler
/// reads the flag and bails out, avoiding a re-entrant lock on the input bar.
fn set_input_provider(input_bar: &Arc<Mutex<InputBar>>, id: &str) {
    let selector = input_bar.lock().unwrap().provider_selector();
    let idx = input_bar.lock().unwrap().provider_index(id);
    let syncing = input_bar.lock().unwrap().syncing_flag();
    let Some(idx) = idx else { return };

    *syncing.lock().unwrap() = true;
    selector.set_selected(idx);
    *syncing.lock().unwrap() = false;
    input_bar.lock().unwrap().refresh_attach_state();
}

/// Fetch the model list for the currently selected provider in the input
/// bar (async, on the Tokio runtime) and populate the model dropdown.
///
/// Falls back to the provider's curated list if the live fetch fails or is
/// empty. `desired_model`, when given, is re-selected after the list loads
/// (used when opening a conversation whose model may not be the first entry).
/// Stale results are discarded if the user switched providers meanwhile.
pub(crate) fn load_models_for_provider(
    input_bar: &Arc<Mutex<InputBar>>,
    manager: &Arc<ProviderManager>,
    runtime: &tokio::runtime::Handle,
    desired_model: Option<String>,
) {
    let pid = match input_bar.lock().unwrap().selected_provider_id() {
        Some(p) => p,
        None => return,
    };
    let Some(provider) = manager.get(&pid) else { return };

    let ib = input_bar.clone();
    let rt = runtime.clone();
    let pid_task = pid.clone();
    let (tx, mut rx) = mpsc::channel::<Vec<ModelInfo>>(1);
    rt.spawn(async move {
        let models = match provider.list_models().await {
            Ok(m) if !m.is_empty() => m,
            _ => curated_models(&pid_task),
        };
        let _ = tx.send(models).await;
    });

    glib::timeout_add_local(
        std::time::Duration::from_millis(15),
        move || match rx.try_recv() {
            Ok(models) => {
                let mut ib_guard = ib.lock().unwrap();
                let current_pid = ib_guard.selected_provider_id();
                if current_pid.as_deref() == Some(pid.as_str()) {
                    ib_guard.update_models(models);
                    if let Some(desired) = &desired_model {
                        ib_guard.select_model_id(desired);
                    }
                }
                glib::ControlFlow::Break
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                glib::ControlFlow::Continue
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                glib::ControlFlow::Break
            }
        },
    );
}

/// Open a media conversation in the chat-like media view: set its kind,
/// reload its persisted results, and switch the content area to the media
/// page. Called from the sidebar quick-access buttons and when a media
/// conversation is selected from the list.
#[allow(clippy::too_many_arguments)]
fn open_media_conversation(
    media_view: &Arc<Mutex<MediaView>>,
    storage: &Arc<StorageManager>,
    manager: &Arc<ProviderManager>,
    runtime: &tokio::runtime::Handle,
    stack: &gtk::Stack,
    media_session: &Arc<Mutex<Option<(Conversation, GenKind)>>>,
    conv: Conversation,
) {
    let kind = match conv.kind {
        ConversationKind::Image => GenKind::Image,
        ConversationKind::Video => GenKind::Video,
        ConversationKind::Audio => GenKind::Audio,
        ConversationKind::Chat => return,
    };

    let providers: Vec<ProviderCap> = manager
        .all()
        .into_iter()
        .map(|p| ProviderCap {
            id: p.id().to_string(),
            name: p.name().to_string(),
            supports_image: p.supports_image_generation(),
            supports_video: p.supports_video_generation(),
        })
        .collect();

    {
        let mut mv = media_view.lock().unwrap();
        mv.clear_results();
        mv.set_kind(kind);
        mv.set_providers(providers);
        mv.show_status("");
    }

    // Reload persisted results from the conversation's assistant messages.
    if let Ok(messages) = storage.list_messages(&conv.id) {
        for msg in messages {
            if msg.role == MessageRole::Assistant {
                if let Ok(items) = serde_json::from_str::<Vec<GeneratedMedia>>(&msg.content) {
                    media_view.lock().unwrap().render_media(&items);
                }
            }
        }
    }

    *media_session.lock().unwrap() = Some((conv, kind));
    stack.set_visible_child_name("media");

    refresh_generate_balance(media_view.clone(), manager, runtime);
}

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    sidebar: Arc<Mutex<SidebarPanel>>,
    chat_view: Arc<Mutex<ChatView>>,
    input_bar: Arc<Mutex<InputBar>>,
    media_view: Arc<Mutex<MediaView>>,
    content_stack: gtk::Stack,
    toast_overlay: adw::ToastOverlay,
    storage: Arc<StorageManager>,
    manager: Arc<ProviderManager>,
    current_session: Arc<Mutex<Option<ChatSession>>>,
    current_media_session: Arc<Mutex<Option<(Conversation, GenKind)>>>,
    runtime: tokio::runtime::Handle,
    cancel_handle: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    downloads: DownloadsIndicator,
}

impl MainWindow {
    pub fn new(
        app: &adw::Application,
        storage: Arc<StorageManager>,
        manager: Arc<ProviderManager>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        let window = adw::ApplicationWindow::new(app);
        window.set_title(Some("GtkSynapse"));
        window.set_default_size(960, 680);
        window.add_css_class("main-window");

        let split_view = adw::NavigationSplitView::new();

        // ── Sidebar Panel ──
        let sidebar = Arc::new(Mutex::new(SidebarPanel::new(storage.clone())));
        let sidebar_page = adw::NavigationPage::new(&sidebar.lock().unwrap().container, "Sidebar");
        sidebar_page.set_title("GtkSynapse");
        split_view.set_sidebar(Some(&sidebar_page));

        // ── Chat Main Area View ──
        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

        // Header
        let header_bar = adw::HeaderBar::new();
        let (menu_btn, chat_menu_settings, chat_menu_about) =
            crate::app::media_view::build_header_menu();
        let downloads = DownloadsIndicator::new(runtime.clone());
        header_bar.pack_end(&downloads.header_button());
        header_bar.pack_end(&menu_btn);
        content_box.append(&header_bar);

        // Chat View area
        let chat_view = Arc::new(Mutex::new(ChatView::new()));
        content_box.append(&chat_view.lock().unwrap().container);

        // Input Bar area
        let input_bar = Arc::new(Mutex::new(InputBar::new()));
        content_box.append(&input_bar.lock().unwrap().container);

        let chat_page = adw::NavigationPage::new(&content_box, "Chat");
        chat_page.set_title("Chat");

        // ── Media Generation View (chat-like) ──
        let media_view = Arc::new(Mutex::new(MediaView::new()));
        let media_page = adw::NavigationPage::new(&media_view.lock().unwrap().container, "Media");
        media_page.set_title("Generate Media");

        // Content stack switches between chat and media pages.
        let content_stack = gtk::Stack::new();
        content_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        content_stack.add_named(&chat_page, Some("chat"));
        content_stack.add_named(&media_page, Some("media"));

        let content_page = adw::NavigationPage::new(&content_stack, "Content");
        split_view.set_content(Some(&content_page));

        // Toast overlay wraps the whole UI (used for Ollama status messages).
        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&split_view));
        window.set_content(Some(&toast_overlay));

        let current_session = Arc::new(Mutex::new(None));
        let current_media_session = Arc::new(Mutex::new(None));
        let cancel_handle = Arc::new(Mutex::new(None));

        let main_window = Self {
            window,
            sidebar,
            chat_view,
            input_bar,
            media_view,
            content_stack,
            toast_overlay,
            storage,
            manager,
            current_session,
            current_media_session,
            runtime,
            cancel_handle,
            downloads,
        };

        main_window.setup_callbacks(chat_menu_settings, chat_menu_about);
        main_window.setup_models();
        main_window.setup_media_view();
        main_window.setup_ollama();

        main_window
    }

    fn setup_callbacks(
        &self,
        chat_menu_settings: Arc<Mutex<Option<Box<dyn Fn() + 'static>>>>,
        chat_menu_about: Arc<Mutex<Option<Box<dyn Fn() + 'static>>>>,
    ) {
        let sidebar_arc = self.sidebar.clone();
        let input_bar_arc = self.input_bar.clone();
        let chat_view_arc = self.chat_view.clone();
        let storage_clone = self.storage.clone();
        let manager_clone = self.manager.clone();
        let session_clone = self.current_session.clone();
        let media_session_clone = self.current_media_session.clone();
        let window_clone = self.window.clone();
        let runtime_handle = self.runtime.clone();
        let cancel_handle = self.cancel_handle.clone();
        let input_bar_status = self.input_bar.clone();
        let media_view_arc = self.media_view.clone();
        let content_stack_clone = self.content_stack.clone();
        let overlay_clone = self.toast_overlay.clone();
        let downloads_clone = self.downloads.clone();

        // New Chat Callback: only show a fresh empty chat screen. A
        // conversation is actually created on the first message sent.
        self.sidebar.lock().unwrap().set_on_new_chat({
            let chat_view_cb = chat_view_arc.clone();
            let session_cb = session_clone.clone();
            let sidebar_cb = sidebar_arc.clone();
            let stack = content_stack_clone.clone();
            move || {
                *session_cb.lock().unwrap() = None;
                sidebar_cb.lock().unwrap().set_fixed_active("new-chat");
                chat_view_cb.lock().unwrap().set_empty();
                stack.set_visible_child_name("chat");
            }
        });

        // Chat selected callback (routes to chat or media by conversation kind)
        self.sidebar.lock().unwrap().set_on_chat_selected({
            let chat_view_cb = chat_view_arc.clone();
            let session_cb = session_clone.clone();
            let storage_cb = storage_clone.clone();
            let stack = content_stack_clone.clone();
            let input_bar_cb = input_bar_arc.clone();
            let manager_cb = manager_clone.clone();
            let runtime_cb = runtime_handle.clone();
            let media_view_cb = media_view_arc.clone();
            let media_session_cb = media_session_clone.clone();
            move |id| {
                if let Ok(conv) = storage_cb.get_conversation(&id) {
                    if conv.kind != ConversationKind::Chat {
                        open_media_conversation(
                            &media_view_cb,
                            &storage_cb,
                            &manager_cb,
                            &runtime_cb,
                            &stack,
                            &media_session_cb,
                            conv,
                        );
                        return;
                    }
                    // Sync the input bar to the conversation's provider and
                    // (re)load its model list, re-selecting the conversation
                    // model once it is available.
                    set_input_provider(&input_bar_cb, &conv.provider_id);
                    load_models_for_provider(
                        &input_bar_cb,
                        &manager_cb,
                        &runtime_cb,
                        Some(conv.model_id.clone()),
                    );

                    let messages = storage_cb.list_messages(&id).unwrap_or_default();
                    let mut session = ChatSession::new(conv);
                    session.messages = messages.clone();
                    *session_cb.lock().unwrap() = Some(session);
                    chat_view_cb.lock().unwrap().set_conversation(
                        &session_cb.lock().unwrap().as_ref().unwrap().conversation,
                        &messages,
                    );
                }
                stack.set_visible_child_name("chat");
            }
        });

        // Chat context menu (rename / favorite / branch / export / delete)
        self.sidebar.lock().unwrap().set_on_chat_action({
            let win = window_clone.clone();
            let storage_cb = storage_clone.clone();
            let session_cb = session_clone.clone();
            let chat_view_cb = chat_view_arc.clone();
            let sidebar_cb = sidebar_arc.clone();
            let stack = content_stack_clone.clone();
            move |action| match action {
                ChatAction::Rename { id, current_title } => {
                    let storage = storage_cb.clone();
                    let session_cb = session_cb.clone();
                    let sidebar_cb = sidebar_cb.clone();
                    dialogs::show_rename_dialog(&win, &current_title, move |new_title| {
                        let _ = storage.rename_conversation(&id, &new_title);
                        if let Some(s) = session_cb.lock().unwrap().as_mut() {
                            if s.conversation.id == id {
                                s.conversation.title = new_title.clone();
                            }
                        }
                        sidebar_cb.lock().unwrap().reload_conversations();
                    });
                }
                ChatAction::Favorite { id, is_favorite } => {
                    let storage = storage_cb.clone();
                    let session_cb = session_cb.clone();
                    let sidebar_cb = sidebar_cb.clone();
                    let _ = storage.toggle_favorite(&id);
                    if let Some(s) = session_cb.lock().unwrap().as_mut() {
                        if s.conversation.id == id {
                            s.conversation.is_favorite = !is_favorite;
                        }
                    }
                    sidebar_cb.lock().unwrap().reload_conversations();
                }
                ChatAction::Duplicate { id } => {
                    let storage = storage_cb.clone();
                    let session_cb = session_cb.clone();
                    let chat_view_cb = chat_view_cb.clone();
                    let sidebar_cb = sidebar_cb.clone();
                    let stack = stack.clone();
                    if let Ok(conv) = storage.get_conversation(&id) {
                        let messages = storage.list_messages(&id).unwrap_or_default();
                        let mut new_conv =
                            Conversation::new(conv.provider_id.clone(), conv.model_id.clone());
                        new_conv.title = format!("{} (branch)", conv.title);
                        new_conv.system_prompt = conv.system_prompt.clone();
                        let _ = storage.upsert_conversation(&new_conv);
                        let new_id = new_conv.id.clone();

                        let mut copied = Vec::with_capacity(messages.len());
                        for m in messages {
                            let attachments =
                                storage.list_attachments(&m.id).unwrap_or_default();
                            let nm = Message {
                                id: uuid::Uuid::new_v4().to_string(),
                                conversation_id: new_id.clone(),
                                role: m.role,
                                content: m.content,
                                created_at: m.created_at,
                                attachments,
                                metadata: m.metadata,
                            };
                            let _ = storage.insert_message(&nm);
                            copied.push(nm);
                        }

                        let session_conv = new_conv.clone();
                        let mut session = ChatSession::new(new_conv);
                        session.messages = copied.clone();
                        *session_cb.lock().unwrap() = Some(session);
                        chat_view_cb
                            .lock()
                            .unwrap()
                            .set_conversation(&session_conv, &copied);
                        sidebar_cb.lock().unwrap().reload_conversations();
                        stack.set_visible_child_name("chat");
                    }
                }
                ChatAction::Delete { id, title } => {
                    let storage = storage_cb.clone();
                    let session_cb = session_cb.clone();
                    let chat_view_cb = chat_view_cb.clone();
                    let sidebar_cb = sidebar_cb.clone();
                    let stack = stack.clone();
                    let win = win.clone();
                    dialogs::show_delete_dialog(&win, &title, move || {
                        let _ = storage.delete_conversation(&id);
                        let active_is_deleted = session_cb
                            .lock()
                            .unwrap()
                            .as_ref()
                            .map(|s| s.conversation.id == id)
                            .unwrap_or(false);
                        if active_is_deleted {
                            *session_cb.lock().unwrap() = None;
                            chat_view_cb.lock().unwrap().set_empty();
                        }
                        sidebar_cb.lock().unwrap().reload_conversations();
                        stack.set_visible_child_name("chat");
                    });
                }
                ChatAction::Export { id, title } => {
                    let storage = storage_cb.clone();
                    let messages = storage.list_messages(&id).unwrap_or_default();
                    let mut md = format!("# {}\n\n", title);
                    for m in messages {
                        let role = match m.role {
                            MessageRole::User => "**You**",
                            MessageRole::Assistant => "**Assistant**",
                            MessageRole::System => "**System**",
                        };
                        md.push_str(&format!("{}\n\n{}\n\n---\n\n", role, m.content));
                    }
                    let default_name = title.replace(' ', "_");
                    dialogs::show_export_dialog(&win, md, &default_name);
                }
            }
        });

        // Settings open callback
        self.sidebar.lock().unwrap().set_on_open_settings({
            let win = window_clone.clone();
            let overlay = overlay_clone.clone();
            let manager = manager_clone.clone();
            let store = storage_clone.clone();
            let input_bar = input_bar_arc.clone();
            let rt = self.runtime.clone();
            let downloads = downloads_clone.clone();
            move || {
                settings::show_settings_dialog(
                    &win,
                    &overlay,
                    store.clone(),
                    manager.clone(),
                    input_bar.clone(),
                    rt.clone(),
                    downloads.clone(),
                );
            }
        });

        // Three-dot header menu callbacks (chat page).
        *chat_menu_settings.lock().unwrap() = Some(Box::new({
            let win = window_clone.clone();
            let overlay = overlay_clone.clone();
            let manager = manager_clone.clone();
            let store = storage_clone.clone();
            let input_bar = input_bar_arc.clone();
            let rt = runtime_handle.clone();
            let downloads = downloads_clone.clone();
            move || {
                settings::show_settings_dialog(
                    &win,
                    &overlay,
                    store.clone(),
                    manager.clone(),
                    input_bar.clone(),
                    rt.clone(),
                    downloads.clone(),
                );
            }
        }));
        *chat_menu_about.lock().unwrap() = Some(Box::new({
            let win = window_clone.clone();
            move || {
                dialogs::show_about_dialog(&win);
            }
        }));

        // Media view header menu callbacks.
        {
            let mut mv = self.media_view.lock().unwrap();
            mv.set_on_settings({
                let win = window_clone.clone();
                let overlay = overlay_clone.clone();
                let manager = manager_clone.clone();
                let store = storage_clone.clone();
                let input_bar = input_bar_arc.clone();
                let rt = runtime_handle.clone();
                let downloads = downloads_clone.clone();
                move || {
                    settings::show_settings_dialog(
                        &win,
                        &overlay,
                        store.clone(),
                        manager.clone(),
                        input_bar.clone(),
                        rt.clone(),
                        downloads.clone(),
                    );
                }
            });
            mv.set_on_about({
                let win = window_clone.clone();
                move || {
                    dialogs::show_about_dialog(&win);
                }
            });
        }

        // Sidebar media buttons act like chats: each kind (Image/Video/Audio)
        // maps to a single persistent conversation. Clicking opens it and
        // selects its row in the list; generations append to it instead of
        // creating a new conversation every time.
        self.sidebar.lock().unwrap().set_on_open_media({
            let storage_cb = storage_clone.clone();
            let media_view_cb = media_view_arc.clone();
            let manager_cb = manager_clone.clone();
            let runtime_cb = runtime_handle.clone();
            let stack = content_stack_clone.clone();
            let media_session_cb = media_session_clone.clone();
            let sidebar_cb = sidebar_arc.clone();
            move |kind: GenKind| {
                let conv_kind = match kind {
                    GenKind::Image => ConversationKind::Image,
                    GenKind::Video => ConversationKind::Video,
                    GenKind::Audio => ConversationKind::Audio,
                };
                let conv = match storage_cb
                    .list_conversations()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|c| c.kind == conv_kind)
                {
                    Some(c) => c,
                    None => {
                        let mut conv = Conversation::new_media(conv_kind);
                        if let Some(p) = manager_cb.all().into_iter().find(|p| match kind {
                            GenKind::Image => p.supports_image_generation(),
                            GenKind::Video => p.supports_video_generation(),
                            GenKind::Audio => false,
                        }) {
                            conv.provider_id = p.id().to_string();
                        }
                        let _ = storage_cb.upsert_conversation(&conv);
                        sidebar_cb.lock().unwrap().reload_conversations();
                        conv
                    }
                };
                open_media_conversation(
                    &media_view_cb,
                    &storage_cb,
                    &manager_cb,
                    &runtime_cb,
                    &stack,
                    &media_session_cb,
                    conv,
                );
                let fixed_name = match kind {
                    GenKind::Image => "image",
                    GenKind::Video => "video",
                    GenKind::Audio => "audio",
                };
                sidebar_cb.lock().unwrap().set_fixed_active(fixed_name);
            }
        });

        // Provider selector in the input bar: when the user picks another
        // provider, reload its model list, persist the new default provider
        // for future chats, and keep the active session's conversation in
        // sync. Deferred to the idle loop so the input-bar mutex is not held
        // while the model list is fetched.
        {
            let ib = self.input_bar.clone();
            let selector = ib.lock().unwrap().provider_selector();
            let syncing = ib.lock().unwrap().syncing_flag();
            let input_bar_arc = ib.clone();
            let manager = manager_clone.clone();
            let runtime = runtime_handle.clone();
            let storage_cb = storage_clone.clone();
            let session_cb = session_clone.clone();
            selector.connect_selected_notify(move |_| {
                if *syncing.lock().unwrap() {
                    return;
                }
                let input_bar_arc = input_bar_arc.clone();
                let manager = manager.clone();
                let runtime = runtime.clone();
                let storage_cb = storage_cb.clone();
                let session_cb = session_cb.clone();
                glib::idle_add_local(move || {
                    let pid = input_bar_arc.lock().unwrap().selected_provider_id();
                    if let Some(pid) = pid {
                        input_bar_arc.lock().unwrap().refresh_attach_state();
                        if let Ok(mut settings) = storage_cb.load_settings() {
                            settings.default_provider_id = pid.clone();
                            let _ = storage_cb.save_settings(&settings);
                        }
                        if let Some(s) = session_cb.lock().unwrap().as_mut() {
                            if s.conversation.provider_id != pid {
                                s.conversation.provider_id = pid.clone();
                                let _ = storage_cb.upsert_conversation(&s.conversation);
                            }
                        }
                        load_models_for_provider(&input_bar_arc, &manager, &runtime, None);
                    }
                    glib::ControlFlow::Break
                });
            });
        }

        // Connect button and event handlers
        {
            let sb = self.sidebar.lock().unwrap();
            sb.connect_events(sidebar_arc.clone());
            sb.connect_buttons(sidebar_arc.clone());
        }
        {
            let ib = self.input_bar.lock().unwrap();
            ib.connect_events(input_bar_arc.clone());
        }

        // Send Message Callback
        let mut ib = self.input_bar.lock().unwrap();
        ib.set_on_send_message({
            let session_cb = session_clone.clone();
            let storage_cb = storage_clone.clone();
            let manager_cb = manager_clone.clone();
            let chat_cb = chat_view_arc.clone();
            let sidebar_cb = sidebar_arc.clone();
            let runtime = runtime_handle.clone();
            let cancel_store = cancel_handle.clone();
            let input_bar_for_status = input_bar_status.clone();
            move |text, attachments, model| {
                tracing::info!("Send message to provider '{}'", model.provider_id);

                // 1. Ensure a session exists and is configured for this model.
                let mut session_guard = session_cb.lock().unwrap();
                if session_guard.is_none() {
                    let conv = Conversation::new(model.provider_id.clone(), model.id.clone());
                    let _ = storage_cb.upsert_conversation(&conv);
                    *session_guard = Some(ChatSession::new(conv));
                    sidebar_cb.lock().unwrap().reload_conversations();
                }
                if let Some(session) = session_guard.as_mut() {
                    session.conversation.model_id = model.id.clone();
                    session.conversation.provider_id = model.provider_id.clone();
                    let _ = storage_cb.upsert_conversation(&session.conversation);
                }
                // Take the session out so we don't hold the GTK-thread lock across the await.
                let session = session_guard.take();
                drop(session_guard);
                let Some(mut session) = session else {
                    return false;
                };
                let conversation_id = session.conversation.id.clone();

                // 2. Persist and render the user message.
                let user_msg = Message::user(conversation_id.clone(), &text);
                let _ = storage_cb.insert_message(&user_msg);
                chat_cb.lock().unwrap().append_message(&user_msg);

                // 3. Resolve the provider through the common trait.
                let provider = match manager_cb.get(&model.provider_id) {
                    Some(p) => p,
                    None => {
                        chat_cb.lock().unwrap().show_error(format!(
                            "Provider '{}' is not available.\nSet it up in Settings and try again.",
                            model.provider_id
                        ));
                        *session_cb.lock().unwrap() = Some(session);
                        return false;
                    }
                };

                // 4. Wire up event channels: tokio task -> GTK main loop poll.
                let (event_tx, mut event_rx) = mpsc::channel::<ChatEvent>(128);
                let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
                *cancel_store.lock().unwrap() = Some(cancel_tx);

                // 5. Show streaming state in the UI.
                chat_cb.lock().unwrap().start_stream_bubble();
                chat_cb.lock().unwrap().set_typing(true);

                // 6. Drain tokio events on the GTK main loop (non-blocking poll).
                let poll_chat = chat_cb.clone();
                let poll_storage = storage_cb.clone();
                let poll_input = input_bar_for_status.clone();
                let poll_cancel = cancel_store.clone();
                let poll_conv = conversation_id.clone();
                glib::timeout_add_local(
                    std::time::Duration::from_millis(15),
                    move || {
                        loop {
                            match event_rx.try_recv() {
                                Ok(ev) => match ev {
                                    ChatEvent::Chunk(chunk) => {
                                        poll_chat.lock().unwrap().append_stream_chunk(&chunk.delta);
                                    }
                                    ChatEvent::Completed { full_text, metadata } => {
                                        poll_chat.lock().unwrap().set_typing(false);
                                        poll_chat.lock().unwrap().end_stream_bubble();
                                        poll_input.lock().unwrap().set_streaming(false);
                                        let mut assistant_msg =
                                            Message::assistant(poll_conv.clone(), &full_text);
                                        assistant_msg.metadata = metadata;
                                        let _ = poll_storage.insert_message(&assistant_msg);
                                        *poll_cancel.lock().unwrap() = None;
                                    }
                                    ChatEvent::Error(msg) => {
                                        poll_chat.lock().unwrap().show_stream_error(&msg);
                                        poll_chat.lock().unwrap().set_typing(false);
                                        poll_input.lock().unwrap().set_streaming(false);
                                        *poll_cancel.lock().unwrap() = None;
                                    }
                                    ChatEvent::Cancelled => {
                                        poll_chat.lock().unwrap().set_typing(false);
                                        poll_chat.lock().unwrap().end_stream_bubble();
                                        poll_input.lock().unwrap().set_streaming(false);
                                        *poll_cancel.lock().unwrap() = None;
                                    }
                                },
                                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                                    return glib::ControlFlow::Break;
                                }
                            }
                        }
                        glib::ControlFlow::Continue
                    },
                );

                // 7. Run the async provider call in the background, then return
                //    the session to the shared slot.
                let session_task = session_cb.clone();
                let error_tx = event_tx.clone();
                runtime.spawn(async move {
                    let res =
                        session.send(provider, text, attachments, event_tx, cancel_rx).await;
                    *session_task.lock().unwrap() = Some(session);
                    if let Err(e) = res {
                        let _ = error_tx.send(ChatEvent::Error(e.to_string())).await;
                    }
                });

                true
            }
        });

        // Cancel generation callback
        ib.set_on_cancel({
            let cancel_store = cancel_handle.clone();
            move || {
                if let Some(tx) = cancel_store.lock().unwrap().take() {
                    let _ = tx.send(());
                }
            }
        });

        // ── Media generation callback ──
        self.media_view.lock().unwrap().set_on_generate({
            let media_view = media_view_arc.clone();
            let storage_cb = storage_clone.clone();
            let media_session_cb = media_session_clone.clone();
            let manager_cb = manager_clone.clone();
            let runtime = runtime_handle.clone();
            move |request| {
                tracing::info!(
                    "Generate {:?} with provider '{}'",
                    request.kind,
                    request.provider_id
                );

                // Resolve the current media conversation. The sidebar buttons
                // open a fresh media view without a conversation, so it is
                // created lazily on the first generation (like New Chat defers
                // conversation creation until the first message).
                let (conv, was_empty) = {
                    let mut guard = media_session_cb.lock().unwrap();
                    match guard.as_ref() {
                        Some((conv, _)) => (conv.clone(), conv.message_count == 0),
                        None => {
                            let conv_kind = match request.kind {
                                GenKind::Image => ConversationKind::Image,
                                GenKind::Video => ConversationKind::Video,
                                GenKind::Audio => ConversationKind::Audio,
                            };
                            let mut conv = Conversation::new_media(conv_kind);
                            if let Some(p) = manager_cb.all().into_iter().find(|p| {
                                match request.kind {
                                    GenKind::Image => p.supports_image_generation(),
                                    GenKind::Video => p.supports_video_generation(),
                                    GenKind::Audio => false,
                                }
                            }) {
                                conv.provider_id = p.id().to_string();
                            }
                            let _ = storage_cb.upsert_conversation(&conv);
                            *guard = Some((conv.clone(), request.kind));
                            (conv, true)
                        }
                    }
                };

                // Persist the prompt as a user message and title the
                // conversation from it when it is still empty.
                let user_msg = Message::user(conv.id.clone(), &request.prompt);
                let _ = storage_cb.insert_message(&user_msg);
                if was_empty {
                    let title: String = request.prompt.chars().take(40).collect();
                    let _ = storage_cb.rename_conversation(&conv.id, &title);
                }

                // Refresh the credit balance so it reflects a freshly saved key.
                refresh_generate_balance(media_view.clone(), &manager_cb, &runtime);

                let provider = match manager_cb.get(&request.provider_id) {
                    Some(p) => p,
                    None => {
                        media_view.lock().unwrap().set_generating(false);
                        let title = match request.kind {
                            GenKind::Image => "Image generation unavailable",
                            GenKind::Video => "Video generation unavailable",
                            GenKind::Audio => "Audio generation unavailable",
                        };
                        media_view.lock().unwrap().show_error(
                            title,
                            &format!(
                                "Provider '{}' is not available. Set it up in Settings.",
                                request.provider_id
                            ),
                        );
                        return;
                    }
                };

                media_view.lock().unwrap().set_generating(true);

                let (event_tx, mut event_rx) = mpsc::channel::<GenerateEvent>(64);
                let media_task = media_view.clone();
                let conversation_id = conv.id.clone();
                let error_kind = request.kind;

                // Background work (tokio): run the generation and collect the
                // results so they can be persisted when the batch finishes.
                let provider_work = provider.clone();
                let request_work = request.clone();
                let storage_task = storage_cb.clone();
                runtime.spawn(async move {
                    let mut persisted: Vec<GeneratedMedia> = Vec::new();
                    let result: anyhow::Result<()> = async {
                        match request_work.kind {
                            GenKind::Image => {
                                let options = ImageGenOptions {
                                    num_images: Some(request_work.num_images),
                                    ..Default::default()
                                };
                                let images = provider_work
                                    .generate_image(&request_work.prompt, options)
                                    .await?;
                                for img in images {
                                    if let Some(b64) = img.base64_data {
                                        let data = base64::engine::general_purpose::STANDARD
                                            .decode(&b64)?;
                                        event_tx
                                            .send(GenerateEvent::Image {
                                                data,
                                                mime: img.mime_type.clone(),
                                            })
                                            .await
                                            .ok();
                                        persisted.push(GeneratedMedia {
                                            kind: "image".to_string(),
                                            mime: img.mime_type,
                                            prompt: request_work.prompt.clone(),
                                            model: request_work.model.clone(),
                                            base64: Some(b64),
                                            url: None,
                                            video_status: None,
                                            message: None,
                                        });
                                    }
                                }
                            }
                            GenKind::Video => {
                                let vreq = VideoRequest {
                                    prompt: request_work.prompt.clone(),
                                    source_image_path: request_work.image_path.clone(),
                                    model: Some(request_work.model.clone()),
                                    duration_seconds: Some(request_work.duration_seconds),
                                    aspect_ratio: Some(request_work.aspect_ratio.clone()),
                                    quality: Some(request_work.quality.clone()),
                                    motion_strength: None,
                                };
                                let mut stream = provider_work.generate_video(vreq).await?;
                                while let Some(item) = stream.next().await {
                                    let progress = item?;
                                    match progress.status {
                                        VideoStatus::Completed => persisted.push(GeneratedMedia {
                                            kind: "video".to_string(),
                                            mime: "video/mp4".to_string(),
                                            prompt: request_work.prompt.clone(),
                                            model: request_work.model.clone(),
                                            base64: None,
                                            url: progress.video_url.clone(),
                                            video_status: Some("completed".to_string()),
                                            message: None,
                                        }),
                                        VideoStatus::Failed => persisted.push(GeneratedMedia {
                                            kind: "video".to_string(),
                                            mime: "video/mp4".to_string(),
                                            prompt: request_work.prompt.clone(),
                                            model: request_work.model.clone(),
                                            base64: None,
                                            url: None,
                                            video_status: Some("failed".to_string()),
                                            message: progress.message.clone(),
                                        }),
                                        _ => {}
                                    }
                                    event_tx.send(GenerateEvent::Video(progress)).await.ok();
                                }
                            }
                            GenKind::Audio => {
                                anyhow::bail!("No provider supports audio generation yet");
                            }
                        }
                        Ok(())
                    }
                    .await;

                    if let Err(e) = result {
                        let _ = event_tx
                            .send(GenerateEvent::Error(friendly_error(&e)))
                            .await;
                    }
                    if !persisted.is_empty() {
                        if let Ok(json) = serde_json::to_string(&persisted) {
                            let assistant_msg = Message::assistant(conversation_id, json);
                            let _ = storage_task.insert_message(&assistant_msg);
                        }
                    }
                    let _ = event_tx.send(GenerateEvent::Done).await;
                });

                // Poll events on the GTK main thread.
                let manager_refresh = manager_cb.clone();
                let rt_refresh = runtime_handle.clone();
                glib::timeout_add_local(
                    std::time::Duration::from_millis(15),
                    move || {
                        let mut had_error = false;
                        loop {
                            match event_rx.try_recv() {
                                Ok(ev) => match ev {
                                    GenerateEvent::Image { data, mime } => {
                                        media_task.lock().unwrap().add_image_result(&data, &mime);
                                    }
                                    GenerateEvent::Video(progress) => {
                                        let task_id = progress.task_id.clone();
                                        let mut m = media_task.lock().unwrap();
                                        if !m.has_video_card(&task_id) {
                                            m.add_video_card(&task_id);
                                        }
                                        m.update_video(&task_id, &progress);
                                        if progress.status
                                            == crate::core::models::VideoStatus::Failed
                                        {
                                            had_error = true;
                                            if let Some(msg) = progress.message {
                                                m.show_error("Video generation failed", &msg);
                                            }
                                        }
                                    }
                                    GenerateEvent::Error(msg) => {
                                        had_error = true;
                                        let title = match error_kind {
                                            GenKind::Image => "Image generation failed",
                                            GenKind::Video => "Video generation failed",
                                            GenKind::Audio => "Audio generation failed",
                                        };
                                        let m = media_task.lock().unwrap();
                                        m.set_generating(false);
                                        m.show_error(title, &msg);
                                    }
                                    GenerateEvent::Done => {
                                        media_task.lock().unwrap().set_generating(false);
                                        if !had_error {
                                            media_task.lock().unwrap().show_status("");
                                        }
                                        refresh_generate_balance(
                                            media_task.clone(),
                                            &manager_refresh,
                                            &rt_refresh,
                                        );
                                        return glib::ControlFlow::Break;
                                    }
                                },
                                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                                    media_task.lock().unwrap().set_generating(false);
                                    return glib::ControlFlow::Break;
                                }
                            }
                        }
                        glib::ControlFlow::Continue
                    },
                );
            }
        });

        // Rebuild the conversation rows now that all sidebar callbacks
        // (including the context-menu action handler) are registered. Rows
        // built earlier captured a `None` callback and would be dead buttons.
        self.sidebar.lock().unwrap().reload_conversations();
    }

    fn setup_models(&self) {
        let input_bar = self.input_bar.clone();
        let manager = self.manager.clone();
        let runtime = self.runtime.clone();

        // Populate the provider dropdown with every chat-capable provider.
        let providers = manager.chat_providers();
        input_bar.lock().unwrap().update_providers(providers);

        // Select the default provider if it is present, then load its models.
        if let Some(active) = manager.active_provider_id() {
            if input_bar.lock().unwrap().provider_index(&active).is_some() {
                set_input_provider(&input_bar, &active);
            }
        }
        load_models_for_provider(&input_bar, &manager, &runtime, None);
    }

    fn setup_media_view(&self) {
        let manager = self.manager.clone();
        let providers: Vec<ProviderCap> = manager
            .all()
            .into_iter()
            .map(|p| ProviderCap {
                id: p.id().to_string(),
                name: p.name().to_string(),
                supports_image: p.supports_image_generation(),
                supports_video: p.supports_video_generation(),
            })
            .collect();

        let media_view = self.media_view.clone();
        media_view.lock().unwrap().set_providers(providers);
        media_view.lock().unwrap().update_video_estimate();
        media_view.lock().unwrap().connect_generate_button(media_view.clone());
        media_view.lock().unwrap().connect_live_estimate(media_view.clone());

        // Refresh the credit balance whenever the provider selection changes.
        let refresh_balance = {
            let media_view = self.media_view.clone();
            let manager = self.manager.clone();
            let rt = self.runtime.clone();
            move || {
                refresh_generate_balance(media_view.clone(), &manager, &rt);
            }
        };

        let media_view = self.media_view.clone();
        let provider_selector = media_view.lock().unwrap().provider_selector();
        let refresh = refresh_balance.clone();
        provider_selector.connect_selected_notify(move |_| {
            let refresh = refresh.clone();
            glib::idle_add_local(move || {
                refresh();
                glib::ControlFlow::Break
            });
        });

        // Refresh the balance whenever the media page is opened.
        let stack = self.content_stack.clone();
        let stack_for_cb = stack.clone();
        let refresh = refresh_balance.clone();
        stack.connect_visible_child_name_notify(move |_| {
            if stack_for_cb.visible_child_name().as_deref() == Some("media") {
                refresh();
            }
        });
    }

    /// Automated Ollama bootstrap, run once when the app opens:
    ///
    /// 1. Asks the user to install Ollama if it is missing.
    /// 2. Otherwise starts the local server automatically (if not running).
    fn setup_ollama(&self) {
        let window = self.window.clone();
        let overlay = self.toast_overlay.clone();
        let manager = self.manager.clone();
        let input_bar = self.input_bar.clone();
        let runtime = self.runtime.clone();

        let (tx, mut rx) = mpsc::channel::<ollama::OllamaStatus>(1);
        let rt = runtime.clone();
        rt.spawn(async move {
            let _ = tx.send(ollama::status().await).await;
        });

        glib::timeout_add_local(std::time::Duration::from_millis(15), move || {
            match rx.try_recv() {
                Ok(st) => {
                    if !st.installed {
                        ollama_dialog::prompt_install_ollama(
                            &window,
                            manager.clone(),
                            input_bar.clone(),
                            &overlay,
                            runtime.clone(),
                        );
                    } else if !st.server_running {
                        overlay.add_toast(adw::Toast::new("Starting local Ollama server…"));
                        let overlay = overlay.clone();
                        let manager = manager.clone();
                        let input_bar = input_bar.clone();
                        let runtime = runtime.clone();
                        let (tx2, mut rx2) = mpsc::channel::<Result<bool, String>>(1);
                        runtime.clone().spawn(async move {
                            let _ = tx2
                                .send(ollama::ensure_server_running().await.map_err(|e| e.to_string()))
                                .await;
                        });
                        glib::timeout_add_local(std::time::Duration::from_millis(15), move || {
                            match rx2.try_recv() {
                                Ok(Ok(_)) => {
                                    overlay.add_toast(adw::Toast::new(
                                        "Local Ollama server is running",
                                    ));
                                    load_models_for_provider(
                                        &input_bar,
                                        &manager,
                                        &runtime,
                                        None,
                                    );
                                    glib::ControlFlow::Break
                                }
                                Ok(Err(e)) => {
                                    overlay.add_toast(adw::Toast::new(&format!(
                                        "Could not start the Ollama server: {}",
                                        e
                                    )));
                                    glib::ControlFlow::Break
                                }
                                Err(mpsc::error::TryRecvError::Empty) => {
                                    glib::ControlFlow::Continue
                                }
                                Err(mpsc::error::TryRecvError::Disconnected) => {
                                    glib::ControlFlow::Break
                                }
                            }
                        });
                    }
                    glib::ControlFlow::Break
                }
                Err(mpsc::error::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::error::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }
}
