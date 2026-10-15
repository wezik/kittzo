use serde::{Deserialize, Serialize};

use crate::domain::version::Version;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(from = "u32", into = "u32")]
pub struct VersionDto(pub Version);

impl From<Version> for VersionDto {
    fn from(value: Version) -> Self {
        VersionDto(value)
    }
}

impl From<VersionDto> for Version {
    fn from(value: VersionDto) -> Self {
        value.0
    }
}

impl From<u32> for VersionDto {
    fn from(value: u32) -> Self {
        VersionDto(value.into())
    }
}

impl From<VersionDto> for u32 {
    fn from(value: VersionDto) -> Self {
        value.0.into()
    }
}
