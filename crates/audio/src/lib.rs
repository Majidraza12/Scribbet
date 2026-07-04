//! Microphone capture for OpenDictate.
//!
//! Owns the capture edge of the pipeline: open an input device via cpal
//! (WASAPI on Windows), convert whatever the device delivers to the
//! pipeline's canonical format — **16 kHz mono f32** ([`TARGET_SAMPLE_RATE`])
//! — and hand it to the downstream compute thread through a wait-free SPSC
//! ring buffer.
//!
//! ```no_run
//! use od_audio::{CaptureConfig, CaptureSession};
//!
//! let (session, mut samples) = CaptureSession::start(&CaptureConfig::default())?;
//! let mut buf = vec![0.0f32; 1600]; // 100 ms
//! loop {
//!     let n = samples.pop_slice(&mut buf);
//!     if n > 0 {
//!         // feed buf[..n] to VAD/STT
//!     }
//!     # break;
//! }
//! session.stop();
//! # Ok::<(), od_audio::AudioError>(())
//! ```
//!
//! Design notes (see `docs/02-architecture.md`):
//! - the cpal callback is real-time safe: no locks, no I/O, no steady-state
//!   allocation; level metering is a polled atomic ([`LevelMeter`]);
//! - overruns drop the *newest* samples and are counted, never blocking;
//! - device hot-swap is session-granular: stop, then start with a new
//!   [`DeviceSelector`]; unplugged devices surface via
//!   [`CaptureSession::is_disconnected`].

#![warn(missing_docs)]

mod capture;
mod device;
mod dsp;
mod error;
mod meter;
mod ring;

pub use capture::{CaptureConfig, CaptureSession};
pub use device::{DeviceSelector, InputDeviceInfo, list_input_devices};
pub use dsp::{LinearResampler, TARGET_SAMPLE_RATE, downmix_interleaved, rms};
pub use error::AudioError;
pub use meter::LevelMeter;
pub use ring::{AudioConsumer, AudioProducer, audio_ring};
