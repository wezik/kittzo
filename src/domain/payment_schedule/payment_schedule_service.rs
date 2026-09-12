use std::sync::Arc;

use time::SignedDuration;

use crate::domain::money::Money;

use super::{PaymentSchedule, PaymentScheduleRepository, Recurrence};

pub struct PaymentScheduleService {
    repo: Arc<dyn PaymentScheduleRepository>,
}
#[derive(Debug, thiserror::Error)]
pub enum PaymentScheduleError {}

impl PaymentScheduleService {
    pub fn new(repo: Arc<dyn PaymentScheduleRepository>) -> Self {
        Self { repo }
    }

    pub async fn create(&self, total: Money, recurrence: Recurrence) -> PaymentSchedule {
        let schedule = PaymentSchedule::new(total, recurrence);
        let created = self.repo.create(schedule).await;
        tracing::info!(schedule_id = %created.id, "created payment schedule");
        created
    }

    pub async fn find_all(&self) -> Vec<PaymentSchedule> {
        self.repo.find_all().await
    }

    pub async fn claim_due(&self, retry_after: SignedDuration) -> Vec<PaymentSchedule> {
        self.repo.claim_due(retry_after).await
    }

    pub async fn update(
        &self,
        schedule: PaymentSchedule,
    ) -> Result<PaymentSchedule, PaymentScheduleError> {
        // TODO: Error handling
        let updated = self.repo.update(schedule).await;
        tracing::info!(schedule_id = %updated.id, "updated payment schedule");
        Ok(updated)
    }
}
