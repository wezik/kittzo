use std::sync::Arc;

use async_trait::async_trait;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use super::confirmation::{Confirmation, ConfirmationSubject, Notifier};
use super::money::Money;
use super::version::Version;

#[derive(Clone, Debug, PartialEq)]
pub struct Payment {
    pub id: Uuid,
    pub version: Version,
    pub total: Money,
    pub created_at: OffsetDateTime,
    pub source: PaymentSource,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaymentSource {
    Manual,
    Schedule {
        schedule_id: Uuid,
        occurrence_date: Date,
    },
}

impl Payment {
    pub fn new(total: Money, source: PaymentSource) -> Self {
        Payment {
            id: Uuid::new_v4(),
            version: Version::FIRST,
            total,
            created_at: OffsetDateTime::now_utc(),
            source,
        }
    }
}

#[async_trait]
pub trait PaymentRepository: Send + Sync {
    /// Creates the `Payment` together with the pending `Confirmation` that gates it, in one
    /// atomic write. This is an infra implementation detail (a single `sqlx::Transaction`),
    /// not something callers need to manage.
    async fn create(&self, payment: Payment, confirmation: Confirmation)
    -> (Payment, Confirmation);
    async fn find_by_id(&self, id: Uuid) -> Option<Payment>;
    async fn find_all(&self) -> Vec<Payment>;
}

pub struct PaymentService {
    repo: Arc<dyn PaymentRepository>,
    notifier: Arc<dyn Notifier>,
}

impl PaymentService {
    pub fn new(repo: Arc<dyn PaymentRepository>, notifier: Arc<dyn Notifier>) -> Self {
        Self { repo, notifier }
    }

    /// Every new payment starts out gated behind a pending confirmation instead of
    /// being immediately active.
    pub async fn create(&self, total: Money, source: PaymentSource) -> Payment {
        let payment = Payment::new(total, source);
        let confirmation = Confirmation::new(ConfirmationSubject::Payment(payment.id));
        let (created, confirmation) = self.repo.create(payment, confirmation).await;
        self.notifier.request(&confirmation).await;
        tracing::info!(payment_id = %created.id, confirmation_id = %confirmation.id, "created payment pending confirmation");
        created
    }

    pub async fn find_by_id(&self, id: Uuid) -> Option<Payment> {
        self.repo.find_by_id(id).await
    }

    pub async fn find_all(&self) -> Vec<Payment> {
        self.repo.find_all().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::notifier::LoggingNotifier;
    use crate::infra::persistence::connect;
    use crate::infra::persistence::sqlx_payment_repository::SqlitePaymentRepository;
    use rusty_money::iso;

    #[test]
    fn new_assigns_distinct_ids() {
        let total = Money::from_minor(1, iso::USD);
        let a = Payment::new(total.clone(), PaymentSource::Manual);
        let b = Payment::new(total, PaymentSource::Manual);

        assert_ne!(a.id, b.id);
    }

    async fn service() -> PaymentService {
        let pool = connect("sqlite::memory:").await.unwrap();
        PaymentService::new(
            Arc::new(SqlitePaymentRepository::new(pool)),
            Arc::new(LoggingNotifier),
        )
    }

    #[tokio::test]
    async fn create_with_manual_source_returns_a_payment() {
        let service = service().await;
        let payment = service
            .create(Money::from_minor(500, iso::USD), PaymentSource::Manual)
            .await;

        assert_eq!(payment.source, PaymentSource::Manual);
        assert_eq!(service.find_by_id(payment.id).await, Some(payment));
    }

    #[tokio::test]
    async fn find_all_returns_every_created_payment() {
        let service = service().await;
        service
            .create(Money::from_minor(100, iso::USD), PaymentSource::Manual)
            .await;
        service
            .create(Money::from_minor(200, iso::USD), PaymentSource::Manual)
            .await;

        assert_eq!(service.find_all().await.len(), 2);
    }
}
