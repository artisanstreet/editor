//! Bounded, validated text values carried by the first native workflow.
//!
//! All bounds are UTF-8 bytes (see [`crate::bounds`]). Validation follows the
//! legacy trim-then-reject rule
//! (`modules/frontend/src/lib/thread-interaction/commands.ts` trims a
//! submission and refuses blank text): a value must keep visible characters
//! after trimming, and its full UTF-8 length must fit the documented ceiling.
//! The original text is stored untrimmed so the domain never silently rewrites
//! client content.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

/// Shape shared by every bounded-text validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextViolation {
    /// No visible characters remained after trimming.
    Blank,
    /// The UTF-8 byte length exceeded the ceiling.
    TooLong { length: usize },
}

/// Checks an external value against a trim-non-empty rule and byte ceiling.
fn validate_bounded_text(value: &str, maximum: usize) -> Result<(), TextViolation> {
    if value.trim().is_empty() {
        return Err(TextViolation::Blank);
    }

    let length = value.len();
    if length > maximum {
        return Err(TextViolation::TooLong { length });
    }

    Ok(())
}

/// Generates one bounded text value over its handwritten error enum.
///
/// Every paired error enum exposes the same two variants, `Blank` and
/// `TooLong { length, maximum }`, so the shared mapping below stays total.
macro_rules! bounded_text {
    (
        $(#[$type_docs:meta])*
        $name:ident, $error_name:ident, $maximum:path
    ) => {
        $(#[$type_docs])*
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Maximum UTF-8 byte length accepted for this value.
            pub const MAX_BYTES: usize = $maximum;

            /// Creates the value after validating the external text.
            ///
            /// # Errors
            ///
            /// Returns the paired error enum's `Blank` variant when no
            /// visible characters remain after trimming, or its `TooLong`
            /// variant carrying the offending length when the text exceeds
            /// `MAX_BYTES` UTF-8 bytes.
            pub fn parse(value: impl Into<String>) -> Result<Self, $error_name> {
                let value = value.into();
                match validate_bounded_text(&value, Self::MAX_BYTES) {
                    Ok(()) => Ok(Self(value)),
                    Err(TextViolation::Blank) => Err($error_name::Blank),
                    Err(TextViolation::TooLong { length }) => Err($error_name::TooLong {
                        length,
                        maximum: Self::MAX_BYTES,
                    }),
                }
            }

            /// Returns the validated text exactly as supplied.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = $error_name;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }
    };
}

bounded_text! {
    /// Title of one thread, validated and bounded for wire transport.
    ThreadTitle, ThreadTitleError, crate::bounds::THREAD_TITLE_MAX_BYTES
}

/// Validation failure for [`ThreadTitle`].
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ThreadTitleError {
    /// No visible characters remained after trimming.
    #[error("thread title must contain visible characters")]
    Blank,
    /// The title exceeded its documented UTF-8 byte ceiling.
    #[error("thread title is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// Offending length in UTF-8 bytes.
        length: usize,
        /// the documented ceiling ([`crate::bounds::THREAD_TITLE_MAX_BYTES`]).
        maximum: usize,
    },
}

bounded_text! {
    /// Forge-supplied display label for a project or directory.
    DisplayName, DisplayNameError, crate::bounds::DISPLAY_NAME_MAX_BYTES
}

/// Validation failure for [`DisplayName`].
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum DisplayNameError {
    /// No visible characters remained after trimming.
    #[error("display name must contain visible characters")]
    Blank,
    /// The display name exceeded its documented UTF-8 byte ceiling.
    #[error("display name is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// Offending length in UTF-8 bytes.
        length: usize,
        /// the documented ceiling ([`crate::bounds::DISPLAY_NAME_MAX_BYTES`]).
        maximum: usize,
    },
}

bounded_text! {
    /// Verbatim project root path description served by Forge.
    ///
    /// Opaque display and reference data only: the domain performs no
    /// canonicalization or containment checks, and host path semantics stay
    /// in backend services.
    RootPath, RootPathError, crate::bounds::ROOT_PATH_MAX_BYTES
}

/// Validation failure for [`RootPath`].
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum RootPathError {
    /// No visible characters remained after trimming.
    #[error("root path must contain visible characters")]
    Blank,
    /// The root path exceeded its documented UTF-8 byte ceiling.
    #[error("root path is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// Offending length in UTF-8 bytes.
        length: usize,
        /// the documented ceiling ([`crate::bounds::ROOT_PATH_MAX_BYTES`]).
        maximum: usize,
    },
}

bounded_text! {
    /// Body of the first durably queued message in a thread.
    ///
    /// Preserves the legacy 65,536 body ceiling while rejecting empty or
    /// whitespace-only submissions before anything durable is created.
    MessageBody, MessageBodyError, crate::bounds::MESSAGE_BODY_MAX_BYTES
}

/// Validation failure for [`MessageBody`].
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum MessageBodyError {
    /// No visible characters remained after trimming.
    #[error("message body must contain visible characters")]
    Blank,
    /// The body exceeded its documented UTF-8 byte ceiling.
    #[error("message body is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// Offending length in UTF-8 bytes.
        length: usize,
        /// the documented ceiling ([`crate::bounds::MESSAGE_BODY_MAX_BYTES`]).
        maximum: usize,
    },
}
