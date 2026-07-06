//! Shared domain types for the OpenDictate pipeline.
//!
//! This crate is a deliberate chokepoint (ADR-16): every stage speaks these
//! types, and nothing here may depend on audio backends, models, or OS APIs.
//! Churn in this crate is a design smell worth feeling.

#![warn(missing_docs)]

use std::time::Duration;

/// Identifies one finalized segment across the pipeline (insertion, history,
/// undo). Monotonically increasing within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SegmentId(pub u64);

/// Where a piece of transcribed text is in its lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    /// Unstable hypothesis; shown in the overlay pill, never inserted.
    Partial,
    /// Committed text; flows on to cleanup and insertion.
    Final,
}

/// A unit of transcribed text moving through the pipeline.
///
/// Produced by the segmenter from raw STT events; mutated in place by the
/// cleanup chain; consumed by the insertion engine.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// Identity for finals; partials reuse the id of the final they will
    /// become, so the overlay can replace in place.
    pub id: SegmentId,
    /// The text (raw from STT until cleanup has run).
    pub text: String,
    /// Partial or final.
    pub kind: SegmentKind,
    /// Offset of the segment's first audio sample from utterance start.
    pub audio_start: Duration,
    /// Offset of the segment's last audio sample from utterance start.
    pub audio_end: Duration,
}

/// Events emitted by a speech-to-text engine while decoding one utterance.
#[derive(Clone, Debug, PartialEq)]
pub enum SttEvent {
    /// A revised, still-unstable hypothesis for the *whole* utterance so far.
    Partial {
        /// Full-utterance hypothesis text.
        text: String,
        /// Length in bytes of the prefix considered stable so far
        /// (local-agreement; always a char boundary of `text`).
        stable_len: usize,
    },
    /// The utterance is done and this is its committed transcription.
    Final {
        /// Committed utterance text.
        text: String,
        /// Duration of audio the engine consumed for this utterance.
        audio_len: Duration,
    },
}

/// Language behavior requested from the STT engine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LanguageHint {
    /// Model decides per utterance (multilingual models only).
    #[default]
    Auto,
    /// Force an ISO 639-1 code ("en", "de", ...).
    Fixed(String),
}

/// Vocabulary bias fed to the STT engine per utterance (custom dictionary
/// terms, jargon). Engines are free to approximate (whisper: initial prompt).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VocabBias {
    /// Terms to bias recognition toward, most important first.
    pub terms: Vec<String>,
}

/// Session lifecycle as shown to the user (tray icon, overlay state color).
///
/// `Inserting` joins in M4 when the insertion engine exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Mic closed, hotkeys armed.
    Idle,
    /// Capturing and transcribing.
    Listening,
    /// Hotkey released; draining the pipeline and finalizing open utterances.
    Finalizing,
}

/// Domain events published on the app-wide event bus (docs/02-architecture.md
/// "Event bus"). Events are *facts* (past tense), fire-and-forget; publishing
/// never blocks a pipeline stage.
///
/// Times are milliseconds so events serialize cleanly over the UI bridge.
/// Variants arrive with the milestone that emits them (`Inserted`,
/// `CommandExecuted`, `Undo`, `ProfileChanged` are M4–M6).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppEvent {
    /// The session controller changed state.
    StateChanged {
        /// New state.
        state: SessionState,
    },
    /// The VAD confirmed speech.
    SpeechStarted {
        /// Position in the capture stream, ms.
        at_ms: u64,
    },
    /// The VAD confirmed the end of a speech run.
    SpeechEnded {
        /// Position in the capture stream, ms.
        at_ms: u64,
    },
    /// A revised streaming hypothesis (overlay only; never inserted).
    PartialUpdated {
        /// Segment id this hypothesis will finalize into.
        segment_id: SegmentId,
        /// Full-utterance hypothesis.
        text: String,
        /// Stable-prefix byte length (char boundary of `text`).
        stable_len: usize,
    },
    /// A final segment is ready for downstream stages.
    FinalReady {
        /// Segment identity.
        segment_id: SegmentId,
        /// Raw STT text (pre-cleanup), kept for history/debugging.
        raw: String,
        /// Text after the cleanup chain — what insertion delivers.
        cleaned: String,
    },
    /// A final segment's text was delivered into the target application.
    Inserted {
        /// Segment that was inserted.
        segment_id: SegmentId,
        /// Delivery mechanism: `"uia"`, `"send_input"`, or `"clipboard"`.
        tier: String,
        /// Wall-clock insertion cost.
        latency_ms: u64,
    },
    /// Insertion failed after all tiers; the text is preserved in the
    /// overlay (and, from M7, history) so nothing is lost.
    InsertFailed {
        /// Segment whose insertion failed.
        segment_id: SegmentId,
        /// Human-readable cause.
        error: String,
    },
    /// An utterance finished decoding (speech end → finals ready). Feeds the
    /// M7 latency HUD; the headline pipeline metric (P1-1).
    UtteranceFinalized {
        /// Wall-clock finalize cost.
        finalize_ms: u64,
    },
    /// The active profile/context changed (settings UI); takes effect from
    /// the next utterance.
    ProfileChanged {
        /// Display name of the new profile.
        name: String,
    },
}

impl serde::Serialize for SegmentId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(self.0)
    }
}

/// One dictionary replacement, resolved from storage into the active profile
/// snapshot (docs/02 "Cleanup chain", processor 3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DictEntry {
    /// The form STT tends to produce ("open dictate", "eye gore").
    pub spoken: String,
    /// The form the user wants inserted ("OpenDictate", "Igor").
    pub written: String,
    /// If true, `spoken` must match exactly; otherwise matching is
    /// case-insensitive on whole-word boundaries.
    pub case_sensitive: bool,
}

/// One user-defined regex → replacement rule (docs/02, processor 8).
/// Rules apply in order; an invalid pattern is skipped with a warning at
/// chain build time, never at segment time.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserRule {
    /// Regex pattern (Rust `regex` syntax).
    pub pattern: String,
    /// Replacement text (`$1`-style capture references allowed).
    pub replacement: String,
}

/// Filler-removal configuration (processor 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillerConfig {
    /// Master toggle.
    pub enabled: bool,
    /// Extra fillers on top of the built-in list (matched position-aware
    /// like the built-ins).
    pub extra: Vec<String>,
}

impl Default for FillerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extra: Vec::new(),
        }
    }
}

/// Spoken-symbol configuration (processor 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolsConfig {
    /// Master toggle.
    pub enabled: bool,
    /// Built-in table name ("general", "programming"); unknown names fall
    /// back to "general" with a warning.
    pub table: String,
}

impl Default for SymbolsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            table: "general".into(),
        }
    }
}

/// Punctuation-repair configuration (processor 5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PunctuationConfig {
    /// Master toggle.
    pub enabled: bool,
    /// Convert spoken punctuation words ("comma", "period", ...) into marks.
    pub spoken: bool,
    /// Append a period when a final segment ends without terminal
    /// punctuation.
    pub ensure_terminal: bool,
}

impl Default for PunctuationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            spoken: true,
            ensure_terminal: true,
        }
    }
}

/// Profile-format final pass configuration (processor 9). All off by
/// default; shipped profiles opt in per context.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormatConfig {
    /// Casing commands: "camel case foo bar" → `fooBar` (also snake/pascal/
    /// kebab/screaming variants).
    pub casing_commands: bool,
    /// Email layout: line break after a greeting line, around sign-offs.
    pub email_layout: bool,
    /// Prefix each final segment with "- " (meeting notes).
    pub bullets: bool,
}

/// Full cleanup-chain configuration; one per profile
/// (docs/02 "Cleanup chain": processors 1–9 in order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupConfig {
    /// Processor 1: whitespace normalization.
    pub whitespace: bool,
    /// Processor 2: filler removal.
    pub fillers: FillerConfig,
    /// Processor 3: dictionary replacements (entries come from the
    /// snapshot's resolved [`DictEntry`] list).
    pub dictionary: bool,
    /// Processor 4: spoken symbols.
    pub symbols: SymbolsConfig,
    /// Processor 5: punctuation repair.
    pub punctuation: PunctuationConfig,
    /// Processor 6: sentence-boundary refinement.
    pub segmentation: bool,
    /// Processor 7: capitalization (sentence starts, standalone "i",
    /// proper nouns). Coding profiles turn this off.
    pub capitalization: bool,
    /// Proper nouns for processor 7 (profile-supplied names).
    pub proper_nouns: Vec<String>,
    /// Processor 8: ordered user regex rules.
    pub user_rules: Vec<UserRule>,
    /// Processor 9: profile-specific final pass.
    pub format: FormatConfig,
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            whitespace: true,
            fillers: FillerConfig::default(),
            dictionary: true,
            symbols: SymbolsConfig::default(),
            punctuation: PunctuationConfig::default(),
            segmentation: true,
            capitalization: true,
            proper_nouns: Vec::new(),
            user_rules: Vec::new(),
            format: FormatConfig::default(),
        }
    }
}

/// The active profile, resolved into one immutable snapshot
/// (docs/02 "Profiles — the configuration unit").
///
/// Built by `od-storage` from a TOML profile plus the dictionary database;
/// swapped atomically between utterances, never mid-segment. STT-facing
/// fields (language, vocab bias) live directly on [`PipelineCtx`] — the
/// snapshot carries what the cleanup chain and rewriter consult.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileSnapshot {
    /// Display name ("General", "Coding", ...).
    pub name: String,
    /// Cleanup-chain configuration.
    pub cleanup: CleanupConfig,
    /// Dictionary set names this profile subscribes to (for UI display and
    /// re-resolution when the dictionary changes).
    pub dictionary_sets: Vec<String>,
    /// Resolved dictionary entries from those sets (processor 3 input).
    pub entries: Vec<DictEntry>,
    /// Cloud policy: hard deny for any network rewriter when false, even if
    /// one is enabled globally (docs/06 T2).
    pub rewriter_allowed: bool,
}

/// Per-utterance context every pipeline stage can consult.
///
/// M2 carried the STT-relevant fields; M5 adds the active-profile snapshot
/// (cleanup config, dictionaries, cloud policy) — see docs/02-architecture.md
/// "Profiles". The whole context is swapped atomically between utterances.
#[derive(Clone, Debug, Default)]
pub struct PipelineCtx {
    /// Language behavior for this utterance.
    pub language: LanguageHint,
    /// Vocabulary bias for this utterance.
    pub vocab: VocabBias,
    /// Active profile snapshot.
    pub profile: std::sync::Arc<ProfileSnapshot>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_ids_order() {
        assert!(SegmentId(1) < SegmentId(2));
    }

    #[test]
    fn stable_len_is_byte_prefix_contract() {
        // Documented contract: stable_len indexes bytes on a char boundary.
        let e = SttEvent::Partial {
            text: "héllo world".into(),
            stable_len: "héllo".len(),
        };
        if let SttEvent::Partial { text, stable_len } = e {
            assert!(text.is_char_boundary(stable_len));
        }
    }
}
