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
        /// Raw STT text (pre-cleanup; `cleaned` field joins in M5).
        raw: String,
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
}

impl serde::Serialize for SegmentId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(self.0)
    }
}

/// Per-utterance context every pipeline stage can consult.
///
/// M2 carries the STT-relevant fields; the active-profile snapshot (cleanup
/// config, dictionaries, cloud policy) lands here in M5 — see
/// docs/02-architecture.md "Profiles".
#[derive(Clone, Debug, Default)]
pub struct PipelineCtx {
    /// Language behavior for this utterance.
    pub language: LanguageHint,
    /// Vocabulary bias for this utterance.
    pub vocab: VocabBias,
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
