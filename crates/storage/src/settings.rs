//! App settings: one JSON file, serde, atomic temp-file-rename writes,
//! no secrets (ADR-10).

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::StorageError;

/// Global (profile-independent) app settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Active profile id ("general", "coding", ...).
    pub active_profile: String,
    /// Capture device name; `None` = system default.
    pub input_device: Option<String>,
    /// Toggle-dictation hotkey (tauri global-shortcut syntax).
    pub hotkey_toggle: String,
    /// Push-to-talk hotkey (tauri global-shortcut syntax).
    pub hotkey_ptt: String,
    /// Whether insertions are recorded to local history (M7).
    pub history_enabled: bool,
    /// Maximum history rows kept; oldest trimmed beyond this.
    pub history_cap: u32,
    /// STT model file name inside the models dir (e.g.
    /// "ggml-large-v3-turbo-q5_0.bin"); `None` = the built-in default
    /// (base.en). A configured file that is missing on disk falls back to
    /// the default with a logged warning — the app must never start deaf.
    pub stt_model: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            active_profile: "general".into(),
            input_device: None,
            hotkey_toggle: "ctrl+shift+space".into(),
            hotkey_ptt: "ctrl+shift+d".into(),
            history_enabled: true,
            history_cap: 500,
            stt_model: None,
        }
    }
}

/// Loads settings; a missing file yields defaults (first run), but a
/// malformed file is an error — never silently discard a user's settings.
pub fn load_settings(path: &Path) -> Result<Settings, StorageError> {
    if !path.is_file() {
        return Ok(Settings::default());
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

/// Writes settings atomically: serialize to a temp file in the same
/// directory, flush, then rename over the target (MoveFileEx semantics on
/// Windows), so a crash can never leave a half-written file.
pub fn save_settings(path: &Path, settings: &Settings) -> Result<(), StorageError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(&mut tmp, settings)?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| StorageError::Io(e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let s = load_settings(&dir.path().join("settings.json")).unwrap();
        assert_eq!(s, Settings::default());
        assert_eq!(s.active_profile, "general");
    }

    #[test]
    fn round_trip_and_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut s = Settings {
            active_profile: "coding".into(),
            ..Settings::default()
        };
        save_settings(&path, &s).unwrap();
        assert_eq!(load_settings(&path).unwrap(), s);

        s.input_device = Some("USB Mic".into());
        save_settings(&path, &s).unwrap();
        assert_eq!(load_settings(&path).unwrap(), s);
    }

    #[test]
    fn malformed_file_is_an_error_not_a_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load_settings(&path).is_err());
    }

    #[test]
    fn unknown_fields_survive_default_merge() {
        // Forward compatibility: extra fields from a newer version load fine.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            "{\"active_profile\": \"email\", \"future_field\": 42}",
        )
        .unwrap();
        assert_eq!(load_settings(&path).unwrap().active_profile, "email");
    }
}
