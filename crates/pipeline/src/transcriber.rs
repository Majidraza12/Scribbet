//! The synchronous transcription core: VAD gate + STT engine + segmenter.
//!
//! Owns the "which samples does the engine hear" decision: audio flows to
//! the engine only between `SpeechStart` and `SpeechEnd` (plus a pre-roll so
//! word onsets clipped by the VAD's confirmation delay are recovered), so
//! STT burns no CPU on silence.
//!
//! Runs on the dedicated compute thread (docs/02-architecture.md threading
//! model); the M3 session controller feeds it from the audio ring.

use std::time::{Duration, Instant};

use od_core_types::{PipelineCtx, Segment};
use od_stt::{SttEngine, SttError};
use od_vad::{SAMPLE_RATE, VadConfig, VadError, VadEvent, VadGate};
use thiserror::Error;

/// Errors surfaced while transcribing a stream.
#[derive(Debug, Error)]
pub enum TranscriberError {
    /// The VAD failed; per the failure policy the caller should fall back to
    /// hotkey-gated (ungated) capture.
    #[error(transparent)]
    Vad(#[from] VadError),
    /// The STT engine failed for this utterance.
    #[error(transparent)]
    Stt(#[from] SttError),
}

/// Configuration for [`Transcriber`].
#[derive(Clone, Debug)]
pub struct TranscriberConfig {
    /// VAD gate tuning.
    pub vad: VadConfig,
    /// Audio included before the detected speech start (recovers onsets the
    /// gate's `min_speech` confirmation delay would clip).
    pub preroll: Duration,
}

impl Default for TranscriberConfig {
    fn default() -> Self {
        Self {
            vad: VadConfig::default(),
            preroll: Duration::from_millis(300),
        }
    }
}

/// Streaming transcriber: feed 16 kHz mono samples, receive [`Segment`]s.
pub struct Transcriber<E: SttEngine> {
    gate: VadGate,
    engine: E,
    segmenter: crate::Segmenter,
    ctx: PipelineCtx,
    preroll_samples: u64,

    /// Rolling recent-audio window backing pre-roll and engine feeds.
    window: Vec<f32>,
    /// Absolute stream offset of `window[0]`.
    window_base: u64,
    /// Absolute stream offset one past the last sample in `window`.
    position: u64,

    in_speech: bool,
    /// Samples up to this absolute offset have been fed to the engine.
    fed_up_to: u64,

    /// Wall-clock duration of the most recent finalization (SpeechEnd →
    /// final segments ready) — the pipeline's headline latency metric.
    last_finalize: Option<Duration>,

    /// Scratch for VAD events (reused across feeds).
    vad_events: Vec<VadEvent>,
}

impl<E: SttEngine> Transcriber<E> {
    /// Creates a transcriber around an engine. `ctx` applies to every
    /// utterance until [`set_ctx`](Self::set_ctx).
    pub fn new(
        config: &TranscriberConfig,
        engine: E,
        ctx: PipelineCtx,
    ) -> Result<Self, TranscriberError> {
        Ok(Self {
            gate: VadGate::new(config.vad.clone())?,
            engine,
            segmenter: crate::Segmenter::new(),
            ctx,
            preroll_samples: (config.preroll.as_secs_f64() * f64::from(SAMPLE_RATE)) as u64,
            window: Vec::with_capacity(4 * SAMPLE_RATE as usize),
            window_base: 0,
            position: 0,
            in_speech: false,
            fed_up_to: 0,
            last_finalize: None,
            vad_events: Vec::new(),
        })
    }

    /// Replaces the per-utterance context (takes effect from the next
    /// utterance; never mid-utterance).
    pub fn set_ctx(&mut self, ctx: PipelineCtx) {
        self.ctx = ctx;
    }

    /// Wall-clock time of the most recent SpeechEnd → finals-ready run.
    pub fn last_finalize_latency(&self) -> Option<Duration> {
        self.last_finalize
    }

    /// The wrapped engine (read-only; used by callers for engine-specific
    /// inspection and by tests).
    pub fn engine(&self) -> &E {
        &self.engine
    }

    /// Feeds a chunk of samples; produced segments (partials and finals) are
    /// appended to `out` in order.
    pub fn feed(
        &mut self,
        samples: &[f32],
        out: &mut Vec<Segment>,
    ) -> Result<(), TranscriberError> {
        self.window.extend_from_slice(samples);
        self.position += samples.len() as u64;

        self.vad_events.clear();
        let mut events = std::mem::take(&mut self.vad_events);
        self.gate.feed(samples, &mut events)?;

        for event in &events {
            match *event {
                VadEvent::SpeechStart { sample } => {
                    self.engine.begin_utterance(&self.ctx)?;
                    self.in_speech = true;
                    self.fed_up_to = sample
                        .saturating_sub(self.preroll_samples)
                        .max(self.window_base);
                }
                VadEvent::SpeechEnd { sample } => {
                    // Deliver speech up to the boundary, then finalize.
                    self.feed_engine_range(self.fed_up_to, sample, out)?;
                    let t0 = Instant::now();
                    for ev in self.engine.end_utterance()? {
                        self.segmenter.on_event(&ev, out);
                    }
                    self.last_finalize = Some(t0.elapsed());
                    tracing::info!(
                        finalize_ms = t0.elapsed().as_millis() as u64,
                        "utterance finalized"
                    );
                    self.in_speech = false;
                    self.fed_up_to = sample;
                }
            }
        }
        self.vad_events = events;

        // Stream the current chunk's speech into the engine.
        if self.in_speech {
            self.feed_engine_range(self.fed_up_to, self.position, out)?;
        }

        self.trim_window();
        Ok(())
    }

    /// Ends the stream (hotkey released / session over): finalizes any open
    /// utterance even though the gate saw no closing silence, and resets all
    /// stream state. Returns finals via `out`.
    pub fn finish(&mut self, out: &mut Vec<Segment>) -> Result<(), TranscriberError> {
        if self.in_speech {
            self.feed_engine_range(self.fed_up_to, self.position, out)?;
            let t0 = Instant::now();
            for ev in self.engine.end_utterance()? {
                self.segmenter.on_event(&ev, out);
            }
            self.last_finalize = Some(t0.elapsed());
            self.in_speech = false;
        }
        self.gate.reset();
        self.window.clear();
        self.window_base = 0;
        self.position = 0;
        self.fed_up_to = 0;
        Ok(())
    }

    /// Feeds `window[from..to)` (absolute offsets) to the engine and routes
    /// any partial events through the segmenter.
    fn feed_engine_range(
        &mut self,
        from: u64,
        to: u64,
        out: &mut Vec<Segment>,
    ) -> Result<(), TranscriberError> {
        if to <= from {
            return Ok(());
        }
        debug_assert!(from >= self.window_base, "range fell out of the window");
        let a = (from - self.window_base) as usize;
        let b = (to - self.window_base) as usize;
        let events = self
            .engine
            .feed(&self.window[a..b.min(self.window.len())])?;
        for ev in events {
            self.segmenter.on_event(&ev, out);
        }
        self.fed_up_to = to;
        Ok(())
    }

    /// Drops window audio no longer needed: everything older than pre-roll
    /// reach behind the newest sample (or behind `fed_up_to` while in
    /// speech, which is always newer).
    fn trim_window(&mut self) {
        let keep_from = self
            .position
            .saturating_sub(self.preroll_samples + 2 * u64::from(SAMPLE_RATE));
        if keep_from > self.window_base {
            let n = (keep_from - self.window_base) as usize;
            self.window.drain(..n);
            self.window_base = keep_from;
        }
    }
}
