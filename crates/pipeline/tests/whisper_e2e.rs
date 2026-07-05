//! End-to-end fixture transcription through the real pipeline:
//! WAV → VAD gate → whisper.cpp → local agreement → segmenter.
//!
//! All `#[ignore]`d: they need the local STT model
//! (`scripts/fetch-models.ps1`). Run with:
//! `cargo test -p od-pipeline --release -- --ignored --nocapture`
//! (release strongly recommended — debug-profile whisper is ~10x slower).

use std::time::Duration;

use od_core_types::{PipelineCtx, Segment, SegmentKind};
use od_pipeline::{Transcriber, TranscriberConfig};
use od_stt::{WhisperConfig, WhisperEngine};

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

/// Lowercase, alphanumeric+spaces only — robust transcript comparison.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.to_lowercase().chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    }
    out.trim().to_owned()
}

fn make_transcriber() -> Transcriber<WhisperEngine> {
    let engine = WhisperEngine::new(WhisperConfig::default())
        .expect("model missing - run scripts/fetch-models.ps1");
    Transcriber::new(
        &TranscriberConfig::default(),
        engine,
        PipelineCtx::default(),
    )
    .expect("init transcriber")
}

/// Streams a fixture in 100 ms chunks, like the real capture loop.
fn transcribe(samples: &[f32]) -> (Vec<Segment>, Option<Duration>) {
    let mut t = make_transcriber();
    let mut out = Vec::new();
    for chunk in samples.chunks(SAMPLE_RATE / 10) {
        t.feed(chunk, &mut out).expect("feed");
    }
    t.finish(&mut out).expect("finish");
    let latency = t.last_finalize_latency();
    (out, latency)
}

fn finals_text(segments: &[Segment]) -> String {
    segments
        .iter()
        .filter(|s| s.kind == SegmentKind::Final)
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
#[ignore = "requires local STT model (scripts/fetch-models.ps1)"]
fn transcribes_hello_world_fixture() {
    let mut samples = load_fixture("hello_world.wav");
    samples.extend(std::iter::repeat_n(0.0f32, SAMPLE_RATE / 2));
    let (segments, _) = transcribe(&samples);

    let text = normalize(&finals_text(&segments));
    println!("transcript: {text:?}");
    assert!(text.contains("hello world"), "got: {text:?}");
    assert!(
        text.contains("test of the dictation system"),
        "got: {text:?}"
    );

    // "Hello world." / "This is a test..." must split into two sentences.
    let finals = segments
        .iter()
        .filter(|s| s.kind == SegmentKind::Final)
        .count();
    assert!(finals >= 2, "expected sentence split, got {segments:?}");
}

#[test]
#[ignore = "requires local STT model (scripts/fetch-models.ps1)"]
fn transcribes_quick_fox_with_streaming_partials() {
    let mut samples = load_fixture("quick_fox.wav");
    samples.extend(std::iter::repeat_n(0.0f32, SAMPLE_RATE / 2));
    let (segments, _) = transcribe(&samples);

    let text = normalize(&finals_text(&segments));
    println!("transcript: {text:?}");
    assert!(
        text.contains("quick brown fox jumps over the lazy dog"),
        "got: {text:?}"
    );

    // Streaming behavior: at least one partial before the final for a ~7 s
    // utterance at a 700 ms decode cadence.
    let partials = segments
        .iter()
        .filter(|s| s.kind == SegmentKind::Partial)
        .count();
    assert!(partials >= 1, "no partials emitted: {segments:?}");
}

#[test]
#[ignore = "requires local STT model (scripts/fetch-models.ps1)"]
fn pause_fixture_yields_two_utterances_and_reports_latency() {
    let mut samples = load_fixture("with_pause.wav");
    samples.extend(std::iter::repeat_n(0.0f32, SAMPLE_RATE / 2));
    let (segments, latency) = transcribe(&samples);

    let text = normalize(&finals_text(&segments));
    println!("transcript: {text:?}");
    assert!(text.contains("first part"), "got: {text:?}");
    assert!(text.contains("second part"), "got: {text:?}");

    // Finalization latency: measured, printed, and bounded loosely. The
    // MVP target is 300 ms p50 (docs/01, success criteria); the current
    // full-re-decode finalization won't hit that yet — the number below is
    // a regression guard, and the optimization (decode only the un-decoded
    // tail at SpeechEnd) is tracked for M9 perf work.
    let latency = latency.expect("latency recorded");
    println!("finalize latency: {} ms", latency.as_millis());
    assert!(
        latency < Duration::from_secs(3),
        "finalization pathologically slow: {latency:?}"
    );
}
