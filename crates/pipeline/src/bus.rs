//! App-wide event bus (docs/02-architecture.md "Event bus").
//!
//! Bounded, drop-on-full broadcast: publishing is fire-and-forget and never
//! blocks the publisher — a slow subscriber loses events (counted and
//! warned) rather than stalling the pipeline. Publishers are per-utterance /
//! per-state-change rate, so the Mutex around the subscriber list is not on
//! the audio hot path.

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use od_core_types::AppEvent;

/// Per-subscriber queue depth. Generous for UI consumers; a subscriber that
/// falls 256 events behind is broken, not unlucky.
const SUBSCRIBER_QUEUE: usize = 256;

/// Broadcast bus for [`AppEvent`]s.
#[derive(Default)]
pub struct EventBus {
    subscribers: Mutex<Vec<SyncSender<AppEvent>>>,
}

impl EventBus {
    /// Creates an empty bus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a subscriber; returns its receiving end.
    pub fn subscribe(&self) -> Receiver<AppEvent> {
        let (tx, rx) = sync_channel(SUBSCRIBER_QUEUE);
        self.subscribers.lock().expect("bus lock").push(tx);
        rx
    }

    /// Publishes an event to all subscribers. Never blocks: full queues drop
    /// the event (warned), disconnected subscribers are removed.
    pub fn publish(&self, event: &AppEvent) {
        let mut subs = self.subscribers.lock().expect("bus lock");
        subs.retain(|tx| match tx.try_send(event.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                tracing::warn!(?event, "event bus subscriber queue full; dropping");
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_core_types::{AppEvent, SessionState};

    fn ev() -> AppEvent {
        AppEvent::StateChanged {
            state: SessionState::Idle,
        }
    }

    #[test]
    fn delivers_to_all_subscribers() {
        let bus = EventBus::new();
        let a = bus.subscribe();
        let b = bus.subscribe();
        bus.publish(&ev());
        assert_eq!(a.try_recv().unwrap(), ev());
        assert_eq!(b.try_recv().unwrap(), ev());
    }

    #[test]
    fn dropped_subscriber_is_pruned() {
        let bus = EventBus::new();
        let a = bus.subscribe();
        drop(a);
        bus.publish(&ev()); // must not panic; subscriber removed
        let b = bus.subscribe();
        bus.publish(&ev());
        assert_eq!(b.try_recv().unwrap(), ev());
    }

    #[test]
    fn full_queue_drops_instead_of_blocking() {
        let bus = EventBus::new();
        let rx = bus.subscribe();
        for _ in 0..SUBSCRIBER_QUEUE + 10 {
            bus.publish(&ev()); // would deadlock here if publish blocked
        }
        // Queue holds exactly its capacity; the overflow was dropped.
        let mut received = 0;
        while rx.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(received, SUBSCRIBER_QUEUE);
    }
}
