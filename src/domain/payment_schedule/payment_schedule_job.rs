use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use time::{OffsetDateTime, SignedDuration};

use crate::domain::payment::{PaymentService, PaymentSource};
use crate::domain::payment_schedule::PaymentScheduleStatus;
use crate::domain::startup_task::StartupTask;

use super::PaymentScheduleService;

#[derive(Debug, Clone, PartialEq)]
pub struct PaymentScheduleJobConfig {
    pub poll_interval_secs: u64,
    pub retry_after_secs: u64,
}

impl Default for PaymentScheduleJobConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 1800,
            retry_after_secs: 1200,
        }
    }
}

impl PaymentScheduleJobConfig {
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_interval_secs)
    }

    pub fn retry_after(&self) -> SignedDuration {
        SignedDuration::seconds(self.retry_after_secs as i64)
    }
}

pub struct PaymentScheduleJob {
    schedule_service: Arc<PaymentScheduleService>,
    payment_service: Arc<PaymentService>,
    config: PaymentScheduleJobConfig,
}

impl PaymentScheduleJob {
    pub fn new(
        schedule_service: Arc<PaymentScheduleService>,
        payment_service: Arc<PaymentService>,
        config: PaymentScheduleJobConfig,
    ) -> Self {
        Self {
            schedule_service,
            payment_service,
            config,
        }
    }

    async fn process(&self) {
        let due_schedules = self
            .schedule_service
            .claim_due(self.config.retry_after())
            .await;


        if due_schedules.is_empty() {
            return;
        }

        tracing::info!("processing payment schedules");

        let now = OffsetDateTime::now_utc();
        let now_date = now.date();

        for mut schedule in due_schedules {
            // produce every occurrence due up to today (usually one, but catches up if the
            // daemon was down for a while), advancing next_due_at past each as it's produced.
            let mut sources = Vec::new();
            while schedule.next_due_at <= now_date {
                sources.push(PaymentSource::Schedule {
                    schedule_id: schedule.id,
                    occurrence_date: schedule.next_due_at,
                });
                schedule.advance_due_date();
            }

            let entries = sources
                .into_iter()
                .map(|source| (schedule.total.clone(), source))
                .collect();
            self.payment_service.create_all(entries).await;

            schedule.updated_at = OffsetDateTime::now_utc();
            schedule.status = PaymentScheduleStatus::Idle;
            self.schedule_service.update(schedule).await;
        }

        let elapsed = OffsetDateTime::now_utc() - now;
        tracing::info!(elapsed_ms = elapsed.whole_milliseconds(), "finished processing payment schedules");
    }
}

#[async_trait]
impl StartupTask for PaymentScheduleJob {
    fn name(&self) -> &str {
        "payment-schedules-processing-job"
    }

    async fn run(&self) {
        loop {
            self.process().await;
            tokio::time::sleep(self.config.poll_interval()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::money::Money;
    use crate::domain::payment::PaymentSource;
    use crate::domain::payment_schedule::{PaymentSchedule, PaymentScheduleStatus, Recurrence};
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
            payment_service.clone(),
            PaymentScheduleJobConfig::default(),
        );
        (job, payment_service, schedule_service)
    }

    // A freshly created schedule is never immediately due (`next_due_at` lands later this
    // month or next), so a "due" fixture has to backdate it directly through the service.
    async fn due_schedule(schedule_service: &PaymentScheduleService) -> PaymentSchedule {
        let created = schedule_service
            .create(
                Money::from_minor(1099, iso::USD),
                Recurrence::Monthly { day_of_month: 1 },
            )
            .await;
        schedule_service
            .update(PaymentSchedule {
                next_due_at: OffsetDateTime::now_utc().date(),
                ..created
            })
            .await
    }

    #[tokio::test]
    async fn processes_a_due_schedule_and_creates_a_scheduled_payment() {
        let (job, payment_service, schedule_service) = job().await;
        let schedule = due_schedule(&schedule_service).await;

        job.process().await;

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
        due_schedule(&schedule_service).await;

        job.process().await;
        job.process().await;

        assert_eq!(payment_service.find_all().await.len(), 1);
    }

    #[tokio::test]
    async fn schedule_not_yet_due_creates_no_payment_and_leaves_it_untouched() {
        let (job, payment_service, schedule_service) = job().await;
        let schedule = schedule_service
            .create(
                Money::from_minor(1099, iso::USD),
                Recurrence::Monthly { day_of_month: 1 },
            )
            .await;

        job.process().await;

        assert_eq!(payment_service.find_all().await.len(), 0);
        let schedules = schedule_service.find_all().await;
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0].status, PaymentScheduleStatus::Idle);
        assert_eq!(schedules[0].version, schedule.version);
    }
}
