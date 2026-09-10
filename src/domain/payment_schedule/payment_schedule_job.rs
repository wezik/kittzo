use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;

use crate::domain::confirmation::{Confirmation, ConfirmationSubject, Notifier};
use crate::domain::payment::{Payment, PaymentSource};
use crate::domain::startup_task::StartupTask;

use super::{PaymentScheduleConfig, PaymentScheduleService};

pub struct PaymentScheduleJob {
    schedule_service: Arc<PaymentScheduleService>,
    notifier: Arc<dyn Notifier>,
    config: PaymentScheduleConfig,
}

impl PaymentScheduleJob {
    pub fn new(
        schedule_service: Arc<PaymentScheduleService>,
        notifier: Arc<dyn Notifier>,
        config: PaymentScheduleConfig,
    ) -> Self {
        Self {
            schedule_service,
            notifier,
            config,
        }
    }

    async fn process_due_occurrences(&self) {
        let now = OffsetDateTime::now_utc();
        for mut schedule in self
            .schedule_service
            .claim_due(self.config.retry_after())
            .await
        {
            let due_payments: Vec<(Payment, Confirmation)> = schedule
                .pending_occurrences(now)
                .into_iter()
                .map(|occurrence_date| {
                    let payment = Payment::new(
                        schedule.total.clone(),
                        PaymentSource::Schedule {
                            schedule_id: schedule.id,
                            occurrence_date,
                        },
                    );
                    let confirmation = Confirmation::new(ConfirmationSubject::Payment(payment.id));
                    (payment, confirmation)
                })
                .collect();

            if due_payments.is_empty() {
                // Nothing due this poll: just release the claim so the next poll can
                // reclaim it. `last_run_at` stays untouched, it only needs to advance
                // when a payment actually fires.
                schedule.release_claim();
                self.schedule_service.update(schedule).await;
                continue;
            }

            // Release the claim and record how far we've caught up, so the next poll
            // (however far in the future) only backfills what's genuinely new. Persisted
            // atomically with the payments themselves: a crash between the two would
            // otherwise let a retry recreate the same occurrence as a duplicate payment.
            schedule.finalize_processing();
            let (_, created) = self
                .schedule_service
                .finalize_with_payments(schedule, due_payments)
                .await;

            for (_, confirmation) in &created {
                self.notifier.request(confirmation).await;
            }
        }
    }
}

#[async_trait]
impl StartupTask for PaymentScheduleJob {
    fn name(&self) -> &str {
        "payment-schedules"
    }

    async fn run(&self) {
        loop {
            self.process_due_occurrences().await;
            tokio::time::sleep(self.config.poll_interval()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::money::Money;
    use crate::domain::payment::{PaymentService, PaymentSource};
    use crate::domain::payment_schedule::{
        PaymentSchedule, PaymentScheduleRepository, PaymentScheduleStatus, Recurrence,
    };
    use crate::infra::notifier::LoggingNotifier;
    use crate::infra::persistence::connect;
    use crate::infra::persistence::sqlx_payment_repository::SqlitePaymentRepository;
    use crate::infra::persistence::sqlx_payment_schedule_repository::SqlitePaymentScheduleRepository;
    use rusty_money::iso;

    async fn job() -> (
        PaymentScheduleJob,
        Arc<PaymentService>,
        Arc<PaymentScheduleService>,
    ) {
        let pool = connect("sqlite::memory:").await.unwrap();
        let payment_service = Arc::new(PaymentService::new(
            Arc::new(SqlitePaymentRepository::new(pool.clone())),
            Arc::new(LoggingNotifier),
        ));
        let schedule_service = Arc::new(PaymentScheduleService::new(Arc::new(
            SqlitePaymentScheduleRepository::new(pool),
        )));
        let job = PaymentScheduleJob::new(
            schedule_service.clone(),
            Arc::new(LoggingNotifier),
            PaymentScheduleConfig::default(),
        );
        (job, payment_service, schedule_service)
    }

    // Created "now" with day_of_month pinned to today: the only due occurrence is
    // today itself, so exactly one payment is expected regardless of what day the
    // test happens to run on (unlike a fixed backdate, which can straddle a
    // variable number of monthly occurrences depending on the calendar).
    fn due_schedule() -> PaymentSchedule {
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
    async fn processes_a_due_schedule_and_creates_a_scheduled_payment() {
        let (job, payment_service, schedule_service) = job().await;
        let seed = due_schedule();
        let schedule = schedule_service.create(seed.total, seed.recurrence).await;

        job.process_due_occurrences().await;

        let payments = payment_service.find_all().await;
        assert_eq!(payments.len(), 1);
        match &payments[0].source {
            PaymentSource::Schedule { schedule_id, .. } => assert_eq!(*schedule_id, schedule.id),
            other => panic!("expected a scheduled payment, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn second_run_creates_no_duplicate_payment_when_nothing_new_is_due() {
        let (job, payment_service, schedule_service) = job().await;
        let seed = due_schedule();
        schedule_service.create(seed.total, seed.recurrence).await;

        job.process_due_occurrences().await;
        job.process_due_occurrences().await;

        assert_eq!(payment_service.find_all().await.len(), 1);
    }

    #[tokio::test]
    async fn schedule_not_yet_due_creates_no_payment_and_leaves_last_run_at_untouched() {
        let pool = connect("sqlite::memory:").await.unwrap();
        let payment_service = Arc::new(PaymentService::new(
            Arc::new(SqlitePaymentRepository::new(pool.clone())),
            Arc::new(LoggingNotifier),
        ));
        let schedule_repo = Arc::new(SqlitePaymentScheduleRepository::new(pool));
        let schedule_service = Arc::new(PaymentScheduleService::new(schedule_repo.clone()));
        let job = PaymentScheduleJob::new(
            schedule_service.clone(),
            Arc::new(LoggingNotifier),
            PaymentScheduleConfig::default(),
        );

        // Backdated into the future relative to real "now" so `due_occurrences` has
        // nothing to report regardless of which day of the month this test runs on.
        let mut seed = PaymentSchedule::new(
            Money::from_minor(1099, iso::USD),
            Recurrence::EveryNMonths {
                interval_months: 1,
                day_of_month: 1,
            },
        );
        seed.created_at += time::Duration::days(60);
        seed.updated_at = seed.created_at;
        seed.next_due_at = seed.recurrence.next_occurrence_after(seed.created_at, None);
        let schedule = schedule_repo.create(seed).await;

        job.process_due_occurrences().await;

        assert_eq!(payment_service.find_all().await.len(), 0);
        let schedules = schedule_service.find_all().await;
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0].last_run_at, None);
        assert_eq!(schedules[0].status, PaymentScheduleStatus::Idle);
        // next_due_at keeps claim_due from touching this row at all, not just from
        // recording a run on it: version is untouched, not merely un-advanced.
        assert_eq!(schedules[0].version.as_u32(), schedule.version.as_u32());
    }
}
