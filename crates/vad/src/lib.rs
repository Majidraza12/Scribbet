//! Voice activity detection: Silero v5 via ONNX Runtime.
//!
//! Two layers:
//! - [`SileroVad`] — raw model wrapper: 32 ms frame in, speech probability out.
//! - [`VadGate`] — the pipeline-facing state machine: stream samples in,
//!   [`VadEvent::SpeechStart`] / [`VadEvent::SpeechEnd`] out, with hysteresis,
//!   a minimum-speech filter against blips, and a configurable hangover so
//!   natural intra-sentence pauses don't split utterances.
//!
//! The model (~2.3 MB, MIT license, from the snakers4/silero-vad project) is
//! embedded in the binary: no download flow, no filesystem dependency, and
//! VAD is always available even before any STT model has been fetched.

#![warn(missing_docs)]

use std::time::Duration;

use ort::session::Session;
use ort::value::Tensor;
use thiserror::Error;

/// Samples per VAD frame at 16 kHz (32 ms). [`VadGate::feed`] accepts any
/// chunk size and buffers internally; this is the model's native granularity
/// and therefore the resolution of all event timestamps.
pub const FRAME_SAMPLES: usize = 512;

/// Sample rate the embedded model expects. Matches the pipeline's canonical
/// rate (`od_audio::TARGET_SAMPLE_RATE`).
pub const SAMPLE_RATE: u32 = 16_000;

/// Silero v5 keeps 64 samples of context from the previous frame.
const CONTEXT_SAMPLES: usize = 64;
/// LSTM state shape is `[2, 1, 128]`.
const STATE_LEN: usize = 2 * 128;

const MODEL_BYTES: &[u8] = include_bytes!("../models/silero_vad.onnx");

/// Errors from VAD model loading or inference.
#[derive(Debug, Error)]
pub enum VadError {
    /// The embedded model failed to load into ONNX Runtime.
    #[error("failed to initialize silero vad: {0}")]
    Init(String),
    /// A frame failed to run through the model.
    #[error("vad inference failed: {0}")]
    Inference(String),
}

/// Raw Silero v5 wrapper: one 512-sample frame in, speech probability out.
pub struct SileroVad {
    session: Session,
    /// LSTM hidden state, carried across frames within a stream.
    state: Vec<f32>,
    /// Tail of the previous frame, prepended per the v5 input contract.
    context: [f32; CONTEXT_SAMPLES],
}

impl SileroVad {
    /// Loads the embedded model.
    pub fn new() -> Result<Self, VadError> {
        let session = Session::builder()
            .and_then(|b| b.commit_from_memory(MODEL_BYTES))
            .map_err(|e| VadError::Init(e.to_string()))?;
        Ok(Self {
            session,
            state: vec![0.0; STATE_LEN],
            context: [0.0; CONTEXT_SAMPLES],
        })
    }

    /// Runs one frame; returns the speech probability in `[0, 1]`.
    ///
    /// # Panics
    ///
    /// Panics if `frame.len() != FRAME_SAMPLES` — the gate always delivers
    /// exact frames; a mismatch is a programming error, not a runtime
    /// condition.
    pub fn process_frame(&mut self, frame: &[f32]) -> Result<f32, VadError> {
        assert_eq!(
            frame.len(),
            FRAME_SAMPLES,
            "vad frames are exactly 512 samples"
        );

        // v5 input: [1, 64 context + 512 new samples].
        let mut input = Vec::with_capacity(CONTEXT_SAMPLES + FRAME_SAMPLES);
        input.extend_from_slice(&self.context);
        input.extend_from_slice(frame);
        self.context
            .copy_from_slice(&frame[FRAME_SAMPLES - CONTEXT_SAMPLES..]);

        let input_t = Tensor::from_array(([1usize, CONTEXT_SAMPLES + FRAME_SAMPLES], input))
            .map_err(|e| VadError::Inference(e.to_string()))?;
        let state_t = Tensor::from_array(([2usize, 1, 128], self.state.clone()))
            .map_err(|e| VadError::Inference(e.to_string()))?;
        let sr_t = Tensor::from_array(([1usize], vec![i64::from(SAMPLE_RATE)]))
            .map_err(|e| VadError::Inference(e.to_string()))?;

        let outputs = self
            .session
            .run(ort::inputs![
                "input" => input_t,
                "state" => state_t,
                "sr" => sr_t,
            ])
            .map_err(|e| VadError::Inference(e.to_string()))?;

        let (_, prob) = outputs["output"]
            .try_extract_tensor::<f32>()
            .map_err(|e| VadError::Inference(e.to_string()))?;
        let (_, new_state) = outputs["stateN"]
            .try_extract_tensor::<f32>()
            .map_err(|e| VadError::Inference(e.to_string()))?;
        self.state.copy_from_slice(new_state);

        Ok(prob[0])
    }

    /// Clears model state (call between independent audio streams).
    pub fn reset(&mut self) {
        self.state.fill(0.0);
        self.context.fill(0.0);
    }
}

/// Configuration for the [`VadGate`] state machine.
#[derive(Clone, Debug)]
pub struct VadConfig {
    /// Probability at or above which a frame counts as speech.
    pub threshold: f32,
    /// Probability below which a frame counts as silence while in speech
    /// (hysteresis; must be `<= threshold`).
    pub exit_threshold: f32,
    /// Speech must persist this long before `SpeechStart` fires (filters
    /// clicks and breaths).
    pub min_speech: Duration,
    /// Silence must persist this long before `SpeechEnd` fires (bridges
    /// natural pauses; this is the dominant term in finalization latency —
    /// see the latency budget in docs/02-architecture.md).
    pub hangover: Duration,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            exit_threshold: 0.35,
            min_speech: Duration::from_millis(96),
            hangover: Duration::from_millis(300),
        }
    }
}

/// Speech boundaries, positioned as absolute sample offsets from the start
/// of the stream fed into the gate (divide by [`SAMPLE_RATE`] for time —
/// or use [`VadEvent::at`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VadEvent {
    /// Speech began at (approximately) this sample offset.
    SpeechStart {
        /// First sample of the detected speech run.
        sample: u64,
    },
    /// Speech ended at this sample offset (the start of the silence run that
    /// confirmed the end, not the moment of confirmation).
    SpeechEnd {
        /// First sample of the confirming silence run.
        sample: u64,
    },
}

impl VadEvent {
    /// The event's position as a duration from stream start.
    pub fn at(&self) -> Duration {
        let sample = match self {
            Self::SpeechStart { sample } | Self::SpeechEnd { sample } => *sample,
        };
        Duration::from_secs_f64(sample as f64 / f64::from(SAMPLE_RATE))
    }
}

#[derive(Clone, Copy, Debug)]
enum GateState {
    Silence,
    /// Speech-like frames seen, waiting for `min_speech`.
    PendingSpeech {
        start: u64,
        frames: u32,
    },
    Speech,
    /// Silence-like frames seen while in speech, waiting for `hangover`.
    PendingSilence {
        start: u64,
        frames: u32,
    },
}

/// Streaming gate: feed arbitrary sample chunks, receive speech boundaries.
pub struct VadGate {
    vad: SileroVad,
    config: VadConfig,
    min_speech_frames: u32,
    hangover_frames: u32,
    state: GateState,
    /// Partial frame buffered between feeds.
    pending: Vec<f32>,
    /// Absolute offset of the next un-processed sample.
    position: u64,
}

impl VadGate {
    /// Creates a gate with the given configuration.
    pub fn new(config: VadConfig) -> Result<Self, VadError> {
        let frame_ms = 1000.0 * FRAME_SAMPLES as f64 / f64::from(SAMPLE_RATE);
        let to_frames = |d: Duration| ((d.as_secs_f64() * 1000.0 / frame_ms).ceil() as u32).max(1);
        Ok(Self {
            vad: SileroVad::new()?,
            min_speech_frames: to_frames(config.min_speech),
            hangover_frames: to_frames(config.hangover),
            config,
            state: GateState::Silence,
            pending: Vec::with_capacity(FRAME_SAMPLES),
            position: 0,
        })
    }

    /// Feeds a chunk of 16 kHz mono samples; boundary events are appended to
    /// `out` in stream order.
    pub fn feed(&mut self, samples: &[f32], out: &mut Vec<VadEvent>) -> Result<(), VadError> {
        let mut remaining = samples;
        while !remaining.is_empty() {
            let take = (FRAME_SAMPLES - self.pending.len()).min(remaining.len());
            self.pending.extend_from_slice(&remaining[..take]);
            remaining = &remaining[take..];
            if self.pending.len() == FRAME_SAMPLES {
                let frame_start = self.position;
                let prob = {
                    let frame: &[f32] = &self.pending;
                    self.vad.process_frame(frame)?
                };
                self.pending.clear();
                self.position += FRAME_SAMPLES as u64;
                self.advance(prob, frame_start, out);
            }
        }
        Ok(())
    }

    /// Whether the gate currently considers the stream to be inside speech
    /// (including the pending-silence hangover window).
    pub fn in_speech(&self) -> bool {
        matches!(
            self.state,
            GateState::Speech | GateState::PendingSilence { .. }
        )
    }

    /// Resets stream state (between capture sessions). Sample positions
    /// restart at zero.
    pub fn reset(&mut self) {
        self.vad.reset();
        self.state = GateState::Silence;
        self.pending.clear();
        self.position = 0;
    }

    fn advance(&mut self, prob: f32, frame_start: u64, out: &mut Vec<VadEvent>) {
        let is_speech = prob >= self.config.threshold;
        let is_silence = prob < self.config.exit_threshold;

        self.state = match self.state {
            GateState::Silence => {
                if is_speech {
                    let next = GateState::PendingSpeech {
                        start: frame_start,
                        frames: 1,
                    };
                    // min_speech of one frame fires immediately.
                    self.maybe_confirm_speech(next, out)
                } else {
                    GateState::Silence
                }
            }
            GateState::PendingSpeech { start, frames } => {
                if is_speech {
                    let next = GateState::PendingSpeech {
                        start,
                        frames: frames + 1,
                    };
                    self.maybe_confirm_speech(next, out)
                } else {
                    // Blip shorter than min_speech: discard.
                    GateState::Silence
                }
            }
            GateState::Speech => {
                if is_silence {
                    let next = GateState::PendingSilence {
                        start: frame_start,
                        frames: 1,
                    };
                    self.maybe_confirm_silence(next, out)
                } else {
                    GateState::Speech
                }
            }
            GateState::PendingSilence { start, frames } => {
                if is_silence {
                    let next = GateState::PendingSilence {
                        start,
                        frames: frames + 1,
                    };
                    self.maybe_confirm_silence(next, out)
                } else if prob >= self.config.threshold {
                    // Pause was shorter than the hangover: still speaking.
                    GateState::Speech
                } else {
                    // Between thresholds: stay pending without progress.
                    GateState::PendingSilence { start, frames }
                }
            }
        };
    }

    fn maybe_confirm_speech(&self, next: GateState, out: &mut Vec<VadEvent>) -> GateState {
        if let GateState::PendingSpeech { start, frames } = next
            && frames >= self.min_speech_frames
        {
            out.push(VadEvent::SpeechStart { sample: start });
            tracing::debug!(sample = start, "speech start");
            return GateState::Speech;
        }
        next
    }

    fn maybe_confirm_silence(&self, next: GateState, out: &mut Vec<VadEvent>) -> GateState {
        if let GateState::PendingSilence { start, frames } = next
            && frames >= self.hangover_frames
        {
            out.push(VadEvent::SpeechEnd { sample: start });
            tracing::debug!(sample = start, "speech end");
            return GateState::Silence;
        }
        next
    }
}
