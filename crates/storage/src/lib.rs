//! Persistence (ADR-10): SQLite repositories behind traits, TOML profiles,
//! and an atomic JSON settings file.
//!
//! This crate is path-agnostic: callers (the desktop app) supply the config
//! and data directories, which keeps every test hermetic under a temp dir.
//! No secrets live here (docs/06); future cloud keys go to the Windows
//! Credential Manager under a cargo feature, never to these files.

#![warn(missing_docs)]

mod dictionary;
mod profiles;
mod settings;

pub use dictionary::{DictionaryRepo, SqliteDictionaryRepo};
pub use profiles::{ProfileStore, ProfileToml, resolve_profile, shipped_profile_ids};
pub use settings::{Settings, load_settings, save_settings};

/// Errors from any storage operation.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// Database open/query failure.
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    /// Filesystem failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Profile TOML failed to parse.
    #[error("profile parse error: {0}")]
    ProfileParse(#[from] toml::de::Error),
    /// Settings JSON failed to parse.
    #[error("settings parse error: {0}")]
    SettingsParse(#[from] serde_json::Error),
    /// Requested profile does not exist (not shipped, not in the user dir).
    #[error("unknown profile {0:?}")]
    UnknownProfile(String),
}
