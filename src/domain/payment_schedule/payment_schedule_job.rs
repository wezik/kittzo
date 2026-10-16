use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use time::OffsetDateTime;

use crate::domain::startup_task::StartupTask;

use super::PaymentScheduleService;

#[derive(Debug, Clone, PartialEq)]
pub struct PaymentScheduleJobConfig {
    pub poll_interval_secs: u64,
}

impl Default for PaymentScheduleJobConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 1800,
        }
    }
}

impl PaymentScheduleJobConfig {
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_interval_secs)
    }
}

pub struct PaymentScheduleJob {
    schedule_service: Arc<PaymentScheduleService>,
    config: PaymentScheduleJobConfig,
}

impl PaymentScheduleJob {
    pub fn new(
        schedule_service: Arc<PaymentScheduleService>,
        config: PaymentScheduleJobConfig,
    ) -> Self {
        Self {
            schedule_service,
            config,
        }
    }
}

#[async_trait]
impl StartupTask for PaymentScheduleJob {
    fn name(&self) -> &str {
        "payment-schedules-job"
    }

    async fn run(&self) {
        loop {
            let today = OffsetDateTime::now_utc().date();
            match self.schedule_service.process_due(today).await {
                Ok(0) => tracing::debug!("no due payment schedules"),
                Ok(created) => tracing::info!(created, "created scheduled payments"),
                Err(err) => tracing::error!(%err, "failed to process payment schedules"),
            }
            tokio::time::sleep(self.config.poll_interval()).await;
        }
    }
}
