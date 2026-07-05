//! Pipeline orchestration.
//!
//! M2 delivers the synchronous compute core used by the session controller
//! (M3): the [`Segmenter`] (raw STT events → sentence-bounded [`Segment`]s)
//! and the [`Transcriber`] (VAD gate + STT engine + segmenter wired
//! together). The tokio actor shell, hotkeys, and state machine arrive in
//! M3 and drive [`Transcriber::feed`] from the compute thread.

#![warn(missing_docs)]

mod segmenter;
mod transcriber;

pub use segmenter::Segmenter;
pub use transcriber::{Transcriber, TranscriberConfig, TranscriberError};
