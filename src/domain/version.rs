#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Version(u32);

impl Version {
    pub const FIRST: Version = Version(0);

    pub fn next(self) -> Version {
        Version(self.0 + 1)
    }
}

impl From<Version> for u32 {
    fn from(value: Version) -> Self {
        value.0
    }
}

impl From<u32> for Version {
    fn from(value: u32) -> Self {
        Version(value)
    }
}

impl From<Version> for i64 {
    fn from(value: Version) -> Self {
        i64::from(value.0)
    }
}

impl TryFrom<i64> for Version {
    type Error = std::num::TryFromIntError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increments_correctly() {
        assert_eq!(Version::FIRST.next(), Version(1));
        assert_eq!(Version::FIRST.next().next(), Version(2));
    }
}
