use async_trait::async_trait;
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
use time::{Date, OffsetDateTime, SignedDuration};
use uuid::Uuid;

use crate::domain::payment_schedule::{
    PaymentSchedule, PaymentScheduleRepository, PaymentScheduleStatus, Recurrence,
};
use crate::domain::version::Version;

use super::total_from_row;

pub struct SqlitePaymentScheduleRepository {
    pool: SqlitePool,
}

impl SqlitePaymentScheduleRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PaymentScheduleRepository for SqlitePaymentScheduleRepository {
    async fn create(&self, schedule: PaymentSchedule) -> PaymentSchedule {
        sqlx::query(
            "INSERT INTO payment_schedules
             (id, version, created_at, updated_at, total_amount, total_currency, recurrence_type, day_of_month, status, next_due_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(schedule.id.to_string())
        .bind(schedule.version.as_u32() as i64)
        .bind(schedule.created_at)
        .bind(schedule.updated_at)
        .bind(schedule.total.amount().to_string())
        .bind(schedule.total.currency_code())
        .bind(recurrence_type(&schedule.recurrence))
        .bind(day_of_month(&schedule.recurrence) as i64)
        .bind(status_str(schedule.status))
        .bind(schedule.next_due_at)
        .execute(&self.pool)
        .await
        .expect("failed to insert payment schedule");

        schedule
    }

    async fn update(&self, schedule: PaymentSchedule) -> PaymentSchedule {
        let next_version = schedule.version.next();

        let result = sqlx::query(
            "UPDATE payment_schedules
             SET version = ?, updated_at = ?, total_amount = ?, total_currency = ?, recurrence_type = ?, day_of_month = ?, status = ?, next_due_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(next_version.as_u32() as i64)
        .bind(schedule.updated_at)
        .bind(schedule.total.amount().to_string())
        .bind(schedule.total.currency_code())
        .bind(recurrence_type(&schedule.recurrence))
        .bind(day_of_month(&schedule.recurrence) as i64)
        .bind(status_str(schedule.status))
        .bind(schedule.next_due_at)
        .bind(schedule.id.to_string())
        .bind(schedule.version.as_u32() as i64)
        .execute(&self.pool)
        .await
        .expect("failed to update payment schedule");

        if result.rows_affected() == 0 {
            panic!(
                "payment schedule {} updated concurrently or missing",
                schedule.id
            );
        }

        PaymentSchedule {
            version: next_version,
            ..schedule
        }
    }

    async fn claim_due(&self, retry_after: SignedDuration) -> Vec<PaymentSchedule> {
        let now = OffsetDateTime::now_utc();
        let retry_cutoff = now - retry_after;

        // Claim respecting retry_after
        sqlx::query(
            "UPDATE payment_schedules
             SET status = 'processing', updated_at = ?, version = version + 1
             WHERE next_due_at <= ? AND (status = 'idle' OR (status = 'processing' AND updated_at <= ?))
             RETURNING *",
        )
        .bind(now)
        .bind(now.date())
        .bind(retry_cutoff)
        .fetch_all(&self.pool)
        .await
        .expect("failed to claim due payment schedules")
        .iter()
        .filter_map(row_to_schedule)
        .collect()
    }

    async fn find_all(&self) -> Vec<PaymentSchedule> {
        sqlx::query("SELECT * FROM payment_schedules ORDER BY created_at")
            .fetch_all(&self.pool)
            .await
            .expect("failed to query payment schedules")
            .iter()
            .filter_map(row_to_schedule)
            .collect()
    }
}

fn recurrence_type(recurrence: &Recurrence) -> &'static str {
    match recurrence {
        Recurrence::Monthly { .. } => "monthly",
    }
}

fn day_of_month(recurrence: &Recurrence) -> u8 {
    match recurrence {
        Recurrence::Monthly { day_of_month } => *day_of_month,
    }
}

fn status_str(status: PaymentScheduleStatus) -> &'static str {
    match status {
        PaymentScheduleStatus::Idle => "idle",
        PaymentScheduleStatus::Processing => "processing",
    }
}

fn row_to_schedule(row: &SqliteRow) -> Option<PaymentSchedule> {
    let id: String = row.get("id");
    let version: i64 = row.get("version");
    let created_at: OffsetDateTime = row.get("created_at");
    let updated_at: OffsetDateTime = row.get("updated_at");
    let total = total_from_row(row)?;
    let recurrence_type: String = row.get("recurrence_type");
    let day_of_month: i64 = row.get("day_of_month");
    let status: String = row.get("status");
    let next_due_at: Date = row.get("next_due_at");

    let recurrence = match recurrence_type.as_str() {
        "monthly" => Recurrence::Monthly {
            day_of_month: day_of_month as u8,
        },
        other => panic!("unknown recurrence_type in db: {other}"),
    };

    Some(PaymentSchedule {
        id: Uuid::parse_str(&id).expect("invalid payment schedule id uuid"),
        version: Version::from_u32(version as u32),
        created_at,
        updated_at,
        total,
        recurrence,
        status: match status.as_str() {
            "idle" => PaymentScheduleStatus::Idle,
            "processing" => PaymentScheduleStatus::Processing,
            other => panic!("unknown status in db: {other}"),
        },
        next_due_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::money::Money;
    use crate::infra::persistence::connect;
    use std::sync::Arc;

    async fn repo() -> SqlitePaymentScheduleRepository {
        let pool = connect("sqlite::memory:").await.unwrap();
        SqlitePaymentScheduleRepository::new(pool)
    }

    // A freshly created schedule's next_due_at is never today (it's always later this
    // month or next), so backdate it directly to get a fixture that's due right now.
    fn idle_schedule() -> PaymentSchedule {
        let mut schedule = PaymentSchedule::new(
            Money::from_parts("USD", "10.99").unwrap(),
            Recurrence::Monthly { day_of_month: 1 },
        );
        schedule.next_due_at = OffsetDateTime::now_utc().date();
        schedule
    }

    #[tokio::test]
    async fn claim_due_claims_an_idle_schedule_and_flips_it_to_processing() {
        let repo = repo().await;
        let schedule = repo.create(idle_schedule()).await;

        let claimed = repo.claim_due(SignedDuration::minutes(30)).await;

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, schedule.id);
        assert_eq!(claimed[0].status, PaymentScheduleStatus::Processing);
        assert_eq!(claimed[0].version, schedule.version.next());
    }

    #[tokio::test]
    async fn claim_due_skips_a_freshly_claimed_processing_schedule() {
        let repo = repo().await;
        repo.create(idle_schedule()).await;
        repo.claim_due(SignedDuration::minutes(30)).await;

        let second = repo.claim_due(SignedDuration::minutes(30)).await;

        assert!(second.is_empty());
    }

    #[tokio::test]
    async fn claim_due_skips_an_idle_schedule_that_is_not_yet_due() {
        let repo = repo().await;
        let mut seed = idle_schedule();
        // pushed 60 days out, so next_due_at is well past today regardless of which
        // day of the month this test runs on.
        seed.next_due_at += time::Duration::days(60);
        repo.create(seed).await;

        let claimed = repo.claim_due(SignedDuration::minutes(30)).await;

        assert!(claimed.is_empty());
    }

    #[tokio::test]
    async fn claim_due_reclaims_a_stale_processing_schedule() {
        let repo = repo().await;
        repo.create(idle_schedule()).await;
        repo.claim_due(SignedDuration::minutes(30)).await;

        let reclaimed = repo.claim_due(SignedDuration::seconds(0)).await;

        assert_eq!(reclaimed.len(), 1);
    }

    #[tokio::test]
    async fn update_persists_changes_and_bumps_version() {
        let repo = repo().await;
        let schedule = repo.create(idle_schedule()).await;
        let mut next = schedule.clone();
        next.status = PaymentScheduleStatus::Processing;
        next.advance_due_date();

        let updated = repo.update(next.clone()).await;

        assert_eq!(updated.version, schedule.version.next());
        assert_eq!(updated.status, PaymentScheduleStatus::Processing);
        assert_eq!(updated.next_due_at, next.next_due_at);
    }

    #[tokio::test]
    #[should_panic(expected = "updated concurrently or missing")]
    async fn update_rejects_a_stale_version() {
        let repo = repo().await;
        let schedule = repo.create(idle_schedule()).await;

        // simulate a concurrent writer having already advanced this schedule's version.
        repo.update(schedule.clone()).await;

        repo.update(schedule).await;
    }

    #[tokio::test]
    async fn find_all_returns_every_schedule() {
        let repo = repo().await;
        repo.create(idle_schedule()).await;
        repo.create(idle_schedule()).await;

        assert_eq!(repo.find_all().await.len(), 2);
    }

    #[tokio::test]
    async fn claim_due_lets_exactly_one_caller_win_a_race() {
        let repo = Arc::new(repo().await);
        repo.create(idle_schedule()).await;

        let mut handles = Vec::new();
        for _ in 0..8 {
            let repo = repo.clone();
            handles.push(tokio::spawn(async move {
                repo.claim_due(SignedDuration::minutes(30)).await
            }));
        }

        let mut claimed_total = 0;
        for handle in handles {
            claimed_total += handle.await.unwrap().len();
        }

        assert_eq!(claimed_total, 1);
    }
}
