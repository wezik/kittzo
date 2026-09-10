use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use super::money::Money;
use super::version::Version;

#[derive(Clone, Debug, PartialEq)]
pub struct Payment {
    pub id: Uuid,
    pub version: Version,
    pub amount: Money,
    pub created_at: OffsetDateTime,
    pub source: PaymentSource,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaymentSource {
    Manual,
}

impl Payment {
    pub fn new(amount: Money, source: PaymentSource) -> Self {
        Payment {
            id: Uuid::new_v4(),
            version: Version::FIRST,
            amount,
            created_at: OffsetDateTime::now_utc(),
            source,
        }
    }
}

#[async_trait]
pub trait PaymentRepository: Send + Sync {
    async fn create(&self, payment: Payment) -> Payment;
    async fn find_by_id(&self, id: Uuid) -> Option<Payment>;
    async fn find_all(&self) -> Vec<Payment>;
}

pub struct PaymentService {
    repo: Arc<dyn PaymentRepository>,
}

impl PaymentService {
    pub fn new(repo: Arc<dyn PaymentRepository>) -> Self {
        Self { repo }
    }

    pub async fn ingest(&self, amount: Money) -> Payment {
        let payment = Payment::new(amount, PaymentSource::Manual);
        let created = self.repo.create(payment).await;
        tracing::info!(payment_id = %created.id, "ingested payment");
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
    use crate::infra::persistence::connect;
    use crate::infra::persistence::sqlx_payment_repository::SqlitePaymentRepository;
    use rusty_money::iso;

    #[test]
    fn new_assigns_distinct_ids() {
        let amount = Money::from_minor(1, iso::USD);
        let a = Payment::new(amount.clone(), PaymentSource::Manual);
        let b = Payment::new(amount, PaymentSource::Manual);

        assert_ne!(a.id, b.id);
    }

    async fn service() -> PaymentService {
        let pool = connect("sqlite::memory:").await.unwrap();
        PaymentService::new(Arc::new(SqlitePaymentRepository::new(pool)))
    }

    #[tokio::test]
    async fn ingest_creates_and_returns_a_payment() {
        let service = service().await;
        let payment = service.ingest(Money::from_minor(500, iso::USD)).await;

        assert_eq!(payment.source, PaymentSource::Manual);
        assert_eq!(service.find_by_id(payment.id).await, Some(payment));
    }

    #[tokio::test]
    async fn find_all_returns_every_ingested_payment() {
        let service = service().await;
        service.ingest(Money::from_minor(100, iso::USD)).await;
        service.ingest(Money::from_minor(200, iso::USD)).await;

        assert_eq!(service.find_all().await.len(), 2);
    }
}
