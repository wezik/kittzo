use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{
    domain::{
        money::Money,
        payment::{Details, Error, Payment, Source},
        transaction::{commit_tx, start_tx},
    },
    infra::persistence::{sqlx_outbox_repository, sqlx_payment_repository},
};

pub struct Service {
    pool: SqlitePool,
}

pub struct ListQuery {
    /// None means all
    pub paid_filter: Option<ListQueryPaidFilter>,
    /// None means all
    pub ack_filter: Option<ListQueryAckFilter>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ListQueryPaidFilter {
    Paid,
    Unpaid,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ListQueryAckFilter {
    Pending,
    Accepted,
}

impl Service {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn list(&self, query: Option<ListQuery>) -> Result<Vec<Payment>, Error> {
        sqlx_payment_repository::find_all(&self.pool, query).await
    }

    pub async fn has_pending_ack(&self) -> Result<bool, Error> {
        sqlx_payment_repository::has_pending_ack(&self.pool).await
    }

    pub async fn create(
        &self,
        total: Money,
        details: Details,
        source: Source,
    ) -> Result<Payment, Error> {
        let (payment, event) = Payment::new(total, details, source);

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
