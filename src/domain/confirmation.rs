use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use super::startup_task::StartupTask;
use super::version::Version;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmationSubject {
    Payment(Uuid),
}

/// `Queued` = created, not yet delivered to Discord. `Awaiting` = delivered, waiting on a
/// human decision. Splitting them is what lets a retry re-send only what never went out,
/// instead of re-pinging everyone still undecided.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmationState {
    Queued,
    Awaiting,
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
    pub fn new(command: CreateConfirmationCommand) -> Self {
        let now = OffsetDateTime::now_utc();
        let (state, decided_at) = if command.automatic_confirmation {
            (ConfirmationState::Approved, Some(now))
        } else {
            (ConfirmationState::Queued, None)
        };

        Confirmation {
            id: Uuid::new_v4(),
            version: Version::FIRST,
            created_at: now,
            updated_at: now,
            subject: command.subject,
            state: state,
            decided_at: decided_at,
        }
    }

    fn mark_awaiting(&mut self) {
        self.state = ConfirmationState::Awaiting;
        self.updated_at = OffsetDateTime::now_utc();
    }

    pub fn decide(&mut self, approved: bool) -> Result<(), AlreadyDecided> {
        if !matches!(
            self.state,
            ConfirmationState::Queued | ConfirmationState::Awaiting
        ) {
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

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait ConfirmationRepository: Send + Sync {
    async fn create(&self, confirmation: Confirmation) -> Confirmation;
    async fn update(&self, confirmation: Confirmation) -> Result<Confirmation, ConfirmationError>;
    async fn find_by_id(&self, id: Uuid) -> Option<Confirmation>;
    async fn find_by_state(&self, state: ConfirmationState) -> Vec<Confirmation>;
    async fn find_by_payment_id(&self, payment_id: Uuid) -> Option<Confirmation>;
}

/// `request` returns whether the message was actually delivered, so a failed send stays
/// `Queued` for the next retry. `poll_decisions` is the catch-up path for adapters that also
/// push decisions in (see `infra::discord`), batched into one round trip.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait Notifier: Send + Sync {
    async fn request(&self, confirmation: &Confirmation) -> bool;
    async fn withdraw(&self, confirmation: &Confirmation);
    async fn poll_decisions(&self, confirmations: &[Confirmation]) -> HashMap<Uuid, bool>;
}

#[derive(Debug, PartialEq)]
pub enum DecideError {
    NotFound,
    AlreadyDecided,
    ConcurrentUpdate,
}

pub struct ConfirmationService {
    repo: Arc<dyn ConfirmationRepository>,
    notifier: Arc<dyn Notifier>,
}

pub struct CreateConfirmationCommand {
    pub subject: ConfirmationSubject,
    pub automatic_confirmation: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfirmationError {
    #[error("confirmation already exists")]
    AlreadyExists,

    #[error("confirmation not found")]
    NotFound,

    #[error("confirmation updated concurrently")]
    ConcurrentUpdate,

    #[error("unexpected persistence error")]
    Persistence,
}

impl ConfirmationService {
    pub fn new(repo: Arc<dyn ConfirmationRepository>, notifier: Arc<dyn Notifier>) -> Self {
        Self { repo, notifier }
    }

    pub async fn create(
        &self,
        cmd: CreateConfirmationCommand,
    ) -> Result<Confirmation, ConfirmationError> {
        let confirmation = Confirmation::new(cmd);
        // TODO: Error handling at repo level
        Ok(self.repo.create(confirmation).await)
    }

    /// A failed delivery stays `Queued` for the next `reconcile` tick - the whole
    /// self-healing mechanism, no outbox.
    pub async fn try_deliver(
        &self,
        mut confirmation: Confirmation,
    ) -> Result<Confirmation, ConfirmationError> {
        if self.notifier.request(&confirmation).await {
            confirmation.mark_awaiting();
            confirmation = self.repo.update(confirmation).await?;
        }
        Ok(confirmation)
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
        let updated = self
            .repo
            .update(confirmation)
            .await
            .map_err(|err| match err {
                ConfirmationError::NotFound => DecideError::NotFound,
                _ => DecideError::ConcurrentUpdate,
            })?;
        self.notifier.withdraw(&updated).await;
        tracing::info!(confirmation_id = %updated.id, approved, "decided confirmation");
        Ok(updated)
    }

    pub async fn find_pending(&self) -> Vec<Confirmation> {
        let mut pending = self.repo.find_by_state(ConfirmationState::Queued).await;
        pending.extend(self.repo.find_by_state(ConfirmationState::Awaiting).await);
        pending.sort_by_key(|confirmation| confirmation.created_at);
        pending
    }

    pub async fn find_by_payment_id(&self, payment_id: Uuid) -> Option<Confirmation> {
        self.repo.find_by_payment_id(payment_id).await
    }

    pub async fn reconcile(&self) {
        for confirmation in self.repo.find_by_state(ConfirmationState::Queued).await {
            if let Err(err) = self.try_deliver(confirmation).await {
                tracing::error!(%err, "failed to deliver confirmation");
            }
        }

        let awaiting = self.repo.find_by_state(ConfirmationState::Awaiting).await;
        if awaiting.is_empty() {
            return;
        }
        for (id, approved) in self.notifier.poll_decisions(&awaiting).await {
            let _ = self.decide(id, approved).await;
        }
    }
}

pub struct ConfirmationReconcileJob {
    service: Arc<ConfirmationService>,
    poll_interval: Duration,
}

impl ConfirmationReconcileJob {
    pub fn new(service: Arc<ConfirmationService>, poll_interval_secs: u64) -> Self {
        Self {
            service,
            poll_interval: Duration::from_secs(poll_interval_secs),
        }
    }
}

#[async_trait]
impl StartupTask for ConfirmationReconcileJob {
    fn name(&self) -> &str {
        "confirmation-reconcile-job"
    }

    async fn run(&self) {
        loop {
            self.service.reconcile().await;
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn some_confirmation_command() -> CreateConfirmationCommand {
        CreateConfirmationCommand {
            subject: ConfirmationSubject::Payment(Uuid::new_v4()),
            automatic_confirmation: false,
        }
    }

    #[test]
    fn new_starts_queued_with_no_decision() {
        let confirmation = Confirmation::new(some_confirmation_command());
        assert_eq!(confirmation.state, ConfirmationState::Queued);
        assert_eq!(confirmation.decided_at, None);
    }

    #[test]
    fn decide_approved_transitions_to_approved_and_stamps_decided_at() {
        let mut confirmation = Confirmation::new(some_confirmation_command());
        confirmation.decide(true).unwrap();
        assert_eq!(confirmation.state, ConfirmationState::Approved);
        assert!(confirmation.decided_at.is_some());
    }

    #[test]
    fn decide_rejected_transitions_to_rejected() {
        let mut confirmation = Confirmation::new(some_confirmation_command());
        confirmation.decide(false).unwrap();
        assert_eq!(confirmation.state, ConfirmationState::Rejected);
    }

    #[test]
    fn decide_twice_rejects_the_second_call() {
        let mut confirmation = Confirmation::new(some_confirmation_command());
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

        fn insert(&self, confirmation: Confirmation) -> Confirmation {
            self.confirmations
                .lock()
                .unwrap()
                .push(confirmation.clone());
            confirmation
        }
    }

    #[async_trait]
    impl ConfirmationRepository for FakeRepo {
        async fn create(&self, confirmation: Confirmation) -> Confirmation {
            self.insert(confirmation)
        }

        async fn update(
            &self,
            confirmation: Confirmation,
        ) -> Result<Confirmation, ConfirmationError> {
            let mut confirmations = self.confirmations.lock().unwrap();
            let existing = confirmations
                .iter_mut()
                .find(|c| c.id == confirmation.id)
                .expect("confirmation not found");
            *existing = confirmation.clone();
            Ok(confirmation)
        }

        async fn find_by_id(&self, id: Uuid) -> Option<Confirmation> {
            self.confirmations
                .lock()
                .unwrap()
                .iter()
                .find(|c| c.id == id)
                .cloned()
        }

        async fn find_by_state(&self, state: ConfirmationState) -> Vec<Confirmation> {
            self.confirmations
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.state == state)
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
        withdrawn: Mutex<Vec<Uuid>>,
        delivers: bool,
        decision: Option<bool>,
    }

    impl FakeNotifier {
        fn new() -> Self {
            Self {
                requested: Mutex::new(Vec::new()),
                withdrawn: Mutex::new(Vec::new()),
                delivers: true,
                decision: None,
            }
        }

        fn failing() -> Self {
            Self {
                delivers: false,
                ..Self::new()
            }
        }

        fn with_decision(decision: bool) -> Self {
            Self {
                decision: Some(decision),
                ..Self::new()
            }
        }
    }

    #[async_trait]
    impl Notifier for FakeNotifier {
        async fn request(&self, confirmation: &Confirmation) -> bool {
            self.requested.lock().unwrap().push(confirmation.id);
            self.delivers
        }

        async fn withdraw(&self, confirmation: &Confirmation) {
            self.withdrawn.lock().unwrap().push(confirmation.id);
        }

        async fn poll_decisions(&self, confirmations: &[Confirmation]) -> HashMap<Uuid, bool> {
            match self.decision {
                Some(decision) => confirmations.iter().map(|c| (c.id, decision)).collect(),
                None => HashMap::new(),
            }
        }
    }

    async fn request(repo: &FakeRepo, service: &ConfirmationService) -> Confirmation {
        let confirmation = repo.insert(Confirmation::new(some_confirmation_command()));
        service.try_deliver(confirmation).await.unwrap()
    }

    fn service(notifier: Arc<FakeNotifier>) -> (Arc<FakeRepo>, ConfirmationService) {
        let repo = Arc::new(FakeRepo::new());
        let service = ConfirmationService::new(repo.clone(), notifier);
        (repo, service)
    }

    #[tokio::test]
    async fn try_deliver_marks_awaiting_on_success() {
        let notifier = Arc::new(FakeNotifier::new());
        let (repo, service) = service(notifier.clone());

        let confirmation = request(&repo, &service).await;

        assert_eq!(confirmation.state, ConfirmationState::Awaiting);
        assert_eq!(*notifier.requested.lock().unwrap(), vec![confirmation.id]);
    }

    #[tokio::test]
    async fn try_deliver_leaves_queued_when_delivery_fails() {
        let (repo, service) = service(Arc::new(FakeNotifier::failing()));

        let confirmation = request(&repo, &service).await;

        assert_eq!(confirmation.state, ConfirmationState::Queued);
    }

    #[tokio::test]
    async fn decide_on_unknown_id_returns_not_found() {
        let (_, service) = service(Arc::new(FakeNotifier::new()));
        assert_eq!(
            service.decide(Uuid::new_v4(), true).await,
            Err(DecideError::NotFound)
        );
    }

    #[tokio::test]
    async fn decide_withdraws_the_notification() {
        let notifier = Arc::new(FakeNotifier::new());
        let (repo, service) = service(notifier.clone());
        let confirmation = request(&repo, &service).await;

        service.decide(confirmation.id, true).await.unwrap();

        assert_eq!(*notifier.withdrawn.lock().unwrap(), vec![confirmation.id]);
    }

    #[tokio::test]
    async fn decide_twice_returns_already_decided_on_the_second_call() {
        let (repo, service) = service(Arc::new(FakeNotifier::new()));
        let confirmation = request(&repo, &service).await;

        service.decide(confirmation.id, true).await.unwrap();
        assert_eq!(
            service.decide(confirmation.id, false).await,
            Err(DecideError::AlreadyDecided)
        );
    }

    #[tokio::test]
    async fn reconcile_retries_only_never_delivered_confirmations() {
        let notifier = Arc::new(FakeNotifier::failing());
        let (repo, service) = service(notifier.clone());
        let never_delivered = request(&repo, &service).await;
        assert_eq!(never_delivered.state, ConfirmationState::Queued);
        notifier.requested.lock().unwrap().clear();

        service.reconcile().await;

        assert_eq!(
            *notifier.requested.lock().unwrap(),
            vec![never_delivered.id]
        );
    }

    #[tokio::test]
    async fn reconcile_applies_an_incoming_approval() {
        let (repo, service) = service(Arc::new(FakeNotifier::new()));
        let confirmation = request(&repo, &service).await;
        assert_eq!(confirmation.state, ConfirmationState::Awaiting);

        let polling_notifier = Arc::new(FakeNotifier::with_decision(true));
        let service = ConfirmationService::new(repo.clone(), polling_notifier.clone());

        service.reconcile().await;

        let updated = repo.find_by_id(confirmation.id).await.unwrap();
        assert_eq!(updated.state, ConfirmationState::Approved);
        assert_eq!(
            *polling_notifier.withdrawn.lock().unwrap(),
            vec![confirmation.id]
        );
    }

    #[tokio::test]
    async fn reconcile_leaves_undecided_confirmations_awaiting() {
        let (repo, service) = service(Arc::new(FakeNotifier::new()));
        let confirmation = request(&repo, &service).await;

        service.reconcile().await;

        let updated = repo.find_by_id(confirmation.id).await.unwrap();
        assert_eq!(updated.state, ConfirmationState::Awaiting);
    }

    #[tokio::test]
    async fn find_pending_excludes_decided_confirmations() {
        let (repo, service) = service(Arc::new(FakeNotifier::new()));
        let pending = request(&repo, &service).await;
        let decided = request(&repo, &service).await;
        service.decide(decided.id, true).await.unwrap();

        let result = service.find_pending().await;
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, pending.id);
    }
}
