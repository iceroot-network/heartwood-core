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
        let _ = CAPACITY;
        todo!()
    }

    /// Emit `event` to every subscriber; with none, the event is dropped.
    pub fn emit(&self, event: Event) {
        let _ = (&self.sender, event);
        todo!()
    }

    /// A new subscriber, which receives the events emitted from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        let _ = &self.sender;
        todo!()
    }
}
