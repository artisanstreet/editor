//! Dependency-free typed helpers for the frontend runtime fixture.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/runtime/fixtures/support.ts`. It models the
//! values produced by the fixture helpers without importing the fixture
//! dataset, a transport client, Effect, or any asynchronous runtime. Preview
//! targets are supplied by the caller and are returned by reference, so their
//! opaque fields and input order remain untouched.

#![allow(clippy::module_name_repetitions)]

/// The journal sequence used by a fixture receipt when no override is given.
pub const DEFAULT_FIXTURE_JOURNAL_SEQUENCE: u64 = 48;

/// The client error code used by every fixture failure.
pub const FIXTURE_FAILURE_CODE: &str = "protocol";

/// The protocol error code used by every fixture failure.
pub const FIXTURE_FAILURE_PROTOCOL_CODE: &str = "fixture_not_found";

/// The retryability value used by every fixture failure.
pub const FIXTURE_FAILURE_RETRYABLE: bool = false;

/// The receipt status returned by the fixture helper.
pub const FIXTURE_RECEIPT_STATUS: &str = "accepted";

/// A typed failure produced by a frontend fixture helper.
///
/// The fixture helper has no transport cause, so [`Self::cause`] is always
/// `None`. The string fields retain the exact client-facing values from the
/// TypeScript error object; only `message` is supplied by the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureClientError {
    /// No transport or runtime cause is attached to a fixture failure.
    pub cause: Option<String>,
    /// Exact client error classification.
    pub code: &'static str,
    /// Caller-supplied failure message.
    pub message: String,
    /// Exact protocol-level fixture classification.
    pub protocol_code: &'static str,
    /// Fixture failures must never be retried.
    pub retryable: bool,
}

impl FixtureClientError {
    /// Creates a fixture failure with the exact fixed fields.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            cause: None,
            code: FIXTURE_FAILURE_CODE,
            message: message.into(),
            protocol_code: FIXTURE_FAILURE_PROTOCOL_CODE,
            retryable: FIXTURE_FAILURE_RETRYABLE,
        }
    }
}

/// Creates the typed failure returned by a missing or unavailable fixture.
#[must_use]
pub fn fixture_failure(message: impl Into<String>) -> FixtureClientError {
    FixtureClientError::new(message)
}

/// A typed accepted receipt produced by a frontend fixture helper.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureReceipt {
    /// Caller-supplied command identity, retained byte-for-byte as UTF-8.
    pub command_id: String,
    /// Journal sequence selected for this receipt.
    pub journal_sequence: u64,
    /// Exact fixture receipt status.
    pub status: &'static str,
}

impl FixtureReceipt {
    /// Creates a receipt, using the fixture default when `journal_sequence`
    /// is absent.
    ///
    /// `Some(0)` and every other supplied value are explicit overrides; only
    /// `None` selects [`DEFAULT_FIXTURE_JOURNAL_SEQUENCE`].
    #[must_use]
    pub fn new(command_id: impl Into<String>, journal_sequence: Option<u64>) -> Self {
        Self {
            command_id: command_id.into(),
            journal_sequence: journal_sequence.unwrap_or(DEFAULT_FIXTURE_JOURNAL_SEQUENCE),
            status: FIXTURE_RECEIPT_STATUS,
        }
    }
}

/// Creates the typed accepted fixture receipt.
#[must_use]
pub fn fixture_receipt(
    command_id: impl Into<String>,
    journal_sequence: Option<u64>,
) -> FixtureReceipt {
    FixtureReceipt::new(command_id, journal_sequence)
}

/// A caller-supplied preview target used by fixture lookup.
///
/// `fields` is intentionally opaque to this module: it can be the native
/// projection of every protocol target field, while this helper only needs
/// the stable target ID. Keeping it generic avoids porting the fixture
/// dataset or introducing a protocol dependency before that boundary is
/// reached.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixturePreviewTarget<T = String> {
    /// Stable target identifier used by lookup.
    pub id: String,
    /// Caller-supplied target fields, preserved without inspection or reorder.
    pub fields: T,
}

impl<T> FixturePreviewTarget<T> {
    /// Creates a caller-supplied target value for fixture lookup.
    #[must_use]
    pub fn new(id: impl Into<String>, fields: T) -> Self {
        Self {
            id: id.into(),
            fields,
        }
    }
}

/// Returns the first supplied preview target whose ID exactly matches
/// `target_id`.
///
/// Matching is case-sensitive and does not normalize or trim either value.
/// The returned reference keeps every target field and the caller's target
/// order intact. An empty slice therefore produces the same typed failure as
/// any other miss.
///
/// # Errors
///
/// Returns [`FixtureClientError`] with the exact message
/// `Unknown fixture preview target: <target_id>` when no target matches.
#[must_use = "handle the fixture preview-target lookup result"]
pub fn fixture_preview_target<T>(
    targets: &[FixturePreviewTarget<T>],
    target_id: impl AsRef<str>,
) -> Result<&FixturePreviewTarget<T>, FixtureClientError> {
    let target_id = target_id.as_ref();
    targets
        .iter()
        .find(|candidate| candidate.id == target_id)
        .ok_or_else(|| fixture_failure(format!("Unknown fixture preview target: {target_id}")))
}
