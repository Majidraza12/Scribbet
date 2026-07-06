//! Insertion history: what the app delivered (or failed to deliver) into
//! target applications, so dictated text is never lost (docs/02 "History").
//!
//! Local-only by design (docs/06 TB1): one table in the app's SQLite file,
//! size-capped, purgeable, and disable-able from settings. Plaintext at
//! rest — see ADR-18 (docs/03) for why SQLCipher was rejected.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::StorageError;

/// One remembered insertion.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HistoryEntry {
    /// Row id (monotonic; newest = highest).
    pub id: i64,
    /// Unix time of the insertion, milliseconds.
    pub ts_ms: i64,
    /// Raw STT text (pre-cleanup), kept for "what did I actually say".
    pub raw: String,
    /// Cleaned text — what was (or would have been) inserted.
    pub cleaned: String,
    /// Profile active at the time ("General", "Coding", ...).
    pub profile: String,
}

/// History repository trait (ADR-10): UI and event bridge talk to this,
/// never to SQL.
pub trait HistoryRepo {
    /// Records one insertion and trims the table to `cap` newest rows.
    fn add(
        &mut self,
        raw: &str,
        cleaned: &str,
        profile: &str,
        cap: u32,
    ) -> Result<(), StorageError>;
    /// Newest-first page of entries.
    fn recent(&self, limit: u32) -> Result<Vec<HistoryEntry>, StorageError>;
    /// Deletes everything. Returns the number of rows removed.
    fn purge(&mut self) -> Result<usize, StorageError>;
    /// Total stored entries.
    fn count(&self) -> Result<u64, StorageError>;
}

const SCHEMA_VERSION: i64 = 1;

/// SQLite-backed history store. Opens its own connection, so it can share
/// the database file with [`SqliteDictionaryRepo`](crate::SqliteDictionaryRepo)
/// (SQLite serializes cross-connection writes internally).
pub struct SqliteHistoryRepo {
    conn: Connection,
}

impl SqliteHistoryRepo {
    /// Opens (and migrates) the history table in the database at `path`.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::from_conn(Connection::open(path)?)
    }

    /// In-memory store (tests).
    pub fn open_in_memory() -> Result<Self, StorageError> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self, StorageError> {
        // The dictionary table owns PRAGMA user_version in this file; the
        // history table versions itself via a meta row instead of fighting
        // over the pragma.
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS history (
                 id      INTEGER PRIMARY KEY,
                 ts_ms   INTEGER NOT NULL,
                 raw     TEXT NOT NULL,
                 cleaned TEXT NOT NULL,
                 profile TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE IF NOT EXISTS history_meta (
                 key   TEXT PRIMARY KEY,
                 value INTEGER NOT NULL
             );
             INSERT OR IGNORE INTO history_meta (key, value) VALUES ('schema', 1);
             COMMIT;",
        )?;
        let version: i64 = conn.query_row(
            "SELECT value FROM history_meta WHERE key = 'schema'",
            [],
            |r| r.get(0),
        )?;
        if version > SCHEMA_VERSION {
            tracing::warn!(
                history_schema = version,
                supported = SCHEMA_VERSION,
                "history table is from a newer app version"
            );
        }
        Ok(Self { conn })
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl HistoryRepo for SqliteHistoryRepo {
    fn add(
        &mut self,
        raw: &str,
        cleaned: &str,
        profile: &str,
        cap: u32,
    ) -> Result<(), StorageError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO history (ts_ms, raw, cleaned, profile) VALUES (?1, ?2, ?3, ?4)",
            params![now_ms(), raw, cleaned, profile],
        )?;
        // Keep the newest `cap` rows; id order is insertion order.
        tx.execute(
            "DELETE FROM history WHERE id NOT IN
             (SELECT id FROM history ORDER BY id DESC LIMIT ?1)",
            params![i64::from(cap.max(1))],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn recent(&self, limit: u32) -> Result<Vec<HistoryEntry>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts_ms, raw, cleaned, profile FROM history
             ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![i64::from(limit)], |r| {
            Ok(HistoryEntry {
                id: r.get(0)?,
                ts_ms: r.get(1)?,
                raw: r.get(2)?,
                cleaned: r.get(3)?,
                profile: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn purge(&mut self) -> Result<usize, StorageError> {
        Ok(self.conn.execute("DELETE FROM history", [])?)
    }

    fn count(&self) -> Result<u64, StorageError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM history", [], |r| r.get::<_, i64>(0))?
            as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_recent_newest_first() {
        let mut h = SqliteHistoryRepo::open_in_memory().unwrap();
        h.add("hello world", "Hello world.", "General", 100)
            .unwrap();
        h.add("second one", "Second one.", "General", 100).unwrap();
        let recent = h.recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].cleaned, "Second one.");
        assert_eq!(recent[1].raw, "hello world");
        assert!(recent[0].ts_ms > 0);
    }

    #[test]
    fn cap_trims_oldest() {
        let mut h = SqliteHistoryRepo::open_in_memory().unwrap();
        for i in 0..10 {
            h.add(&format!("raw {i}"), &format!("clean {i}"), "General", 3)
                .unwrap();
        }
        let recent = h.recent(100).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].cleaned, "clean 9");
        assert_eq!(recent[2].cleaned, "clean 7");
        assert_eq!(h.count().unwrap(), 3);
    }

    #[test]
    fn purge_empties() {
        let mut h = SqliteHistoryRepo::open_in_memory().unwrap();
        h.add("a", "A", "General", 10).unwrap();
        h.add("b", "B", "General", 10).unwrap();
        assert_eq!(h.purge().unwrap(), 2);
        assert_eq!(h.count().unwrap(), 0);
        assert!(h.recent(10).unwrap().is_empty());
    }

    #[test]
    fn shares_file_with_dictionary() {
        use crate::{DictionaryRepo, SqliteDictionaryRepo};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        let mut dict = SqliteDictionaryRepo::open(&path).unwrap();
        dict.add(
            "user",
            &od_core_types::DictEntry {
                spoken: "a".into(),
                written: "A".into(),
                case_sensitive: false,
            },
        )
        .unwrap();
        let mut h = SqliteHistoryRepo::open(&path).unwrap();
        h.add("raw", "clean", "General", 10).unwrap();
        // Both live in one file; neither clobbers the other.
        assert_eq!(dict.entries(&["user".to_owned()]).unwrap().len(), 1);
        assert_eq!(h.count().unwrap(), 1);
    }

    #[test]
    fn cap_of_zero_keeps_one() {
        // Guard against a misconfigured cap deleting the row just written.
        let mut h = SqliteHistoryRepo::open_in_memory().unwrap();
        h.add("a", "A", "General", 0).unwrap();
        assert_eq!(h.count().unwrap(), 1);
    }
}
