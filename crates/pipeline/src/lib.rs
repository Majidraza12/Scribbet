//! Pipeline orchestration.
//!
//! Layers (bottom-up):
//! - [`Segmenter`]: raw STT events → sentence-bounded [`od_core_types::Segment`]s.
//! - [`Transcriber`]: VAD gate + STT engine + segmenter, synchronous.
//! - [`EventBus`]: bounded drop-on-full broadcast of [`od_core_types::AppEvent`]s.
//! - [`spawn`] / [`SessionHandle`]: the session-controller thread owning the
//!   microphone and driving the transcriber (M3).

#![warn(missing_docs)]

mod bus;
mod controller;
mod segmenter;
mod transcriber;

pub use bus::EventBus;
pub use controller::{SessionCommand, SessionHandle, spawn};
pub use segmenter::Segmenter;
pub use transcriber::{Transcriber, TranscriberConfig, TranscriberError};
