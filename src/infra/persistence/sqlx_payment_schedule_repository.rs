use async_trait::async_trait;
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
use time::{OffsetDateTime, SignedDuration};
use uuid::Uuid;

use crate::domain::money::Money;
use crate::domain::payment_schedule::{
    PaymentSchedule, PaymentScheduleRepository, PaymentScheduleStatus, Recurrence,
};
use crate::domain::version::Version;

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
        let (interval_months, day_of_month) = recurrence_parts(&schedule.recurrence);
        sqlx::query(
            "INSERT INTO payment_schedules
             (id, version, created_at, updated_at, total, recurrence_type, interval_months, day_of_month, last_run_at, status)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(schedule.id.to_string())
        .bind(schedule.version.as_u32() as i64)
        .bind(schedule.created_at)
        .bind(schedule.updated_at)
        .bind(&schedule.total)
        .bind(recurrence_type(&schedule.recurrence))
        .bind(interval_months as i64)
        .bind(day_of_month as i64)
        .bind(schedule.last_run_at)
        .bind(status_str(schedule.status))
        .execute(&self.pool)
        .await
        .expect("failed to insert payment schedule");

        schedule
    }

    async fn update(&self, schedule: PaymentSchedule) -> PaymentSchedule {
        let (interval_months, day_of_month) = recurrence_parts(&schedule.recurrence);
        let next_version = schedule.version.next();

        let result = sqlx::query(
            "UPDATE payment_schedules
             SET version = ?, updated_at = ?, total = ?, recurrence_type = ?, interval_months = ?, day_of_month = ?, last_run_at = ?, status = ?
             WHERE id = ? AND version = ?",
        )
        .bind(next_version.as_u32() as i64)
        .bind(schedule.updated_at)
        .bind(&schedule.total)
        .bind(recurrence_type(&schedule.recurrence))
        .bind(interval_months as i64)
        .bind(day_of_month as i64)
        .bind(schedule.last_run_at)
        .bind(status_str(schedule.status))
        .bind(schedule.id.to_string())
        .bind(schedule.version.as_u32() as i64)
        .execute(&self.pool)
        .await
        .expect("failed to update payment schedule");

        if result.rows_affected() == 0 {
            panic!("payment schedule {} updated concurrently or missing", schedule.id);
        }

        PaymentSchedule {
            version: next_version,
            ..schedule
        }
    }

    /// Claims every schedule that's a *candidate* for a run: idle, or stuck in
    /// `processing` past `retry_after` (a previous claimer likely crashed). This is
    /// deliberately coarser than [`PaymentSchedule::pending_occurrences`] — the caller
    /// (the future schedule job) re-checks that per claimed schedule and releases it
    /// with no payment created if nothing's actually due. Keeping the recurrence math
    /// out of SQL avoids a second, drift-prone copy of it; the WHERE clause only needs
    /// to make the idle->processing flip a single atomic statement so concurrent
    /// callers can't double-claim the same row.
    async fn claim_due(&self, retry_after: SignedDuration) -> Vec<PaymentSchedule> {
        let now = OffsetDateTime::now_utc();
        let retry_cutoff = now - retry_after;

        sqlx::query(
            "UPDATE payment_schedules
             SET status = 'processing', updated_at = ?, version = version + 1
             WHERE status = 'idle' OR (status = 'processing' AND updated_at <= ?)
             RETURNING *",
        )
        .bind(now)
        .bind(retry_cutoff)
        .fetch_all(&self.pool)
        .await
        .expect("failed to claim due payment schedules")
        .iter()
        .map(row_to_schedule)
        .collect()
    }

    async fn find_all(&self) -> Vec<PaymentSchedule> {
        sqlx::query("SELECT * FROM payment_schedules ORDER BY created_at")
            .fetch_all(&self.pool)
            .await
            .expect("failed to query payment schedules")
            .iter()
            .map(row_to_schedule)
            .collect()
    }
}

fn recurrence_type(recurrence: &Recurrence) -> &'static str {
    match recurrence {
        Recurrence::EveryNMonths { .. } => "every_n_months",
    }
}

fn recurrence_parts(recurrence: &Recurrence) -> (u32, u8) {
    match recurrence {
        Recurrence::EveryNMonths {
            interval_months,
            day_of_month,
        } => (*interval_months, *day_of_month),
    }
}

fn status_str(status: PaymentScheduleStatus) -> &'static str {
    match status {
        PaymentScheduleStatus::Idle => "idle",
        PaymentScheduleStatus::Processing => "processing",
    }
}

fn row_to_schedule(row: &SqliteRow) -> PaymentSchedule {
    let id: String = row.get("id");
    let version: i64 = row.get("version");
    let created_at: OffsetDateTime = row.get("created_at");
    let updated_at: OffsetDateTime = row.get("updated_at");
    let total: Money = row.get("total");
    let recurrence_type: String = row.get("recurrence_type");
    let interval_months: i64 = row.get("interval_months");
    let day_of_month: i64 = row.get("day_of_month");
    let last_run_at: Option<OffsetDateTime> = row.get("last_run_at");
    let status: String = row.get("status");

    let recurrence = match recurrence_type.as_str() {
        "every_n_months" => Recurrence::EveryNMonths {
            interval_months: interval_months as u32,
            day_of_month: day_of_month as u8,
        },
        other => panic!("unknown recurrence_type in db: {other}"),
    };

    PaymentSchedule {
        id: Uuid::parse_str(&id).expect("invalid payment schedule id uuid"),
        version: Version::from_u32(version as u32),
        created_at,
        updated_at,
        total,
        recurrence,
        last_run_at,
        status: match status.as_str() {
            "idle" => PaymentScheduleStatus::Idle,
            "processing" => PaymentScheduleStatus::Processing,
            other => panic!("unknown status in db: {other}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::persistence::connect;
    use rusty_money::iso;
    use std::sync::Arc;

    async fn repo() -> SqlitePaymentScheduleRepository {
        let pool = connect("sqlite::memory:").await.unwrap();
        SqlitePaymentScheduleRepository::new(pool)
    }

    fn idle_schedule() -> PaymentSchedule {
        PaymentSchedule::new(
            Money::from_minor(1099, iso::USD),
            Recurrence::EveryNMonths {
                interval_months: 1,
                day_of_month: 5,
            },
        )
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
        let mut schedule = repo.create(idle_schedule()).await;
        schedule.finalize_processing();

        let updated = repo.update(schedule.clone()).await;

        assert_eq!(updated.version, schedule.version.next());
        assert_eq!(updated.status, PaymentScheduleStatus::Idle);
        assert!(updated.last_run_at.is_some());
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
