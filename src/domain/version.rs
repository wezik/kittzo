/// Version of a domain object.
///
/// Domain objects are versioned to deal with concurrent modification: each action on an
/// object increments its version, and callers can pass back the version they last saw so
/// the system can tell a stale write from a current one and reject it, rather than
/// silently overwriting a concurrent change.
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
    fn next_increments() {
        assert_eq!(Version::FIRST.next(), Version(2));
        assert_eq!(Version::FIRST.next().next(), Version(3));
    }
}
