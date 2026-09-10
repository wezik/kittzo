use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;

/// Wire-format newtype for response/request DTOs: use this as the field type instead
/// of repeating `#[serde(with = "time::serde::rfc3339")]` (and its `::option` variant)
/// on every timestamp field — `Option<OffsetDateTimeDto>` just works, no separate
/// annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct OffsetDateTimeDto(pub OffsetDateTime);

impl From<OffsetDateTime> for OffsetDateTimeDto {
    fn from(value: OffsetDateTime) -> Self {
        OffsetDateTimeDto(value)
    }
}

impl From<OffsetDateTimeDto> for OffsetDateTime {
    fn from(value: OffsetDateTimeDto) -> Self {
        value.0
    }
}

impl Serialize for OffsetDateTimeDto {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        time::serde::rfc3339::serialize(&self.0, serializer)
    }
}

impl<'de> Deserialize<'de> for OffsetDateTimeDto {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        time::serde::rfc3339::deserialize(deserializer).map(OffsetDateTimeDto)
    }
}
