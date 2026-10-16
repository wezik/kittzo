use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{
    app::transaction::{commit_tx, start_tx},
    domain::{
        money::Money,
        payment::{self, Details, ListQuery, Payment, Source},
    },
    infra::persistence::{self, sqlx_outbox_repository, sqlx_payment_repository},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Domain(#[from] payment::Error),

    #[error("payment already exists")]
    AlreadyExists,

    #[error("payment not found")]
    NotFound,

    #[error("payment updated concurrently")]
    ConcurrentUpdate,

    #[error("unexpected persistence error")]
    Persistence,
}

impl From<persistence::Error> for Error {
    fn from(err: persistence::Error) -> Self {
        match err {
            persistence::Error::AlreadyExists => Error::AlreadyExists,
            persistence::Error::NotFound => Error::NotFound,
            persistence::Error::ConcurrentUpdate => Error::ConcurrentUpdate,
            persistence::Error::Unexpected => Error::Persistence,
        }
    }
}

pub struct Service {
    pool: SqlitePool,
}

impl Service {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn list(&self, query: Option<ListQuery>) -> Result<Vec<Payment>, Error> {
        Ok(sqlx_payment_repository::find_all(&self.pool, query).await?)
    }

    pub async fn has_pending_ack(&self) -> Result<bool, Error> {
        Ok(sqlx_payment_repository::has_pending_ack(&self.pool).await?)
    }

    pub async fn create(
        &self,
        total: Money,
        details: Details,
        source: Source,
    ) -> Result<Payment, Error> {
        let (payment, event) = Payment::new(total, details, source);
        self.insert(payment, event).await
    }

    pub async fn insert(&self, payment: Payment, event: payment::Event) -> Result<Payment, Error> {
        let mut tx = start_tx(&self.pool).await?;

        let payment = sqlx_payment_repository::create(&mut *tx, &payment).await?;
        sqlx_outbox_repository::create(&mut *tx, event).await?;

        commit_tx(tx).await?;
        Ok(payment)
    }

    pub async fn pay(&self, id: Uuid) -> Result<Payment, Error> {
        let mut payment = sqlx_payment_repository::find(&self.pool, id).await?;
        let event = payment.pay()?;

        let mut tx = start_tx(&self.pool).await?;

        let payment = sqlx_payment_repository::update(&mut *tx, &payment).await?;
        sqlx_outbox_repository::create(&mut *tx, event).await?;

        commit_tx(tx).await?;
        Ok(payment)
    }

    pub async fn ack(&self, id: Uuid, accepted: bool) -> Result<Payment, Error> {
        let mut payment = sqlx_payment_repository::find(&self.pool, id).await?;
        let event = payment.ack(accepted)?;

        let mut tx = start_tx(&self.pool).await?;

        let payment = sqlx_payment_repository::update(&mut *tx, &payment).await?;
        sqlx_outbox_repository::create(&mut *tx, event).await?;

        commit_tx(tx).await?;
        Ok(payment)
    }
}
