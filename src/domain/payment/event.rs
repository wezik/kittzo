use uuid::Uuid;

use crate::domain::event::OutboxMessage;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Event {
    Created { id: Uuid },
    Acked { id: Uuid },
    Finished { id: Uuid },
}

impl Event {
    pub const CREATED: &str = "PAYMENT_CREATED";
    pub const ACKED: &str = "PAYMENT_ACKED";
    pub const FINISHED: &str = "PAYMENT_FINISHED";
}

impl From<Event> for OutboxMessage {
    fn from(event: Event) -> Self {
        let (event_type, id) = match event {
            Event::Created { id } => (Event::CREATED, id),
            Event::Acked { id } => (Event::ACKED, id),
            Event::Finished { id } => (Event::FINISHED, id),
        };
        OutboxMessage::new(event_type, id.to_string(), &event)
    }
}
