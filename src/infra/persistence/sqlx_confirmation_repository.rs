use async_trait::async_trait;
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
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
        .execute(&self.pool)
        .await
        .expect("failed to insert confirmation");

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

    async fn find_pending(&self) -> Vec<Confirmation> {
        sqlx::query("SELECT * FROM confirmations WHERE state = 'pending' ORDER BY created_at")
            .fetch_all(&self.pool)
            .await
            .expect("failed to query pending confirmations")
            .iter()
            .map(row_to_confirmation)
            .collect()
    }
}

pub(crate) fn subject_parts(subject: &ConfirmationSubject) -> (&'static str, String) {
    match subject {
        ConfirmationSubject::Payment(id) => ("payment", id.to_string()),
    }
}

pub(crate) fn state_str(state: ConfirmationState) -> &'static str {
    match state {
        ConfirmationState::Pending => "pending",
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
            "pending" => ConfirmationState::Pending,
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
    use crate::infra::persistence::connect;

    async fn repo() -> SqliteConfirmationRepository {
        let pool = connect("sqlite::memory:").await.unwrap();
        SqliteConfirmationRepository::new(pool)
    }

    fn pending_confirmation() -> Confirmation {
        Confirmation::new(ConfirmationSubject::Payment(Uuid::new_v4()))
    }

    #[tokio::test]
    async fn create_and_find_by_id_round_trips() {
        let repo = repo().await;
        let confirmation = pending_confirmation();

        let created = repo.create(confirmation.clone()).await;
        assert_eq!(created, confirmation);

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
        let mut confirmation = repo.create(pending_confirmation()).await;
        confirmation.decide(true).unwrap();

        let updated = repo.update(confirmation.clone()).await;

        assert_eq!(updated.version, confirmation.version.next());
        assert_eq!(updated.state, ConfirmationState::Approved);
        assert!(updated.decided_at.is_some());
    }

    #[tokio::test]
    async fn find_pending_excludes_decided_confirmations() {
        let repo = repo().await;
        let pending = repo.create(pending_confirmation()).await;
        let mut decided = repo.create(pending_confirmation()).await;
        decided.decide(false).unwrap();
        repo.update(decided).await;

        let result = repo.find_pending().await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, pending.id);
    }
}
