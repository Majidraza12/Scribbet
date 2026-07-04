//! Error type for the audio crate.
//!
//! cpal error types are deliberately not re-exported: they are flattened to
//! strings so that downstream crates (`od-pipeline`) don't couple to cpal's
//! error surface, which has churned across cpal releases.

use thiserror::Error;

/// Errors produced by device enumeration and capture-session management.
#[derive(Debug, Error)]
pub enum AudioError {
    /// No input device is available on the system (or none is set as default).
    #[error("no audio input device available")]
    NoDevice,

    /// A device requested by name was not found among the system's input devices.
    #[error("audio input device not found: {0:?}")]
    DeviceNotFound(String),

    /// The host failed to enumerate audio devices.
    #[error("failed to enumerate audio devices: {0}")]
    Enumerate(String),

    /// The device rejected the query for its default input configuration.
    #[error("failed to query device configuration: {0}")]
    DeviceConfig(String),

    /// The device reports a sample format this crate does not convert from.
    #[error("unsupported input sample format: {0}")]
    UnsupportedFormat(String),

    /// Building the input stream failed (device busy, format mismatch, ...).
    #[error("failed to build audio input stream: {0}")]
    StreamBuild(String),

    /// Starting playback of the built stream failed.
    #[error("failed to start audio input stream: {0}")]
    StreamStart(String),
}
