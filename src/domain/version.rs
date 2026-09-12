#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Version(u32);

impl Version {
    pub const FIRST: Version = Version(1);

    pub fn next(self) -> Version {
        Version(self.0 + 1)
    }

    pub fn as_u32(self) -> u32 {
        self.0
    }

    pub fn from_u32(value: u32) -> Version {
        Version(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increments_correctly() {
        assert_eq!(Version::FIRST.next(), Version(2));
        assert_eq!(Version::FIRST.next().next(), Version(3));
    }
}
