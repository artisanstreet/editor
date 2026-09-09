//! Hermes version parsing and minimum-version gate.
//!
//! TypeScript evidence (`modules/engines/src/hermes/service.ts`):
//! versions come from `hermes --version` standard output matched with
//! `/Hermes Agent v(\d+)\.(\d+)\.(\d+)/i`, then gated lexicographically
//! against `minimum_hermes_version` (`[0, 20, 0]`). The native parser accepts
//! the same unanchored, case-insensitive prefix and additionally captures an
//! optional `-prerelease` suffix, which the TypeScript match silently skips.

use std::fmt;

/// Minimum supported Hermes version, mirroring `minimum_hermes_version`.
pub const MINIMUM_HERMES_VERSION: [u64; 3] = [0, 20, 0];

/// A parsed Hermes `MAJOR.MINOR.PATCH` version with optional prerelease.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HermesVersion {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: Option<String>,
}

impl HermesVersion {
    /// Returns the major component.
    #[must_use]
    pub const fn major(&self) -> u64 {
        self.major
    }

    /// Returns the minor component.
    #[must_use]
    pub const fn minor(&self) -> u64 {
        self.minor
    }

    /// Returns the patch component.
    #[must_use]
    pub const fn patch(&self) -> u64 {
        self.patch
    }

    /// Returns the captured prerelease suffix without the leading `-`, if any.
    #[must_use]
    pub fn prerelease(&self) -> Option<&str> {
        self.prerelease.as_deref()
    }

    /// Returns the numeric `[major, minor, patch]` triple.
    #[must_use]
    pub const fn triple(&self) -> [u64; 3] {
        [self.major, self.minor, self.patch]
    }

    /// Returns whether this version passes the minimum-version gate.
    ///
    /// Compares lexicographically exactly like the TypeScript loop: the first
    /// nonzero component difference decides, and an all-equal version passes.
    #[must_use]
    pub fn meets_minimum(&self) -> bool {
        let [major, minor, patch] = self.triple();
        let [minimum_major, minimum_minor, minimum_patch] = MINIMUM_HERMES_VERSION;
        if major != minimum_major {
            return major > minimum_major;
        }
        if minor != minimum_minor {
            return minor > minimum_minor;
        }
        patch >= minimum_patch
    }
}

impl fmt::Display for HermesVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(prerelease) = &self.prerelease {
            write!(formatter, "-{prerelease}")?;
        }
        Ok(())
    }
}

/// Failure from [`parse_hermes_version`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HermesVersionError {
    /// No `Hermes Agent vMAJOR.MINOR.PATCH` pattern was present.
    Unrecognized,
}

impl fmt::Display for HermesVersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unrecognized => {
                formatter.write_str("hermes returned an unrecognized version string")
            }
        }
    }
}

impl std::error::Error for HermesVersionError {}

fn take_number(text: &str) -> Option<(u64, &str)> {
    let length = text.bytes().take_while(u8::is_ascii_digit).count();
    if length == 0 {
        return None;
    }
    let (digits, rest) = text.split_at(length);
    let mut value: u64 = 0;
    for byte in digits.bytes() {
        value = value.checked_mul(10)?.checked_add(u64::from(byte - b'0'))?;
    }
    Some((value, rest))
}

fn is_prerelease_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-'
}

/// Parses a Hermes version from `--version` standard output.
///
/// Matches the TypeScript pattern case-insensitively and unanchored, then
/// captures an optional `-prerelease` suffix (`[0-9A-Za-z.-]+`). Any other
/// trailing text is ignored exactly like the unanchored TypeScript match.
///
/// # Errors
///
/// Returns [`HermesVersionError::Unrecognized`] when the output holds no
/// `Hermes Agent vMAJOR.MINOR.PATCH` pattern or a component overflows `u64`.
pub fn parse_hermes_version(output: &str) -> Result<HermesVersion, HermesVersionError> {
    const PREFIX: &[u8] = b"hermes agent v";
    let bytes = output.as_bytes();
    let mut start = None;
    if bytes.len() >= PREFIX.len() {
        for index in 0..=bytes.len() - PREFIX.len() {
            if bytes[index..index + PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
                // The match is pure ASCII, so both ends are char boundaries.
                start = Some(index);
                break;
            }
        }
    }
    let start = start.ok_or(HermesVersionError::Unrecognized)?;
    let mut rest = output
        .get(start + PREFIX.len()..)
        .ok_or(HermesVersionError::Unrecognized)?;
    let (major, tail) = take_number(rest).ok_or(HermesVersionError::Unrecognized)?;
    rest = tail
        .strip_prefix('.')
        .ok_or(HermesVersionError::Unrecognized)?;
    let (minor, tail) = take_number(rest).ok_or(HermesVersionError::Unrecognized)?;
    rest = tail
        .strip_prefix('.')
        .ok_or(HermesVersionError::Unrecognized)?;
    let (patch, tail) = take_number(rest).ok_or(HermesVersionError::Unrecognized)?;
    rest = tail;
    let mut prerelease = None;
    if let Some(dashed) = rest.strip_prefix('-') {
        let length = dashed
            .bytes()
            .take_while(|byte| is_prerelease_byte(*byte))
            .count();
        if length > 0 {
            prerelease = dashed.get(..length).map(str::to_string);
        }
        // A lone `-` with no valid prerelease body (or any other trailing
        // text) is ignored exactly like the unanchored TypeScript match.
    }
    Ok(HermesVersion {
        major,
        minor,
        patch,
        prerelease,
    })
}
