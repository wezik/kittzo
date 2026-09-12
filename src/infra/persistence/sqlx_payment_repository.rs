use async_trait::async_trait;
use sqlx::SqlitePool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::domain::money::Money;
use crate::domain::payment::{Payment, PaymentError, PaymentRepository, PaymentSource};
use crate::domain::version::Version;

pub struct SqlitePaymentRepository {
    pool: SqlitePool,
}

impl SqlitePaymentRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

// sqlite's `Uuid` decode expects a 16-byte BLOB, but ids are stored as TEXT (`.to_string()`),
// so `id`/`schedule_id` are decoded as `String` here and parsed by hand in `row_to_payment`.
#[derive(sqlx::FromRow)]
struct PaymentRow {
    id: String,
    version: i64,
    total_amount: String,
    total_currency: String,
    created_at: OffsetDateTime,
    source_type: String,
    schedule_id: Option<String>,
    occurrence_date: Option<Date>,
}

#[async_trait]
impl PaymentRepository for SqlitePaymentRepository {
    async fn create(&self, payment: Payment) -> Result<Payment, PaymentError> {
        let (source_name, schedule_id, occurrence_date) = match payment.source {
            PaymentSource::Manual => ("manual", None, None),
            PaymentSource::Schedule {
                schedule_id,
                occurrence_date,
            } => (
                "schedule",
                Some(schedule_id.to_string()),
                Some(occurrence_date),
            ),
        };

        let row = sqlx::query_as::<_, PaymentRow>(
            "
            INSERT INTO payments (
                id,
                version,
                total_amount,
                total_currency,
                created_at,
                source_type,
                schedule_id,
                occurrence_date
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO NOTHING
            RETURNING
                id,
                version,
                total_amount,
                total_currency,
                created_at,
                source_type,
                schedule_id,
                occurrence_date
            ",
        )
        .bind(payment.id.to_string())
        .bind(Version::FIRST.as_u32() as i64)
        .bind(payment.total.amount().to_string())
        .bind(payment.total.currency_code())
        .bind(payment.created_at)
        .bind(source_name)
        .bind(schedule_id)
        .bind(occurrence_date)
        .fetch_optional(&self.pool)
        .await
        .map_err(|sqlx_err| {
            tracing::error!(%sqlx_err, "unexpected sqlx error");
            PaymentError::Persistence
        })?
        .ok_or(PaymentError::AlreadyExists)?;

        row_to_payment(row)
    }

    async fn find_by_id(&self, id: Uuid) -> Result<Option<Payment>, PaymentError> {
        sqlx::query_as::<_, PaymentRow>("SELECT * FROM payments WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|sqlx_err| {
                tracing::error!(%sqlx_err, "unexpected sqlx error");
                PaymentError::Persistence
            })?
            .map(row_to_payment)
            .transpose()
    }

    async fn find_all(&self) -> Result<Vec<Payment>, PaymentError> {
        sqlx::query_as::<_, PaymentRow>("SELECT * FROM payments ORDER BY created_at")
            .fetch_all(&self.pool)
            .await
            .map_err(|sqlx_err| {
                tracing::error!(%sqlx_err, "unexpected sqlx error");
                PaymentError::Persistence
            })?
            .into_iter()
            .map(row_to_payment)
            .collect()
    }
}

fn row_to_payment(row: PaymentRow) -> Result<Payment, PaymentError> {
    let id = Uuid::parse_str(&row.id).map_err(|err| {
        tracing::error!(%err, id = %row.id, "invalid payment id uuid");
        PaymentError::Persistence
    })?;
    let version = Version::from_u32(row.version as u32);

    let amount = row.total_amount;
    let currency = row.total_currency;

    let total = Money::from_parts(&currency, &amount).map_err(|err| {
        tracing::error!(%err, %amount, %currency, "failed to deserialize money type");
        PaymentError::Persistence
    })?;

    let source = (match row.source_type.as_str() {
        "manual" => Ok(PaymentSource::Manual),
        "schedule" => {
            let schedule_id = row.schedule_id.ok_or_else(|| {
                tracing::error!(
                    payment_id = %id,
                    source_type = %row.source_type,
                    "payment has schedule source type but schedule_id is missing"
                );
                PaymentError::Persistence
            })?;
            let schedule_id = Uuid::parse_str(&schedule_id).map_err(|err| {
                tracing::error!(%err, payment_id = %id, "invalid schedule id uuid");
                PaymentError::Persistence
            })?;

            let occurrence_date = row.occurrence_date.ok_or_else(|| {
                tracing::error!(
                    payment_id = %id,
                    source_type = %row.source_type,
                    "payment has schedule source type but occurence_date is missing"
                );
                PaymentError::Persistence
            })?;

            Ok(PaymentSource::Schedule {
                schedule_id,
                occurrence_date,
            })
        }

        _ => {
            tracing::error!(
                payment_id = %id,
                source_type = %row.source_type,
                "unrecognized source type"
            );
            Err(PaymentError::Persistence)
        }
    })?;

    Ok(Payment {
        id,
        version,
        total,
        created_at: row.created_at,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::persistence::connect;

    async fn repo() -> SqlitePaymentRepository {
        let pool = connect("sqlite::memory:").await.unwrap();
        SqlitePaymentRepository::new(pool)
    }

    fn manual_payment() -> Payment {
        Payment::new(
            Money::from_parts("USD", "10.99").unwrap(),
            PaymentSource::Manual,
        )
    }

    #[tokio::test]
    async fn create_returns_the_created_payment() {
        // given
        let repo = repo().await;
        let payment = manual_payment();

        // when
        let created = repo.create(payment.clone()).await.unwrap();

        // then
        assert_eq!(created, payment);
    }

    #[tokio::test]
    async fn create_rejects_a_duplicate_id() {
        // given
        let repo = repo().await;
        let payment = manual_payment();
        repo.create(payment.clone()).await.unwrap();

        // when
        let err = repo.create(payment).await.unwrap_err();

        // then
        assert!(matches!(err, PaymentError::AlreadyExists));
    }

    #[tokio::test]
    async fn find_by_id_returns_the_payment() {
        // given
        let repo = repo().await;
        let payment = manual_payment();
        repo.create(payment.clone()).await.unwrap();

        // when
        let found = repo.find_by_id(payment.id).await.unwrap();

        // then
        assert_eq!(found, Some(payment));
    }

    #[tokio::test]
    async fn find_by_id_missing_returns_none() {
        // given
        let repo = repo().await;

        // when
        let found = repo.find_by_id(Uuid::new_v4()).await.unwrap();

        // then
        assert_eq!(found, None);
    }

    #[tokio::test]
    async fn find_all_returns_every_payment() {
        // given
        let repo = repo().await;
        let a = manual_payment();
        let b = manual_payment();
        repo.create(a.clone()).await.unwrap();
        repo.create(b.clone()).await.unwrap();

        // when
        let all = repo.find_all().await.unwrap();

        // then
        assert_eq!(all.len(), 2);
        assert!(all.contains(&a));
        assert!(all.contains(&b));
    }
}
