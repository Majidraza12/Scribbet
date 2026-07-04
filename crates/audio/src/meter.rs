//! Input level metering for the UI, real-time safe.
//!
//! The capture callback cannot send on an allocating channel, so the meter is
//! a single atomic the callback stores into and the UI polls — the "RMS level
//! meter event stream" is a poller over this atomic at whatever frame rate
//! the overlay wants.

use std::sync::atomic::{AtomicU32, Ordering};

/// Smoothed input level, shared between the capture callback (writer) and UI
/// pollers (readers).
///
/// Attack is instantaneous (a loud block registers immediately); release is
/// exponential so the meter decays smoothly instead of flickering.
#[derive(Debug)]
pub struct LevelMeter {
    /// Current level as `f32` bits (atomics have no native f32 on stable).
    bits: AtomicU32,
    /// Release coefficient in `(0, 1]`: fraction of the gap toward a quieter
    /// RMS applied per block. `1.0` disables smoothing.
    release: f32,
}

impl LevelMeter {
    /// Creates a meter with the given release coefficient, clamped to
    /// `(0, 1]`. `0.25` is a good default at typical (~10 ms) callback rates.
    pub fn new(release: f32) -> Self {
        Self {
            bits: AtomicU32::new(0.0f32.to_bits()),
            release: release.clamp(f32::EPSILON, 1.0),
        }
    }

    /// Feeds one block of mono samples. Called from the capture callback.
    pub fn update_block(&self, samples: &[f32]) {
        let block_rms = crate::dsp::rms(samples);
        let previous = self.level();
        let next = if block_rms >= previous {
            block_rms
        } else {
            previous + (block_rms - previous) * self.release
        };
        self.bits.store(next.to_bits(), Ordering::Relaxed);
    }

    /// Current smoothed level in `[0, 1]`-ish RMS units (can exceed 1.0 for
    /// clipping input).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }

    /// Resets the meter to silence (used when a session stops).
    pub fn reset(&self) {
        self.bits.store(0.0f32.to_bits(), Ordering::Relaxed);
    }
}

impl Default for LevelMeter {
    fn default() -> Self {
        Self::new(0.25)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attack_is_instant() {
        let meter = LevelMeter::new(0.25);
        meter.update_block(&[0.5; 256]);
        assert!((meter.level() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn release_decays_gradually() {
        let meter = LevelMeter::new(0.25);
        meter.update_block(&[0.8; 256]);
        meter.update_block(&[0.0; 256]);
        let after_one = meter.level();
        assert!((after_one - 0.6).abs() < 1e-6, "got {after_one}");
        meter.update_block(&[0.0; 256]);
        assert!(meter.level() < after_one);
    }

    #[test]
    fn reset_returns_to_silence() {
        let meter = LevelMeter::default();
        meter.update_block(&[1.0; 64]);
        meter.reset();
        assert_eq!(meter.level(), 0.0);
    }
}
