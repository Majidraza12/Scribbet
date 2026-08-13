//! Dictionary repository: user vocabulary, grouped into named sets
//! ("user", "programming", "medical", ...). Profiles subscribe to sets;
//! the resolved entries feed cleanup processor 3 and the STT vocab bias.

use std::path::Path;

use od_core_types::DictEntry;
use rusqlite::{Connection, params};

use crate::StorageError;

/// Repository trait (ADR-10): the pipeline and UI talk to this, never to
/// SQL, so tests can fake it and the backend can change.
pub trait DictionaryRepo {
    /// Adds or replaces an entry in a set (upsert on `(set, spoken)`).
    fn add(&mut self, set: &str, entry: &DictEntry) -> Result<(), StorageError>;
    /// Removes an entry; returns whether it existed.
    fn remove(&mut self, set: &str, spoken: &str) -> Result<bool, StorageError>;
    /// All entries of the given sets, longest spoken form first.
    fn entries(&self, sets: &[String]) -> Result<Vec<DictEntry>, StorageError>;
    /// Distinct written forms of the given sets — the STT vocabulary bias
    /// (we bias toward what should be *produced*, i.e. the written form).
    fn vocab_terms(&self, sets: &[String]) -> Result<Vec<String>, StorageError>;
    /// All set names present in the store.
    fn sets(&self) -> Result<Vec<String>, StorageError>;
}

const SCHEMA_VERSION: i64 = 1;

/// SQLite-backed dictionary repository (rusqlite, bundled).
pub struct SqliteDictionaryRepo {
    conn: Connection,
}

impl SqliteDictionaryRepo {
    /// Opens (and migrates) the database at `path`, creating it if needed.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::from_conn(Connection::open(path)?)
    }

    /// In-memory database (tests, ephemeral sessions).
    pub fn open_in_memory() -> Result<Self, StorageError> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self, StorageError> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE IF NOT EXISTS dict_entries (
                     id             INTEGER PRIMARY KEY,
                     set_name       TEXT NOT NULL,
                     spoken         TEXT NOT NULL,
                     written        TEXT NOT NULL,
                     case_sensitive INTEGER NOT NULL DEFAULT 0,
                     UNIQUE (set_name, spoken)
                 );
                 CREATE INDEX IF NOT EXISTS idx_dict_set ON dict_entries (set_name);
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        if version > SCHEMA_VERSION {
            tracing::warn!(
                db_version = version,
                supported = SCHEMA_VERSION,
                "dictionary db is from a newer app version"
            );
        }
        Ok(Self { conn })
    }
}

/// Builds `?, ?, ?` for a set-name IN clause.
fn placeholders(n: usize) -> String {
    let mut s = String::with_capacity(n * 3);
    for i in 0..n {
        if i > 0 {
            s.push_str(", ");
        }
        s.push('?');
    }
    s
}

impl DictionaryRepo for SqliteDictionaryRepo {
    fn add(&mut self, set: &str, entry: &DictEntry) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO dict_entries (set_name, spoken, written, case_sensitive)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (set_name, spoken)
             DO UPDATE SET written = ?3, case_sensitive = ?4",
            params![
                set,
                entry.spoken,
                entry.written,
                entry.case_sensitive as i64
            ],
        )?;
        Ok(())
    }

    fn remove(&mut self, set: &str, spoken: &str) -> Result<bool, StorageError> {
        let n = self.conn.execute(
            "DELETE FROM dict_entries WHERE set_name = ?1 AND spoken = ?2",
            params![set, spoken],
        )?;
        Ok(n > 0)
    }

    fn entries(&self, sets: &[String]) -> Result<Vec<DictEntry>, StorageError> {
        if sets.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT spoken, written, case_sensitive FROM dict_entries
             WHERE set_name IN ({})
             ORDER BY length(spoken) DESC, spoken",
            placeholders(sets.len())
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(sets), |r| {
            Ok(DictEntry {
                spoken: r.get(0)?,
                written: r.get(1)?,
                case_sensitive: r.get::<_, i64>(2)? != 0,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn vocab_terms(&self, sets: &[String]) -> Result<Vec<String>, StorageError> {
        if sets.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT DISTINCT written FROM dict_entries
             WHERE set_name IN ({})
             ORDER BY written",
            placeholders(sets.len())
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(sets), |r| r.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn sets(&self) -> Result<Vec<String>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT set_name FROM dict_entries ORDER BY set_name")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(spoken: &str, written: &str) -> DictEntry {
        DictEntry {
            spoken: spoken.into(),
            written: written.into(),
            case_sensitive: false,
        }
    }

    fn seeded() -> SqliteDictionaryRepo {
        let mut repo = SqliteDictionaryRepo::open_in_memory().unwrap();
        repo.add("user", &entry("open dictate", "Scribbet"))
            .unwrap();
        repo.add("programming", &entry("kubernetes", "Kubernetes"))
            .unwrap();
        repo.add("programming", &entry("post gres", "Postgres"))
            .unwrap();
        repo
    }

    #[test]
    fn crud_round_trip() {
        let mut repo = seeded();
        let sets = vec!["user".to_owned(), "programming".to_owned()];
        assert_eq!(repo.entries(&sets).unwrap().len(), 3);
        // longest spoken first (matcher precondition)
        assert_eq!(repo.entries(&sets).unwrap()[0].spoken, "open dictate");

        assert!(repo.remove("user", "open dictate").unwrap());
        assert!(!repo.remove("user", "open dictate").unwrap());
        assert_eq!(repo.entries(&sets).unwrap().len(), 2);
    }

    #[test]
    fn upsert_replaces_written_form() {
        let mut repo = seeded();
        repo.add("user", &entry("open dictate", "scribbet.dev"))
            .unwrap();
        let e = repo.entries(&["user".to_owned()]).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].written, "scribbet.dev");
    }

    #[test]
    fn case_sensitive_flag_round_trips() {
        let mut repo = SqliteDictionaryRepo::open_in_memory().unwrap();
        repo.add(
            "user",
            &DictEntry {
                spoken: "Jason".into(),
                written: "JSON".into(),
                case_sensitive: true,
            },
        )
        .unwrap();
        assert!(repo.entries(&["user".to_owned()]).unwrap()[0].case_sensitive);
    }

    #[test]
    fn vocab_terms_are_written_forms() {
        let repo = seeded();
        assert_eq!(
            repo.vocab_terms(&["programming".to_owned()]).unwrap(),
            vec!["Kubernetes".to_owned(), "Postgres".to_owned()]
        );
    }

    #[test]
    fn sets_listed() {
        let repo = seeded();
        assert_eq!(
            repo.sets().unwrap(),
            vec!["programming".to_owned(), "user".to_owned()]
        );
    }

    #[test]
    fn empty_sets_query_is_empty() {
        let repo = seeded();
        assert!(repo.entries(&[]).unwrap().is_empty());
        assert!(repo.vocab_terms(&[]).unwrap().is_empty());
    }

    #[test]
    fn persists_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dict.db");
        {
            let mut repo = SqliteDictionaryRepo::open(&path).unwrap();
            repo.add("user", &entry("a", "A")).unwrap();
        }
        let repo = SqliteDictionaryRepo::open(&path).unwrap();
        assert_eq!(repo.entries(&["user".to_owned()]).unwrap().len(), 1);
    }
}
