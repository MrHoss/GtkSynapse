//! Chat session manager. Handles message context, windowing, and
//! orchestration between the UI and the provider registry.

use anyhow::Result;
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::core::models::{
    ChatRequest, Conversation, Message, MessageMetadata, MessageRole, StreamChunk,
};
use crate::core::provider::AiProvider;

/// Sent from the background worker to the UI thread.
#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// A new chunk of text arrived during streaming.
    Chunk(StreamChunk),
    /// Generation completed. The field contains the full assembled text.
    Completed {
        full_text: String,
        metadata: Option<MessageMetadata>,
    },
    /// An error occurred.
    Error(String),
    /// Generation was cancelled by the user.
    Cancelled,
}

/// Manages a single chat session (conversation + context window).
pub struct ChatSession {
    pub conversation: Conversation,
    pub messages: Vec<Message>,
    /// Maximum number of messages to include in the context window.
    pub max_context_messages: usize,
    /// Whether streaming is enabled for this session.
    pub stream: bool,
}

impl ChatSession {
    pub fn new(conversation: Conversation) -> Self {
        Self {
            conversation,
            messages: Vec::new(),
            max_context_messages: 20,
            stream: true,
        }
    }

    /// Returns the messages that should be sent to the provider,
    /// respecting the configured context window.
    pub fn context_messages(&self) -> Vec<Message> {
        let start = if self.messages.len() > self.max_context_messages {
            self.messages.len() - self.max_context_messages
        } else {
            0
        };
        self.messages[start..].to_vec()
    }

    /// Add a message to the session.
    pub fn push_message(&mut self, msg: Message) {
        self.messages.push(msg);
        self.conversation.message_count = self.messages.len();
    }

    /// Send a user message and stream the response back via the given channel.
    ///
    /// This should be called from a Tokio task (not the GTK main thread).
    pub async fn send(
        &mut self,
        provider: Arc<dyn AiProvider>,
        user_content: String,
        attachments: Vec<crate::core::models::Attachment>,
        tx: mpsc::Sender<ChatEvent>,
        mut cancel_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()> {
        // Build user message
        let mut user_msg = Message::user(self.conversation.id.clone(), &user_content);
        user_msg.attachments = attachments;
        self.push_message(user_msg);

        let request = ChatRequest {
            model_id: self.conversation.model_id.clone(),
            messages: self.context_messages(),
            stream: self.stream,
            max_tokens: None,
            temperature: Some(0.7),
            system_prompt: self.conversation.system_prompt.clone(),
        };

        if self.stream && provider.supports_streaming() {
            let mut stream = provider.send_message_stream(request).await?;
            let mut full_text = String::new();
            let mut final_metadata = None;

            loop {
                tokio::select! {
                    // Check for cancellation
                    _ = &mut cancel_rx => {
                        let _ = tx.send(ChatEvent::Cancelled).await;
                        return Ok(());
                    }
                    chunk_opt = stream.next() => {
                        match chunk_opt {
                            Some(Ok(chunk)) => {
                                full_text.push_str(&chunk.delta);
                                if chunk.is_done {
                                    final_metadata = chunk.metadata.clone();
                                }
                                let _ = tx.send(ChatEvent::Chunk(chunk)).await;
                            }
                            Some(Err(e)) => {
                                let _ = tx.send(ChatEvent::Error(e.to_string())).await;
                                return Ok(());
                            }
                            None => break,
                        }
                    }
                }
            }

            // Store assistant message
            let mut assistant_msg = Message::assistant(
                self.conversation.id.clone(),
                &full_text,
            );
            assistant_msg.metadata = final_metadata.clone();
            self.push_message(assistant_msg);

            let _ = tx.send(ChatEvent::Completed {
                full_text,
                metadata: final_metadata,
            }).await;
        } else {
            // Non-streaming fallback
            let response = provider.send_message(request).await?;
            let mut assistant_msg = Message::assistant(
                self.conversation.id.clone(),
                &response.content,
            );
            assistant_msg.metadata = Some(response.metadata.clone());
            self.push_message(assistant_msg.clone());

            // Emit a single chunk + completed event so the UI code path is uniform
            let _ = tx.send(ChatEvent::Chunk(StreamChunk {
                delta: response.content.clone(),
                is_done: true,
                metadata: Some(response.metadata.clone()),
            })).await;
            let _ = tx.send(ChatEvent::Completed {
                full_text: response.content,
                metadata: Some(response.metadata),
            }).await;
        }

        Ok(())
    }

    /// Clear all messages (start fresh in the same conversation).
    pub fn clear(&mut self) {
        self.messages.clear();
        self.conversation.message_count = 0;
    }

    /// Update the conversation title based on the first user message.
    pub fn auto_title(&mut self) {
        if self.conversation.title == "New Chat" {
            if let Some(first_user) = self.messages.iter().find(|m| m.role == MessageRole::User) {
                let title: String = first_user.content.chars().take(50).collect();
                let title = title.trim().to_string();
                if !title.is_empty() {
                    self.conversation.title = if title.len() == 50 {
                        format!("{}…", title)
                    } else {
                        title
                    };
                }
            }
        }
    }
}
