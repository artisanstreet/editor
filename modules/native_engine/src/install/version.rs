//! Managed engine release versions and their total order.
//!
//! Vendor feeds publish `X.Y.Z` releases, optionally with a pre-release
//! suffix (`0.0.0-beta-19271`). Ordering compares the numeric release first;
//! a release without a suffix sorts after the same release with one, and
//! suffixes compare as natural text so `beta-9999 < beta-10000`.

use std::{cmp::Ordering, fmt};

const MAX_VERSION_BYTES: usize = 128;

/// One validated engine release version.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct EngineVersion {
    release: [u64; 3],
    suffix: Option<String>,
    text: String,
}

impl EngineVersion {
    /// Parses an exact `X.Y.Z` or `X.Y.Z-suffix` version.
    ///
    /// Suffixes contain only ASCII alphanumerics, `.` and `-`. Build metadata
    /// (`+…`), leading `v`, and surrounding text are rejected: feeds must
    /// publish exact versions.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        if value.is_empty() || value.len() > MAX_VERSION_BYTES || !value.is_ascii() {
            return None;
        }
        let (release, suffix) = value
            .split_once('-')
            .map_or((value, None), |(release, suffix)| (release, Some(suffix)));
        let mut parts = release.split('.');
        let mut numbers = [0_u64; 3];
        for number in &mut numbers {
            let part = parts.next()?;
            if part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
            {
                return None;
            }
            *number = part.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        if let Some(suffix) = suffix
            && (suffix.is_empty()
                || !suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
        {
            return None;
        }
        Some(Self {
            release: numbers,
            suffix: suffix.map(str::to_owned),
            text: value.to_owned(),
        })
    }

    /// Returns the exact version text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Returns whether this version carries a pre-release suffix.
    #[must_use]
    pub fn has_suffix(&self) -> bool {
        self.suffix.is_some()
    }

    /// Returns the pre-release suffix, if any.
    #[must_use]
    pub fn suffix(&self) -> Option<&str> {
        self.suffix.as_deref()
    }
}

impl fmt::Display for EngineVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

impl Ord for EngineVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        self.release
            .cmp(&other.release)
            .then_with(|| match (&self.suffix, &other.suffix) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(left), Some(right)) => natural_cmp(left, right),
            })
    }
}

impl PartialOrd for EngineVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Compares text by alternating digit and non-digit runs, digits numerically.
fn natural_cmp(left: &str, right: &str) -> Ordering {
    let mut left_runs = runs(left);
    let mut right_runs = runs(right);
    loop {
        match (left_runs.next(), right_runs.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left), Some(right)) => {
                let ordering = match (is_digits(left), is_digits(right)) {
                    (true, true) => {
                        let left = left.trim_start_matches('0');
                        let right = right.trim_start_matches('0');
                        left.len().cmp(&right.len()).then_with(|| left.cmp(right))
                    }
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    (false, false) => left.cmp(right),
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

fn is_digits(run: &str) -> bool {
    run.bytes().all(|byte| byte.is_ascii_digit())
}

fn runs(value: &str) -> impl Iterator<Item = &str> {
    let bytes = value.as_bytes();
    let mut start = 0;
    std::iter::from_fn(move || {
        if start >= bytes.len() {
            return None;
        }
        let digit = bytes[start].is_ascii_digit();
        let end = bytes[start..]
            .iter()
            .position(|byte| byte.is_ascii_digit() != digit)
            .map_or(bytes.len(), |offset| start + offset);
        let run = &value[start..end];
        start = end;
        Some(run)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(value: &str) -> EngineVersion {
        EngineVersion::parse(value).unwrap()
    }

    #[test]
    fn parses_exact_release_versions_only() {
        for valid in [
            "2.1.282",
            "0.156.0",
            "0.0.0-beta-19271",
            "0.158.0-alpha.2.1",
        ] {
            assert_eq!(version(valid).as_str(), valid);
        }
        for invalid in [
            "",
            "2.1",
            "2.1.282.1",
            "v2.1.282",
            "2.1.282+build",
            "2.1.x",
            "02.1.0",
            "2.1.0-",
            "2.1.0-a b",
            " 2.1.0",
        ] {
            assert!(EngineVersion::parse(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn orders_releases_numerically_and_suffixes_naturally() {
        assert!(version("2.1.282") > version("2.1.220"));
        assert!(version("2.10.0") > version("2.9.9"));
        assert!(version("0.157.0") > version("0.157.0-alpha.2"));
        assert!(version("0.0.0-beta-19271") > version("0.0.0-beta-17778"));
        assert!(version("0.0.0-beta-10000") > version("0.0.0-beta-9999"));
        assert!(version("0.158.0-alpha.10") > version("0.158.0-alpha.9"));
        assert_eq!(
            version("1.0.0").cmp(&version("1.0.0")),
            std::cmp::Ordering::Equal
        );
    }
}
