use sqlx::{Row, Sqlite, SqlitePool, sqlite::SqliteRow};
use time::{OffsetDateTime, SignedDuration};
use uuid::Uuid;

use crate::domain::event::OutboxMessage;

use super::{Error, unexpected};

/// Takes any executor (`&pool` for a standalone write, `&mut *tx` to join a transaction a
/// caller already opened, e.g. alongside `sqlx_payment_repository::create`).
pub async fn create<'e, E>(executor: E, msg: impl Into<OutboxMessage>) -> Result<(), Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let msg = msg.into();
    sqlx::query(
        "INSERT INTO outbox
         (id, event_type, aggregate_id, payload, created_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(msg.id.to_string())
    .bind(msg.event_type.clone())
    .bind(msg.aggregate_id.clone())
    .bind(msg.payload)
    .bind(OffsetDateTime::now_utc())
    .execute(executor)
    .await
    .map_err(unexpected)?;
    tracing::info!(id = %msg.id, event_type = %msg.event_type, aggregate_id = %msg.aggregate_id, "created outbox message");
    Ok(())
}

/// Atomically claims up to `limit` unprocessed messages (never claimed, or claimed longer
/// ago than `retry_after` without being marked processed - same reclaim-on-staleness shape
/// as `sqlx_payment_schedule_repository::claim_due`).
pub async fn claim_batch(
    pool: &SqlitePool,
    retry_after: SignedDuration,
    limit: i64,
) -> Result<Vec<OutboxMessage>, Error> {
    let now = OffsetDateTime::now_utc();
    let retry_cutoff = now - retry_after;

    sqlx::query(
        "UPDATE outbox
         SET claimed_at = ?
         WHERE id IN (
             SELECT id FROM outbox
             WHERE processed_at IS NULL AND (claimed_at IS NULL OR claimed_at <= ?)
             ORDER BY rowid
             LIMIT ?
         )
         RETURNING *",
    )
    .bind(now)
    .bind(retry_cutoff)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(unexpected)
    .map(|rows| rows.iter().map(row_to_message).collect())
}

pub async fn mark_processed(pool: &SqlitePool, id: Uuid) -> Result<(), Error> {
    let result = sqlx::query("UPDATE outbox SET processed_at = ? WHERE id = ?")
        .bind(OffsetDateTime::now_utc())
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(unexpected)?;
    if result.rows_affected() == 0 {
        tracing::error!(%id, "outbox message to mark processed not found, it will be redelivered");
    }
    Ok(())
}

fn row_to_message(row: &SqliteRow) -> OutboxMessage {
    let id: String = row.get("id");
    OutboxMessage {
        id: Uuid::parse_str(&id).expect("invalid outbox id uuid"),
        event_type: row.get("event_type"),
        aggregate_id: row.get("aggregate_id"),
        payload: row.get("payload"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::payment::Event;
    use crate::infra::persistence::connect;

    async fn pool() -> SqlitePool {
        connect("sqlite::memory:").await.unwrap()
    }

    #[tokio::test]
    async fn create_then_claim_returns_the_message() {
        // given
        let pool = pool().await;
        let aggregate_id = Uuid::new_v4();
        let event = Event::Created { id: aggregate_id };

        // when
        create(&pool, event).await.unwrap();
        let claimed = claim_batch(&pool, SignedDuration::seconds(30), 10)
            .await
            .unwrap();

        // then
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].event_type, "PAYMENT_CREATED");
        assert_eq!(claimed[0].aggregate_id, aggregate_id.to_string());
    }

    #[tokio::test]
    async fn a_processed_message_is_not_claimed_again() {
        // given
        let pool = pool().await;
        create(&pool, Event::Created { id: Uuid::new_v4() })
            .await
            .unwrap();
        let claimed = claim_batch(&pool, SignedDuration::seconds(30), 10)
            .await
            .unwrap();

        // when
        mark_processed(&pool, claimed[0].id).await.unwrap();
        let second = claim_batch(&pool, SignedDuration::seconds(30), 10)
            .await
            .unwrap();

        // then
        assert!(second.is_empty());
    }

    #[tokio::test]
    async fn a_freshly_claimed_message_is_not_reclaimed_before_retry_after_elapses() {
        // given
        let pool = pool().await;
        create(&pool, Event::Created { id: Uuid::new_v4() })
            .await
            .unwrap();
        claim_batch(&pool, SignedDuration::seconds(30), 10)
            .await
            .unwrap();

        // when
        let second = claim_batch(&pool, SignedDuration::seconds(30), 10)
            .await
            .unwrap();

        // then
        assert!(second.is_empty());
    }

    #[tokio::test]
    async fn a_deferred_message_is_reclaimed_once_retry_after_elapses() {
        // given
        let pool = pool().await;
        create(&pool, Event::Created { id: Uuid::new_v4() })
            .await
            .unwrap();
        let first = claim_batch(&pool, SignedDuration::seconds(30), 10)
            .await
            .unwrap();

        // when
        let reclaimed = claim_batch(&pool, SignedDuration::seconds(0), 10)
            .await
            .unwrap();

        // then
        assert_eq!(reclaimed.len(), 1);
        assert_eq!(reclaimed[0].id, first[0].id);
    }

    #[tokio::test]
    async fn claim_batch_returns_insertion_order() {
        // given
        let pool = pool().await;
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        create(&pool, Event::Created { id: first_id })
            .await
            .unwrap();
        create(&pool, Event::Created { id: second_id })
            .await
            .unwrap();

        // when
        let claimed = claim_batch(&pool, SignedDuration::seconds(30), 10)
            .await
            .unwrap();

        // then
        assert_eq!(claimed[0].aggregate_id, first_id.to_string());
        assert_eq!(claimed[1].aggregate_id, second_id.to_string());
    }

    #[tokio::test]
    async fn claim_batch_respects_the_limit() {
        // given
        let pool = pool().await;
        create(&pool, Event::Created { id: Uuid::new_v4() })
            .await
            .unwrap();
        create(&pool, Event::Created { id: Uuid::new_v4() })
            .await
            .unwrap();

        // when
        let claimed = claim_batch(&pool, SignedDuration::seconds(30), 1)
            .await
            .unwrap();

        // then
        assert_eq!(claimed.len(), 1);
    }
}
