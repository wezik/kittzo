use async_trait::async_trait;
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::domain::money::Money;
use crate::domain::payment::{Payment, PaymentRepository, PaymentSource};
use crate::domain::version::Version;

pub struct SqlitePaymentRepository {
    pool: SqlitePool,
}

impl SqlitePaymentRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PaymentRepository for SqlitePaymentRepository {
    async fn create(&self, payment: Payment) -> Payment {
        let (source_type, schedule_id, due_date) = source_columns(&payment.source);

        sqlx::query(
            "INSERT INTO payments (id, version, amount, created_at, source_type, schedule_id, due_date)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(payment.id.to_string())
        .bind(payment.version.as_u32() as i64)
        .bind(&payment.amount)
        .bind(payment.created_at)
        .bind(source_type)
        .bind(schedule_id)
        .bind(due_date)
        .execute(&self.pool)
        .await
        .expect("failed to insert payment");

        payment
    }

    async fn find_by_id(&self, id: Uuid) -> Option<Payment> {
        sqlx::query("SELECT * FROM payments WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .expect("failed to query payment by id")
            .map(|row| row_to_payment(&row))
    }

    async fn find_all(&self) -> Vec<Payment> {
        sqlx::query("SELECT * FROM payments ORDER BY created_at")
            .fetch_all(&self.pool)
            .await
            .expect("failed to query payments")
            .iter()
            .map(row_to_payment)
            .collect()
    }
}

fn source_columns(source: &PaymentSource) -> (&'static str, Option<String>, Option<Date>) {
    match source {
        PaymentSource::Manual => ("manual", None, None),
        PaymentSource::Ingested => ("ingested", None, None),
        PaymentSource::Schedule {
            schedule_id,
            due_date,
        } => ("schedule", Some(schedule_id.to_string()), Some(*due_date)),
    }
}

fn row_to_payment(row: &SqliteRow) -> Payment {
    let id: String = row.get("id");
    let version: i64 = row.get("version");
    let amount: Money = row.get("amount");
    let created_at: OffsetDateTime = row.get("created_at");
    let source_type: String = row.get("source_type");

    let source = match source_type.as_str() {
        "manual" => PaymentSource::Manual,
        "ingested" => PaymentSource::Ingested,
        "schedule" => {
            let schedule_id: String = row.get("schedule_id");
            let due_date: Date = row.get("due_date");
            PaymentSource::Schedule {
                schedule_id: Uuid::parse_str(&schedule_id).expect("invalid schedule_id uuid"),
                due_date,
            }
        }
        other => panic!("unknown source_type in db: {other}"),
    };

    Payment {
        id: Uuid::parse_str(&id).expect("invalid payment id uuid"),
        version: Version::from_u32(version as u32),
        amount,
        created_at,
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::persistence::connect;
    use rusty_money::iso;

    async fn repo() -> SqlitePaymentRepository {
        let pool = connect("sqlite::memory:").await.unwrap();
        SqlitePaymentRepository::new(pool)
    }

    fn manual_payment() -> Payment {
        Payment::new(Money::from_minor(1099, iso::USD), PaymentSource::Manual)
    }

    #[tokio::test]
    async fn create_and_find_by_id_round_trips() {
        let repo = repo().await;
        let payment = manual_payment();

        let created = repo.create(payment.clone()).await;
        assert_eq!(created, payment);

        let found = repo.find_by_id(payment.id).await.unwrap();
        assert_eq!(found, payment);
    }

    #[tokio::test]
    async fn find_by_id_missing_returns_none() {
        let repo = repo().await;
        assert_eq!(repo.find_by_id(Uuid::new_v4()).await, None);
    }

    #[tokio::test]
    async fn find_all_returns_every_payment() {
        let repo = repo().await;
        let a = manual_payment();
        let b = manual_payment();
        repo.create(a.clone()).await;
        repo.create(b.clone()).await;

        let all = repo.find_all().await;
        assert_eq!(all.len(), 2);
        assert!(all.contains(&a));
        assert!(all.contains(&b));
    }

    #[tokio::test]
    async fn schedule_source_round_trips() {
        let repo = repo().await;
        let payment = Payment::new(
            Money::from_minor(500, iso::USD),
            PaymentSource::Schedule {
                schedule_id: Uuid::new_v4(),
                due_date: Date::from_calendar_date(2026, time::Month::January, 15).unwrap(),
            },
        );
        repo.create(payment.clone()).await;

        let found = repo.find_by_id(payment.id).await.unwrap();
        assert_eq!(found, payment);
    }
}
