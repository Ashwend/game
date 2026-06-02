//! Minimal semantic-version parse + compare for release tags.
//!
//! The only versions this code ever compares are the running build's
//! `CARGO_PKG_VERSION` (`MAJOR.MINOR.PATCH`) and GitHub release tags
//! (`vMAJOR.MINOR.PATCH`, produced by the release pipeline). That's the whole
//! grammar, so a three-`u32` newtype is enough, the full `semver` crate
//! (pre-release identifiers, build metadata, range matching) would be dead
//! weight here.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version {
    pub(crate) major: u32,
    pub(crate) minor: u32,
    pub(crate) patch: u32,
}

impl Version {
    /// Parse `MAJOR.MINOR.PATCH`, tolerating a leading `v`/`V` and surrounding
    /// whitespace (release tags carry the `v`; `CARGO_PKG_VERSION` does not).
    /// Returns `None` for anything that isn't exactly three numeric fields, so
    /// a malformed or non-semver tag is simply ignored by the caller rather
    /// than mis-ordered.
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        let trimmed = trimmed.strip_prefix(['v', 'V']).unwrap_or(trimmed);
        let mut parts = trimmed.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        // Reject trailing junk like `1.2.3.4` or `1.2.3-rc1` so we never claim
        // an unparseable tail is "equal".
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    /// The running build's version, parsed from `CARGO_PKG_VERSION`. Panics
    /// only if the crate's own version stopped being valid semver, which would
    /// be a build-time mistake, not a runtime condition.
    pub(crate) fn current() -> Self {
        Self::parse(crate::protocol::GAME_VERSION)
            .expect("CARGO_PKG_VERSION is always MAJOR.MINOR.PATCH")
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_v_prefixed() {
        assert_eq!(
            Version::parse("1.2.3"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(Version::parse("v0.16.2"), Version::parse("0.16.2"));
        assert_eq!(Version::parse("  V10.0.1  "), Version::parse("10.0.1"));
    }

    #[test]
    fn rejects_non_three_field_versions() {
        assert_eq!(Version::parse("1.2"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse("1.2.x"), None);
        assert_eq!(Version::parse("1.2.3-rc1"), None);
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("v"), None);
    }

    #[test]
    fn orders_by_major_then_minor_then_patch() {
        let a = Version::parse("0.16.2").unwrap();
        let b = Version::parse("0.17.0").unwrap();
        let c = Version::parse("1.0.0").unwrap();
        assert!(a < b);
        assert!(b < c);
        assert!(a < c);
        assert_eq!(a, Version::parse("v0.16.2").unwrap());
        // Patch-level ordering.
        assert!(Version::parse("0.16.1").unwrap() < Version::parse("0.16.10").unwrap());
    }

    #[test]
    fn current_matches_cargo_pkg_version() {
        // Whatever the crate version is, it must parse and round-trip.
        let current = Version::current();
        assert_eq!(current.to_string(), crate::protocol::GAME_VERSION);
    }
}
