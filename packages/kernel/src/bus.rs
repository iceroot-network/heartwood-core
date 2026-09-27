//! The event bus: the node's events by the reference's names, to every subscriber, in the order
//! they were emitted. The node emits after each commit; nothing in a relay consumes the events
//! beyond its own ordered listeners, and the external payloads (webhooks, the API's socket) come
//! with their first consumer.

use tokio::sync::broadcast;

use crate::events::Event;

/// How many events a slow subscriber may fall behind before it misses the oldest.
pub const CAPACITY: usize = 1024;

/// The event bus.
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> EventBus {
        EventBus::new()
    }
}

impl EventBus {
    /// A bus with no subscriber.
    pub fn new() -> EventBus {
        EventBus {
            sender: broadcast::channel(CAPACITY).0,
        }
    }

    /// Emit `event` to every subscriber; with none, the event is dropped.
    pub fn emit(&self, event: Event) {
        // With no subscriber the send fails, and the event is dropped as documented.
        let _ = self.sender.send(event);
    }

    /// A new subscriber, which receives the events emitted from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subscriber_hears_the_events_after_it_subscribed_in_order() {
        let bus = EventBus::new();
        bus.emit(Event::BlockApplied);
        let (mut first, mut second) = (bus.subscribe(), bus.clone().subscribe());
        bus.emit(Event::PeerAdded);
        bus.emit(Event::PeerRemoved);
        for receiver in [&mut first, &mut second] {
            assert_eq!(receiver.try_recv(), Ok(Event::PeerAdded));
            assert_eq!(receiver.try_recv(), Ok(Event::PeerRemoved));
            assert!(receiver.try_recv().is_err());
        }
    }
}
