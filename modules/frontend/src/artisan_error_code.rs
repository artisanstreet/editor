//! Stable, safe error codes for failures visible in the Artisan frontend.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/errors/artisan-error-code.ts`. The enum is the
//! complete allowlist of user-visible failures. Its code and message mappings
//! are fixed at compile time, so transport or provider causes never enter the
//! rendered text.

#![allow(clippy::module_name_repetitions)]

use std::fmt;
use std::str::FromStr;

/// One of the stable error codes that the Artisan frontend may expose.
///
/// This enum is intentionally exhaustive. A transport error must first be
/// mapped to one of these safe frontend outcomes; arbitrary transport causes
/// are not represented here and cannot be formatted for the user.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ArtisanErrorCode {
    /// The client or its event capacity has been exhausted.
    ClientCapacityExceeded,
    /// Required local client configuration is missing or invalid.
    ClientConfiguration,
    /// The client connection has already been disposed.
    ClientDisposed,
    /// Client and local-service state no longer agree.
    ClientStateFailure,
    /// The local Forge service could not be reached.
    ConnectionUnavailable,
    /// Project hydration failed.
    HydrationProjectsFailed,
    /// Session-default hydration failed.
    HydrationSessionDefaultsFailed,
    /// Thread hydration failed.
    HydrationThreadsFailed,
    /// The window must be paired with Forge.
    PairingRequired,
    /// Client and Forge could not agree on their local protocol.
    ProtocolFailure,
    /// The local transport response was malformed.
    TransportMalformed,
}

impl ArtisanErrorCode {
    /// Every stable code, in the same canonical order as the TypeScript map.
    pub const ALL: [Self; 11] = [
        Self::ClientCapacityExceeded,
        Self::ClientConfiguration,
        Self::ClientDisposed,
        Self::ClientStateFailure,
        Self::ConnectionUnavailable,
        Self::HydrationProjectsFailed,
        Self::HydrationSessionDefaultsFailed,
        Self::HydrationThreadsFailed,
        Self::PairingRequired,
        Self::ProtocolFailure,
        Self::TransportMalformed,
    ];

    /// Returns the exact stable code sent across the frontend boundary.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClientCapacityExceeded => "CLIENT_CAPACITY_EXCEEDED",
            Self::ClientConfiguration => "CLIENT_CONFIGURATION",
            Self::ClientDisposed => "CLIENT_DISPOSED",
            Self::ClientStateFailure => "CLIENT_STATE_FAILURE",
            Self::ConnectionUnavailable => "CONNECTION_UNAVAILABLE",
            Self::HydrationProjectsFailed => "HYDRATION_PROJECTS_FAILED",
            Self::HydrationSessionDefaultsFailed => "HYDRATION_SESSION_DEFAULTS_FAILED",
            Self::HydrationThreadsFailed => "HYDRATION_THREADS_FAILED",
            Self::PairingRequired => "PAIRING_REQUIRED",
            Self::ProtocolFailure => "PROTOCOL_FAILURE",
            Self::TransportMalformed => "TRANSPORT_MALFORMED",
        }
    }

    /// Returns the exact user-visible message for this code.
    ///
    /// The returned message is fixed for each enum variant. No caller-supplied
    /// transport or provider detail is accepted by this mapping.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::ClientCapacityExceeded => "Artisan is temporarily at capacity. Please try again.",
            Self::ClientConfiguration => {
                "Artisan needs local configuration before it can continue."
            }
            Self::ClientDisposed => "The Artisan connection was closed. Please reconnect.",
            Self::ClientStateFailure => {
                "Artisan lost synchronization with the local service. Please reconnect."
            }
            Self::ConnectionUnavailable => "Artisan could not reach the local Forge service.",
            Self::HydrationProjectsFailed => "Artisan could not load your projects.",
            Self::HydrationSessionDefaultsFailed => "Artisan could not load your session defaults.",
            Self::HydrationThreadsFailed => "Artisan could not load your threads.",
            Self::PairingRequired => "This Artisan window needs to be paired with Forge.",
            Self::ProtocolFailure => "Artisan and Forge could not agree on their local protocol.",
            Self::TransportMalformed => "Artisan received an invalid local transport response.",
        }
    }

    /// Parses one exact stable code.
    ///
    /// Parsing is case-sensitive and does not trim or accept aliases. Unknown
    /// input is rejected instead of being converted into a catch-all variant.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "CLIENT_CAPACITY_EXCEEDED" => Some(Self::ClientCapacityExceeded),
            "CLIENT_CONFIGURATION" => Some(Self::ClientConfiguration),
            "CLIENT_DISPOSED" => Some(Self::ClientDisposed),
            "CLIENT_STATE_FAILURE" => Some(Self::ClientStateFailure),
            "CONNECTION_UNAVAILABLE" => Some(Self::ConnectionUnavailable),
            "HYDRATION_PROJECTS_FAILED" => Some(Self::HydrationProjectsFailed),
            "HYDRATION_SESSION_DEFAULTS_FAILED" => Some(Self::HydrationSessionDefaultsFailed),
            "HYDRATION_THREADS_FAILED" => Some(Self::HydrationThreadsFailed),
            "PAIRING_REQUIRED" => Some(Self::PairingRequired),
            "PROTOCOL_FAILURE" => Some(Self::ProtocolFailure),
            "TRANSPORT_MALFORMED" => Some(Self::TransportMalformed),
            _ => None,
        }
    }
}

impl fmt::Display for ArtisanErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ArtisanErrorCode {
    type Err = UnknownArtisanErrorCode;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input).ok_or(UnknownArtisanErrorCode)
    }
}

/// Payload-free error returned when a stable Artisan error code is unknown.
///
/// Keeping this error unit-like is deliberate: rejected external input is not
/// retained or exposed through formatting or debug output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownArtisanErrorCode;

impl fmt::Display for UnknownArtisanErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unknown Artisan error code")
    }
}

impl std::error::Error for UnknownArtisanErrorCode {}

/// Formats a stable Artisan error without exposing a transport cause.
#[must_use]
pub const fn format_artisan_error(code: ArtisanErrorCode) -> &'static str {
    code.message()
}
