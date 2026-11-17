use std::time::Duration;

use sqlx::SqlitePool;
use time::SignedDuration;

use tokio::sync::broadcast;

use crate::domain::event::OutboxMessage;
use crate::infra::persistence::{self, sqlx_outbox_repository as outbox_repo};

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(default)]
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
        SignedDuration::seconds(i64::try_from(self.retry_after_secs).unwrap_or(i64::MAX))
    }
}

pub fn event_bus() -> broadcast::Sender<OutboxMessage> {
    broadcast::channel(256).0
}

pub struct OutboxRelayJob {
    pool: SqlitePool,
    bus: broadcast::Sender<OutboxMessage>,
    config: OutboxRelayConfig,
}

impl OutboxRelayJob {
    pub fn new(
        pool: SqlitePool,
        bus: broadcast::Sender<OutboxMessage>,
        config: OutboxRelayConfig,
    ) -> Self {
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
            let _ = self.bus.send(message);
            outbox_repo::mark_processed(&self.pool, id).await?;
        }
        Ok(())
    }

    pub async fn run(self) {
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
    use uuid::Uuid;

    use super::*;
    use crate::infra::persistence::connect;

    async fn some_event(pool: &SqlitePool) -> String {
        use crate::domain::payment::Event;
        let aggregate_id = Uuid::new_v4();
        outbox_repo::create(pool, Event::Created { id: aggregate_id })
            .await
            .unwrap();
        aggregate_id.to_string()
    }

    fn job(pool: SqlitePool) -> (OutboxRelayJob, broadcast::Receiver<OutboxMessage>) {
        job_with(pool, OutboxRelayConfig::default())
    }

    fn job_with(
        pool: SqlitePool,
        config: OutboxRelayConfig,
    ) -> (OutboxRelayJob, broadcast::Receiver<OutboxMessage>) {
        let bus = event_bus();
        let rx = bus.subscribe();
        (OutboxRelayJob::new(pool, bus, config), rx)
    }

    fn published(rx: &mut broadcast::Receiver<OutboxMessage>) -> Vec<String> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|message| message.aggregate_id)
            .collect()
    }

    #[tokio::test]
    async fn processes_an_unprocessed_message_and_marks_it_processed() {
        let pool = connect("sqlite::memory:").await.unwrap();
        some_event(&pool).await;
        let (job, mut rx) = job(pool.clone());

        job.process().await.unwrap();

        assert_eq!(published(&mut rx).len(), 1);
        let remaining = outbox_repo::claim_batch(&pool, SignedDuration::seconds(0), 10)
            .await
            .unwrap();
        assert!(remaining.is_empty());
    }

    #[tokio::test]
    async fn processes_messages_in_insertion_order() {
        let pool = connect("sqlite::memory:").await.unwrap();
        let first = some_event(&pool).await;
        let second = some_event(&pool).await;
        let (job, mut rx) = job(pool.clone());

        job.process().await.unwrap();

        assert_eq!(published(&mut rx), vec![first, second]);
    }

    #[tokio::test]
    async fn respects_the_batch_limit() {
        let pool = connect("sqlite::memory:").await.unwrap();
        some_event(&pool).await;
        some_event(&pool).await;
        let config = OutboxRelayConfig {
            batch_limit: 1,
            ..OutboxRelayConfig::default()
        };
        let (job, mut rx) = job_with(pool.clone(), config);

        job.process().await.unwrap();

        assert_eq!(published(&mut rx).len(), 1);
    }
}
