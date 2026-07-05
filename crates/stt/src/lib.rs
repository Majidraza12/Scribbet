//! Speech-to-text: the [`SttEngine`] trait and the whisper.cpp backend.
//!
//! The trait is the pipeline's seam (docs/02-architecture.md): the default
//! backend is [`WhisperEngine`] (whisper.cpp, ADR-5); a Moonshine ONNX
//! backend can slot in post-v1 without touching callers.
//!
//! Streaming model: whisper has no native streaming, so [`WhisperEngine`]
//! re-decodes the growing utterance buffer at a configurable cadence and
//! runs the hypotheses through [`LocalAgreement`] — a prefix two consecutive
//! decodes agree on is considered stable and emitted as a partial. The final
//! decode on `end_utterance` is authoritative.

#![warn(missing_docs)]

mod agreement;
mod whisper;

use od_core_types::{PipelineCtx, SttEvent};
use thiserror::Error;

pub use agreement::LocalAgreement;
pub use whisper::{WhisperConfig, WhisperEngine, default_model_path};

/// Errors from STT engine construction or decoding.
#[derive(Debug, Error)]
pub enum SttError {
    /// The model file is missing or unreadable.
    #[error("stt model not available at {path}: {reason} (run scripts/fetch-models.ps1)")]
    ModelUnavailable {
        /// Path that was tried.
        path: String,
        /// Underlying cause.
        reason: String,
    },
    /// The backend failed to decode audio.
    #[error("stt decode failed: {0}")]
    Decode(String),
}

/// A speech-to-text engine decoding one utterance at a time.
///
/// Lifecycle: `begin_utterance` → any number of `feed`s (each may emit
/// partial events) → `end_utterance` (emits the final). Engines are reused
/// across utterances; `begin_utterance` resets per-utterance state.
pub trait SttEngine {
    /// Starts a new utterance with the given context (language hint,
    /// vocabulary bias).
    fn begin_utterance(&mut self, ctx: &PipelineCtx) -> Result<(), SttError>;

    /// Feeds 16 kHz mono samples of the current utterance. May return
    /// `Partial` events when the engine chooses to decode.
    fn feed(&mut self, samples: &[f32]) -> Result<Vec<SttEvent>, SttError>;

    /// Declares the utterance complete and returns its `Final` event
    /// (possibly preceded by a last partial).
    fn end_utterance(&mut self) -> Result<Vec<SttEvent>, SttError>;
}
