//! whisper.cpp backend for [`SttEngine`].
//!
//! Streaming is emulated (ADR-5): the utterance buffer is re-decoded at a
//! cadence ([`WhisperConfig::decode_interval`] of *new* audio), hypotheses
//! are stabilized by [`LocalAgreement`], and `end_utterance` performs the
//! authoritative full decode. Utterances are hotkey/VAD-bounded, so buffers
//! stay short; a hard window cap protects the decoder from pathological
//! cases near whisper's 30 s context limit.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use od_core_types::{LanguageHint, PipelineCtx, SttEvent};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::{LocalAgreement, SttEngine, SttError};

/// Pipeline sample rate (matches `od_audio::TARGET_SAMPLE_RATE`).
const SAMPLE_RATE: usize = 16_000;
/// whisper.cpp rejects inputs shorter than ~1 s; shorter buffers are padded.
const MIN_DECODE_SAMPLES: usize = SAMPLE_RATE + SAMPLE_RATE / 10;
/// Hard cap below whisper's 30 s window.
const MAX_WINDOW_SAMPLES: usize = 29 * SAMPLE_RATE;

/// Configuration for [`WhisperEngine`].
#[derive(Clone, Debug)]
pub struct WhisperConfig {
    /// Path to a ggml/gguf whisper model (see `scripts/fetch-models.ps1`).
    pub model_path: PathBuf,
    /// Minimum *new* audio between partial decodes. Lower = snappier
    /// partials, more CPU burned on re-decoding.
    pub decode_interval: Duration,
    /// Decoder threads; `None` = physical parallelism, capped at 4 (base.en
    /// gains little beyond that, and this is a background workload).
    pub threads: Option<usize>,
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            model_path: default_model_path(),
            decode_interval: Duration::from_millis(700),
            threads: None,
        }
    }
}

/// Default location of the dev-fetched STT model
/// (`%LOCALAPPDATA%/OpenDictate/models/`).
pub fn default_model_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("OpenDictate")
        .join("models")
        .join("ggml-base.en-q5_1.bin")
}

/// [`SttEngine`] backed by whisper.cpp.
pub struct WhisperEngine {
    ctx: WhisperContext,
    config: WhisperConfig,
    threads: i32,
    /// Utterance audio accumulated so far.
    buffer: Vec<f32>,
    /// Buffer length at the last partial decode.
    decoded_up_to: usize,
    agreement: LocalAgreement,
    /// Forced language for this utterance (`None` = model auto-detect).
    language: Option<String>,
    /// Initial prompt built from the vocabulary bias.
    prompt: String,
    utterance_active: bool,
}

impl WhisperEngine {
    /// Loads the model. Expensive (~100 ms mmap + graph setup); construct
    /// once and reuse across utterances.
    pub fn new(config: WhisperConfig) -> Result<Self, SttError> {
        let path = &config.model_path;
        if !path.is_file() {
            return Err(SttError::ModelUnavailable {
                path: path.display().to_string(),
                reason: "file not found".into(),
            });
        }
        let path_str = path.to_str().ok_or_else(|| SttError::ModelUnavailable {
            path: path.display().to_string(),
            reason: "path is not valid UTF-8".into(),
        })?;

        let ctx = WhisperContext::new_with_params(path_str, WhisperContextParameters::default())
            .map_err(|e| SttError::ModelUnavailable {
                path: path.display().to_string(),
                reason: e.to_string(),
            })?;

        let threads = config
            .threads
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get().min(4)))
            .max(1) as i32;

        Ok(Self {
            ctx,
            config,
            threads,
            buffer: Vec::with_capacity(30 * SAMPLE_RATE),
            decoded_up_to: 0,
            agreement: LocalAgreement::new(),
            language: None,
            prompt: String::new(),
            utterance_active: false,
        })
    }

    /// Runs one full decode of the current buffer; returns the hypothesis.
    fn decode(&mut self) -> Result<String, SttError> {
        let start = Instant::now();

        // Clamp to the window cap (keeps the decoder well under whisper's
        // 30 s context); pad short buffers up to the decoder's minimum.
        let from = self.buffer.len().saturating_sub(MAX_WINDOW_SAMPLES);
        let mut samples: Vec<f32> = self.buffer[from..].to_vec();
        if samples.len() < MIN_DECODE_SAMPLES {
            samples.resize(MIN_DECODE_SAMPLES, 0.0);
        }

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        // NOTE (P1-1): the whisper.cpp `audio_ctx` shortcut (attend only the
        // frames the clip fills) was tried here and rejected with data: on
        // base.en q5_1 a truncated positional context makes greedy decoding
        // unstable — repeat loops ("2 2 2…"), dropped tails, and bimodal
        // 0.4 s/17 s decode times — in every combination with
        // single_segment/no_timestamps (docs/04, P1-1 entry). Full-window
        // decodes are stable at ~1 s; the path to ≤300 ms is a streaming
        // backend (Moonshine, post-v1), not this knob.
        params.set_n_threads(self.threads);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_no_context(true);
        if let Some(lang) = &self.language {
            params.set_language(Some(lang));
        }
        if !self.prompt.is_empty() {
            params.set_initial_prompt(&self.prompt);
        }

        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| SttError::Decode(e.to_string()))?;
        state
            .full(params, &samples)
            .map_err(|e| SttError::Decode(e.to_string()))?;

        let n = state.full_n_segments();
        let mut text = String::new();
        for i in 0..n {
            let seg = state
                .get_segment(i)
                .ok_or_else(|| SttError::Decode(format!("segment {i} missing")))?;
            let seg_text = seg.to_str().map_err(|e| SttError::Decode(e.to_string()))?;
            text.push_str(seg_text);
        }
        let text = text.trim().to_owned();

        tracing::debug!(
            audio_ms = samples.len() * 1000 / SAMPLE_RATE,
            decode_ms = start.elapsed().as_millis() as u64,
            "whisper decode"
        );
        Ok(text)
    }
}

impl SttEngine for WhisperEngine {
    fn begin_utterance(&mut self, ctx: &PipelineCtx) -> Result<(), SttError> {
        self.buffer.clear();
        self.decoded_up_to = 0;
        self.agreement.reset();
        self.language = match &ctx.language {
            LanguageHint::Auto => None,
            LanguageHint::Fixed(code) => Some(code.clone()),
        };
        self.prompt = if ctx.vocab.terms.is_empty() {
            String::new()
        } else {
            // Whisper has no true vocabulary biasing; an initial prompt
            // containing the terms is the standard approximation.
            format!("Glossary: {}.", ctx.vocab.terms.join(", "))
        };
        self.utterance_active = true;
        Ok(())
    }

    fn feed(&mut self, samples: &[f32]) -> Result<Vec<SttEvent>, SttError> {
        assert!(self.utterance_active, "feed outside begin/end utterance");
        self.buffer.extend_from_slice(samples);

        let interval_samples =
            (self.config.decode_interval.as_secs_f64() * SAMPLE_RATE as f64) as usize;
        if self.buffer.len() - self.decoded_up_to < interval_samples.max(1) {
            return Ok(Vec::new());
        }
        self.decoded_up_to = self.buffer.len();

        let text = self.decode()?;
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let stable_len = self.agreement.push(&text);
        Ok(vec![SttEvent::Partial { text, stable_len }])
    }

    fn end_utterance(&mut self) -> Result<Vec<SttEvent>, SttError> {
        assert!(self.utterance_active, "end_utterance without begin");
        self.utterance_active = false;

        let audio_len = Duration::from_secs_f64(self.buffer.len() as f64 / SAMPLE_RATE as f64);
        if self.buffer.is_empty() {
            return Ok(vec![SttEvent::Final {
                text: String::new(),
                audio_len,
            }]);
        }
        let text = self.decode()?;
        self.buffer.clear();
        self.decoded_up_to = 0;
        self.agreement.reset();
        Ok(vec![SttEvent::Final { text, audio_len }])
    }
}
