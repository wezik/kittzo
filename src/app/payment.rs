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

    #[error(transparent)]
    Persistence(persistence::Error),
}

impl From<persistence::Error> for Error {
    fn from(err: persistence::Error) -> Self {
        match err {
            persistence::Error::AlreadyExists => Error::AlreadyExists,
            persistence::Error::NotFound => Error::NotFound,
            persistence::Error::ConcurrentUpdate => Error::ConcurrentUpdate,
            err => Error::Persistence(err),
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

    pub async fn list(&self, query: ListQuery) -> Result<Vec<Payment>, Error> {
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

    pub async fn import(&self, payments: Vec<Payment>) -> Result<usize, Error> {
        let mut tx = start_tx(&self.pool).await?;
        let mut imported = 0;
        for payment in &payments {
            match sqlx_payment_repository::create(&mut *tx, payment).await {
                Ok(_) => {
                    let event = payment::Event::Created { id: payment.id };
                    sqlx_outbox_repository::create(&mut *tx, event).await?;
                    imported += 1;
                }
                Err(persistence::Error::AlreadyExists) => {}
                Err(err) => return Err(err.into()),
            }
        }
        commit_tx(tx).await?;
        Ok(imported)
    }

    pub async fn pay(&self, id: Uuid) -> Result<Payment, Error> {
        let mut payment = sqlx_payment_repository::find(&self.pool, id).await?;
        let event = payment.pay()?;

        let mut tx = start_tx(&self.pool).await?;

        let payment = sqlx_payment_repository::update(&mut tx, &payment).await?;
        sqlx_outbox_repository::create(&mut *tx, event).await?;

        commit_tx(tx).await?;
        Ok(payment)
    }

    pub async fn ack(&self, id: Uuid, accepted: bool) -> Result<Payment, Error> {
        let mut payment = sqlx_payment_repository::find(&self.pool, id).await?;
        let event = payment.acknowledge(accepted)?;

        let mut tx = start_tx(&self.pool).await?;

        let payment = sqlx_payment_repository::update(&mut tx, &payment).await?;
        sqlx_outbox_repository::create(&mut *tx, event).await?;

        commit_tx(tx).await?;
        Ok(payment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::payment::{State, some_payment};
    use crate::infra::persistence::connect;
    use time::SignedDuration;

    #[tokio::test]
    async fn import_keeps_state_and_skips_existing_ids() {
        let pool = connect("sqlite::memory:").await.unwrap();
        let service = Service::new(pool.clone());
        let existing = service
            .insert(some_payment(), payment::Event::Created { id: Uuid::nil() })
            .await
            .unwrap();
        let mut paid = some_payment();
        paid.pay().unwrap();

        let imported = service
            .import(vec![existing.clone(), paid.clone()])
            .await
            .unwrap();

        assert_eq!(imported, 1);
        let stored = service.list(ListQuery::default()).await.unwrap();
        assert_eq!(stored, vec![existing, paid.clone()]);
        assert!(matches!(stored[1].state(), State::Paid { .. }));
        let events = sqlx_outbox_repository::claim_batch(&pool, SignedDuration::ZERO, 10)
            .await
            .unwrap();
        assert_eq!(
            events
                .into_iter()
                .map(|e| e.aggregate_id)
                .collect::<Vec<_>>(),
            vec![Uuid::nil().to_string(), paid.id.to_string()]
        );
    }
}
