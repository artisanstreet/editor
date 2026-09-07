//! Dependency-free boundary for one synchronous browser-DOM operation.
//!
//! This is the native counterpart of `lib/browser/dom.ts`. The caller supplies
//! the operation result; this module only preserves successful values and maps
//! a safe error-message projection to a typed failure. It does not retain an
//! opaque host cause, access a DOM, or provide a browser/runtime integration.

use std::error::Error;
use std::fmt;

/// The exact message used when a host operation does not supply an error
/// message.
pub const BROWSER_DOM_OPERATION_FAILED: &str = "Browser DOM operation failed.";

/// The safe error projection supplied by a synchronous host operation.
///
/// A later host adapter can turn its own error's message into
/// [`BrowserDomCause::Error`]
/// with [`Self::from_error`]. Any non-error or opaque value must use
/// [`Self::Opaque`], which deliberately carries no raw payload across this
/// boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BrowserDomCause {
    /// An error supplied an explicitly reader-facing message.
    Error(String),
    /// The operation failed with a non-error or otherwise opaque value.
    Opaque,
}

impl BrowserDomCause {
    /// Creates a cause carrying the exact supplied reader-facing message.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error(message.into())
    }

    /// Converts a Rust error's display message into the safe error projection.
    ///
    /// The error itself is not retained. Its [`fmt::Display`] output is the
    /// message supplied by this operation, matching the legacy boundary's
    /// use of an `Error`'s message while keeping the raw cause out of the
    /// frontend failure value.
    #[must_use]
    pub fn from_error(error: impl Error) -> Self {
        Self::error(error.to_string())
    }

    /// Creates an opaque cause that must use the fixed fallback message.
    #[must_use]
    pub const fn opaque() -> Self {
        Self::Opaque
    }

    fn into_message(self) -> String {
        match self {
            Self::Error(message) => message,
            Self::Opaque => BROWSER_DOM_OPERATION_FAILED.to_owned(),
        }
    }
}

/// A typed failure carrying only the safe reader-facing message.
///
/// No raw host cause is stored. Consequently, formatting and the
/// [`Error::source`] method cannot expose an opaque value through formatting
/// or an error chain.
#[must_use = "a browser DOM failure should be handled or returned"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserDomFailure {
    message: String,
}

impl BrowserDomFailure {
    /// Creates a failure from an already safe reader-facing message.
    #[must_use = "a browser DOM failure should be handled or returned"]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Maps the supplied error projection to its exact safe message.
    #[must_use = "a browser DOM failure should be handled or returned"]
    pub fn from_cause(cause: BrowserDomCause) -> Self {
        Self::new(cause.into_message())
    }

    /// Returns the exact reader-facing message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for BrowserDomFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for BrowserDomFailure {}

/// Runs one supplied synchronous host operation exactly once.
///
/// Successful values are returned without copying or transforming them. A
/// supplied error message, including an empty or Unicode message, is retained
/// byte-for-byte. An opaque cause becomes
/// [`BROWSER_DOM_OPERATION_FAILED`]. No retry, host access, or asynchronous
/// scheduling is performed here.
///
/// # Errors
///
/// Returns [`BrowserDomFailure`] when the supplied operation reports a
/// [`BrowserDomCause::Error`] or [`BrowserDomCause::Opaque`].
#[must_use = "a browser DOM operation result must be handled"]
pub fn run_browser_dom<Value, Operation>(operation: Operation) -> Result<Value, BrowserDomFailure>
where
    Operation: FnOnce() -> Result<Value, BrowserDomCause>,
{
    operation().map_err(BrowserDomFailure::from_cause)
}
