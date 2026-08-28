//! Dependency-free policy for retaining project identities for picker rows.
//!
//! The legacy controller combines admission, retry scheduling, and ordered
//! retention with transport and Effect runtime concerns. This module keeps
//! those policy decisions as explicit data. A caller owns the client request,
//! timer, and response delivery; the policy only describes what to do and
//! applies an already-decoded successful response.

use std::time::Duration;

/// Maximum number of project identities retained by the picker policy.
pub const MAX_RETAINED_PROJECT_IDENTITIES: usize = 128;

/// Initial delay in the cold-start exponential retry schedule.
pub const COLD_START_RETRY_INITIAL_DELAY: Duration = Duration::from_millis(100);

/// Elapsed-duration bound for the cold-start retry schedule.
pub const COLD_START_RETRY_SCHEDULE_DURATION: Duration = Duration::from_secs(5);

/// Opaque identity metadata retained for a project.
///
/// The policy intentionally does not reproduce the transport or protocol
/// identity variants. The metadata type is supplied by the caller and may be
/// any already-decoded presentation value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectIdentity<T = String> {
    /// Stable project identifier used for replacement and recency.
    pub project_id: String,
    /// Caller-owned identity metadata.
    pub metadata: T,
}

impl<T> ProjectIdentity<T> {
    /// Creates an identity from its stable project identifier and metadata.
    #[must_use]
    pub fn new(project_id: impl Into<String>, metadata: T) -> Self {
        Self {
            project_id: project_id.into(),
            metadata,
        }
    }
}

/// Admission decision for a requested refresh.
///
/// Empty input is deliberately represented as `NoOp`, so callers do not
/// create a client request for it. Non-empty input remains one batch lookup;
/// this policy does not split, deduplicate, or otherwise rewrite the request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefreshAdmission {
    /// No client operation is needed.
    NoOp,
    /// Perform one lookup with the supplied project identifiers.
    BatchLookup { project_ids: Vec<String> },
}

/// Admits a project-identity refresh without invoking a client.
#[must_use]
pub fn admit_refresh(project_ids: &[String]) -> RefreshAdmission {
    if project_ids.is_empty() {
        RefreshAdmission::NoOp
    } else {
        RefreshAdmission::BatchLookup {
            project_ids: project_ids.to_vec(),
        }
    }
}

/// One explicit decision from the cold-start retry schedule.
///
/// `attempt` is one-based and counts retries after the initial client
/// attempt. `elapsed` is the schedule elapsed time observed when this retry
/// is admitted, not wall-clock time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryIntent {
    /// Retry after `delay`, provided the caller owns timer execution.
    Retry {
        /// One-based retry number.
        attempt: u32,
        /// Delay before this retry.
        delay: Duration,
        /// Schedule elapsed time observed before this retry's delay.
        elapsed: Duration,
    },
    /// The bounded schedule has no further retry.
    Exhausted,
}

/// State for the legacy cold-start exponential retry schedule.
///
/// This type computes retry intent only. It does not sleep, observe time, or
/// execute a client operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColdStartRetryState {
    next_attempt: u32,
    next_delay: Duration,
    elapsed: Duration,
    exhausted: bool,
}

impl ColdStartRetryState {
    /// Creates fresh retry state beginning with a 100 ms retry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_attempt: 1,
            next_delay: COLD_START_RETRY_INITIAL_DELAY,
            elapsed: Duration::ZERO,
            exhausted: false,
        }
    }

    /// Returns the next retry intent and advances the schedule when bounded.
    pub fn next_intent(&mut self) -> RetryIntent {
        if self.exhausted {
            return RetryIntent::Exhausted;
        }

        if self.elapsed > COLD_START_RETRY_SCHEDULE_DURATION {
            self.exhausted = true;
            return RetryIntent::Exhausted;
        }

        let intent = RetryIntent::Retry {
            attempt: self.next_attempt,
            delay: self.next_delay,
            elapsed: self.elapsed,
        };

        self.elapsed = self
            .elapsed
            .checked_add(self.next_delay)
            .unwrap_or(Duration::MAX);
        self.next_attempt = self.next_attempt.saturating_add(1);
        self.next_delay = self.next_delay.checked_mul(2).unwrap_or(Duration::MAX);
        intent
    }

    /// Returns whether this schedule can produce another retry intent.
    #[must_use]
    pub const fn is_exhausted(self) -> bool {
        self.exhausted
    }
}

impl Default for ColdStartRetryState {
    fn default() -> Self {
        Self::new()
    }
}

/// State of retained project identities, ordered from oldest to newest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectIdentityState<T = String> {
    identities: Vec<ProjectIdentity<T>>,
}

impl<T> ProjectIdentityState<T> {
    /// Creates an empty retained-identity state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            identities: Vec::new(),
        }
    }

    /// Returns identities from oldest to newest.
    #[must_use]
    pub fn identities(&self) -> &[ProjectIdentity<T>] {
        &self.identities
    }

    /// Returns the number of retained identities.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.identities.len()
    }

    /// Returns whether no identities are retained.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.identities.is_empty()
    }

    /// Applies a complete successful response and reports the state action.
    ///
    /// Each returned project ID is deleted before its replacement is pushed,
    /// making every returned identity newest in response order. The complete
    /// response is applied before oldest-first capacity eviction.
    pub fn apply_response(&mut self, identities: Vec<ProjectIdentity<T>>) -> RetentionAction {
        let returned_project_ids = identities
            .iter()
            .map(|identity| identity.project_id.clone())
            .collect();
        let mut retained = std::mem::take(&mut self.identities);

        for identity in identities {
            if let Some(index) = retained
                .iter()
                .position(|current| current.project_id == identity.project_id)
            {
                retained.remove(index);
            }
            retained.push(identity);
        }

        let mut evicted_project_ids = Vec::new();
        while retained.len() > MAX_RETAINED_PROJECT_IDENTITIES {
            let oldest = retained.remove(0);
            evicted_project_ids.push(oldest.project_id);
        }

        self.identities = retained;
        RetentionAction {
            returned_project_ids,
            evicted_project_ids,
        }
    }
}

impl<T> Default for ProjectIdentityState<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Observable state action produced after a successful response is applied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetentionAction {
    /// Returned project IDs, in the response's original order.
    pub returned_project_ids: Vec<String>,
    /// Project IDs evicted oldest-first after the complete response.
    pub evicted_project_ids: Vec<String>,
}

/// Complete dependency-light project-identity policy state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectIdentityPolicy<T = String> {
    state: ProjectIdentityState<T>,
}

impl<T> ProjectIdentityPolicy<T> {
    /// Creates a policy with no retained identities.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: ProjectIdentityState::new(),
        }
    }

    /// Returns the current retained-identity state.
    #[must_use]
    pub const fn state(&self) -> &ProjectIdentityState<T> {
        &self.state
    }

    /// Computes refresh admission for the supplied project IDs.
    #[must_use]
    pub fn admit_refresh(&self, project_ids: &[String]) -> RefreshAdmission {
        admit_refresh(project_ids)
    }

    /// Applies a successful response while preserving a client failure.
    ///
    /// An error is returned unchanged and leaves retained state untouched;
    /// transport-specific error types remain outside this policy.
    ///
    /// # Errors
    ///
    /// Returns the client error unchanged when `result` is `Err`; retained
    /// state is not modified in that case.
    pub fn apply_client_result<E>(
        &mut self,
        result: Result<Vec<ProjectIdentity<T>>, E>,
    ) -> Result<RetentionAction, E> {
        result.map(|response| self.state.apply_response(response))
    }
}

impl<T> Default for ProjectIdentityPolicy<T> {
    fn default() -> Self {
        Self::new()
    }
}
