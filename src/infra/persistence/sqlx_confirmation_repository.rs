use async_trait::async_trait;
use sqlx::{Row, Sqlite, SqlitePool, sqlite::SqliteRow};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::confirmation::{
    Confirmation, ConfirmationRepository, ConfirmationState, ConfirmationSubject,
};
use crate::domain::version::Version;

pub struct SqliteConfirmationRepository {
    pool: SqlitePool,
}

impl SqliteConfirmationRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ConfirmationRepository for SqliteConfirmationRepository {
    async fn create(&self, confirmation: Confirmation) -> Confirmation {
        insert_confirmation(&self.pool, &confirmation).await;
        confirmation
    }

    async fn update(&self, confirmation: Confirmation) -> Confirmation {
        let next_version = confirmation.version.next();

        let result = sqlx::query(
            "UPDATE confirmations
             SET version = ?, updated_at = ?, state = ?, decided_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(next_version.as_u32() as i64)
        .bind(confirmation.updated_at)
        .bind(state_str(confirmation.state))
        .bind(confirmation.decided_at)
        .bind(confirmation.id.to_string())
        .bind(confirmation.version.as_u32() as i64)
        .execute(&self.pool)
        .await
        .expect("failed to update confirmation");

        if result.rows_affected() == 0 {
            panic!(
                "confirmation {} updated concurrently or missing",
                confirmation.id
            );
        }

        Confirmation {
            version: next_version,
            ..confirmation
        }
    }

    async fn find_by_id(&self, id: Uuid) -> Option<Confirmation> {
        sqlx::query("SELECT * FROM confirmations WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .expect("failed to query confirmation by id")
            .map(|row| row_to_confirmation(&row))
    }

    async fn find_by_state(&self, state: ConfirmationState) -> Vec<Confirmation> {
        sqlx::query("SELECT * FROM confirmations WHERE state = ? ORDER BY created_at")
            .bind(state_str(state))
            .fetch_all(&self.pool)
            .await
            .expect("failed to query confirmations by state")
            .iter()
            .map(row_to_confirmation)
            .collect()
    }

    async fn find_by_payment_id(&self, payment_id: Uuid) -> Option<Confirmation> {
        sqlx::query("SELECT * FROM confirmations WHERE subject_type = 'payment' AND subject_id = ?")
            .bind(payment_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .expect("failed to query confirmation by payment id")
            .map(|row| row_to_confirmation(&row))
    }
}

// takes any executor so a caller that needs the write inside a larger transaction can share it.
pub(crate) async fn insert_confirmation<'e, E>(executor: E, confirmation: &Confirmation)
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
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
    .execute(executor)
    .await
    .expect("failed to insert confirmation");
}

pub(crate) fn subject_parts(subject: &ConfirmationSubject) -> (&'static str, String) {
    match subject {
        ConfirmationSubject::Payment(id) => ("payment", id.to_string()),
    }
}

pub(crate) fn state_str(state: ConfirmationState) -> &'static str {
    match state {
        ConfirmationState::Queued => "queued",
        ConfirmationState::Awaiting => "awaiting",
        ConfirmationState::Approved => "approved",
        ConfirmationState::Rejected => "rejected",
    }
}

fn row_to_confirmation(row: &SqliteRow) -> Confirmation {
    let id: String = row.get("id");
    let version: i64 = row.get("version");
    let created_at: OffsetDateTime = row.get("created_at");
    let updated_at: OffsetDateTime = row.get("updated_at");
    let subject_type: String = row.get("subject_type");
    let subject_id: String = row.get("subject_id");
    let state: String = row.get("state");
    let decided_at: Option<OffsetDateTime> = row.get("decided_at");

    let subject = match subject_type.as_str() {
        "payment" => ConfirmationSubject::Payment(
            Uuid::parse_str(&subject_id).expect("invalid subject id uuid"),
        ),
        other => panic!("unknown subject_type in db: {other}"),
    };

    Confirmation {
        id: Uuid::parse_str(&id).expect("invalid confirmation id uuid"),
        version: Version::from_u32(version as u32),
        created_at,
        updated_at,
        subject,
        state: match state.as_str() {
            "queued" => ConfirmationState::Queued,
            "awaiting" => ConfirmationState::Awaiting,
            "approved" => ConfirmationState::Approved,
            "rejected" => ConfirmationState::Rejected,
            other => panic!("unknown state in db: {other}"),
        },
        decided_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::confirmation::CreateConfirmationCommand;
    use crate::infra::persistence::connect;

    async fn repo() -> SqliteConfirmationRepository {
        let pool = connect("sqlite::memory:").await.unwrap();
        SqliteConfirmationRepository::new(pool)
    }

    async fn queued(repo: &SqliteConfirmationRepository, payment_id: Uuid) -> Confirmation {
        let confirmation = Confirmation::new(CreateConfirmationCommand {
            subject: ConfirmationSubject::Payment(payment_id),
            automatic_confirmation: false,
        });
        insert_confirmation(&repo.pool, &confirmation).await;
        confirmation
    }

    async fn some_queued(repo: &SqliteConfirmationRepository) -> Confirmation {
        queued(repo, Uuid::new_v4()).await
    }

    #[tokio::test]
    async fn create_and_find_by_id_round_trips() {
        let repo = repo().await;
        let confirmation = Confirmation::new(CreateConfirmationCommand {
            subject: ConfirmationSubject::Payment(Uuid::new_v4()),
            automatic_confirmation: false,
        });

        let created = repo.create(confirmation.clone()).await;
        assert_eq!(created, confirmation);

        let found = repo.find_by_id(confirmation.id).await.unwrap();
        assert_eq!(found, confirmation);
    }

    #[tokio::test]
    async fn insert_and_find_by_id_round_trips() {
        let repo = repo().await;
        let confirmation = some_queued(&repo).await;

        let found = repo.find_by_id(confirmation.id).await.unwrap();
        assert_eq!(found, confirmation);
    }

    #[tokio::test]
    async fn find_by_id_missing_returns_none() {
        let repo = repo().await;
        assert_eq!(repo.find_by_id(Uuid::new_v4()).await, None);
    }

    #[tokio::test]
    async fn update_persists_decision_and_bumps_version() {
        let repo = repo().await;
        let mut confirmation = some_queued(&repo).await;
        confirmation.decide(true).unwrap();

        let updated = repo.update(confirmation.clone()).await;

        assert_eq!(updated.version, confirmation.version.next());
        assert_eq!(updated.state, ConfirmationState::Approved);
        assert!(updated.decided_at.is_some());
    }

    #[tokio::test]
    async fn find_by_payment_id_returns_its_confirmation() {
        let repo = repo().await;
        let payment_id = Uuid::new_v4();
        let confirmation = queued(&repo, payment_id).await;

        let found = repo.find_by_payment_id(payment_id).await.unwrap();
        assert_eq!(found.id, confirmation.id);
        assert_eq!(repo.find_by_payment_id(Uuid::new_v4()).await, None);
    }

    #[tokio::test]
    async fn find_by_state_matches_only_that_state() {
        let repo = repo().await;
        let still_queued = some_queued(&repo).await;
        let mut awaiting = some_queued(&repo).await;
        awaiting.state = ConfirmationState::Awaiting;
        let awaiting = repo.update(awaiting).await;
        let mut decided = some_queued(&repo).await;
        decided.decide(true).unwrap();
        repo.update(decided).await;

        let result = repo.find_by_state(ConfirmationState::Queued).await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, still_queued.id);

        let result = repo.find_by_state(ConfirmationState::Awaiting).await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, awaiting.id);
    }
}
