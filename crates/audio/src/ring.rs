//! Lock-free SPSC ring buffer for 16 kHz mono f32 samples.
//!
//! Thin wrapper over [`rtrb`] adding slice-oriented push/pop and a shared
//! overrun counter. Policy on overrun: **drop the newest samples** and count
//! them — the capture callback must never block, and with a ~30 s buffer an
//! overrun means the consumer stalled pathologically; the counter surfaces
//! that in metrics instead of hiding it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Creates a ring buffer holding `capacity` samples, returning the two ends.
///
/// The producer end goes to the capture callback (real-time thread); the
/// consumer end goes to the VAD/STT compute thread.
pub fn audio_ring(capacity: usize) -> (AudioProducer, AudioConsumer) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity);
    let overruns = Arc::new(AtomicU64::new(0));
    (
        AudioProducer {
            inner: producer,
            overruns: Arc::clone(&overruns),
        },
        AudioConsumer {
            inner: consumer,
            overruns,
        },
    )
}

/// Writing end of the audio ring. Wait-free; safe to use from the real-time
/// capture callback.
pub struct AudioProducer {
    inner: rtrb::Producer<f32>,
    overruns: Arc<AtomicU64>,
}

impl AudioProducer {
    /// Pushes as many samples from `samples` as fit, returning how many were
    /// written. Samples that don't fit are dropped and added to the overrun
    /// counter.
    pub fn push_slice(&mut self, samples: &[f32]) -> usize {
        let writable = samples.len().min(self.inner.slots());
        if writable > 0 {
            // `slots()` is the number of slots free from this end's view, so
            // the chunk request cannot fail.
            let chunk = self
                .inner
                .write_chunk_uninit(writable)
                .expect("slots() guarantees capacity");
            chunk.fill_from_iter(samples[..writable].iter().copied());
        }
        let dropped = samples.len() - writable;
        if dropped > 0 {
            self.overruns.fetch_add(dropped as u64, Ordering::Relaxed);
        }
        writable
    }

    /// Total samples dropped because the buffer was full.
    pub fn overrun_count(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }
}

/// Reading end of the audio ring.
pub struct AudioConsumer {
    inner: rtrb::Consumer<f32>,
    overruns: Arc<AtomicU64>,
}

impl AudioConsumer {
    /// Pops up to `out.len()` samples into `out`, returning how many were
    /// written. Returns `0` immediately when the buffer is empty.
    pub fn pop_slice(&mut self, out: &mut [f32]) -> usize {
        let readable = out.len().min(self.inner.slots());
        if readable == 0 {
            return 0;
        }
        let chunk = self
            .inner
            .read_chunk(readable)
            .expect("slots() guarantees availability");
        let (first, second) = chunk.as_slices();
        out[..first.len()].copy_from_slice(first);
        out[first.len()..first.len() + second.len()].copy_from_slice(second);
        chunk.commit_all();
        readable
    }

    /// Number of samples currently buffered.
    pub fn len(&self) -> usize {
        self.inner.slots()
    }

    /// Whether the buffer currently holds no samples.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the producer end has been dropped (capture session ended).
    pub fn is_abandoned(&self) -> bool {
        self.inner.is_abandoned()
    }

    /// Total samples dropped by the producer because the buffer was full.
    pub fn overrun_count(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_samples() {
        let (mut tx, mut rx) = audio_ring(64);
        let input: Vec<f32> = (0..40).map(|i| i as f32 / 40.0).collect();
        assert_eq!(tx.push_slice(&input), 40);
        assert_eq!(rx.len(), 40);

        let mut out = vec![0.0f32; 40];
        assert_eq!(rx.pop_slice(&mut out), 40);
        assert_eq!(out, input);
        assert!(rx.is_empty());
    }

    #[test]
    fn overrun_drops_newest_and_counts() {
        let (mut tx, mut rx) = audio_ring(8);
        let input: Vec<f32> = (0..12).map(|i| i as f32).collect();
        assert_eq!(tx.push_slice(&input), 8);
        assert_eq!(tx.overrun_count(), 4);
        assert_eq!(rx.overrun_count(), 4);

        // The oldest samples (0..8) survived; the newest were dropped.
        let mut out = vec![0.0f32; 8];
        assert_eq!(rx.pop_slice(&mut out), 8);
        assert_eq!(out, (0..8).map(|i| i as f32).collect::<Vec<_>>());
    }

    #[test]
    fn pop_from_empty_returns_zero() {
        let (_tx, mut rx) = audio_ring(8);
        let mut out = vec![0.0f32; 4];
        assert_eq!(rx.pop_slice(&mut out), 0);
    }

    #[test]
    fn wraparound_roundtrip() {
        let (mut tx, mut rx) = audio_ring(16);
        let mut scratch = [0.0f32; 16];
        // Push/pop repeatedly so the internal indices wrap several times.
        for round in 0..10 {
            let input: Vec<f32> = (0..11).map(|i| (round * 11 + i) as f32).collect();
            assert_eq!(tx.push_slice(&input), 11);
            assert_eq!(rx.pop_slice(&mut scratch[..11]), 11);
            assert_eq!(&scratch[..11], input.as_slice());
        }
    }

    #[test]
    fn abandonment_is_visible_to_consumer() {
        let (tx, rx) = audio_ring(8);
        assert!(!rx.is_abandoned());
        drop(tx);
        assert!(rx.is_abandoned());
    }
}
