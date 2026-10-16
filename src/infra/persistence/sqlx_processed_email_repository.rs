use std::collections::HashSet;

use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

pub use super::Error;
use super::unexpected;

pub async fn finished(pool: &SqlitePool) -> Result<HashSet<String>, Error> {
    Ok(sqlx::query(
        "SELECT message_id FROM processed_emails WHERE outcome IN ('processed', 'failed')",
    )
    .fetch_all(pool)
    .await
    .map_err(unexpected)?
    .iter()
    .map(|row| row.get("message_id"))
    .collect())
}

pub async fn mark_processed(
    pool: &SqlitePool,
    message_id: &str,
    payments_created: usize,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO processed_emails (message_id, outcome, payments_created, attempts, updated_at)
         VALUES (?, 'processed', ?, 1, ?)
         ON CONFLICT(message_id) DO UPDATE SET
             outcome = 'processed',
             payments_created = excluded.payments_created,
             attempts = attempts + 1,
             last_error = NULL,
             updated_at = excluded.updated_at",
    )
    .bind(message_id)
    .bind(payments_created as i64)
    .bind(OffsetDateTime::now_utc())
    .execute(pool)
    .await
    .map_err(unexpected)?;
    Ok(())
}

pub async fn record_failure(
    pool: &SqlitePool,
    message_id: &str,
    error: &str,
    max_attempts: u32,
) -> Result<u32, Error> {
    let attempts: i64 = sqlx::query(
        "INSERT INTO processed_emails (message_id, outcome, attempts, last_error, updated_at)
         VALUES (?1, CASE WHEN ?2 <= 1 THEN 'failed' ELSE 'retrying' END, 1, ?3, ?4)
         ON CONFLICT(message_id) DO UPDATE SET
             attempts = attempts + 1,
             outcome = CASE WHEN attempts + 1 >= ?2 THEN 'failed' ELSE 'retrying' END,
             last_error = excluded.last_error,
             updated_at = excluded.updated_at
         RETURNING attempts",
    )
    .bind(message_id)
    .bind(max_attempts as i64)
    .bind(error)
    .bind(OffsetDateTime::now_utc())
    .fetch_one(pool)
    .await
    .map_err(unexpected)?
    .get("attempts");
    Ok(attempts as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::persistence::connect;

    #[tokio::test]
    async fn processed_emails_are_finished() {
        let pool = connect("sqlite::memory:").await.unwrap();

        mark_processed(&pool, "<a@x>", 2).await.unwrap();

        assert_eq!(
            finished(&pool).await.unwrap(),
            HashSet::from(["<a@x>".to_string()])
        );
    }

    #[tokio::test]
    async fn failures_count_up_and_finish_at_the_limit() {
        let pool = connect("sqlite::memory:").await.unwrap();

        assert_eq!(
            record_failure(&pool, "<a@x>", "timeout", 3).await.unwrap(),
            1
        );
        assert_eq!(
            record_failure(&pool, "<a@x>", "timeout", 3).await.unwrap(),
            2
        );
        assert!(finished(&pool).await.unwrap().is_empty());
        assert_eq!(
            record_failure(&pool, "<a@x>", "timeout", 3).await.unwrap(),
            3
        );

        assert!(finished(&pool).await.unwrap().contains("<a@x>"));
    }

    #[tokio::test]
    async fn a_retry_that_succeeds_is_processed() {
        let pool = connect("sqlite::memory:").await.unwrap();
        record_failure(&pool, "<a@x>", "timeout", 3).await.unwrap();

        mark_processed(&pool, "<a@x>", 1).await.unwrap();

        assert!(finished(&pool).await.unwrap().contains("<a@x>"));
    }
}
