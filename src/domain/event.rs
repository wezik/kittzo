use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxMessage {
    pub id: Uuid,
    pub event_type: String,
    pub aggregate_id: String,
    pub payload: Vec<u8>,
}

impl OutboxMessage {
    pub fn new<T: Serialize>(event_type: &str, aggregate_id: String, payload: &T) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_type: event_type.to_string(),
            aggregate_id,
            payload: serde_json::to_vec(payload).expect("serialize"),
        }
    }
}
