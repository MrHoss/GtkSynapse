//! Conversation history management — search, favorites, export.

use anyhow::Result;
use std::sync::Arc;

use crate::core::models::{Conversation, Message};
use crate::storage::StorageManager;

/// High-level API over stored conversations.
pub struct HistoryManager {
    storage: Arc<StorageManager>,
}

impl HistoryManager {
    pub fn new(storage: Arc<StorageManager>) -> Self {
        Self { storage }
    }

    /// Return all conversations ordered by updated_at DESC.
    pub fn list_conversations(&self) -> Result<Vec<Conversation>> {
        self.storage.list_conversations()
    }

    /// Search conversations and messages by keyword.
    pub fn search(&self, query: &str) -> Result<Vec<Conversation>> {
        self.storage.search_conversations(query)
    }

    /// Toggle the favorite flag for a conversation.
    pub fn toggle_favorite(&self, id: &str) -> Result<bool> {
        self.storage.toggle_favorite(id)
    }

    /// Rename a conversation.
    pub fn rename(&self, id: &str, new_title: &str) -> Result<()> {
        self.storage.rename_conversation(id, new_title)
    }

    /// Permanently delete a conversation and all its messages.
    pub fn delete(&self, id: &str) -> Result<()> {
        self.storage.delete_conversation(id)
    }

    /// Load all messages for a conversation.
    pub fn messages(&self, conversation_id: &str) -> Result<Vec<Message>> {
        self.storage.list_messages(conversation_id)
    }

    /// Export a conversation as a Markdown string.
    pub fn export_markdown(&self, conversation_id: &str) -> Result<String> {
        let conv = self.storage.get_conversation(conversation_id)?;
        let messages = self.storage.list_messages(conversation_id)?;

        let mut output = format!("# {}\n\n", conv.title);
        output.push_str(&format!(
            "_Provider: {} | Model: {} | Created: {}_\n\n---\n\n",
            conv.provider_id,
            conv.model_id,
            conv.created_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M"),
        ));

        for msg in messages {
            let role_label = match msg.role {
                crate::core::models::MessageRole::User => "**You**",
                crate::core::models::MessageRole::Assistant => "**Assistant**",
                crate::core::models::MessageRole::System => "_System_",
            };
            output.push_str(&format!("{}\n\n{}\n\n---\n\n", role_label, msg.content));
        }

        Ok(output)
    }
}
