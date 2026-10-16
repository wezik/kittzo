use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::SqlitePool;
use time::SignedDuration;

use crate::app::event_bus::EventBus;
use crate::infra::persistence::{self, sqlx_outbox_repository as outbox_repo};

use super::startup_task::StartupTask;

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxRelayConfig {
    pub poll_interval_secs: u64,
    pub retry_after_secs: u64,
    pub batch_limit: i64,
}

impl Default for OutboxRelayConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 1,
            retry_after_secs: 30,
            batch_limit: 100,
        }
    }
}

impl OutboxRelayConfig {
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_interval_secs)
    }

    pub fn retry_after(&self) -> SignedDuration {
        SignedDuration::seconds(self.retry_after_secs as i64)
    }
}

/// Drains the outbox and publishes each message onto the in-process `EventBus` - the
/// transactional-outbox half of "back up requests to slow Discord": the write into `outbox`
/// already happened durably alongside whatever domain change produced it, this just delivers
/// it on a schedule instead of inline. Publishing itself can't fail (fire-and-forget, in
/// memory), so every claimed message is marked processed right after; a subscriber that fails
/// to handle it (e.g. Discord being down) is responsible for its own retry.
pub struct OutboxRelayJob {
    pool: SqlitePool,
    bus: Arc<dyn EventBus>,
    config: OutboxRelayConfig,
}

impl OutboxRelayJob {
    pub fn new(pool: SqlitePool, bus: Arc<dyn EventBus>, config: OutboxRelayConfig) -> Self {
        Self { pool, bus, config }
    }

    async fn process(&self) -> Result<(), persistence::Error> {
        let claimed = outbox_repo::claim_batch(
            &self.pool,
            self.config.retry_after(),
            self.config.batch_limit,
        )
        .await?;
        for message in claimed {
            let id = message.id;
            self.bus.publish(message);
            outbox_repo::mark_processed(&self.pool, id).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl StartupTask for OutboxRelayJob {
    fn name(&self) -> &str {
        "outbox-relay-job"
    }

    async fn run(&self) {
        loop {
            if let Err(err) = self.process().await {
                tracing::error!(%err, "failed to process outbox");
            }
            tokio::time::sleep(self.config.poll_interval()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::sync::broadcast;
    use uuid::Uuid;

    use super::*;
    use crate::domain::event::{Codec, OutboxMessage};
    use crate::infra::codec::JsonCodec;
    use crate::infra::persistence::connect;

    struct FakeBus {
        published: Mutex<Vec<String>>,
    }

    impl FakeBus {
        fn new() -> Self {
            Self {
                published: Mutex::new(Vec::new()),
            }
        }
    }

    impl EventBus for FakeBus {
        fn publish(&self, message: OutboxMessage) {
            self.published.lock().unwrap().push(message.aggregate_id);
        }

        fn subscribe(&self) -> broadcast::Receiver<OutboxMessage> {
            broadcast::channel(1).1
        }
    }

    async fn some_event(pool: &SqlitePool) -> String {
        use crate::domain::payment::Event;
        let aggregate_id = Uuid::new_v4();
        outbox_repo::create(pool, Event::Created { id: aggregate_id })
            .await
            .unwrap();
        aggregate_id.to_string()
    }

    fn job(pool: SqlitePool, bus: Arc<dyn EventBus>) -> OutboxRelayJob {
        OutboxRelayJob::new(pool, bus, OutboxRelayConfig::default())
    }

    #[tokio::test]
    async fn processes_an_unprocessed_message_and_marks_it_processed() {
        // given
        let pool = connect("sqlite::memory:").await.unwrap();
        some_event(&pool).await;
        let bus = Arc::new(FakeBus::new());
        let job = job(pool.clone(), bus.clone());

        // when
        job.process().await.unwrap();

        // then
        assert_eq!(bus.published.lock().unwrap().len(), 1);
        let remaining = outbox_repo::claim_batch(&pool, SignedDuration::seconds(0), 10)
            .await
            .unwrap();
        assert!(remaining.is_empty());
    }

    #[tokio::test]
    async fn processes_messages_in_insertion_order() {
        // given
        let pool = connect("sqlite::memory:").await.unwrap();
        let first = some_event(&pool).await;
        let second = some_event(&pool).await;
        let bus = Arc::new(FakeBus::new());
        let job = job(pool.clone(), bus.clone());

        // when
        job.process().await.unwrap();

        // then
        assert_eq!(*bus.published.lock().unwrap(), vec![first, second]);
    }

    #[tokio::test]
    async fn respects_the_batch_limit() {
        // given
        let pool = connect("sqlite::memory:").await.unwrap();
        some_event(&pool).await;
        some_event(&pool).await;
        let bus = Arc::new(FakeBus::new());
        let mut config = OutboxRelayConfig::default();
        config.batch_limit = 1;
        let job = OutboxRelayJob::new(pool.clone(), bus.clone(), config);

        // when
        job.process().await.unwrap();

        // then
        assert_eq!(bus.published.lock().unwrap().len(), 1);
    }

    #[test]
    fn decode_round_trips_through_json_codec() {
        use crate::domain::payment::Event;
        let event = Event::Created { id: Uuid::new_v4() };
        let msg = OutboxMessage::from(event.clone());
        let decoded: Event = JsonCodec::decode(&msg.payload).unwrap();
        assert_eq!(decoded, event);
    }
}
