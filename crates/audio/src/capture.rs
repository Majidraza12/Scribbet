//! Capture sessions: an open input stream feeding the audio ring.
//!
//! One [`CaptureSession`] = one open device stream. The cpal callback runs on
//! a real-time thread and does exactly four things: convert/downmix into a
//! pre-allocated scratch buffer, update the level meter atomic, resample into
//! a second scratch buffer, push into the wait-free ring. No locks, no I/O,
//! and no allocation in steady state (scratch buffers are pre-sized; a device
//! delivering callbacks larger than one second would grow them once).
//!
//! `cpal::Stream` is not `Send`, so a session must be created, used, and
//! dropped on the same thread — by design the session controller's thread
//! (see docs/02-architecture.md, threading model).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::device::{self, DeviceSelector};
use crate::dsp::{self, LinearResampler, TARGET_SAMPLE_RATE};
use crate::error::AudioError;
use crate::meter::LevelMeter;
use crate::ring::{self, AudioConsumer, AudioProducer};

/// Configuration for opening a capture session.
#[derive(Clone, Debug)]
pub struct CaptureConfig {
    /// Which input device to open.
    pub device: DeviceSelector,
    /// How much 16 kHz audio the ring buffer holds before overrunning.
    pub buffer_len: Duration,
    /// Release coefficient for the level meter (see [`LevelMeter::new`]).
    pub meter_release: f32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            device: DeviceSelector::SystemDefault,
            buffer_len: Duration::from_secs(30),
            meter_release: 0.25,
        }
    }
}

/// An open microphone stream delivering 16 kHz mono f32 into an
/// [`AudioConsumer`].
///
/// The stream stops and the device is released when the session is dropped;
/// [`stop`](Self::stop) exists to make that explicit at call sites.
pub struct CaptureSession {
    stream: cpal::Stream,
    meter: Arc<LevelMeter>,
    disconnected: Arc<AtomicBool>,
    device_name: String,
    input_sample_rate: u32,
    input_channels: u16,
}

impl CaptureSession {
    /// Opens the selected device and starts capturing.
    ///
    /// Returns the session (keep it alive; dropping it stops capture) and the
    /// consumer end of the ring buffer for the downstream compute thread.
    pub fn start(config: &CaptureConfig) -> Result<(Self, AudioConsumer), AudioError> {
        let device = device::find_input_device(&config.device)?;
        let device_name = device
            .name()
            .unwrap_or_else(|_| String::from("<unnamed device>"));

        let supported = device
            .default_input_config()
            .map_err(|e| AudioError::DeviceConfig(e.to_string()))?;
        let sample_format = supported.sample_format();
        let stream_config: cpal::StreamConfig = supported.config();
        let input_sample_rate = stream_config.sample_rate.0;
        let input_channels = stream_config.channels;

        let capacity = (f64::from(TARGET_SAMPLE_RATE) * config.buffer_len.as_secs_f64())
            .max(f64::from(TARGET_SAMPLE_RATE)) as usize;
        let (producer, consumer) = ring::audio_ring(capacity);

        let meter = Arc::new(LevelMeter::new(config.meter_release));
        let disconnected = Arc::new(AtomicBool::new(false));

        let state = CallbackState::new(
            input_channels as usize,
            input_sample_rate,
            producer,
            Arc::clone(&meter),
        );

        let stream = build_stream(
            &device,
            &stream_config,
            sample_format,
            state,
            Arc::clone(&disconnected),
        )?;
        stream
            .play()
            .map_err(|e| AudioError::StreamStart(e.to_string()))?;

        tracing::info!(
            device = %device_name,
            rate = input_sample_rate,
            channels = input_channels,
            format = %sample_format,
            "capture session started"
        );

        Ok((
            Self {
                stream,
                meter,
                disconnected,
                device_name,
                input_sample_rate,
                input_channels,
            },
            consumer,
        ))
    }

    /// Smoothed input level for UI polling.
    pub fn meter(&self) -> Arc<LevelMeter> {
        Arc::clone(&self.meter)
    }

    /// True once the device has become unavailable (unplugged, default
    /// changed by the OS in exclusive setups). The owner should stop this
    /// session and start a new one, typically on [`DeviceSelector::SystemDefault`].
    pub fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Acquire)
    }

    /// Name of the device this session captures from.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The device's native sample rate (before resampling to 16 kHz).
    pub fn input_sample_rate(&self) -> u32 {
        self.input_sample_rate
    }

    /// The device's native channel count (before downmix to mono).
    pub fn input_channels(&self) -> u16 {
        self.input_channels
    }

    /// Stops capture and releases the device.
    pub fn stop(self) {
        // Pausing before drop makes the release deterministic rather than
        // relying on drop order inside cpal.
        if let Err(e) = self.stream.pause() {
            tracing::debug!("pausing stream on stop: {e}");
        }
        self.meter.reset();
        tracing::info!(device = %self.device_name, "capture session stopped");
    }
}

/// Everything the real-time callback owns.
struct CallbackState {
    channels: usize,
    resampler: LinearResampler,
    producer: AudioProducer,
    meter: Arc<LevelMeter>,
    /// Scratch for converted/downmixed mono at the device rate.
    mono: Vec<f32>,
    /// Scratch for resampled 16 kHz output.
    resampled: Vec<f32>,
    /// Overruns already reported to the log, to avoid per-callback spam.
    logged_overruns: u64,
}

impl CallbackState {
    fn new(
        channels: usize,
        input_rate: u32,
        producer: AudioProducer,
        meter: Arc<LevelMeter>,
    ) -> Self {
        Self {
            channels,
            resampler: LinearResampler::new(input_rate, TARGET_SAMPLE_RATE),
            producer,
            meter,
            // One second of headroom each; real callbacks are ~10-100 ms.
            mono: Vec::with_capacity(input_rate as usize),
            resampled: Vec::with_capacity(TARGET_SAMPLE_RATE as usize),
            logged_overruns: 0,
        }
    }

    fn on_data<T>(&mut self, data: &[T])
    where
        T: cpal::Sample,
        f32: FromSample<T>,
    {
        self.mono.clear();
        dsp::downmix_interleaved(data, self.channels, &mut self.mono);
        self.meter.update_block(&self.mono);

        self.resampled.clear();
        self.resampler.process(&self.mono, &mut self.resampled);
        self.producer.push_slice(&self.resampled);

        let overruns = self.producer.overrun_count();
        if overruns > self.logged_overruns {
            tracing::warn!(
                dropped = overruns - self.logged_overruns,
                total = overruns,
                "audio ring overrun: consumer is not keeping up"
            );
            self.logged_overruns = overruns;
        }
    }
}

/// Builds the input stream for whichever sample format the device reports.
fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: SampleFormat,
    state: CallbackState,
    disconnected: Arc<AtomicBool>,
) -> Result<cpal::Stream, AudioError> {
    match format {
        SampleFormat::F32 => build_typed::<f32>(device, config, state, disconnected),
        SampleFormat::I16 => build_typed::<i16>(device, config, state, disconnected),
        SampleFormat::U16 => build_typed::<u16>(device, config, state, disconnected),
        SampleFormat::I32 => build_typed::<i32>(device, config, state, disconnected),
        SampleFormat::U8 => build_typed::<u8>(device, config, state, disconnected),
        SampleFormat::I8 => build_typed::<i8>(device, config, state, disconnected),
        SampleFormat::F64 => build_typed::<f64>(device, config, state, disconnected),
        other => Err(AudioError::UnsupportedFormat(other.to_string())),
    }
}

fn build_typed<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut state: CallbackState,
    disconnected: Arc<AtomicBool>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data, _info| state.on_data(data),
            move |err| {
                tracing::error!("audio input stream error: {err}");
                if matches!(err, cpal::StreamError::DeviceNotAvailable) {
                    disconnected.store(true, Ordering::Release);
                }
            },
            None,
        )
        .map_err(|e| AudioError::StreamBuild(e.to_string()))
}
