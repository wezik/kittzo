use tokio::sync::broadcast;

use crate::domain::event::OutboxMessage;
use crate::domain::event_bus::EventBus;

/// ponytail: fixed capacity ring buffer - a subscriber that falls behind by more than this
/// many messages gets `Lagged` and skips ahead, rather than the bus growing unbounded.
const CAPACITY: usize = 256;

pub struct TokioEventBus {
    sender: broadcast::Sender<OutboxMessage>,
}

impl TokioEventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(CAPACITY);
        Self { sender }
    }
}

impl Default for TokioEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus for TokioEventBus {
    fn publish(&self, message: OutboxMessage) {
        // Err just means no subscribers are currently listening - fine, nothing to do with it.
        tracing::debug!(
            event_type = message.event_type,
            "sending message to event bus"
        );
        let _ = self.sender.send(message);
    }

    fn subscribe(&self) -> broadcast::Receiver<OutboxMessage> {
        self.sender.subscribe()
    }
}
