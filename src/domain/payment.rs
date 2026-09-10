use async_trait::async_trait;
use time::{Date, OffsetDateTime};
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
    Schedule { schedule_id: Uuid, due_date: Date },
    Ingested,
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusty_money::iso;

    #[test]
    fn new_assigns_distinct_ids() {
        let amount = Money::from_minor(1, iso::USD);
        let a = Payment::new(amount.clone(), PaymentSource::Manual);
        let b = Payment::new(amount, PaymentSource::Manual);

        assert_ne!(a.id, b.id);
    }
}
