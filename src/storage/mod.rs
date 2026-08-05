//! Persistence layer — SQLite-backed storage for the application.

pub mod sqlite;

pub use sqlite::StorageManager;
