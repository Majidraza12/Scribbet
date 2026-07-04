//! Sample-format conversion, channel downmix, and sample-rate conversion.
//!
//! Everything in this module is pure and allocation-free apart from appending
//! to caller-provided `Vec`s, which the capture callback pre-sizes so the
//! real-time path does not allocate in steady state.
//!
//! The resampler is linear-interpolating. For 16 kHz speech feeding a
//! Whisper-class STT model this is adequate (the model is robust to mild
//! aliasing above ~7 kHz); if fixture-driven accuracy tests in M2 show
//! degradation we can swap in a windowed-sinc kernel behind the same API.

use cpal::{FromSample, Sample};

/// The sample rate every consumer downstream of capture receives (Hz).
///
/// 16 kHz mono f32 is the native input format of Whisper-class STT models and
/// of Silero VAD, so conversion happens exactly once, at the capture edge.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Downmixes interleaved multi-channel samples to mono f32 by averaging
/// channels, converting from the device's native sample type on the fly.
///
/// Output is appended to `out`. `input.len()` must be a multiple of
/// `channels`; a trailing partial frame (which a conforming backend never
/// produces) is dropped.
///
/// # Panics
///
/// Panics if `channels == 0`.
pub fn downmix_interleaved<T>(input: &[T], channels: usize, out: &mut Vec<f32>)
where
    T: Sample,
    f32: FromSample<T>,
{
    assert!(channels > 0, "channel count must be non-zero");
    if channels == 1 {
        out.extend(input.iter().map(|s| f32::from_sample(*s)));
        return;
    }
    let scale = 1.0 / channels as f32;
    for frame in input.chunks_exact(channels) {
        let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
        out.push(sum * scale);
    }
}

/// Streaming linear-interpolation resampler for mono f32 audio.
///
/// Feed arbitrary-sized chunks with [`process`](Self::process); output is
/// identical to processing the concatenated stream in one call (the
/// fractional read position and the last input sample carry across calls),
/// so callback-sized chunking never affects the result.
#[derive(Debug)]
pub struct LinearResampler {
    in_rate: u64,
    out_rate: u64,
    /// Read position within the virtual buffer `[carry, chunk...]`, as a
    /// rational number of input samples: `pos_num / out_rate`. Integer
    /// arithmetic makes the resampler *exactly* chunk-invariant — a float
    /// phase accumulator drifts when re-anchored at chunk seams, so output
    /// would depend on callback size.
    pos_num: u64,
    /// Last sample of the previous chunk, for interpolation across the seam.
    carry: f32,
    /// Whether `carry`/`pos_num` have been seeded by a first chunk.
    primed: bool,
    /// Fast path: input already at the output rate.
    passthrough: bool,
}

impl LinearResampler {
    /// Creates a resampler converting `in_rate` Hz to `out_rate` Hz.
    ///
    /// # Panics
    ///
    /// Panics if either rate is zero.
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        assert!(in_rate > 0 && out_rate > 0, "sample rates must be non-zero");
        Self {
            in_rate: u64::from(in_rate),
            out_rate: u64::from(out_rate),
            pos_num: 0,
            carry: 0.0,
            primed: false,
            passthrough: in_rate == out_rate,
        }
    }

    /// Resamples `input`, appending the produced samples to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        if self.passthrough {
            out.extend_from_slice(input);
            return;
        }
        if !self.primed {
            // Seed the carry with the first sample and start reading at the
            // first real sample (index 1 of the virtual buffer), so the first
            // output equals `input[0]` exactly.
            self.carry = input[0];
            self.pos_num = self.out_rate;
            self.primed = true;
        }

        // Virtual buffer v of length `input.len() + 1`:
        //   v[0] = carry (last sample of the previous chunk)
        //   v[i] = input[i - 1]
        let v_len = input.len() + 1;
        let v = |i: usize| -> f32 { if i == 0 { self.carry } else { input[i - 1] } };

        let end_num = (v_len as u64 - 1) * self.out_rate;
        while self.pos_num < end_num {
            let i = (self.pos_num / self.out_rate) as usize;
            let frac = (self.pos_num % self.out_rate) as f32 / self.out_rate as f32;
            let a = v(i);
            let b = v(i + 1);
            out.push(a + (b - a) * frac);
            self.pos_num += self.in_rate;
        }

        // Re-anchor the position so v[0] of the *next* call is this chunk's
        // final sample. Exact: no drift regardless of chunk sizes.
        self.pos_num -= end_num;
        self.carry = input[input.len() - 1];
    }
}

/// Root-mean-square amplitude of a block of samples; `0.0` for an empty block.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generates `secs` seconds of a sine wave at `freq` Hz / `rate` Hz.
    fn sine(freq: f32, rate: u32, secs: f32, amplitude: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| amplitude * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    /// Estimates a sine's frequency from its zero-crossing count.
    fn zero_crossing_freq(samples: &[f32], rate: u32) -> f32 {
        let crossings = samples
            .windows(2)
            .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
            .count();
        crossings as f32 * rate as f32 / (2.0 * samples.len() as f32)
    }

    #[test]
    fn downmix_stereo_averages_channels() {
        let interleaved = [1.0f32, 0.0, 0.5, 0.5, -1.0, 1.0];
        let mut out = Vec::new();
        downmix_interleaved(&interleaved, 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5, 0.0]);
    }

    #[test]
    fn downmix_mono_is_identity() {
        let input = [0.25f32, -0.75, 1.0];
        let mut out = Vec::new();
        downmix_interleaved(&input, 1, &mut out);
        assert_eq!(out, input.to_vec());
    }

    #[test]
    fn downmix_converts_i16() {
        let input = [i16::MAX, 0, i16::MIN];
        let mut out = Vec::new();
        downmix_interleaved(&input, 1, &mut out);
        assert!((out[0] - 1.0).abs() < 1e-3);
        assert!(out[1].abs() < 1e-6);
        assert!((out[2] + 1.0).abs() < 1e-3);
    }

    #[test]
    fn resample_passthrough_is_exact() {
        let input = sine(440.0, TARGET_SAMPLE_RATE, 0.1, 0.8);
        let mut rs = LinearResampler::new(TARGET_SAMPLE_RATE, TARGET_SAMPLE_RATE);
        let mut out = Vec::new();
        rs.process(&input, &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn resample_48k_to_16k_length_ratio() {
        let input = sine(440.0, 48_000, 1.0, 0.8);
        let mut rs = LinearResampler::new(48_000, TARGET_SAMPLE_RATE);
        let mut out = Vec::new();
        rs.process(&input, &mut out);
        let expected = TARGET_SAMPLE_RATE as isize;
        assert!(
            (out.len() as isize - expected).abs() <= 2,
            "got {} samples, expected ~{expected}",
            out.len()
        );
    }

    #[test]
    fn resample_44_1k_to_16k_length_ratio() {
        let input = sine(440.0, 44_100, 1.0, 0.8);
        let mut rs = LinearResampler::new(44_100, TARGET_SAMPLE_RATE);
        let mut out = Vec::new();
        rs.process(&input, &mut out);
        let expected = TARGET_SAMPLE_RATE as isize;
        assert!(
            (out.len() as isize - expected).abs() <= 2,
            "got {} samples, expected ~{expected}",
            out.len()
        );
    }

    #[test]
    fn resample_preserves_dc() {
        let input = vec![0.5f32; 4800];
        let mut rs = LinearResampler::new(48_000, TARGET_SAMPLE_RATE);
        let mut out = Vec::new();
        rs.process(&input, &mut out);
        assert!(!out.is_empty());
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn resample_preserves_sine_frequency() {
        let input = sine(440.0, 48_000, 1.0, 0.8);
        let mut rs = LinearResampler::new(48_000, TARGET_SAMPLE_RATE);
        let mut out = Vec::new();
        rs.process(&input, &mut out);
        let freq = zero_crossing_freq(&out, TARGET_SAMPLE_RATE);
        assert!(
            (freq - 440.0).abs() < 5.0,
            "estimated {freq} Hz, expected ~440 Hz"
        );
    }

    #[test]
    fn resample_chunked_equals_whole() {
        let input = sine(440.0, 44_100, 0.25, 0.8);

        let mut whole = Vec::new();
        LinearResampler::new(44_100, TARGET_SAMPLE_RATE).process(&input, &mut whole);

        let mut chunked = Vec::new();
        let mut rs = LinearResampler::new(44_100, TARGET_SAMPLE_RATE);
        // Awkward chunk size on purpose: exercises the carry/phase seam.
        for chunk in input.chunks(487) {
            rs.process(chunk, &mut chunked);
        }

        assert_eq!(whole, chunked);
    }

    #[test]
    fn rms_of_full_scale_sine_is_inv_sqrt2() {
        let input = sine(440.0, 48_000, 1.0, 1.0);
        let value = rms(&input);
        assert!(
            (value - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3,
            "got {value}"
        );
    }

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0.0; 128]), 0.0);
    }
}
