use tokio::sync::broadcast;

use crate::domain::event::OutboxMessage;

/// In-process pub/sub port: `OutboxRelayJob` publishes what the durable `outbox` table already
/// persisted, subscribers get a fan-out clone. No delivery guarantee beyond "was in memory when
/// published" - durability and retry-after-crash live in `outbox`, not here. A subscriber that
/// needs to survive a failed handling of its own (e.g. Discord being down) re-publishes.
pub trait EventBus: Send + Sync {
    fn publish(&self, message: OutboxMessage);
    fn subscribe(&self) -> broadcast::Receiver<OutboxMessage>;
}
