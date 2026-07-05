//! Transcriber flow tests.
//!
//! The CI-safe tests drive a real VAD (embedded model) over TTS fixtures
//! with a mock STT engine, verifying gating, pre-roll, and event routing.
//! The whisper end-to-end tests at the bottom are `#[ignore]`d — they need
//! the local STT model (`scripts/fetch-models.ps1`):
//! `cargo test -p od-pipeline -- --ignored`

use std::time::Duration;

use od_core_types::{PipelineCtx, Segment, SegmentKind, SttEvent};
use od_pipeline::{Transcriber, TranscriberConfig};
use od_stt::{SttEngine, SttError};

const SAMPLE_RATE: usize = 16_000;

fn load_fixture(name: &str) -> Vec<f32> {
    let path = format!(
        "{}/../../testdata/speech/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut reader = hound::WavReader::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {path}: {e} (run scripts/gen-fixtures.ps1)"));
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / f32::from(i16::MAX))
        .collect()
}

/// Records every call; emits one partial per feed and a canned final.
#[derive(Default)]
struct MockEngine {
    begun: usize,
    ended: usize,
    feeds: usize,
    samples_fed: usize,
}

impl SttEngine for MockEngine {
    fn begin_utterance(&mut self, _ctx: &PipelineCtx) -> Result<(), SttError> {
        self.begun += 1;
        Ok(())
    }

    fn feed(&mut self, samples: &[f32]) -> Result<Vec<SttEvent>, SttError> {
        self.feeds += 1;
        self.samples_fed += samples.len();
        Ok(vec![SttEvent::Partial {
            text: format!("partial {}", self.feeds),
            stable_len: 0,
        }])
    }

    fn end_utterance(&mut self) -> Result<Vec<SttEvent>, SttError> {
        self.ended += 1;
        Ok(vec![SttEvent::Final {
            text: format!("Utterance {}.", self.ended),
            audio_len: Duration::from_secs_f64(self.samples_fed as f64 / SAMPLE_RATE as f64),
        }])
    }
}

/// Feeds a fixture in 100 ms chunks; returns segments (and the engine for
/// inspection via the returned transcriber).
fn run(samples: &[f32]) -> (Vec<Segment>, Transcriber<MockEngine>) {
    let mut t = Transcriber::new(
        &TranscriberConfig::default(),
        MockEngine::default(),
        PipelineCtx::default(),
    )
    .expect("init");
    let mut out = Vec::new();
    for chunk in samples.chunks(SAMPLE_RATE / 10) {
        t.feed(chunk, &mut out).expect("feed");
    }
    t.finish(&mut out).expect("finish");
    (out, t)
}

#[test]
fn silence_reaches_no_engine() {
    let silence = vec![0.0f32; SAMPLE_RATE * 2];
    let (out, _t) = run(&silence);
    assert!(out.is_empty(), "got {out:?}");
}

#[test]
fn speech_produces_partials_then_final() {
    // quick_fox is one continuous sentence: exactly one utterance expected.
    // (hello_world has a natural TTS pause between sentences that correctly
    // splits it at the default 300 ms hangover.)
    let mut samples = load_fixture("quick_fox.wav");
    samples.extend(std::iter::repeat_n(0.0f32, SAMPLE_RATE / 2));
    let (out, t) = run(&samples);

    let partials = out
        .iter()
        .filter(|s| s.kind == SegmentKind::Partial)
        .count();
    let finals: Vec<_> = out
        .iter()
        .filter(|s| s.kind == SegmentKind::Final)
        .collect();
    assert!(partials > 0, "no partials: {out:?}");
    assert_eq!(finals.len(), 1, "expected one utterance: {out:?}");

    // Partials precede the final.
    let last_partial = out.iter().rposition(|s| s.kind == SegmentKind::Partial);
    let final_pos = out.iter().position(|s| s.kind == SegmentKind::Final);
    assert!(last_partial.unwrap() < final_pos.unwrap());

    // Finalization latency was measured.
    assert!(t.last_finalize_latency().is_some());
}

#[test]
fn pause_yields_two_utterances() {
    let mut samples = load_fixture("with_pause.wav");
    samples.extend(std::iter::repeat_n(0.0f32, SAMPLE_RATE / 2));
    let (out, _t) = run(&samples);

    let finals = out.iter().filter(|s| s.kind == SegmentKind::Final).count();
    assert_eq!(finals, 2, "900 ms pause must split utterances: {out:?}");
}

#[test]
fn engine_hears_speech_duration_not_stream_duration() {
    let mut samples = vec![0.0f32; SAMPLE_RATE * 2]; // 2 s leading silence
    let speech = load_fixture("hello_world.wav");
    let speech_len = speech.len();
    samples.extend(speech);
    samples.extend(std::iter::repeat_n(0.0f32, SAMPLE_RATE)); // 1 s tail

    let (_out, t) = run(&samples);
    let heard = t.engine().samples_fed;

    // Engine must hear roughly the speech (plus pre-roll and hangover),
    // never the whole stream with its 3 s of silence.
    let slack = SAMPLE_RATE; // 1 s combined pre-roll/hangover/boundary slack
    assert!(
        heard <= speech_len + slack,
        "engine heard {heard} samples for {speech_len} of speech"
    );
    assert!(heard > speech_len / 2, "engine heard too little: {heard}");
}
