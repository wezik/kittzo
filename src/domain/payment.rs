use std::sync::Arc;

use async_trait::async_trait;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::domain::confirmation::{ConfirmationError, CreateConfirmationCommand};

use super::confirmation::{ConfirmationService, ConfirmationSubject};
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

#[derive(Debug, thiserror::Error)]
pub enum PaymentError {
    #[error("payment already exists")]
    AlreadyExists,

    #[error("failed to create ack request")]
    FailedToCreateAckRequest(#[from] ConfirmationError),

    #[error("unexpected persistence error")]
    Persistence,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait PaymentRepository: Send + Sync {
    async fn create(&self, payment: Payment) -> Result<Payment, PaymentError>;
    async fn find_by_id(&self, id: Uuid) -> Result<Option<Payment>, PaymentError>;
    async fn find_all(&self) -> Result<Vec<Payment>, PaymentError>;
}

pub struct CreatePaymentCommand {
    pub total: Money,
    pub source: PaymentSource,
    pub auto_ack: bool,
}

pub struct PaymentService {
    repo: Arc<dyn PaymentRepository>,
    confirmations: Arc<ConfirmationService>,
}

impl PaymentService {
    pub fn new(repo: Arc<dyn PaymentRepository>, confirmations: Arc<ConfirmationService>) -> Self {
        Self {
            repo,
            confirmations,
        }
    }

    pub async fn create(&self, cmd: CreatePaymentCommand) -> Result<Payment, PaymentError> {
        let entity = Payment::new(cmd.total, cmd.source);
        let payment = self.repo.create(entity).await?;

        // SYNC CONFIRMATIONS
        // TODO: This part is destined to be hidden behind an event / eventbus?
        // the shape is still to be decided and the op will be performed asnchronously.
        let confirmation_cmd = CreateConfirmationCommand {
            subject: ConfirmationSubject::Payment(payment.id),
            automatic_confirmation: cmd.auto_ack,
        };

        self.confirmations.create(confirmation_cmd).await?;
        // SYNC CONFIRMATIONS

        tracing::info!(payment_id = %payment.id, "created payment");
        Ok(payment)
    }

    pub async fn find_by_id(&self, id: Uuid) -> Result<Option<Payment>, PaymentError> {
        self.repo.find_by_id(id).await
    }

    pub async fn find_all(&self) -> Result<Vec<Payment>, PaymentError> {
        self.repo.find_all().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::confirmation::{MockConfirmationRepository, MockNotifier};

    fn usd(amount: &str) -> Money {
        Money::from_parts("USD", amount).unwrap()
    }

    fn some_payment() -> Payment {
        Payment::new(usd("10.00"), PaymentSource::Manual)
    }

    fn service(repo: MockPaymentRepository) -> PaymentService {
        // So far mocked directly in here, but in the desired shape confirmations
        // are sent to the outbound through an event layer, which would get mocked.
        let mut confirmation_repo = MockConfirmationRepository::new();
        confirmation_repo
            .expect_create()
            .returning(|confirmation| confirmation);

        let confirmations = Arc::new(ConfirmationService::new(
            Arc::new(confirmation_repo),
            Arc::new(MockNotifier::new()),
        ));
        PaymentService::new(Arc::new(repo), confirmations)
    }

    #[tokio::test]
    async fn create_returns_the_created_payment() {
        // given
        let expected = some_payment();
        let returned = expected.clone();
        let mut repo = MockPaymentRepository::new();
        repo.expect_create()
            .returning(move |_| Ok(returned.clone()));

        // when
        let payment = service(repo)
            .create(CreatePaymentCommand {
                total: usd("5.00"),
                source: PaymentSource::Manual,
                auto_ack: true,
            })
            .await
            .unwrap();

        // then
        assert_eq!(payment.id, expected.id);
        assert_eq!(payment.source, expected.source);
    }

    #[tokio::test]
    async fn find_by_id_returns_payment() {
        // given
        let expected = some_payment();
        let returned = expected.clone();
        let mut repo = MockPaymentRepository::new();
        repo.expect_find_by_id()
            .returning(move |_| Ok(Some(returned.clone())));

        // when
        let payment = service(repo).find_by_id(expected.id).await.unwrap();

        // then
        assert_eq!(payment.unwrap().id, expected.id);
    }

    #[tokio::test]
    async fn find_all_returns_all_payments() {
        // given
        let expected = vec![some_payment(), some_payment()];

        let mut repo = MockPaymentRepository::new();
        let returned = expected.clone();
        repo.expect_find_all()
            .returning(move || Ok(returned.clone()));

        // when
        let payments = service(repo).find_all().await.unwrap();

        // then
        assert_eq!(payments.len(), 2);
        assert_eq!(payments[0].id, expected[0].id);
        assert_eq!(payments[1].id, expected[1].id);
    }
}
