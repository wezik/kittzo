use serde::{Serialize, de::DeserializeOwned};

use crate::domain::event::Codec;

pub struct JsonCodec;
impl Codec for JsonCodec {
    fn encode<T: Serialize>(value: &T) -> Vec<u8> {
        serde_json::to_vec(value).expect("serialize")
    }

    fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>> {
        Ok(serde_json::from_slice(bytes)?)
    }
}
