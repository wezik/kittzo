use async_trait::async_trait;
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::domain::confirmation::Confirmation;
use crate::domain::money::Money;
use crate::domain::payment::{Payment, PaymentRepository, PaymentSource};
use crate::domain::version::Version;

use super::sqlx_confirmation_repository::{state_str, subject_parts};

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
    async fn create(
        &self,
        payment: Payment,
        confirmation: Confirmation,
    ) -> (Payment, Confirmation) {
        let mut tx = self
            .pool
            .begin()
            .await
            .expect("failed to begin payment creation transaction");

        let (schedule_id, occurrence_date) = schedule_source_parts(&payment.source);
        sqlx::query(
            "INSERT INTO payments (id, version, total, created_at, source_type, schedule_id, occurrence_date)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(payment.id.to_string())
        .bind(payment.version.as_u32() as i64)
        .bind(&payment.total)
        .bind(payment.created_at)
        .bind(source_type(&payment.source))
        .bind(schedule_id)
        .bind(occurrence_date)
        .execute(&mut *tx)
        .await
        .expect("failed to insert payment");

        let (subject_type, subject_id) = subject_parts(&confirmation.subject);
        sqlx::query(
            "INSERT INTO confirmations
             (id, version, created_at, updated_at, subject_type, subject_id, state, decided_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(confirmation.id.to_string())
        .bind(confirmation.version.as_u32() as i64)
        .bind(confirmation.created_at)
        .bind(confirmation.updated_at)
        .bind(subject_type)
        .bind(subject_id)
        .bind(state_str(confirmation.state))
        .bind(confirmation.decided_at)
        .execute(&mut *tx)
        .await
        .expect("failed to insert confirmation");

        tx.commit()
            .await
            .expect("failed to commit payment creation transaction");

        (payment, confirmation)
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

pub(crate) fn source_type(source: &PaymentSource) -> &'static str {
    match source {
        PaymentSource::Manual => "manual",
        PaymentSource::Schedule { .. } => "schedule",
    }
}

pub(crate) fn schedule_source_parts(source: &PaymentSource) -> (Option<String>, Option<Date>) {
    match source {
        PaymentSource::Manual => (None, None),
        PaymentSource::Schedule {
            schedule_id,
            occurrence_date,
        } => (Some(schedule_id.to_string()), Some(*occurrence_date)),
    }
}

fn row_to_payment(row: &SqliteRow) -> Payment {
    let id: String = row.get("id");
    let version: i64 = row.get("version");
    let total: Money = row.get("total");
    let created_at: OffsetDateTime = row.get("created_at");
    let source_type: String = row.get("source_type");

    let source = match source_type.as_str() {
        "manual" => PaymentSource::Manual,
        "schedule" => {
            let schedule_id: String = row.get("schedule_id");
            let occurrence_date: Date = row.get("occurrence_date");
            PaymentSource::Schedule {
                schedule_id: Uuid::parse_str(&schedule_id).expect("invalid schedule id uuid"),
                occurrence_date,
            }
        }
        other => panic!("unknown source_type in db: {other}"),
    };

    Payment {
        id: Uuid::parse_str(&id).expect("invalid payment id uuid"),
        version: Version::from_u32(version as u32),
        total,
        created_at,
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::confirmation::ConfirmationSubject;
    use crate::infra::persistence::connect;
    use crate::infra::persistence::sqlx_confirmation_repository::SqliteConfirmationRepository;
    use rusty_money::iso;

    async fn repo() -> SqlitePaymentRepository {
        let pool = connect("sqlite::memory:").await.unwrap();
        SqlitePaymentRepository::new(pool)
    }

    fn manual_payment() -> Payment {
        Payment::new(Money::from_minor(1099, iso::USD), PaymentSource::Manual)
    }

    fn confirmation_for(payment: &Payment) -> Confirmation {
        Confirmation::new(ConfirmationSubject::Payment(payment.id))
    }

    #[tokio::test]
    async fn create_and_find_by_id_round_trips() {
        let repo = repo().await;
        let payment = manual_payment();

        let (created, _) = repo
            .create(payment.clone(), confirmation_for(&payment))
            .await;
        assert_eq!(created, payment);

        let found = repo.find_by_id(payment.id).await.unwrap();
        assert_eq!(found, payment);
    }

    #[tokio::test]
    async fn create_also_persists_the_pending_confirmation() {
        let pool = connect("sqlite::memory:").await.unwrap();
        let repo = SqlitePaymentRepository::new(pool.clone());
        let confirmation_repo = SqliteConfirmationRepository::new(pool);
        let payment = manual_payment();
        let confirmation = confirmation_for(&payment);

        repo.create(payment, confirmation.clone()).await;

        use crate::domain::confirmation::ConfirmationRepository;
        let found = confirmation_repo.find_by_id(confirmation.id).await.unwrap();
        assert_eq!(found.subject, confirmation.subject);
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
        repo.create(a.clone(), confirmation_for(&a)).await;
        repo.create(b.clone(), confirmation_for(&b)).await;

        let all = repo.find_all().await;
        assert_eq!(all.len(), 2);
        assert!(all.contains(&a));
        assert!(all.contains(&b));
    }
}
