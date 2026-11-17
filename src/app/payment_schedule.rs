use sqlx::SqlitePool;
use time::Date;
use uuid::Uuid;

use crate::app::transaction::{commit_tx, start_tx};
use crate::domain::money::Money;
use crate::domain::payment::{self, AccountNumber};
use crate::domain::payment_schedule::{PaymentSchedule, Recurrence};
use crate::infra::persistence::{
    self, sqlx_outbox_repository, sqlx_payment_repository, sqlx_payment_schedule_repository,
};

pub struct Service {
    pool: SqlitePool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("payment schedule already exists")]
    AlreadyExists,

    #[error("payment schedule not found")]
    NotFound,

    #[error("payment schedule updated concurrently")]
    ConcurrentUpdate,

    #[error(transparent)]
    Payment(#[from] payment::Error),

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

impl Service {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        total: Money,
        title_template: String,
        account_number: AccountNumber,
        recipient: String,
        recurrence: Recurrence,
        starts_at: Date,
    ) -> Result<PaymentSchedule, Error> {
        let schedule = PaymentSchedule::new(
            total,
            title_template,
            account_number,
            recipient,
            recurrence,
            starts_at,
        )?;
        Ok(sqlx_payment_schedule_repository::create(&self.pool, &schedule).await?)
    }

    pub async fn find_all(&self) -> Result<Vec<PaymentSchedule>, Error> {
        Ok(sqlx_payment_schedule_repository::find_all(&self.pool).await?)
    }

    pub async fn import(&self, schedules: Vec<PaymentSchedule>) -> Result<usize, Error> {
        let mut tx = start_tx(&self.pool).await?;
        let mut imported = 0;
        for schedule in &schedules {
            match sqlx_payment_schedule_repository::create(&mut *tx, schedule).await {
                Ok(_) => imported += 1,
                Err(persistence::Error::AlreadyExists) => {}
                Err(err) => return Err(err.into()),
            }
        }
        commit_tx(tx).await?;
        Ok(imported)
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), Error> {
        Ok(sqlx_payment_schedule_repository::delete(&self.pool, id).await?)
    }

    pub async fn process_due(&self, today: Date) -> Result<usize, Error> {
        let due = sqlx_payment_schedule_repository::find_due(&self.pool, today).await?;
        let mut created = 0;
        for schedule in due {
            let id = schedule.id;
            match self.process(schedule, today).await {
                Ok(count) => created += count,
                Err(err) => tracing::error!(%err, schedule_id = %id, "failed to process schedule"),
            }
        }
        Ok(created)
    }

    async fn process(&self, mut schedule: PaymentSchedule, today: Date) -> Result<usize, Error> {
        let produced = schedule.produce(today)?;
        let count = produced.len();

        let mut tx = start_tx(&self.pool).await?;
        for (payment, event) in produced {
            sqlx_payment_repository::create(&mut *tx, &payment).await?;
            sqlx_outbox_repository::create(&mut *tx, event).await?;
        }
        sqlx_payment_schedule_repository::update(&mut tx, &schedule).await?;
        commit_tx(tx).await?;

        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use time::{Duration, OffsetDateTime};

    use super::*;
    use crate::domain::payment::Source;
    use crate::domain::payment_schedule::some_schedule;
    use crate::infra::persistence::connect;

    async fn setup() -> (Service, SqlitePool) {
        let pool = connect("sqlite::memory:").await.unwrap();
        (Service::new(pool.clone()), pool)
    }

    fn today() -> Date {
        OffsetDateTime::now_utc().date()
    }

    async fn seed(pool: &SqlitePool, next_due_at: Date) -> PaymentSchedule {
        let schedule = PaymentSchedule {
            next_due_at,
            ..some_schedule("Czynsz {MM}/{YYYY}", 1)
        };
        sqlx_payment_schedule_repository::create(pool, &schedule)
            .await
            .unwrap()
    }

    async fn payments(pool: &SqlitePool) -> Vec<payment::Payment> {
        sqlx_payment_repository::find_all(pool, payment::ListQuery::default())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn due_schedule_creates_a_payment_and_advances() {
        let (service, pool) = setup().await;
        let schedule = seed(&pool, today()).await;

        let created = service.process_due(today()).await.unwrap();

        assert_eq!(created, 1);
        let payments = payments(&pool).await;
        assert_eq!(payments.len(), 1);
        assert_eq!(payments[0].details, schedule.next_details);
        assert!(matches!(
            payments[0].source,
            Source::Schedule { schedule_id, occurrence_date }
                if schedule_id == schedule.id && occurrence_date == today()
        ));
        let stored = service.find_all().await.unwrap();
        assert!(stored[0].next_due_at > today());
        assert_eq!(stored[0].version, schedule.version.next());
    }

    #[tokio::test]
    async fn second_run_creates_no_duplicate() {
        let (service, pool) = setup().await;
        seed(&pool, today()).await;

        service.process_due(today()).await.unwrap();
        service.process_due(today()).await.unwrap();

        assert_eq!(payments(&pool).await.len(), 1);
    }

    #[tokio::test]
    async fn schedule_not_yet_due_is_left_untouched() {
        let (service, pool) = setup().await;
        let schedule = seed(&pool, today() + Duration::days(60)).await;

        assert_eq!(service.process_due(today()).await.unwrap(), 0);

        assert!(payments(&pool).await.is_empty());
        assert_eq!(service.find_all().await.unwrap(), vec![schedule]);
    }

    #[tokio::test]
    async fn lost_version_race_rolls_back_the_payments() {
        let (service, pool) = setup().await;
        let schedule = seed(&pool, today()).await;
        let mut conn = pool.acquire().await.unwrap();
        sqlx_payment_schedule_repository::update(&mut conn, &schedule)
            .await
            .unwrap();
        drop(conn);

        let err = service.process(schedule, today()).await.unwrap_err();

        assert!(matches!(err, Error::ConcurrentUpdate));
        assert!(payments(&pool).await.is_empty());
    }

    #[tokio::test]
    async fn create_rejects_a_blank_recipient() {
        let (service, _) = setup().await;
        let err = service
            .create(
                Money::from_parts("PLN", "1.00").unwrap(),
                "foo".into(),
                AccountNumber::try_from("61109010140000071219812874".to_string()).unwrap(),
                String::new(),
                Recurrence::Monthly { day_of_month: 1 },
                today(),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            Error::Payment(payment::Error::InvalidRecipient)
        ));
    }

    #[tokio::test]
    async fn import_skips_existing_ids() {
        let (service, pool) = setup().await;
        let existing = seed(&pool, today()).await;
        let new = some_schedule("foo", 5);

        let imported = service
            .import(vec![existing.clone(), new.clone()])
            .await
            .unwrap();

        assert_eq!(imported, 1);
        assert_eq!(service.find_all().await.unwrap(), vec![existing, new]);
    }
}
