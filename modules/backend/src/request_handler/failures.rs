//! Shared protocol-outcome framing and failure classification for the
//! request handler.
//!
//! Each helper here is pure: it maps one repository, admission, or
//! controller error into the protocol's typed `ProtocolFailure` contract
//! without touching durable state. The parent module imports these items
//! privately, so the root dispatcher, descendant handler modules, and
//! the path-linked composer-state leaf resolve them through one name.

use artisan_database::RepositoryError;
use artisan_domain::{DirectoryId, RequestId};
use artisan_protocol::{ErrorCode, ErrorDetail, ProtocolFailure, ResponsePayload, ServerResponse};

use crate::command_admission::{CommandOriginClockError, CommandOriginEntropyError};
use crate::conversation_subscription_preparation::PrepareSubscriptionError;
use crate::conversation_subscription_registry::RegisterError;
use crate::run_cancellation::RunCancellationError;

use super::{
    BOUNDED_DETAIL_FALLBACK, RESNAPSHOT_REQUIRED_DETAIL, SUBSCRIPTION_GENERATION_EXHAUSTED_DETAIL,
};

/// Wraps a successful payload in the response correlated to its request.
pub(super) fn outcome(request_id: &RequestId, payload: ResponsePayload) -> ServerResponse {
    ServerResponse {
        request_id: request_id.clone(),
        payload,
    }
}

/// Classifies a repository failure into the stable protocol vocabulary.
///
/// Unknown client-named state maps to the entity-specific codes; reuse of a
/// persisted request identity for a different command kind or immutable
/// payload answers the dedicated non-retryable idempotency-conflict code so
/// the originally accepted outcome stands; deterministic client mistakes such
/// as a first message that already exists from another request stay
/// non-retryable input failures; persisted-state problems stay internal
/// without retry hope; only database-operation failures admit that an
/// identical later request may succeed.
pub(super) fn repository_failure(
    error: &RepositoryError,
    request_id: &RequestId,
) -> ProtocolFailure {
    use RepositoryError as Failure;

    let (code, retryable) = match error {
        Failure::ProjectNotFound { .. } => (ErrorCode::ProjectUnknown, false),
        Failure::ThreadNotFound { .. } => (ErrorCode::ThreadUnknown, false),
        Failure::ThreadEngineNotConfigured { .. } | Failure::FirstMessageAlreadyExists { .. } => {
            (ErrorCode::InvalidInput, false)
        }
        Failure::EngineConfigRevisionConflict { .. } => (ErrorCode::EngineConfigConflict, false),
        Failure::IdempotencyConflict { .. } => (ErrorCode::IdempotencyConflict, false),
        Failure::ProjectConflict { .. }
        | Failure::ThreadConflict { .. }
        | Failure::MessageConflict { .. }
        | Failure::InvalidChronology { .. }
        | Failure::CorruptData { .. }
        | Failure::Invariant { .. }
        | Failure::ThreadListing { .. }
        | Failure::ProjectListing { .. }
        | Failure::InvalidDispatchLeaseWindow { .. }
        | Failure::DispatchAttemptLimit { .. }
        | Failure::DispatchNotFound { .. }
        | Failure::InvalidDispatchState { .. }
        | Failure::DispatchOwnerMismatch { .. }
        | Failure::DispatchLeaseExpired { .. } => (ErrorCode::Internal, false),
        Failure::Database { .. } => (ErrorCode::Internal, true),
    };
    typed_failure(code, error.to_string(), retryable, request_id)
}

/// Maps a subscription-preparation failure without exposing its durable
/// cursor or client request values.
pub(super) fn preparation_failure(
    error: PrepareSubscriptionError,
    request_id: &RequestId,
) -> ProtocolFailure {
    match error {
        PrepareSubscriptionError::Repository(error) => repository_failure(&error, request_id),
        PrepareSubscriptionError::ResnapshotRequired { .. } => typed_failure(
            ErrorCode::InvalidInput,
            RESNAPSHOT_REQUIRED_DETAIL,
            false,
            request_id,
        ),
        PrepareSubscriptionError::Register(RegisterError::GenerationExhausted) => typed_failure(
            ErrorCode::Internal,
            SUBSCRIPTION_GENERATION_EXHAUSTED_DETAIL,
            false,
            request_id,
        ),
    }
}

/// Builds the typed failure for an operation without a backing capability.
///
/// The failure stays internal and non-retryable: repeating the identical
/// request against this build deterministically fails again, while the detail
/// records exactly which capability is absent rather than claiming an effect.
pub(super) fn unbacked_failure(request_id: &RequestId, operation: &str) -> ProtocolFailure {
    typed_failure(
        ErrorCode::Internal,
        format!("{operation} is not backed by a Forge capability in this build"),
        false,
        request_id,
    )
}

/// Maps the live cancellation registry's fail-closed errors to one bounded,
/// payload-free protocol failure. Registry details are intentionally not
/// exposed because they contain no client-actionable state.
pub(super) fn run_cancellation_failure(
    _error: RunCancellationError,
    request_id: &RequestId,
) -> ProtocolFailure {
    typed_failure(
        ErrorCode::Internal,
        "live run cancellation registry is unavailable",
        false,
        request_id,
    )
}

/// Builds the typed failure for an unresolvable opaque directory identity.
pub(super) fn unknown_directory_failure(
    request_id: &RequestId,
    directory: &DirectoryId,
) -> ProtocolFailure {
    typed_failure(
        ErrorCode::DirectoryUnknown,
        format!("directory `{directory}` is not known to this Forge build"),
        false,
        request_id,
    )
}

/// Builds the typed failure for a fresh-command entropy acquisition failure.
///
/// Entropy unavailability is an environmental fault, not a client mistake:
/// nothing was persisted, no receipt was recorded, and the identical retry
/// may succeed once the platform provider recovers, so the failure stays
/// internal and retryable. The detail carries only the bounded typed cause —
/// never command payloads.
pub(super) fn origin_entropy_failure(
    error: &CommandOriginEntropyError,
    request_id: &RequestId,
) -> ProtocolFailure {
    typed_failure(
        ErrorCode::Internal,
        format!("fresh command could not acquire durable identity entropy: {error}"),
        true,
        request_id,
    )
}

/// Builds the typed failure for a fresh-command instant acquisition failure.
///
/// A clock reading outside the signed millisecond range is likewise
/// environmental and left nothing behind: conversion refuses to truncate or
/// clamp, so the failure stays internal, payload-free, correlated, and
/// retryable on the same terms as the entropy fault.
pub(super) fn origin_clock_failure(
    error: CommandOriginClockError,
    request_id: &RequestId,
) -> ProtocolFailure {
    typed_failure(
        ErrorCode::Internal,
        format!("fresh command could not acquire an acceptance instant: {error}"),
        true,
        request_id,
    )
}

/// Builds the typed failure for a forged identity failing identifier
/// validation.
///
/// The bounded hex encoder cannot emit invalid identifier text, so this
/// records a deterministic internal defect instead of fabricating success.
/// It stays non-retryable because repeating through the same defective
/// encoder cannot mint differently, and payload-free because only the
/// Forge-owned kind is named.
pub(super) fn forged_identity_failure(
    kind: &'static str,
    request_id: &RequestId,
) -> ProtocolFailure {
    typed_failure(
        ErrorCode::Internal,
        format!("forged {kind} identity failed identifier validation"),
        false,
        request_id,
    )
}

/// Bounds a diagnostic text into the protocol's failure contract.
pub(super) fn typed_failure(
    code: ErrorCode,
    detail_text: impl Into<String>,
    retryable: bool,
    request_id: &RequestId,
) -> ProtocolFailure {
    let detail = ErrorDetail::parse(detail_text).ok().unwrap_or_else(|| {
        ErrorDetail::parse(BOUNDED_DETAIL_FALLBACK)
            .expect("static fallback detail satisfies the protocol bound")
    });
    ProtocolFailure {
        code,
        detail,
        retryable,
        request_id: Some(request_id.clone()),
    }
}
