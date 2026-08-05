//! SQLite persistence layer — conversations, messages, attachments, settings.
//!
//! Uses `rusqlite` with the "bundled" feature so no system SQLite is required.
//! The database is created at `~/.local/share/gtksynapse/gtksynapse.db`.

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::core::models::{
    AppSettings, Attachment, AttachmentKind, Conversation, ConversationKind, Message,
    MessageMetadata, MessageRole, ThemePreference,
};

// ─── StorageManager ──────────────────────────────────────────

/// Thread-safe wrapper around the SQLite connection.
pub struct StorageManager {
    conn: Mutex<Connection>,
}

impl StorageManager {
    /// Open (or create) the database at the default location.
    pub fn open() -> Result<Self> {
        let db_path = default_db_path()?;
        tracing::info!("Opening database at {:?}", db_path);

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create data directory")?;
        }

        let conn = Connection::open(&db_path).context("Failed to open SQLite database")?;

        let manager = Self {
            conn: Mutex::new(conn),
        };
        manager.initialize_schema()?;
        Ok(manager)
    }

    /// Open an in-memory database (for tests).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let manager = Self {
            conn: Mutex::new(conn),
        };
        manager.initialize_schema()?;
        Ok(manager)
    }

    // ── Schema ────────────────────────────────────────────────────

    fn initialize_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(SCHEMA_SQL)
            .context("Failed to initialize database schema")?;
        migrate_kind_column(&conn)?;
        Ok(())
    }

    // ── Conversations ─────────────────────────────────────────────

    /// Insert or replace a conversation.
    pub fn upsert_conversation(&self, conv: &Conversation) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO conversations
             (id, title, provider_id, model_id, kind, created_at, updated_at, is_favorite, system_prompt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                conv.id,
                conv.title,
                conv.provider_id,
                conv.model_id,
                conv.kind.as_str(),
                conv.created_at.timestamp(),
                conv.updated_at.timestamp(),
                conv.is_favorite as i64,
                conv.system_prompt,
            ],
        )?;
        Ok(())
    }

    /// Load a single conversation by ID.
    pub fn get_conversation(&self, id: &str) -> Result<Conversation> {
        let conn = self.conn.lock().unwrap();
        let conv = conn.query_row(
            "SELECT c.id, c.title, c.provider_id, c.model_id, c.kind,
                    c.created_at, c.updated_at, c.is_favorite, c.system_prompt,
                    COUNT(m.id) as msg_count
             FROM conversations c
             LEFT JOIN messages m ON m.conversation_id = c.id
             WHERE c.id = ?1
             GROUP BY c.id",
            params![id],
            row_to_conversation,
        )?;
        Ok(conv)
    }

    /// List all conversations, newest first.
    pub fn list_conversations(&self) -> Result<Vec<Conversation>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.title, c.provider_id, c.model_id, c.kind,
                    c.created_at, c.updated_at, c.is_favorite, c.system_prompt,
                    COUNT(m.id) as msg_count
             FROM conversations c
             LEFT JOIN messages m ON m.conversation_id = c.id
             GROUP BY c.id
             ORDER BY c.updated_at DESC",
        )?;
        let convs = stmt
            .query_map([], row_to_conversation)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(convs)
    }

    /// Full-text search over titles and message content.
    pub fn search_conversations(&self, query: &str) -> Result<Vec<Conversation>> {
        let conn = self.conn.lock().unwrap();
        let like = format!("%{}%", query.to_lowercase());
        let mut stmt = conn.prepare(
            "SELECT DISTINCT c.id, c.title, c.provider_id, c.model_id, c.kind,
                    c.created_at, c.updated_at, c.is_favorite, c.system_prompt,
                    COUNT(m.id) as msg_count
             FROM conversations c
             LEFT JOIN messages m ON m.conversation_id = c.id
             WHERE LOWER(c.title) LIKE ?1 OR LOWER(m.content) LIKE ?1
             GROUP BY c.id
             ORDER BY c.updated_at DESC",
        )?;
        let convs = stmt
            .query_map(params![like], row_to_conversation)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(convs)
    }

    /// Toggle the favorite flag. Returns the new value.
    pub fn toggle_favorite(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let current: i64 = conn.query_row(
            "SELECT is_favorite FROM conversations WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        let new_val = if current == 0 { 1i64 } else { 0i64 };
        conn.execute(
            "UPDATE conversations SET is_favorite = ?1 WHERE id = ?2",
            params![new_val, id],
        )?;
        Ok(new_val == 1)
    }

    /// Rename a conversation.
    pub fn rename_conversation(&self, id: &str, title: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title, Utc::now().timestamp(), id],
        )?;
        Ok(())
    }

    /// Update the updated_at timestamp of a conversation.
    pub fn touch_conversation(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![Utc::now().timestamp(), id],
        )?;
        Ok(())
    }

    /// Delete a conversation and all its messages (CASCADE).
    pub fn delete_conversation(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM conversations WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ── Messages ──────────────────────────────────────────────────

    /// Insert a message into the database.
    pub fn insert_message(&self, msg: &Message) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let metadata_json = msg
            .metadata
            .as_ref()
            .and_then(|m| serde_json::to_string(m).ok());
        conn.execute(
            "INSERT OR REPLACE INTO messages
             (id, conversation_id, role, content, created_at, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                msg.id,
                msg.conversation_id,
                msg.role.as_str(),
                msg.content,
                msg.created_at.timestamp(),
                metadata_json,
            ],
        )?;

        // Insert attachments
        for att in &msg.attachments {
            self.insert_attachment_inner(&conn, att, Some(&msg.id))?;
        }

        Ok(())
    }

    /// List all messages in a conversation, ordered chronologically.
    pub fn list_messages(&self, conversation_id: &str) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, role, content, created_at, metadata
             FROM messages
             WHERE conversation_id = ?1
             ORDER BY created_at ASC",
        )?;
        let messages = stmt
            .query_map(params![conversation_id], |row| {
                let role_str: String = row.get(2)?;
                let role = match role_str.as_str() {
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    _ => MessageRole::System,
                };
                let ts: i64 = row.get(4)?;
                let metadata_json: Option<String> = row.get(5)?;
                Ok(Message {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    role,
                    content: row.get(3)?,
                    created_at: Utc.timestamp_opt(ts, 0).single().unwrap_or_default(),
                    attachments: Vec::new(), // loaded separately if needed
                    metadata: metadata_json.and_then(|j| serde_json::from_str(&j).ok()),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(messages)
    }

    /// List attachments attached to a message.
    pub fn list_attachments(&self, message_id: &str) -> Result<Vec<Attachment>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, message_id, file_path, mime_type, file_name, size_bytes
             FROM attachments WHERE message_id = ?1",
        )?;
        let attachments = stmt
            .query_map(params![message_id], |row| {
                let mime: String = row.get(3)?;
                Ok(Attachment {
                    id: row.get(0)?,
                    message_id: row.get(1)?,
                    file_path: PathBuf::from(row.get::<_, String>(2)?),
                    mime_type: mime.clone(),
                    kind: AttachmentKind::from_mime(&mime),
                    file_name: row.get(4)?,
                    size_bytes: row.get::<_, i64>(5)? as u64,
                    remote_url: None,
                    remote_id: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(attachments)
    }

    // ── Attachments ───────────────────────────────────────────────

    fn insert_attachment_inner(
        &self,
        conn: &Connection,
        att: &Attachment,
        message_id: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO attachments
             (id, message_id, file_path, mime_type, file_name, size_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                att.id,
                message_id,
                att.file_path.to_string_lossy().as_ref(),
                att.mime_type,
                att.file_name,
                att.size_bytes as i64,
            ],
        )?;
        Ok(())
    }

    // ── Settings ──────────────────────────────────────────────────

    /// Load application settings (or return defaults).
    pub fn load_settings(&self) -> Result<AppSettings> {
        let json = self.get_setting("app_settings")?;
        match json {
            Some(j) => Ok(serde_json::from_str(&j)?),
            None => Ok(AppSettings::default()),
        }
    }

    /// Save application settings.
    pub fn save_settings(&self, settings: &AppSettings) -> Result<()> {
        let json = serde_json::to_string(settings)?;
        self.set_setting("app_settings", &json)
    }

    /// Get a raw setting by key.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT value FROM app_settings WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        );
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Set a raw setting by key.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO app_settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }
}

// ─── Helpers ─────────────────────────────────────────────────

fn row_to_conversation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
    let created_ts: i64 = row.get(5)?;
    let updated_ts: i64 = row.get(6)?;
    let is_fav: i64 = row.get(7)?;
    let msg_count: i64 = row.get(9)?;

    Ok(Conversation {
        id: row.get(0)?,
        title: row.get(1)?,
        provider_id: row.get(2)?,
        model_id: row.get(3)?,
        kind: ConversationKind::from_str(&row.get::<_, String>(4)?),
        created_at: Utc
            .timestamp_opt(created_ts, 0)
            .single()
            .unwrap_or_default(),
        updated_at: Utc
            .timestamp_opt(updated_ts, 0)
            .single()
            .unwrap_or_default(),
        is_favorite: is_fav != 0,
        system_prompt: row.get(8)?,
        message_count: msg_count as usize,
    })
}

/// Add the `kind` column to `conversations` for databases created before it
/// existed. `CREATE TABLE IF NOT EXISTS` does not modify existing tables, so
/// this is checked explicitly on every startup.
fn migrate_kind_column(conn: &Connection) -> Result<()> {
    let has_kind: bool = conn
        .prepare("PRAGMA table_info(conversations)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .iter()
        .any(|name| name == "kind");
    if !has_kind {
        conn.execute(
            "ALTER TABLE conversations ADD COLUMN kind TEXT NOT NULL DEFAULT 'chat'",
            [],
        )?;
    }
    Ok(())
}

fn default_db_path() -> Result<PathBuf> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gtksynapse");
    Ok(data_dir.join("gtksynapse.db"))
}

// ─── SQL Schema ──────────────────────────────────────────────

const SCHEMA_SQL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS conversations (
    id          TEXT    PRIMARY KEY,
    title       TEXT    NOT NULL,
    provider_id TEXT    NOT NULL,
    model_id    TEXT    NOT NULL,
    kind        TEXT    NOT NULL DEFAULT 'chat',
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    is_favorite INTEGER NOT NULL DEFAULT 0,
    system_prompt TEXT
);

CREATE TABLE IF NOT EXISTS messages (
    id              TEXT    PRIMARY KEY,
    conversation_id TEXT    NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT    NOT NULL CHECK(role IN ('user','assistant','system')),
    content         TEXT    NOT NULL,
    created_at      INTEGER NOT NULL,
    metadata        TEXT
);

CREATE TABLE IF NOT EXISTS attachments (
    id          TEXT    PRIMARY KEY,
    message_id  TEXT    REFERENCES messages(id) ON DELETE CASCADE,
    file_path   TEXT    NOT NULL,
    mime_type   TEXT    NOT NULL,
    file_name   TEXT    NOT NULL,
    size_bytes  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS app_settings (
    key     TEXT PRIMARY KEY,
    value   TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_conv
    ON messages (conversation_id, created_at);

CREATE INDEX IF NOT EXISTS idx_conversations_updated
    ON conversations (updated_at DESC);
"#;

// ─── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::Conversation;

    #[test]
    fn test_conversation_roundtrip() {
        let storage = StorageManager::open_in_memory().unwrap();
        let conv = Conversation::new("ollama", "llama3.1:8b");
        storage.upsert_conversation(&conv).unwrap();
        let loaded = storage.get_conversation(&conv.id).unwrap();
        assert_eq!(conv.id, loaded.id);
        assert_eq!(conv.title, loaded.title);
    }

    #[test]
    fn test_message_storage() {
        let storage = StorageManager::open_in_memory().unwrap();
        let conv = Conversation::new("ollama", "llama3.1:8b");
        storage.upsert_conversation(&conv).unwrap();

        let msg = Message::user(conv.id.clone(), "Hello!");
        storage.insert_message(&msg).unwrap();

        let messages = storage.list_messages(&conv.id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hello!");
    }

    #[test]
    fn test_settings_roundtrip() {
        let storage = StorageManager::open_in_memory().unwrap();
        let mut settings = AppSettings::default();
        settings.max_context_messages = 42;
        storage.save_settings(&settings).unwrap();
        let loaded = storage.load_settings().unwrap();
        assert_eq!(loaded.max_context_messages, 42);
    }
}
