use std::sync::Arc;

use time::SignedDuration;
use uuid::Uuid;

use crate::domain::money::Money;
use crate::domain::payment::Details;
use crate::infra::persistence;

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
    Persistence(#[from] persistence::Error),
}

impl PaymentScheduleService {
    pub fn new(repo: Arc<dyn PaymentScheduleRepository>) -> Self {
        Self { repo }
    }

    pub async fn create(
        &self,
        total: Money,
        details: Details,
        recurrence: Recurrence,
    ) -> Result<PaymentSchedule, PaymentScheduleError> {
        let schedule = self
            .repo
            .create(PaymentSchedule::new(total, details, recurrence))
            .await?;

        tracing::info!(schedule_id = %schedule.id, "created payment schedule");
        Ok(schedule)
    }

    pub async fn find_all(&self) -> Result<Vec<PaymentSchedule>, PaymentScheduleError> {
        self.repo.find_all().await
    }

    pub async fn edit(
        &self,
        id: Uuid,
        total: Money,
        details: Details,
        recurrence: Recurrence,
    ) -> Result<PaymentSchedule, PaymentScheduleError> {
        let mut schedule = self
            .repo
            .find_all()
            .await?
            .into_iter()
            .find(|schedule| schedule.id == id)
            .ok_or(PaymentScheduleError::NotFound)?;
        schedule.edit(total, details, recurrence)?;
        let updated = self.repo.update(schedule).await?;
        tracing::info!(schedule_id = %id, "edited payment schedule");
        Ok(updated)
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), PaymentScheduleError> {
        self.repo.delete(id).await?;
        tracing::info!(schedule_id = %id, "deleted payment schedule");
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::payment::some_standard_details;
    use crate::infra::persistence::connect;
    use crate::infra::persistence::sqlx_payment_schedule_repository::SqlitePaymentScheduleRepository;

    async fn service() -> PaymentScheduleService {
        let pool = connect("sqlite::memory:").await.unwrap();
        PaymentScheduleService::new(Arc::new(SqlitePaymentScheduleRepository::new(pool)))
    }

    fn pln(amount: &str) -> Money {
        Money::from_parts("PLN", amount).unwrap()
    }

    #[tokio::test]
    async fn edit_persists_the_new_values() {
        let service = service().await;
        let schedule = service
            .create(
                pln("2700.00"),
                some_standard_details(),
                Recurrence::Monthly { day_of_month: 5 },
            )
            .await
            .unwrap();

        let edited = service
            .edit(
                schedule.id,
                pln("27000.00"),
                some_standard_details(),
                Recurrence::Monthly { day_of_month: 7 },
            )
            .await
            .unwrap();

        let stored = service.find_all().await.unwrap();
        assert_eq!(stored, vec![edited.clone()]);
        assert_eq!(edited.total, pln("27000.00"));
        assert_eq!(edited.recurrence, Recurrence::Monthly { day_of_month: 7 });
        assert_eq!(edited.next_due_at.month(), schedule.next_due_at.month());
        assert_eq!(edited.next_due_at.day(), 7);
    }

    #[tokio::test]
    async fn edit_of_a_missing_schedule_is_not_found() {
        let service = service().await;
        let err = service
            .edit(
                Uuid::new_v4(),
                pln("1.00"),
                some_standard_details(),
                Recurrence::Monthly { day_of_month: 1 },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, PaymentScheduleError::NotFound));
    }
}
