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
