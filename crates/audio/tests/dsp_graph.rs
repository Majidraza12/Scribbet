//! Integration tests for the capture DSP graph, minus the physical device:
//! synthetic device frames → downmix → resample → ring buffer → consumer,
//! exactly the path the cpal callback drives, plus a WAV round-trip to prove
//! the output is well-formed audio.
//!
//! The live-microphone test at the bottom is `#[ignore]`d: it needs real
//! hardware and a mic permission, so it runs on demand
//! (`cargo test -p od-audio -- --ignored`), not in CI.

use od_audio::{LinearResampler, TARGET_SAMPLE_RATE, audio_ring, downmix_interleaved, rms};

/// Simulated device: 48 kHz stereo i16, 440 Hz tone, delivered in
/// callback-sized bursts.
fn synthetic_device_frames(secs: f32) -> Vec<i16> {
    const RATE: u32 = 48_000;
    let frames = (RATE as f32 * secs) as usize;
    let mut interleaved = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f32 / RATE as f32;
        let sample =
            (0.6 * (2.0 * std::f32::consts::PI * 440.0 * t).sin() * f32::from(i16::MAX)) as i16;
        interleaved.push(sample); // L
        interleaved.push(sample); // R
    }
    interleaved
}

/// Runs the callback path over callback-sized chunks, returning everything
/// the consumer drained.
fn run_graph(device_frames: &[i16], chunk_frames: usize) -> Vec<f32> {
    let mut resampler = LinearResampler::new(48_000, TARGET_SAMPLE_RATE);
    let (mut producer, mut consumer) = audio_ring(TARGET_SAMPLE_RATE as usize * 30);

    let mut mono = Vec::new();
    let mut resampled = Vec::new();
    let mut collected = Vec::new();
    let mut drain = vec![0.0f32; 4096];

    for chunk in device_frames.chunks(chunk_frames * 2) {
        // The three steps the real callback performs:
        mono.clear();
        downmix_interleaved(chunk, 2, &mut mono);
        resampled.clear();
        resampler.process(&mono, &mut resampled);
        let pushed = producer.push_slice(&resampled);
        assert_eq!(pushed, resampled.len(), "ring overran unexpectedly");

        // Consumer drains concurrently in production; interleaved here.
        loop {
            let n = consumer.pop_slice(&mut drain);
            if n == 0 {
                break;
            }
            collected.extend_from_slice(&drain[..n]);
        }
    }
    assert_eq!(consumer.overrun_count(), 0);
    collected
}

#[test]
fn graph_produces_expected_duration_and_signal() {
    let device_frames = synthetic_device_frames(1.0);
    let output = run_graph(&device_frames, 480); // 10 ms callbacks

    // 1 s of input must yield ~1 s at 16 kHz.
    let expected = TARGET_SAMPLE_RATE as isize;
    assert!(
        (output.len() as isize - expected).abs() <= 4,
        "got {} samples, expected ~{expected}",
        output.len()
    );

    // Signal integrity: no NaN/inf, sane level for a 0.6-amplitude sine
    // (RMS ≈ 0.6 / √2 ≈ 0.424), frequency preserved.
    assert!(output.iter().all(|s| s.is_finite()));
    let level = rms(&output);
    assert!((level - 0.424).abs() < 0.02, "rms {level}");

    let crossings = output
        .windows(2)
        .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
        .count();
    let freq = crossings as f32 * TARGET_SAMPLE_RATE as f32 / (2.0 * output.len() as f32);
    assert!((freq - 440.0).abs() < 5.0, "estimated {freq} Hz");
}

#[test]
fn graph_output_survives_wav_round_trip() {
    let device_frames = synthetic_device_frames(0.5);
    let output = run_graph(&device_frames, 441); // awkward chunk size

    let path = std::env::temp_dir().join("od_audio_dsp_graph_roundtrip.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("create wav");
    for &sample in &output {
        let quantized = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        writer.write_sample(quantized).expect("write sample");
    }
    writer.finalize().expect("finalize wav");

    let mut reader = hound::WavReader::open(&path).expect("open wav");
    assert_eq!(reader.spec().sample_rate, TARGET_SAMPLE_RATE);
    assert_eq!(reader.spec().channels, 1);
    let read_back: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
    assert_eq!(read_back.len(), output.len());

    // Spot-check quantization error stays within one LSB-ish of the source.
    for (&orig, &q) in output.iter().zip(&read_back) {
        let restored = f32::from(q) / f32::from(i16::MAX);
        assert!((orig - restored).abs() < 2.0 / f32::from(i16::MAX));
    }

    std::fs::remove_file(&path).ok();
}

/// Live hardware smoke test — run manually with:
/// `cargo test -p od-audio -- --ignored`
#[test]
#[ignore = "requires a physical microphone and OS mic permission"]
fn live_capture_smoke() {
    use od_audio::{CaptureConfig, CaptureSession};
    use std::time::{Duration, Instant};

    let (session, mut consumer) = match CaptureSession::start(&CaptureConfig::default()) {
        Ok(pair) => pair,
        Err(od_audio::AudioError::NoDevice) => {
            eprintln!("no input device; skipping");
            return;
        }
        Err(e) => panic!("capture start failed: {e}"),
    };

    let mut collected = Vec::new();
    let mut buf = vec![0.0f32; 1600];
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        let n = consumer.pop_slice(&mut buf);
        if n > 0 {
            collected.extend_from_slice(&buf[..n]);
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    session.stop();

    // ~500 ms of audio at 16 kHz, allowing generous startup slack.
    assert!(
        collected.len() > TARGET_SAMPLE_RATE as usize / 4,
        "captured only {} samples",
        collected.len()
    );
    assert!(collected.iter().all(|s| s.is_finite()));
}
