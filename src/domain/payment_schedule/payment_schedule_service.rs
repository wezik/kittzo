use std::sync::Arc;

use time::SignedDuration;

use crate::domain::money::Money;

use super::{PaymentSchedule, PaymentScheduleRepository, Recurrence};

pub struct PaymentScheduleService {
    repo: Arc<dyn PaymentScheduleRepository>,
}
#[derive(Debug, thiserror::Error)]
pub enum PaymentScheduleError {
    #[error("payment schedule already exists")]
    AlreadyExists,

    #[error("payment schedule not found")]
    NotFound,

    #[error("payment schedule updated concurrently")]
    ConcurrentUpdate,

    #[error("unexpected persistence error")]
    Persistence,
}

impl PaymentScheduleService {
    pub fn new(repo: Arc<dyn PaymentScheduleRepository>) -> Self {
        Self { repo }
    }

    pub async fn create(
        &self,
        total: Money,
        recurrence: Recurrence,
    ) -> Result<PaymentSchedule, PaymentScheduleError> {
        let schedule = self
            .repo
            .create(PaymentSchedule::new(total, recurrence))
            .await?;

        tracing::info!(schedule_id = %schedule.id, "created payment schedule");
        Ok(schedule)
    }

    pub async fn find_all(&self) -> Result<Vec<PaymentSchedule>, PaymentScheduleError> {
        self.repo.find_all().await
    }

    pub async fn claim_due(
        &self,
        retry_after: SignedDuration,
    ) -> Result<Vec<PaymentSchedule>, PaymentScheduleError> {
        let claimed = self.repo.claim_due(retry_after).await?;
        tracing::debug!(due = %claimed.len(), "claimed due payment schedules");
        Ok(claimed)
    }

    pub async fn update(
        &self,
        schedule: PaymentSchedule,
    ) -> Result<PaymentSchedule, PaymentScheduleError> {
        let updated = self.repo.update(schedule).await?;
        tracing::info!(schedule_id = %updated.id, "updated payment schedule");
        Ok(updated)
    }
}
