//! Latency benchmark for the whisper backend.
//!
//! Replays the fixture WAVs through the real `SttEngine` lifecycle —
//! `begin_utterance` → chunked `feed`s → `end_utterance` — and reports the
//! distribution of finalize latency, which is the number a user actually
//! feels: the gap between releasing the hotkey and the text landing.
//!
//! Audio is fed in 100 ms chunks rather than one slab so the engine performs
//! the same interim partial decodes it would during real dictation. Timing a
//! single bulk decode would flatter the result.
//!
//! Usage:
//!   cargo run --release -p od-stt --example bench -- [--iterations N] [--model PATH]
//!
//! Build with `--features vulkan` for the GPU path; without it this measures
//! the CPU fallback.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use od_core_types::{PipelineCtx, SttEvent};
use od_stt::{SttEngine, WhisperConfig, WhisperEngine, default_model_path};

const SAMPLE_RATE: usize = 16_000;
/// Audio callback size in the real pipeline; matched here so the count of
/// interim decodes is representative.
const CHUNK_MS: usize = 100;

const FIXTURES: &[&str] = &["hello_world", "quick_fox", "two_sentences", "with_pause"];

struct Args {
    iterations: usize,
    model: PathBuf,
}

fn parse_args() -> Args {
    let mut iterations = 20;
    let mut model = default_model_path();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--iterations" | "-n" => {
                iterations = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--iterations needs a number");
            }
            "--model" | "-m" => {
                model = it.next().map(PathBuf::from).expect("--model needs a path");
            }
            other => panic!("unknown argument: {other}"),
        }
    }
    Args { iterations, model }
}

/// Loads a 16-bit mono 16 kHz fixture as f32 samples in [-1, 1].
fn load_wav(name: &str) -> Vec<f32> {
    let path = format!("testdata/speech/{name}.wav");
    let mut reader = hound::WavReader::open(&path)
        .unwrap_or_else(|e| panic!("open {path}: {e} (run from the repo root)"));
    let spec = reader.spec();
    assert_eq!(spec.channels, 1, "{path}: expected mono");
    assert_eq!(
        spec.sample_rate as usize, SAMPLE_RATE,
        "{path}: expected 16 kHz"
    );
    reader
        .samples::<i16>()
        .map(|s| s.expect("read sample") as f32 / i16::MAX as f32)
        .collect()
}

/// Percentile by nearest-rank over an already-sorted slice.
fn pct(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

struct Stats {
    finalize: Vec<Duration>,
    /// Total time spent inside `feed` — the interim partial decodes that run
    /// while the user is still talking.
    feed_total: Vec<Duration>,
    audio: Duration,
}

fn main() {
    let args = parse_args();

    println!("model:      {}", args.model.display());
    println!(
        "backend:    {}",
        if cfg!(feature = "vulkan") {
            "vulkan (GPU, CPU fallback at runtime if no device)"
        } else {
            "cpu"
        }
    );
    println!("iterations: {} per fixture\n", args.iterations);

    // Model load is a one-off cost paid at startup, not per utterance, so it
    // is timed separately — it feeds the cold-start number, not latency.
    let load_start = Instant::now();
    let mut engine = WhisperEngine::new(WhisperConfig {
        model_path: args.model.clone(),
        ..Default::default()
    })
    .expect("construct engine");
    let load = load_start.elapsed();
    println!("model load: {:.0} ms\n", ms(load));

    let ctx = PipelineCtx::default();
    let chunk = SAMPLE_RATE * CHUNK_MS / 1000;

    let mut all_finalize: Vec<Duration> = Vec::new();
    let mut rows: Vec<(String, Stats)> = Vec::new();

    for name in FIXTURES {
        let samples = load_wav(name);
        let audio = Duration::from_secs_f64(samples.len() as f64 / SAMPLE_RATE as f64);
        let mut stats = Stats {
            finalize: Vec::new(),
            feed_total: Vec::new(),
            audio,
        };

        // One warm-up pass so shader compilation and the first allocation
        // don't land in the measured set.
        run_once(&mut engine, &ctx, &samples, chunk);

        for _ in 0..args.iterations {
            let (feed_total, finalize, _text) = run_once(&mut engine, &ctx, &samples, chunk);
            stats.feed_total.push(feed_total);
            stats.finalize.push(finalize);
            all_finalize.push(finalize);
        }

        stats.finalize.sort();
        stats.feed_total.sort();
        rows.push((name.to_string(), stats));
    }

    // Show what the model actually transcribed, so a fast-but-wrong result
    // can't pass as a good one.
    println!("transcripts");
    for name in FIXTURES {
        let samples = load_wav(name);
        let (_, _, text) = run_once(&mut engine, &ctx, &samples, chunk);
        println!("  {name:<14} {}", text.trim());
    }

    println!("\nfinalize latency — hotkey release to committed text");
    println!(
        "  {:<14} {:>7} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "fixture", "audio", "p50", "p90", "p95", "max", "RTF"
    );
    for (name, s) in &rows {
        let p50 = pct(&s.finalize, 50.0);
        println!(
            "  {:<14} {:>6.1}s {:>7.0}ms {:>7.0}ms {:>7.0}ms {:>7.0}ms {:>8.3}",
            name,
            s.audio.as_secs_f64(),
            ms(p50),
            ms(pct(&s.finalize, 90.0)),
            ms(pct(&s.finalize, 95.0)),
            ms(pct(&s.finalize, 100.0)),
            p50.as_secs_f64() / s.audio.as_secs_f64(),
        );
    }

    println!("\ninterim decodes — GPU/CPU work while the user is still talking");
    println!(
        "  {:<14} {:>10} {:>10}",
        "fixture", "total p50", "per audio s"
    );
    for (name, s) in &rows {
        let p50 = pct(&s.feed_total, 50.0);
        println!(
            "  {:<14} {:>9.0}ms {:>9.0}ms",
            name,
            ms(p50),
            ms(p50) / s.audio.as_secs_f64(),
        );
    }

    all_finalize.sort();
    println!(
        "\nall fixtures pooled (n={}): p50 {:.0} ms · p90 {:.0} ms · p95 {:.0} ms · p99 {:.0} ms · max {:.0} ms",
        all_finalize.len(),
        ms(pct(&all_finalize, 50.0)),
        ms(pct(&all_finalize, 90.0)),
        ms(pct(&all_finalize, 95.0)),
        ms(pct(&all_finalize, 99.0)),
        ms(pct(&all_finalize, 100.0)),
    );
}

/// One full utterance. Returns (time inside `feed`, time inside
/// `end_utterance`, final text).
fn run_once(
    engine: &mut WhisperEngine,
    ctx: &PipelineCtx,
    samples: &[f32],
    chunk: usize,
) -> (Duration, Duration, String) {
    engine.begin_utterance(ctx).expect("begin");

    let mut feed_total = Duration::ZERO;
    for part in samples.chunks(chunk) {
        let t = Instant::now();
        engine.feed(part).expect("feed");
        feed_total += t.elapsed();
    }

    let t = Instant::now();
    let events = engine.end_utterance().expect("end");
    let finalize = t.elapsed();

    let text = events
        .into_iter()
        .find_map(|e| match e {
            SttEvent::Final { text, .. } => Some(text),
            _ => None,
        })
        .unwrap_or_default();

    (feed_total, finalize, text)
}
