//! Fresh-command admission inputs acquired at the process boundary.
//!
//! Forge owns durable identity: a fresh `CreateThread` or
//! `QueueFirstMessage` command receives a Forge-minted thread or message
//! identity plus one acceptance instant before any persistence runs
//! (`modules/domain/src/identifiers.rs`). This module owns exactly that
//! nondeterministic acquisition: operating-system entropy becomes bounded
//! opaque identity text, and the system clock becomes one signed
//! Unix-millisecond instant. It owns no credential material or
//! authentication lifecycle, no listener or runtime supervisor, no database
//! connection, and no dispatch executor, and it persists nothing itself.
//!
//! [`CommandOrigin`] deliberately stays narrow so external tests can inject
//! deterministic identities, instants, and acquisition failures without a
//! mock repository or an admission bypass. Production answers through
//! `RequestHandler::new`, which always consults the real
//! [`SystemCommandOrigin`].

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use artisan_domain::UnixMillis;
use thiserror::Error;

/// Cryptographically random bytes behind every forged identity.
///
/// 128 random bits keep accidental identity collisions negligible while the
/// encoded form stays far below the shared identifier byte ceiling.
const IDENTITY_RANDOM_BYTES: usize = 16;

/// UTF-8 length of one encoded identity: two lowercase hex digits per byte.
const IDENTITY_ENCODED_BYTES: usize = IDENTITY_RANDOM_BYTES * 2;

/// Lowercase hex alphabet used by the bounded identity encoder.
const IDENTITY_HEX_DIGITS: [char; 16] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
];

/// Operating-system entropy was unavailable while minting a fresh identity.
#[derive(Debug, Error)]
#[error("operating-system entropy failed while minting a fresh command identity: {source}")]
pub struct CommandOriginEntropyError {
    /// Typed source returned by the platform entropy provider.
    #[source]
    source: getrandom::Error,
}

impl CommandOriginEntropyError {
    /// Reports the entropy-unavailable failure kind with the provider's
    /// unexpected-error placeholder.
    ///
    /// Production minting always constructs this error from the real
    /// `getrandom::fill` result; injected origins use this constructor to
    /// exercise the admission failure policy without linking the platform
    /// provider themselves. Fabricating the error value claims no effect and
    /// bypasses nothing: admission sequencing and repository authority stay
    /// identical.
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            source: getrandom::Error::UNEXPECTED,
        }
    }
}

impl From<getrandom::Error> for CommandOriginEntropyError {
    fn from(source: getrandom::Error) -> Self {
        Self { source }
    }
}

/// The system clock offered no instant representable as signed epoch millis.
///
/// Conversion refuses to truncate, wrap, or clamp a reading: a magnitude
/// outside the signed `i64` millisecond range surfaces as this typed
/// failure instead of a silently shifted chronology.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[error("system clock produced no instant representable as signed unix milliseconds")]
pub struct CommandOriginClockError;

/// Narrow real-acquisition boundary behind fresh-command nondeterminism.
///
/// Implementations answer two questions only: which opaque identity text a
/// fresh thread or message carries, and which signed Unix-millisecond
/// instant accepted it. They hold no state between calls, keep no borrow
/// alive across an await, and never observe request payloads, receipts, or
/// persisted state: the request handler consults an origin only after
/// correlation validation and a receipt-lookup miss, so queries, exact
/// replays, and persisted conflicts never reach it.
pub trait CommandOrigin: fmt::Debug + Send + Sync {
    /// Mints one bounded opaque identity for a fresh thread or message.
    ///
    /// The returned text must satisfy the shared wire-identifier rule; the
    /// handler still applies the existing `ThreadId`/`MessageId` validation
    /// to every minted value.
    ///
    /// # Errors
    ///
    /// Returns [`CommandOriginEntropyError`] when the platform entropy
    /// provider fails. Nothing has been persisted and no partial admission
    /// state exists, so the identical request may simply try again later.
    fn mint_identity(&self) -> Result<String, CommandOriginEntropyError>;

    /// Acquires one acceptance instant as signed Unix epoch milliseconds.
    ///
    /// Instants before 1970 are valid negative domain values, mirroring the
    /// schema's deliberate signed millisecond columns.
    ///
    /// # Errors
    ///
    /// Returns [`CommandOriginClockError`] when the reading cannot convert
    /// into the complete signed range without narrowing.
    fn acceptance_instant(&self) -> Result<UnixMillis, CommandOriginClockError>;
}

/// Production origin answering from operating-system entropy and the wall
/// clock.
///
/// Identity text is opaque routing data, not secret capability material: it
/// is never scrubbed, rotated, or authenticated. This boundary therefore
/// reuses only the credential code's typed `getrandom::fill` idiom and none
/// of its secret lifecycle.
#[derive(Debug)]
pub struct SystemCommandOrigin;

impl CommandOrigin for SystemCommandOrigin {
    fn mint_identity(&self) -> Result<String, CommandOriginEntropyError> {
        // 128 random bits per identity keep collision odds negligible while
        // the encoded form stays far under the shared identifier ceiling.
        let mut material = [0_u8; IDENTITY_RANDOM_BYTES];
        getrandom::fill(&mut material)?;
        Ok(encode_identity(&material))
    }

    fn acceptance_instant(&self) -> Result<UnixMillis, CommandOriginClockError> {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(ahead_of_epoch) => Ok(UnixMillis::from_millis(representable_millis(
                ahead_of_epoch,
            )?)),
            Err(before_epoch) => {
                let magnitude = representable_millis(before_epoch.duration())?;
                let millis = magnitude.checked_neg().ok_or(CommandOriginClockError)?;
                Ok(UnixMillis::from_millis(millis))
            }
        }
    }
}

/// Converts a positive clock difference without truncation or wrapping.
fn representable_millis(duration: Duration) -> Result<i64, CommandOriginClockError> {
    i64::try_from(duration.as_millis()).map_err(|_| CommandOriginClockError)
}

/// Encodes random bytes as lowercase hex inside the identifier bound.
fn encode_identity(material: &[u8; IDENTITY_RANDOM_BYTES]) -> String {
    let mut identity = String::with_capacity(IDENTITY_ENCODED_BYTES);
    for &byte in material {
        identity.push(IDENTITY_HEX_DIGITS[usize::from(byte >> 4)]);
        identity.push(IDENTITY_HEX_DIGITS[usize::from(byte & 0x0F)]);
    }
    identity
}
