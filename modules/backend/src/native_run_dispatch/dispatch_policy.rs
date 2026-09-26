//! Dispatcher policy classification.
//!
//! Pure decision functions and reason strings for the configured-run
//! scheduler: persisted-settings reads, snapshot-fenced launch authority,
//! one-prompt binding authorization, committed-notifier gating, permanent
//! repository failures, and continuation refusal diagnostics. None of these
//! touch the database, provider, or dispatcher state.

use std::time::Duration;

use artisan_database::sqlite_write_retry;
use artisan_database::{
    LaunchClaimedRunOutcome, RepositoryError, RunLaunchError, SessionContinuationIncompatibility,
    SessionContinuationIncompatible, SessionContinuationUnavailable,
    SessionContinuationUnavailableReason,
};

/// Decision made before any provider launch is permitted.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SettingsLoadDecision {
    /// A validated immutable settings snapshot is ready for the launch fence.
    Ready(Box<artisan_database::ThreadEngineSettings>),
    /// The claim must be returned for a bounded later attempt.
    Requeue(&'static str),
    /// The claim contains a permanent configuration or project defect.
    Fail(&'static str),
}

/// Classifies the persisted settings read used by the production dispatcher.
///
/// This decision is intentionally separated from provider code: a missing or
/// temporarily unreadable configuration can only requeue, so no authority
/// resolution, process spawn, or session request can happen on that branch.
pub(crate) fn classify_settings_load(
    result: Result<
        Option<artisan_database::ThreadEngineSettings>,
        artisan_database::RepositoryError,
    >,
) -> SettingsLoadDecision {
    match result {
        Ok(Some(settings)) => SettingsLoadDecision::Ready(Box::new(settings)),
        Ok(None) => SettingsLoadDecision::Requeue("engine unconfigured"),
        Err(error) if is_permanent_configuration_error(&error) => {
            SettingsLoadDecision::Fail("engine settings corrupt")
        }
        Err(_) => SettingsLoadDecision::Requeue("engine settings unavailable"),
    }
}

/// Authority classification for one snapshot-fenced launch attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LaunchAuthority {
    /// This call durably created the assistant run and may contact a provider.
    Started,
    /// The durable call was replayed; no second provider effect is permitted.
    Replay,
    /// The snapshot fence rejected the attempt; the claim must be requeued.
    Requeue,
}

pub(crate) fn classify_launch_result(
    result: &Result<LaunchClaimedRunOutcome, RunLaunchError>,
) -> LaunchAuthority {
    match result {
        Ok(LaunchClaimedRunOutcome::Started(_)) => LaunchAuthority::Started,
        Ok(LaunchClaimedRunOutcome::AlreadyStarted(_)) => LaunchAuthority::Replay,
        Err(_) => LaunchAuthority::Requeue,
    }
}

/// Whether a durable provider bind authorizes the one prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromptAuthorization {
    /// The current owner durably created the binding and may authorize once.
    Authorize,
    /// An unknown prior binding owns the provider session; do not prompt.
    DoNotAuthorize,
}

pub(crate) const fn prompt_authorization_after_binding(already_bound: bool) -> PromptAuthorization {
    if already_bound {
        PromptAuthorization::DoNotAuthorize
    } else {
        PromptAuthorization::Authorize
    }
}

/// Executes a notifier hint only after the caller has observed a committed
/// or idempotently replayed SQLite result.
pub(crate) fn notify_after_commit(notified_commit: bool, notify: impl FnOnce()) -> bool {
    if notified_commit {
        notify();
        true
    } else {
        false
    }
}

/// Precise, bounded dispatcher diagnostic for a blocked provider continuation.
///
/// Unsupported interrupted sessions stay blocked. Codex sessions with validated
/// bindings proceed through thread/resume; a missing rollout still fails closed,
/// without a silent fresh start or replaying the old prompt. The returned text is persisted as the dispatch `last_error` the
/// composer lip already renders, so the reader sees exactly why the send
/// cannot proceed on this thread.
pub(crate) fn continuation_unavailable_reason(
    unavailable: &SessionContinuationUnavailable,
) -> &'static str {
    match unavailable.reason {
        SessionContinuationUnavailableReason::ActiveRun => {
            "provider continuation unavailable: another run is still active"
        }
        SessionContinuationUnavailableReason::UnboundSettledRun => {
            "provider continuation unavailable: the prior run produced output without a provider session; start a new chat to continue"
        }
        SessionContinuationUnavailableReason::AmbiguousRun => {
            "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue"
        }
        SessionContinuationUnavailableReason::CandidateLimit => {
            "provider continuation unavailable: history limit reached"
        }
    }
}

/// Precise, bounded dispatcher diagnostic for a scope-mismatched continuation.
///
/// A mismatch fails closed: the dispatcher never resumes across engines or
/// profiles and never starts silently on a fresh session here. Only runs that
/// reached their provider get this far (never-started runs are not history),
/// so the engine refusal tells the reader how to proceed.
pub(crate) fn continuation_incompatible_reason(
    incompatible: &SessionContinuationIncompatible,
) -> &'static str {
    match incompatible.reason {
        SessionContinuationIncompatibility::Engine => {
            "provider continuation incompatible: this chat already has replies from another engine; switch back to that engine or start a new chat"
        }
        SessionContinuationIncompatibility::Profile => {
            "provider continuation incompatible: the prior run used a different profile"
        }
        SessionContinuationIncompatibility::ProviderBindingVersion => {
            "provider continuation incompatible: the prior binding version is unsupported"
        }
        SessionContinuationIncompatibility::ProviderBindingEngine => {
            "provider continuation incompatible: the prior binding names a different engine"
        }
        SessionContinuationIncompatibility::ProviderBindingProfile => {
            "provider continuation incompatible: the prior binding names a different profile"
        }
    }
}

pub(super) fn is_permanent_configuration_error(error: &artisan_database::RepositoryError) -> bool {
    matches!(
        error,
        artisan_database::RepositoryError::CorruptData { .. }
            | artisan_database::RepositoryError::Invariant { .. }
            | artisan_database::RepositoryError::ProjectNotFound { .. }
            | artisan_database::RepositoryError::ThreadNotFound { .. }
    )
}

/// Classified disposition of one failed dispatch-claim attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaimFailureDisposition {
    /// SQLite writer-lock contention; retry on the shared bounded schedule.
    WriterContention,
    /// Any other database failure; retry on the dispatcher backoff interval.
    Database,
}

/// Classifies one failed claim attempt.
///
/// A database failure can never be mistaken for an empty queue: both
/// dispositions schedule a bounded retry and the caller surfaces the exact
/// error, but only writer-lock contention follows the shared exponential
/// write-retry schedule.
pub(crate) fn classify_claim_failure(error: &RepositoryError) -> ClaimFailureDisposition {
    match error {
        RepositoryError::Database { source, .. }
            if sqlite_write_retry::is_retryable_write_error(source) =>
        {
            ClaimFailureDisposition::WriterContention
        }
        _ => ClaimFailureDisposition::Database,
    }
}

/// Returns the bounded delay before the next claim attempt.
///
/// `completed_failures` is the number of consecutive failures already
/// observed. Writer contention follows the shared write-retry schedule until
/// it is exhausted and then keeps retrying on the dispatcher's configured
/// backoff; every other failure retries on that configured backoff.
pub(crate) fn claim_failure_backoff(
    disposition: ClaimFailureDisposition,
    completed_failures: u32,
    fallback: Duration,
) -> Duration {
    match disposition {
        ClaimFailureDisposition::WriterContention => {
            sqlite_write_retry::retry_delay_for(completed_failures).unwrap_or(fallback)
        }
        ClaimFailureDisposition::Database => fallback,
    }
}

#[cfg(test)]
mod claim_failure_tests {
    use std::borrow::Cow;
    use std::error::Error as StdError;
    use std::sync::Arc;
    use std::time::Duration;

    use sea_orm::sqlx::error::{DatabaseError, ErrorKind};
    use sea_orm::{DbErr, RuntimeErr};

    use super::{
        ClaimFailureDisposition, RepositoryError, claim_failure_backoff, classify_claim_failure,
    };

    #[derive(Debug)]
    struct FakeDatabaseError {
        code: i32,
    }

    impl std::fmt::Display for FakeDatabaseError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "(code: {}) database is locked", self.code)
        }
    }

    impl StdError for FakeDatabaseError {}

    impl DatabaseError for FakeDatabaseError {
        fn message(&self) -> &'static str {
            "database is locked"
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Owned(self.code.to_string()))
        }

        fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    fn driver_error(code: i32) -> RepositoryError {
        RepositoryError::Database {
            operation: "claim next message dispatch",
            source: DbErr::Exec(RuntimeErr::SqlxError(Arc::new(
                sea_orm::sqlx::Error::Database(Box::new(FakeDatabaseError { code })),
            ))),
        }
    }

    #[test]
    fn writer_contention_follows_the_shared_schedule_then_the_fallback() {
        let fallback = Duration::from_millis(250);
        let error = driver_error(517);

        assert_eq!(
            classify_claim_failure(&error),
            ClaimFailureDisposition::WriterContention
        );
        assert_eq!(
            claim_failure_backoff(ClaimFailureDisposition::WriterContention, 0, fallback),
            Duration::from_millis(5)
        );
        assert_eq!(
            claim_failure_backoff(ClaimFailureDisposition::WriterContention, 7, fallback),
            Duration::from_millis(640)
        );
        assert_eq!(
            claim_failure_backoff(ClaimFailureDisposition::WriterContention, 8, fallback),
            fallback
        );
    }

    #[test]
    fn other_database_failures_use_the_configured_fallback() {
        let fallback = Duration::from_millis(250);

        for error in [
            driver_error(19),
            RepositoryError::Database {
                operation: "claim next message dispatch",
                source: DbErr::Query(RuntimeErr::Internal("syntax error".to_owned())),
            },
        ] {
            assert_eq!(
                classify_claim_failure(&error),
                ClaimFailureDisposition::Database
            );
            assert_eq!(
                claim_failure_backoff(ClaimFailureDisposition::Database, 0, fallback),
                fallback
            );
        }
    }
}

#[cfg(test)]
mod settings_decision_tests {
    use super::{SettingsLoadDecision, classify_settings_load};

    #[test]
    fn missing_thread_configuration_requeues_with_the_actionable_reason() {
        assert_eq!(
            classify_settings_load(Ok(None)),
            SettingsLoadDecision::Requeue("engine unconfigured")
        );
    }
}

#[cfg(test)]
mod continuation_reason_tests {
    use super::{continuation_incompatible_reason, continuation_unavailable_reason};
    use artisan_database::{
        SessionContinuationIncompatibility, SessionContinuationIncompatible,
        SessionContinuationUnavailable, SessionContinuationUnavailableReason,
    };

    fn unavailable(reason: SessionContinuationUnavailableReason) -> SessionContinuationUnavailable {
        SessionContinuationUnavailable {
            run_id: artisan_domain::RunId::parse("run-a").expect("run id"),
            reason,
        }
    }

    fn incompatible(reason: SessionContinuationIncompatibility) -> SessionContinuationIncompatible {
        SessionContinuationIncompatible {
            run_id: artisan_domain::RunId::parse("run-a").expect("run id"),
            reason,
        }
    }

    #[test]
    fn interrupted_run_names_unknown_outcome_and_new_chat() {
        assert_eq!(
            continuation_unavailable_reason(&unavailable(
                SessionContinuationUnavailableReason::AmbiguousRun
            )),
            "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue"
        );
    }

    #[test]
    fn active_and_unbound_blocks_keep_their_exact_causes() {
        assert_eq!(
            continuation_unavailable_reason(&unavailable(
                SessionContinuationUnavailableReason::ActiveRun
            )),
            "provider continuation unavailable: another run is still active"
        );
        assert_eq!(
            continuation_unavailable_reason(&unavailable(
                SessionContinuationUnavailableReason::UnboundSettledRun
            )),
            "provider continuation unavailable: the prior run produced output without a provider session; start a new chat to continue"
        );
        assert_eq!(
            continuation_unavailable_reason(&unavailable(
                SessionContinuationUnavailableReason::CandidateLimit
            )),
            "provider continuation unavailable: history limit reached"
        );
    }

    #[test]
    fn scope_mismatches_name_the_exact_fence() {
        assert_eq!(
            continuation_incompatible_reason(&incompatible(
                SessionContinuationIncompatibility::Engine
            )),
            "provider continuation incompatible: this chat already has replies from another engine; switch back to that engine or start a new chat"
        );
        assert_eq!(
            continuation_incompatible_reason(&incompatible(
                SessionContinuationIncompatibility::Profile
            )),
            "provider continuation incompatible: the prior run used a different profile"
        );
        assert_eq!(
            continuation_incompatible_reason(&incompatible(
                SessionContinuationIncompatibility::ProviderBindingVersion
            )),
            "provider continuation incompatible: the prior binding version is unsupported"
        );
        assert_eq!(
            continuation_incompatible_reason(&incompatible(
                SessionContinuationIncompatibility::ProviderBindingEngine
            )),
            "provider continuation incompatible: the prior binding names a different engine"
        );
        assert_eq!(
            continuation_incompatible_reason(&incompatible(
                SessionContinuationIncompatibility::ProviderBindingProfile
            )),
            "provider continuation incompatible: the prior binding names a different profile"
        );
    }
}
