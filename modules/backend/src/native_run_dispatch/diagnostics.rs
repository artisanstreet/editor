//! Process-log diagnostics for one dispatched run.
//!
//! A run that stops early settles with a fixed, user-facing outcome
//! (`provider_interrupted`, `provider_failed`). That outcome never says which
//! step stopped the run, so every dispatcher failure path reports here: the
//! run it belongs to, the step that failed, and the complete cause chain.
//!
//! Lines carry identities, counters, and typed error text only; message
//! bodies, provider payloads, and credentials never reach them. Cause text is
//! rendered through [`ErrorChain`], which escapes control characters.

use std::error::Error;
use std::fmt;

use artisan_database::RunBatchScope;
use artisan_domain::{EngineId, ErrorChain, MessageId, RunId, ThreadId};

type Cause = Box<dyn Error + Send + Sync + 'static>;

/// One dispatcher step that failed, with the failure that stopped it.
///
/// `Display` names the step; the stopping failure is the `source()`, so
/// [`ErrorChain`] renders `step: cause: cause`.
#[derive(Debug)]
pub(crate) struct StepError {
    step: &'static str,
    cause: Option<Cause>,
}

impl StepError {
    /// A step its own invariant refused, with no underlying error value.
    #[must_use]
    pub(crate) const fn refused(step: &'static str) -> Self {
        Self { step, cause: None }
    }

    /// A step stopped by `cause`.
    #[must_use]
    pub(crate) fn failed(step: &'static str, cause: impl Into<Cause>) -> Self {
        Self {
            step,
            cause: Some(cause.into()),
        }
    }
}

impl fmt::Display for StepError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.step)
    }
}

impl Error for StepError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause
            .as_ref()
            .map(|cause| cause.as_ref() as &(dyn Error + 'static))
    }
}

/// Identifies one run in a diagnostic line as `thread <id> run <id>`.
pub(super) struct RunLabel<'a>(pub(super) &'a RunBatchScope<'a>);

impl fmt::Display for RunLabel<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "thread {} run {}",
            self.0.launched.thread_id, self.0.launched.run_id
        )
    }
}

/// Reports the step that made the dispatcher end a live run as interrupted.
///
/// `progress_uncertain` tells whether a durable write may have landed without
/// its receipt: `false` means the step failed before anything was written.
pub(super) fn report_interrupted(
    scope: &RunBatchScope<'_>,
    engine: EngineId,
    batch_sequence: i64,
    progress_uncertain: bool,
    error: &StepError,
) {
    let progress = if progress_uncertain {
        "durable progress uncertain"
    } else {
        "nothing written by the failed step"
    };
    eprintln!(
        "native run interrupted ({}, engine {engine:?}, batch {batch_sequence}, {progress}): {}",
        RunLabel(scope),
        ErrorChain(error)
    );
}

/// Reports a failed step that left the run's outcome to a later path.
pub(super) fn report_step_failure(scope: &RunBatchScope<'_>, error: &StepError) {
    eprintln!(
        "native run step failed ({}): {}",
        RunLabel(scope),
        ErrorChain(error)
    );
}

/// Reports why one steered message did not reach the provider or the
/// transcript. The sender still receives its typed acknowledgement; this line
/// carries the cause that acknowledgement cannot.
pub(super) fn report_steer_failure(
    scope: &RunBatchScope<'_>,
    message_id: &MessageId,
    error: &StepError,
) {
    eprintln!(
        "native run steer failed ({}, message {message_id}): {}",
        RunLabel(scope),
        ErrorChain(error)
    );
}

/// Reports the failure behind a claimed message's requeue, refusal, or
/// abandoned launch. The dispatch row stores only a short fixed reason.
pub(super) fn report_claim_failure(message_id: &MessageId, error: &StepError) {
    eprintln!(
        "native run claim failed (message {message_id}): {}",
        ErrorChain(error)
    );
}

/// Reports why a launched run was abandoned before its turn was consumed.
/// The run keeps its launched row, so lease recovery settles it later.
pub(super) fn report_abandoned(thread_id: &ThreadId, run_id: &RunId, error: &StepError) {
    eprintln!(
        "native run abandoned before its turn was consumed (thread {thread_id} run {run_id}): {}",
        ErrorChain(error)
    );
}

#[cfg(test)]
mod tests {
    use super::StepError;
    use artisan_domain::ErrorChain;

    #[derive(Debug, thiserror::Error)]
    #[error("database is locked")]
    struct Locked;

    #[test]
    fn a_failed_step_renders_its_cause_after_the_step() {
        let error = StepError::failed("committing the text append", Locked);
        assert_eq!(
            ErrorChain(&error).to_string(),
            "committing the text append: database is locked"
        );
    }

    #[test]
    fn a_refused_step_renders_the_step_alone() {
        let error = StepError::refused("minting the patch id");
        assert_eq!(ErrorChain(&error).to_string(), "minting the patch id");
    }
}
