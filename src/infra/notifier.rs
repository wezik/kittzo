use std::collections::HashMap;

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::confirmation::{Confirmation, Notifier};

/// Placeholder `Notifier` until a real adapter (e.g. Discord) exists. Logs the
/// confirmation request so `ConfirmationService` has a real port to call in the meantime.
pub struct LoggingNotifier;

#[async_trait]
impl Notifier for LoggingNotifier {
    async fn request(&self, confirmation: &Confirmation) -> bool {
        tracing::info!(
            confirmation_id = %confirmation.id,
            subject = ?confirmation.subject,
            "confirmation requested"
        );
        true
    }

    async fn withdraw(&self, confirmation: &Confirmation) {
        tracing::info!(confirmation_id = %confirmation.id, "confirmation withdrawn");
    }

    async fn poll_decisions(&self, _confirmations: &[Confirmation]) -> HashMap<Uuid, bool> {
        HashMap::new()
    }
}
