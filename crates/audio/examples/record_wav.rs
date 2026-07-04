//! Manual verification tool for M1: records a few seconds from the default
//! (or named) microphone to a 16 kHz mono WAV and prints live levels.
//!
//! ```text
//! cargo run -p od-audio --example record_wav [seconds] [device name]
//! ```
//!
//! Listen to the output file to verify capture, downmix, and resampling by
//! ear — the ROADMAP M1 "WAV dump" check.

use std::time::{Duration, Instant};

use od_audio::{
    CaptureConfig, CaptureSession, DeviceSelector, TARGET_SAMPLE_RATE, list_input_devices,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(3);
    let device = match args.next() {
        Some(name) => DeviceSelector::ByName(name),
        None => DeviceSelector::SystemDefault,
    };

    println!("Input devices:");
    for info in list_input_devices()? {
        let marker = if info.is_default { "  (default)" } else { "" };
        println!("  - {}{marker}", info.name);
    }

    let config = CaptureConfig {
        device,
        ..CaptureConfig::default()
    };
    let (session, mut consumer) = CaptureSession::start(&config)?;
    let meter = session.meter();
    println!(
        "\nRecording {seconds}s from {:?} ({} Hz, {} ch native)...",
        session.device_name(),
        session.input_sample_rate(),
        session.input_channels(),
    );

    let mut samples = Vec::with_capacity(TARGET_SAMPLE_RATE as usize * seconds as usize);
    let mut buf = vec![0.0f32; 3200];
    let started = Instant::now();
    let mut last_print = Instant::now();

    while started.elapsed() < Duration::from_secs(seconds) {
        let n = consumer.pop_slice(&mut buf);
        if n > 0 {
            samples.extend_from_slice(&buf[..n]);
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
        if last_print.elapsed() >= Duration::from_millis(250) {
            let bar_len = (meter.level() * 50.0).min(50.0) as usize;
            println!("level |{:<50}|", "#".repeat(bar_len));
            last_print = Instant::now();
        }
        if session.is_disconnected() {
            eprintln!("device disconnected; stopping early");
            break;
        }
    }
    session.stop();

    let path = std::env::temp_dir().join("od_audio_recording.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec)?;
    for &s in &samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16)?;
    }
    writer.finalize()?;

    println!(
        "\nWrote {} samples ({:.2}s) to {}",
        samples.len(),
        samples.len() as f32 / TARGET_SAMPLE_RATE as f32,
        path.display()
    );
    println!("Ring overruns: {}", consumer.overrun_count());
    Ok(())
}
