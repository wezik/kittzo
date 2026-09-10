use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use super::version::Version;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmationSubject {
    Payment(Uuid),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmationState {
    Pending,
    Approved,
    Rejected,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Confirmation {
    pub id: Uuid,
    pub version: Version,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,

    pub subject: ConfirmationSubject,
    pub state: ConfirmationState,
    pub decided_at: Option<OffsetDateTime>,
}

#[derive(Debug, PartialEq)]
pub struct AlreadyDecided;

impl Confirmation {
    pub fn new(subject: ConfirmationSubject) -> Self {
        let now = OffsetDateTime::now_utc();
        Confirmation {
            id: Uuid::new_v4(),
            version: Version::FIRST,
            created_at: now,
            updated_at: now,
            subject,
            state: ConfirmationState::Pending,
            decided_at: None,
        }
    }

    pub fn decide(&mut self, approved: bool) -> Result<(), AlreadyDecided> {
        if self.state != ConfirmationState::Pending {
            return Err(AlreadyDecided);
        }
        let now = OffsetDateTime::now_utc();
        self.state = if approved {
            ConfirmationState::Approved
        } else {
            ConfirmationState::Rejected
        };
        self.updated_at = now;
        self.decided_at = Some(now);
        Ok(())
    }
}

#[async_trait]
pub trait ConfirmationRepository: Send + Sync {
    async fn create(&self, confirmation: Confirmation) -> Confirmation;
    async fn update(&self, confirmation: Confirmation) -> Confirmation;
    async fn find_by_id(&self, id: Uuid) -> Option<Confirmation>;
    async fn find_pending(&self) -> Vec<Confirmation>;
    async fn find_by_payment_id(&self, payment_id: Uuid) -> Option<Confirmation>;
}

/// Human-in-the-loop notification port. `request` is fire-and-forget from the
/// service's point of view - the reply path is an infra concern, added once
/// an adapter that supports replies actually needs it.
#[async_trait]
pub trait Notifier: Send + Sync {
    async fn request(&self, confirmation: &Confirmation);
}

#[derive(Debug, PartialEq)]
pub enum DecideError {
    NotFound,
    AlreadyDecided,
}

pub struct ConfirmationService {
    repo: Arc<dyn ConfirmationRepository>,
    notifier: Arc<dyn Notifier>,
}

impl ConfirmationService {
    pub fn new(repo: Arc<dyn ConfirmationRepository>, notifier: Arc<dyn Notifier>) -> Self {
        Self { repo, notifier }
    }

    pub async fn request(&self, subject: ConfirmationSubject) -> Confirmation {
        let confirmation = Confirmation::new(subject);
        let created = self.repo.create(confirmation).await;
        self.notifier.request(&created).await;
        tracing::info!(confirmation_id = %created.id, "requested confirmation");
        created
    }

    pub async fn decide(&self, id: Uuid, approved: bool) -> Result<Confirmation, DecideError> {
        let mut confirmation = self
            .repo
            .find_by_id(id)
            .await
            .ok_or(DecideError::NotFound)?;
        confirmation
            .decide(approved)
            .map_err(|_| DecideError::AlreadyDecided)?;
        let updated = self.repo.update(confirmation).await;
        tracing::info!(confirmation_id = %updated.id, approved, "decided confirmation");
        Ok(updated)
    }

    pub async fn find_pending(&self) -> Vec<Confirmation> {
        self.repo.find_pending().await
    }

    pub async fn find_by_payment_id(&self, payment_id: Uuid) -> Option<Confirmation> {
        self.repo.find_by_payment_id(payment_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn new_starts_pending_with_no_decision() {
        let confirmation = Confirmation::new(ConfirmationSubject::Payment(Uuid::new_v4()));
        assert_eq!(confirmation.state, ConfirmationState::Pending);
        assert_eq!(confirmation.decided_at, None);
    }

    #[test]
    fn decide_approved_transitions_to_approved_and_stamps_decided_at() {
        let mut confirmation = Confirmation::new(ConfirmationSubject::Payment(Uuid::new_v4()));
        confirmation.decide(true).unwrap();
        assert_eq!(confirmation.state, ConfirmationState::Approved);
        assert!(confirmation.decided_at.is_some());
    }

    #[test]
    fn decide_rejected_transitions_to_rejected() {
        let mut confirmation = Confirmation::new(ConfirmationSubject::Payment(Uuid::new_v4()));
        confirmation.decide(false).unwrap();
        assert_eq!(confirmation.state, ConfirmationState::Rejected);
    }

    #[test]
    fn decide_twice_rejects_the_second_call() {
        let mut confirmation = Confirmation::new(ConfirmationSubject::Payment(Uuid::new_v4()));
        confirmation.decide(true).unwrap();
        assert_eq!(confirmation.decide(false), Err(AlreadyDecided));
        assert_eq!(confirmation.state, ConfirmationState::Approved);
    }

    struct FakeRepo {
        confirmations: Mutex<Vec<Confirmation>>,
    }

    impl FakeRepo {
        fn new() -> Self {
            Self {
                confirmations: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ConfirmationRepository for FakeRepo {
        async fn create(&self, confirmation: Confirmation) -> Confirmation {
            self.confirmations
                .lock()
                .unwrap()
                .push(confirmation.clone());
            confirmation
        }

        async fn update(&self, confirmation: Confirmation) -> Confirmation {
            let mut confirmations = self.confirmations.lock().unwrap();
            let existing = confirmations
                .iter_mut()
                .find(|c| c.id == confirmation.id)
                .expect("confirmation not found");
            *existing = confirmation.clone();
            confirmation
        }

        async fn find_by_id(&self, id: Uuid) -> Option<Confirmation> {
            self.confirmations
                .lock()
                .unwrap()
                .iter()
                .find(|c| c.id == id)
                .cloned()
        }

        async fn find_pending(&self) -> Vec<Confirmation> {
            self.confirmations
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.state == ConfirmationState::Pending)
                .cloned()
                .collect()
        }

        async fn find_by_payment_id(&self, payment_id: Uuid) -> Option<Confirmation> {
            self.confirmations
                .lock()
                .unwrap()
                .iter()
                .find(|c| c.subject == ConfirmationSubject::Payment(payment_id))
                .cloned()
        }
    }

    struct FakeNotifier {
        requested: Mutex<Vec<Uuid>>,
    }

    impl FakeNotifier {
        fn new() -> Self {
            Self {
                requested: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Notifier for FakeNotifier {
        async fn request(&self, confirmation: &Confirmation) {
            self.requested.lock().unwrap().push(confirmation.id);
        }
    }

    fn service() -> (Arc<FakeRepo>, Arc<FakeNotifier>, ConfirmationService) {
        let repo = Arc::new(FakeRepo::new());
        let notifier = Arc::new(FakeNotifier::new());
        let service = ConfirmationService::new(repo.clone(), notifier.clone());
        (repo, notifier, service)
    }

    #[tokio::test]
    async fn request_persists_a_pending_confirmation_and_notifies() {
        let (_, notifier, service) = service();
        let confirmation = service
            .request(ConfirmationSubject::Payment(Uuid::new_v4()))
            .await;

        assert_eq!(confirmation.state, ConfirmationState::Pending);
        assert_eq!(*notifier.requested.lock().unwrap(), vec![confirmation.id]);
    }

    #[tokio::test]
    async fn decide_on_unknown_id_returns_not_found() {
        let (_, _, service) = service();
        assert_eq!(
            service.decide(Uuid::new_v4(), true).await,
            Err(DecideError::NotFound)
        );
    }

    #[tokio::test]
    async fn decide_twice_returns_already_decided_on_the_second_call() {
        let (_, _, service) = service();
        let confirmation = service
            .request(ConfirmationSubject::Payment(Uuid::new_v4()))
            .await;

        service.decide(confirmation.id, true).await.unwrap();
        assert_eq!(
            service.decide(confirmation.id, false).await,
            Err(DecideError::AlreadyDecided)
        );
    }

    #[tokio::test]
    async fn find_pending_excludes_decided_confirmations() {
        let (_, _, service) = service();
        let pending = service
            .request(ConfirmationSubject::Payment(Uuid::new_v4()))
            .await;
        let decided = service
            .request(ConfirmationSubject::Payment(Uuid::new_v4()))
            .await;
        service.decide(decided.id, true).await.unwrap();

        let result = service.find_pending().await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, pending.id);
    }
}
