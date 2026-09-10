use async_trait::async_trait;
use sqlx::{Row, Sqlite, SqlitePool, sqlite::SqliteRow};
use time::{Date, OffsetDateTime, SignedDuration};
use uuid::Uuid;

use crate::domain::confirmation::Confirmation;
use crate::domain::money::Money;
use crate::domain::payment::Payment;
use crate::domain::payment_schedule::{
    PaymentSchedule, PaymentScheduleRepository, PaymentScheduleStatus, Recurrence,
};
use crate::domain::version::Version;

use super::sqlx_confirmation_repository::insert_confirmation;
use super::sqlx_payment_repository::insert_payment;

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
             (id, version, created_at, updated_at, total, recurrence_type, interval_months, day_of_month, last_run_at, status, next_due_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(schedule.next_due_at)
        .execute(&self.pool)
        .await
        .expect("failed to insert payment schedule");

        schedule
    }

    async fn update(&self, schedule: PaymentSchedule) -> PaymentSchedule {
        let next_version = schedule.version.next();
        let rows_affected = update_schedule_row(&self.pool, &schedule, next_version).await;

        if rows_affected == 0 {
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

    /// Inserts every due payment (and its pending confirmation) and advances the schedule's
    /// watermark in one transaction, so a crash partway through can never leave a payment
    /// committed without the watermark that would stop it from being recreated on retry.
    async fn finalize_with_payments(
        &self,
        schedule: PaymentSchedule,
        payments: Vec<(Payment, Confirmation)>,
    ) -> (PaymentSchedule, Vec<(Payment, Confirmation)>) {
        let mut tx = self
            .pool
            .begin()
            .await
            .expect("failed to begin schedule finalize transaction");

        for (payment, confirmation) in &payments {
            insert_payment(&mut *tx, payment).await;
            insert_confirmation(&mut *tx, confirmation).await;
        }

        let next_version = schedule.version.next();
        let rows_affected = update_schedule_row(&mut *tx, &schedule, next_version).await;
        if rows_affected == 0 {
            panic!(
                "payment schedule {} updated concurrently or missing",
                schedule.id
            );
        }

        tx.commit()
            .await
            .expect("failed to commit schedule finalize transaction");

        (
            PaymentSchedule {
                version: next_version,
                ..schedule
            },
            payments,
        )
    }

    /// Claims every schedule that's a *candidate* for a run: idle and due (`next_due_at`
    /// at/before today), or stuck in `processing` past `retry_after` (a previous claimer
    /// likely crashed, reclaimed unconditionally since it needs to finish regardless of
    /// `next_due_at`). `next_due_at` is a cache of [`PaymentSchedule::next_due_at`]
    /// (recurrence math stays in Rust, not duplicated here) kept fresh by `create` and
    /// `finalize_processing`; a `NULL` (pre-migration row that predates the column) is
    /// treated as due so it gets claimed once and self-heals. The idle branch is
    /// deliberately coarser than [`PaymentSchedule::pending_occurrences`]. The caller
    /// (the schedule job) re-checks that per claimed schedule and releases it with no
    /// payment created if it turns out nothing's actually due (e.g. `next_due_at` was
    /// stale). The WHERE clause only needs to make the idle->processing flip a single
    /// atomic statement so concurrent callers can't double-claim the same row.
    async fn claim_due(&self, retry_after: SignedDuration) -> Vec<PaymentSchedule> {
        let now = OffsetDateTime::now_utc();
        let retry_cutoff = now - retry_after;

        sqlx::query(
            "UPDATE payment_schedules
             SET status = 'processing', updated_at = ?, version = version + 1
             WHERE (status = 'idle' AND (next_due_at IS NULL OR next_due_at <= ?))
                OR (status = 'processing' AND updated_at <= ?)
             RETURNING *",
        )
        .bind(now)
        .bind(now.date())
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

/// Updates a schedule row against any executor (pool or transaction), so
/// `finalize_with_payments` can share this instead of duplicating the update. Returns rows
/// affected, letting each caller decide how to react to a lost optimistic-concurrency race.
async fn update_schedule_row<'e, E>(
    executor: E,
    schedule: &PaymentSchedule,
    next_version: Version,
) -> u64
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let (interval_months, day_of_month) = recurrence_parts(&schedule.recurrence);

    sqlx::query(
        "UPDATE payment_schedules
         SET version = ?, updated_at = ?, total = ?, recurrence_type = ?, interval_months = ?, day_of_month = ?, last_run_at = ?, status = ?, next_due_at = ?
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
    .bind(schedule.next_due_at)
    .bind(schedule.id.to_string())
    .bind(schedule.version.as_u32() as i64)
    .execute(executor)
    .await
    .expect("failed to update payment schedule")
    .rows_affected()
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
    let next_due_at: Option<Date> = row.get("next_due_at");

    let recurrence = match recurrence_type.as_str() {
        "every_n_months" => Recurrence::EveryNMonths {
            interval_months: interval_months as u32,
            day_of_month: day_of_month as u8,
        },
        other => panic!("unknown recurrence_type in db: {other}"),
    };

    // NULL only for a row written before the `next_due_at` column existed; recompute it
    // from the recurrence so the value self-heals on the next write to this row.
    let next_due_at =
        next_due_at.unwrap_or_else(|| recurrence.next_occurrence_after(created_at, last_run_at));

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
        next_due_at,
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

    // day_of_month pinned to today so this schedule is always due regardless of what day
    // the test happens to run on (claim_due now filters on next_due_at).
    fn idle_schedule() -> PaymentSchedule {
        let today = OffsetDateTime::now_utc();
        PaymentSchedule::new(
            Money::from_minor(1099, iso::USD),
            Recurrence::EveryNMonths {
                interval_months: 1,
                day_of_month: today.day(),
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
        let mut schedule = repo.create(idle_schedule()).await;
        schedule.finalize_processing();

        let updated = repo.update(schedule.clone()).await;

        assert_eq!(updated.version, schedule.version.next());
        assert_eq!(updated.status, PaymentScheduleStatus::Idle);
        assert!(updated.last_run_at.is_some());
    }

    #[tokio::test]
    async fn finalize_with_payments_persists_payments_confirmations_and_watermark() {
        use crate::domain::confirmation::{
            Confirmation, ConfirmationRepository, ConfirmationSubject,
        };
        use crate::domain::payment::{Payment, PaymentRepository, PaymentSource};
        use crate::infra::persistence::connect;
        use crate::infra::persistence::sqlx_confirmation_repository::SqliteConfirmationRepository;
        use crate::infra::persistence::sqlx_payment_repository::SqlitePaymentRepository;

        let pool = connect("sqlite::memory:").await.unwrap();
        let repo = SqlitePaymentScheduleRepository::new(pool.clone());
        let payment_repo = SqlitePaymentRepository::new(pool.clone());
        let confirmation_repo = SqliteConfirmationRepository::new(pool);

        repo.create(idle_schedule()).await;
        let mut schedule = repo.claim_due(SignedDuration::minutes(30)).await[0].clone();
        schedule.finalize_processing();

        let occurrence_date = schedule.created_at.date();
        let payment = Payment::new(
            schedule.total.clone(),
            PaymentSource::Schedule {
                schedule_id: schedule.id,
                occurrence_date,
            },
        );
        let confirmation = Confirmation::new(ConfirmationSubject::Payment(payment.id));

        let (updated_schedule, created) = repo
            .finalize_with_payments(
                schedule.clone(),
                vec![(payment.clone(), confirmation.clone())],
            )
            .await;

        assert_eq!(updated_schedule.version, schedule.version.next());
        assert_eq!(updated_schedule.status, PaymentScheduleStatus::Idle);
        assert_eq!(created.len(), 1);
        assert_eq!(payment_repo.find_by_id(payment.id).await, Some(payment));
        assert_eq!(
            confirmation_repo
                .find_by_id(confirmation.id)
                .await
                .unwrap()
                .subject,
            confirmation.subject
        );
    }

    #[tokio::test]
    #[should_panic(expected = "updated concurrently or missing")]
    async fn finalize_with_payments_rejects_a_stale_schedule_version() {
        let repo = repo().await;
        let schedule = repo.create(idle_schedule()).await;

        // simulate a concurrent writer having already advanced this schedule's version.
        let mut ahead = schedule.clone();
        ahead.finalize_processing();
        repo.update(ahead).await;

        repo.finalize_with_payments(schedule, Vec::new()).await;
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
