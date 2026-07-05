//! Fixture-driven tests for the VAD gate: real TTS speech and synthetic
//! silence/noise through the full model.

use std::time::Duration;

use od_vad::{SAMPLE_RATE, VadConfig, VadEvent, VadGate};

/// Loads a 16 kHz mono s16 fixture into f32 samples.
fn load_fixture(name: &str) -> Vec<f32> {
    let path = format!(
        "{}/../../testdata/speech/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut reader = hound::WavReader::open(&path)
        .unwrap_or_else(|e| panic!("open fixture {path}: {e} (run scripts/gen-fixtures.ps1)"));
    assert_eq!(reader.spec().sample_rate, SAMPLE_RATE);
    assert_eq!(reader.spec().channels, 1);
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / f32::from(i16::MAX))
        .collect()
}

/// Feeds samples in awkward chunk sizes and returns all events.
fn run_gate(samples: &[f32], config: VadConfig) -> Vec<VadEvent> {
    let mut gate = VadGate::new(config).expect("init gate");
    let mut events = Vec::new();
    for chunk in samples.chunks(1234) {
        gate.feed(chunk, &mut events).expect("feed");
    }
    events
}

#[test]
fn silence_produces_no_events() {
    let silence = vec![0.0f32; SAMPLE_RATE as usize * 2];
    let events = run_gate(&silence, VadConfig::default());
    assert!(events.is_empty(), "got {events:?}");
}

#[test]
fn low_level_noise_produces_no_events() {
    // Deterministic pseudo-noise at -40 dBFS-ish: below any speech pattern.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let noise: Vec<f32> = (0..SAMPLE_RATE as usize * 2)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed as f32 / u64::MAX as f32 - 0.5) * 0.02
        })
        .collect();
    let events = run_gate(&noise, VadConfig::default());
    assert!(events.is_empty(), "got {events:?}");
}

#[test]
fn speech_fixture_produces_one_bounded_segment() {
    let mut samples = load_fixture("hello_world.wav");
    // Ensure enough trailing silence for the hangover to confirm the end.
    samples.extend(std::iter::repeat_n(0.0f32, SAMPLE_RATE as usize / 2));

    let events = run_gate(&samples, VadConfig::default());
    let starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, VadEvent::SpeechStart { .. }))
        .collect();
    let ends: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, VadEvent::SpeechEnd { .. }))
        .collect();

    assert!(!starts.is_empty(), "no speech detected: {events:?}");
    assert_eq!(starts.len(), ends.len(), "unbalanced events: {events:?}");

    // First event must be a start, and starts/ends must alternate.
    let mut expect_start = true;
    for e in &events {
        match e {
            VadEvent::SpeechStart { .. } => {
                assert!(expect_start, "two starts in a row: {events:?}");
                expect_start = false;
            }
            VadEvent::SpeechEnd { .. } => {
                assert!(!expect_start, "end before start: {events:?}");
                expect_start = true;
            }
        }
    }

    // Boundaries are sane: speech starts within the first second (TTS begins
    // promptly) and the last end is before the padded stream's end.
    assert!(
        events[0].at() < Duration::from_secs(1),
        "late start: {events:?}"
    );
    let total = Duration::from_secs_f64(samples.len() as f64 / f64::from(SAMPLE_RATE));
    assert!(events.last().unwrap().at() < total);
}

#[test]
fn pause_fixture_produces_two_segments() {
    let mut samples = load_fixture("with_pause.wav");
    samples.extend(std::iter::repeat_n(0.0f32, SAMPLE_RATE as usize / 2));

    // 900 ms scripted pause, 300 ms hangover: must split into two segments.
    let events = run_gate(&samples, VadConfig::default());
    let starts = events
        .iter()
        .filter(|e| matches!(e, VadEvent::SpeechStart { .. }))
        .count();
    assert!(
        starts >= 2,
        "expected the 900 ms pause to split speech, got {events:?}"
    );

    // With a hangover longer than the pause, it must NOT split. The real
    // silence gap is ~1.8 s (900 ms scripted break plus the TTS voice's
    // natural sentence-boundary silence on both sides), so bridge with 2.5 s.
    let bridge = VadConfig {
        hangover: Duration::from_millis(2500),
        ..VadConfig::default()
    };
    let events = run_gate(&samples, bridge);
    let starts = events
        .iter()
        .filter(|e| matches!(e, VadEvent::SpeechStart { .. }))
        .count();
    assert_eq!(
        starts, 1,
        "long hangover should bridge the pause: {events:?}"
    );
}

#[test]
fn reset_restarts_positions() {
    let samples = load_fixture("hello_world.wav");
    let mut gate = VadGate::new(VadConfig::default()).expect("init");
    let mut events = Vec::new();
    gate.feed(&samples, &mut events).expect("feed");
    assert!(!events.is_empty());

    gate.reset();
    let mut events2 = Vec::new();
    gate.feed(&samples, &mut events2).expect("feed after reset");
    // Same stream after reset yields the same first boundary.
    assert_eq!(events.first(), events2.first());
}
