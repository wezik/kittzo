use uuid::Uuid;

use crate::domain::event::OutboxMessage;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Event {
    Created { id: Uuid },
    Acked { id: Uuid },
    Finished { id: Uuid },
}

impl From<Event> for OutboxMessage {
    fn from(event: Event) -> Self {
        let (msg_key, agg_key) = match event {
            Event::Created { id } => ("PAYMENT_CREATED", id),
            Event::Acked { id } => ("PAYMENT_ACKED", id),
            Event::Finished { id } => ("PAYMENT_FINISHED", id),
        };
        OutboxMessage::new(msg_key.into(), agg_key.into(), &event)
    }
}
