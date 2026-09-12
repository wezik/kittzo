use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use time::{OffsetDateTime, SignedDuration};

use crate::domain::payment::{CreatePaymentCommand, PaymentService, PaymentSource};
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
            // produce every occurrence due up to today
            // usually one, but catches up if the daemon was down for a while,
            // advancing next_due_at past each as it's produced.
            while schedule.next_due_at <= now_date {
                let occurence_date = schedule.next_due_at;
                let source = PaymentSource::Schedule {
                    schedule_id: schedule.id,
                    occurrence_date: occurence_date,
                };

                let cmd = CreatePaymentCommand {
                    total: schedule.total,
                    source: source,
                    auto_ack: true, // scheduled payments by default dont require ack's
                };

                match self.payment_service.create(cmd).await {
                    Ok(_) => {
                        schedule.advance_due_date();
                    }
                    Err(err) => {
                        tracing::error!(
                            err = %err,
                           schedule_id = %schedule.id,
                           occurence_date = %occurence_date,
                           "failed to create scheduled payment"
                        );

                        break;
                    }
                }
            }

            schedule.updated_at = OffsetDateTime::now_utc();
            // set back to idle no matter the processing status, it is just a claim information
            // TODO: move away from status and identify more proper claiming model
            // status itself can be computed from rules at read time
            schedule.status = PaymentScheduleStatus::Idle;
            if let Err(err) = self.schedule_service.update(schedule).await {
                tracing::error!(%err, "failed to update payment schedule");
            }
        }

        let elapsed = OffsetDateTime::now_utc() - now;
        tracing::info!(
            elapsed_ms = elapsed.whole_milliseconds(),
            "finished processing payment schedules"
        );
    }
}

#[async_trait]
impl StartupTask for PaymentScheduleJob {
    fn name(&self) -> &str {
        "payment-schedules-job"
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
    use crate::domain::confirmation::ConfirmationService;
    use crate::domain::money::Money;
    use crate::domain::payment::PaymentSource;
    use crate::domain::payment_schedule::{PaymentSchedule, PaymentScheduleStatus, Recurrence};
    use crate::infra::notifier::LoggingNotifier;
    use crate::infra::persistence::connect;
    use crate::infra::persistence::sqlx_confirmation_repository::SqliteConfirmationRepository;
    use crate::infra::persistence::sqlx_payment_repository::SqlitePaymentRepository;
    use crate::infra::persistence::sqlx_payment_schedule_repository::SqlitePaymentScheduleRepository;

    async fn job() -> (
        PaymentScheduleJob,
        Arc<PaymentService>,
        Arc<PaymentScheduleService>,
    ) {
        let pool = connect("sqlite::memory:").await.unwrap();
        let confirmations = Arc::new(ConfirmationService::new(
            Arc::new(SqliteConfirmationRepository::new(pool.clone())),
            Arc::new(LoggingNotifier),
        ));
        let payment_service = Arc::new(PaymentService::new(
            Arc::new(SqlitePaymentRepository::new(pool.clone())),
            confirmations,
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
                Money::from_parts("USD", "10.99").unwrap(),
                Recurrence::Monthly { day_of_month: 1 },
            )
            .await;
        schedule_service
            .update(PaymentSchedule {
                next_due_at: OffsetDateTime::now_utc().date(),
                ..created
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn processes_a_due_schedule_and_creates_a_scheduled_payment() {
        let (job, payment_service, schedule_service) = job().await;
        let schedule = due_schedule(&schedule_service).await;

        job.process().await;

        let payments = payment_service.find_all().await.unwrap();
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

        assert_eq!(payment_service.find_all().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn schedule_not_yet_due_creates_no_payment_and_leaves_it_untouched() {
        let (job, payment_service, schedule_service) = job().await;
        let schedule = schedule_service
            .create(
                Money::from_parts("USD", "10.99").unwrap(),
                Recurrence::Monthly { day_of_month: 1 },
            )
            .await;

        job.process().await;

        assert_eq!(payment_service.find_all().await.unwrap().len(), 0);
        let schedules = schedule_service.find_all().await;
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0].status, PaymentScheduleStatus::Idle);
        assert_eq!(schedules[0].version, schedule.version);
    }
}
