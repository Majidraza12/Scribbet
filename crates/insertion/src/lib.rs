//! Universal text insertion: the [`TextInserter`] trait and its Windows
//! backend.
//!
//! Three tiers, tried in order per target (docs/02, ADR-8):
//! 1. **UIA ValuePattern** — atomic set for empty editable fields; UIA also
//!    supplies the editability/password probe the other tiers rely on.
//! 2. **SendInput unicode** — synthesized key events; works in nearly
//!    everything with a caret.
//! 3. **Clipboard paste-and-restore** — set clipboard, send Ctrl+V, restore;
//!    last resort, and the *preferred* tier for terminals (quirk table)
//!    where per-character key events are slow or mangled.
//!
//! Safety rules (docs/06 T8): insertion targets only the focus captured via
//! [`TextInserter::capture_focus`]; if the foreground window changed by
//! insert time the inserter re-verifies and follows the *user's* new focus
//! rather than typing into a stale window blindly. Password fields never go
//! through the clipboard tier.

#![warn(missing_docs)]

use std::time::Duration;

use thiserror::Error;

#[cfg(windows)]
mod windows_impl;
#[cfg(windows)]
pub use windows_impl::WindowsInserter;
#[cfg(windows)]
pub use windows_impl::copy_to_clipboard;

/// Non-Windows stub: reports the clipboard as unavailable.
#[cfg(not(windows))]
pub fn copy_to_clipboard(_text: &str) -> Result<(), String> {
    Err("clipboard unavailable on this platform".into())
}

/// Identifies a top-level window (HWND on Windows) without exposing
/// platform handle types to portable code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowId(pub isize);

/// The insertion target, captured at hotkey press.
#[derive(Clone, Debug)]
pub struct FocusInfo {
    /// Foreground window at capture time.
    pub window: WindowId,
    /// Owning process id.
    pub pid: u32,
    /// Executable name, lowercase (`"notepad.exe"`); drives the quirk table.
    pub process: String,
    /// Window title at capture time (matched, never persisted — docs/02
    /// context-detection privacy note).
    pub title: String,
}

/// Which mechanism ultimately delivered the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertionTier {
    /// UIA ValuePattern set.
    Uia,
    /// Synthesized unicode key events.
    SendInput,
    /// Clipboard swap + Ctrl+V + restore.
    Clipboard,
}

/// Successful insertion report.
#[derive(Clone, Copy, Debug)]
pub struct InsertOutcome {
    /// Tier that delivered the text.
    pub tier: InsertionTier,
    /// Wall-clock cost of the insertion.
    pub duration: Duration,
}

/// Insertion failures.
#[derive(Debug, Error)]
pub enum InsertError {
    /// No usable foreground window (e.g. desktop or a protected window).
    #[error("no insertable window has focus")]
    NoFocus,
    /// The target window disappeared between capture and insertion.
    #[error("target window is gone")]
    TargetGone,
    /// Every applicable tier failed; carries the last tier's error text.
    #[error("all insertion tiers failed: {0}")]
    AllTiersFailed(String),
    /// Platform call failure outside the tier fallback path.
    #[error("platform error: {0}")]
    Platform(String),
}

/// A text-insertion backend. One per platform (Windows in v1; macOS AX and
/// Linux AT-SPI backends are post-v1 — ADR-17).
pub trait TextInserter {
    /// Snapshots the current foreground target. Called at hotkey press, so
    /// text lands where the user started dictating.
    fn capture_focus(&mut self) -> Result<FocusInfo, InsertError>;

    /// Inserts `text` at the caret of the target application.
    fn insert(&mut self, text: &str, target: &FocusInfo) -> Result<InsertOutcome, InsertError>;
}

impl InsertionTier {
    /// Stable string form used in [`od-core-types`]-level events.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Uia => "uia",
            Self::SendInput => "send_input",
            Self::Clipboard => "clipboard",
        }
    }
}

/// A no-op inserter for tests and platforms without a backend yet: focus
/// capture and insertion always fail with [`InsertError::NoFocus`], which
/// the pipeline treats as "display-only mode".
#[derive(Debug, Default)]
pub struct NullInserter;

impl TextInserter for NullInserter {
    fn capture_focus(&mut self) -> Result<FocusInfo, InsertError> {
        Err(InsertError::NoFocus)
    }

    fn insert(&mut self, _text: &str, _target: &FocusInfo) -> Result<InsertOutcome, InsertError> {
        Err(InsertError::NoFocus)
    }
}

/// Per-application overrides for tier selection and pacing.
#[derive(Clone, Copy, Debug)]
pub struct AppQuirk {
    /// Preferred tier (skips earlier tiers; later tiers still act as
    /// fallback in declared order).
    pub prefer: InsertionTier,
    /// Delay after a clipboard paste before restoring the clipboard, if this
    /// app is slow to consume WM_PASTE.
    pub paste_settle: Duration,
    /// Pause inserted between SendInput batches for apps that drop rapid
    /// synthetic input.
    pub keystroke_pacing: Duration,
}

impl Default for AppQuirk {
    fn default() -> Self {
        Self {
            prefer: InsertionTier::Uia,
            paste_settle: Duration::from_millis(150),
            keystroke_pacing: Duration::ZERO,
        }
    }
}

/// Built-in quirk table (process name, lowercase → quirk). User-editable
/// overrides arrive with settings in M7.
pub fn builtin_quirk(process: &str) -> AppQuirk {
    match process {
        // Terminals: per-character synthetic input is slow and some shells
        // interpret it; a single paste is what users expect.
        "windowsterminal.exe"
        | "wt.exe"
        | "conhost.exe"
        | "mintty.exe"
        | "alacritty.exe"
        | "wezterm-gui.exe" => AppQuirk {
            prefer: InsertionTier::Clipboard,
            paste_settle: Duration::from_millis(250),
            ..AppQuirk::default()
        },
        // RDP/VM viewers forward raw input; paste avoids per-key latency.
        "mstsc.exe" | "vmconnect.exe" => AppQuirk {
            prefer: InsertionTier::Clipboard,
            paste_settle: Duration::from_millis(400),
            ..AppQuirk::default()
        },
        // Electron/Chromium apps: UIA ValuePattern "succeeds" against hidden
        // accessibility nodes without touching the visible editor (observed
        // in Cursor — SetValue reported ok, no text on screen). Skip UIA and
        // type real keystrokes, which web UIs handle exactly like a user.
        "cursor.exe" | "code.exe" | "chrome.exe" | "msedge.exe" | "brave.exe" | "firefox.exe"
        | "discord.exe" | "slack.exe" | "notion.exe" | "obsidian.exe" | "teams.exe" => AppQuirk {
            prefer: InsertionTier::SendInput,
            ..AppQuirk::default()
        },
        _ => AppQuirk::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_quirk_prefers_uia_tier() {
        let q = builtin_quirk("notepad.exe");
        assert_eq!(q.prefer, InsertionTier::Uia);
    }

    #[test]
    fn terminals_prefer_clipboard() {
        assert_eq!(
            builtin_quirk("windowsterminal.exe").prefer,
            InsertionTier::Clipboard
        );
        assert_eq!(builtin_quirk("mintty.exe").prefer, InsertionTier::Clipboard);
    }

    #[test]
    fn quirk_lookup_is_case_sensitive_lowercase_contract() {
        // FocusInfo::process is documented lowercase; the table relies on it.
        assert_eq!(
            builtin_quirk("WindowsTerminal.exe").prefer,
            InsertionTier::Uia
        );
    }
}
