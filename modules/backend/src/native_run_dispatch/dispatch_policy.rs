//! Dispatcher policy classification.
//!
//! Pure decision functions and reason strings for the configured-run
//! scheduler: persisted-settings reads, snapshot-fenced launch authority,
//! one-prompt binding authorization, committed-notifier gating, permanent
//! repository failures, and continuation refusal diagnostics. None of these
//! touch the database, provider, or dispatcher state.

use artisan_database::{
    LaunchClaimedRunOutcome, RunLaunchError, SessionContinuationIncompatibility,
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
/// Interrupted runs stay blocked: `thread/resume` against a missing rollout
/// fails `-32600`, so no silent fresh start or old-prompt replay is attempted
/// here. The returned text is persisted as the dispatch `last_error` the
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
            "provider continuation unavailable: the prior run has no provider session"
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
/// profiles and never starts silently on a fresh session here.
pub(crate) fn continuation_incompatible_reason(
    incompatible: &SessionContinuationIncompatible,
) -> &'static str {
    match incompatible.reason {
        SessionContinuationIncompatibility::Engine => {
            "provider continuation incompatible: the prior run used a different engine"
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
            "provider continuation unavailable: the prior run has no provider session"
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
            "provider continuation incompatible: the prior run used a different engine"
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
