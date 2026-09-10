use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;

use crate::domain::payment::{PaymentService, PaymentSource};
use crate::domain::startup_task::StartupTask;

use super::{PaymentScheduleConfig, PaymentScheduleService};

pub struct PaymentScheduleJob {
    payment_service: Arc<PaymentService>,
    schedule_service: Arc<PaymentScheduleService>,
    config: PaymentScheduleConfig,
}

impl PaymentScheduleJob {
    pub fn new(
        payment_service: Arc<PaymentService>,
        schedule_service: Arc<PaymentScheduleService>,
        config: PaymentScheduleConfig,
    ) -> Self {
        Self {
            payment_service,
            schedule_service,
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
            for occurrence_date in schedule.pending_occurrences(now) {
                let source = PaymentSource::Schedule {
                    schedule_id: schedule.id,
                    occurrence_date,
                };
                self.payment_service
                    .create(schedule.total.clone(), source)
                    .await;
            }

            // release the claim and record how far we've caught up, so the next
            // poll (however far in the future) only backfills what's genuinely new.
            schedule.finalize_processing();
            self.schedule_service.update(schedule).await;
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
    use crate::domain::payment_schedule::{PaymentSchedule, Recurrence};
    use crate::infra::persistence::connect;
    use crate::infra::persistence::sqlx_payment_repository::SqlitePaymentRepository;
    use crate::infra::persistence::sqlx_payment_schedule_repository::SqlitePaymentScheduleRepository;
    use rusty_money::iso;

    async fn job() -> (PaymentScheduleJob, Arc<PaymentService>, Arc<PaymentScheduleService>) {
        let pool = connect("sqlite::memory:").await.unwrap();
        let payment_service = Arc::new(PaymentService::new(Arc::new(SqlitePaymentRepository::new(
            pool.clone(),
        ))));
        let schedule_service = Arc::new(PaymentScheduleService::new(Arc::new(
            SqlitePaymentScheduleRepository::new(pool),
        )));
        let job = PaymentScheduleJob::new(
            payment_service.clone(),
            schedule_service.clone(),
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
}
