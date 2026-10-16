use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

pub trait Codec {
    fn encode<T: Serialize>(value: &T) -> Vec<u8>;
    fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboxMessage {
    pub id: Uuid,
    /// Used for identifying messages, deserialization, and grouping
    pub event_type: String,
    /// Determines ordering, messages using the same key will be processed in FIFO order
    pub aggregate_id: String,
    /// Payload stored in bytes
    pub payload: Vec<u8>,
}

impl OutboxMessage {
    pub fn new<T: Serialize>(message_key: String, aggregate_key: String, payload: &T) -> Self {
        Self {
            id: Uuid::new_v4(),
            // TODO: temporary mapping for new variables lives here, align [OutboxMessage] with new naming
            event_type: message_key,
            aggregate_id: aggregate_key,
            payload: serde_json::to_vec(payload).expect("serialize"), // acceptable unwrap
        }
    }

    pub fn to<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.payload).expect("deserialize") // acceptable unwrap
    }

    // pub fn from_event<C: Codec, E: DomainEvent>(event: &E) -> Self {
    //     Self {
    //         id: Uuid::new_v4(),
    //         event_type: event.event_type().to_string(),
    //         aggregate_id: event.aggregate_id().to_string(),
    //         payload: C::encode(event),
    //     }
    // }
}
