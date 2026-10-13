//! Semantic version ordering for the update check.
//!
//! A release tag is compared against `CARGO_PKG_VERSION`. Both are
//! `MAJOR.MINOR.PATCH` with an optional leading `v`, and the comparison must be
//! numeric: `0.2.10` is newer than `0.2.9`, which a string compare gets wrong.
//!
//! Nothing else about a version is modelled — no pre-release, no build
//! metadata. The release chain publishes plain stable tags and
//! `releases/latest` excludes pre-releases and drafts by definition, so a
//! parser that accepted a suffix would be inventing a policy the product does
//! not have. A form this type cannot read is `None`, never a guessed value;
//! the caller decides what to say about it.

/// A parsed `MAJOR.MINOR.PATCH`.
///
/// Ordering is derived component by component, which is exactly the semantic
/// comparison the update check needs.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// Parses `1.2.3` or `v1.2.3`.
    ///
    /// A missing component, a non-digit, or a trailing component (such as the
    /// `-rc1` of a pre-release) is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.');
        let major: u64 = parts.next()?.parse().ok()?;
        let minor: u64 = parts.next()?.parse().ok()?;
        let patch: u64 = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_is_numeric_not_lexical() {
        let older = Version::parse("0.2.9").unwrap();
        let newer = Version::parse("0.2.10").unwrap();
        assert!(newer > older, "0.2.10 must be newer than 0.2.9");
        assert!(Version::parse("0.10.0").unwrap() > Version::parse("0.9.9").unwrap());
        assert!(Version::parse("1.0.0").unwrap() > Version::parse("0.99.99").unwrap());
    }

    #[test]
    fn a_leading_v_is_the_same_version() {
        assert_eq!(
            Version::parse("v0.2.6"),
            Version::parse("0.2.6"),
            "a tag prefix is not part of the version"
        );
    }

    #[test]
    fn only_the_plain_stable_form_is_read() {
        for rejected in [
            "",
            "0",
            "0.2",
            "0.2.6-rc1",
            "0.2.6.1",
            "v",
            "x.y.z",
            "-1.0.0",
        ] {
            assert_eq!(Version::parse(rejected), None, "{rejected:?} is not stable");
        }
    }

    #[test]
    fn display_round_trips_the_parsed_form() {
        let version = Version::parse("v0.2.6").unwrap();
        assert_eq!(version.to_string(), "0.2.6");
    }
}
