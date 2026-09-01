//! Bounded single-pass startup-reconciliation sweep coordinator.
//!
//! The coordinator owns no storage, custody, or provider contact. It calls the
//! accepted read-only discovery once in its existing deterministic order
//! (`lease_expires_at ASC, run_id ASC`) and then disposes each discovered
//! candidate sequentially through the accepted atomic database API. Each
//! per-candidate disposition is one committed transaction; earlier transactions
//! may already be durable when a later candidate fails, so the typed error
//! never claims the whole pass rolled back.
//!
//! Identity material is caller-injected through a synchronous patch-ID source.
//! The coordinator validates patch-shape agreement (`item_patch` is `Some`
//! exactly when the candidate carries an assistant item, and turn/item
//! identities are distinct) before any mutation of that candidate. It never
//! mints randomness, reads a clock, or persists secrets.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_database::{
    Repository, StartupReconciliationCandidate, StartupReconciliationDisposition,
    StartupReconciliationDispositionError, StartupReconciliationError, StartupReconciliationQuery,
};
use artisan_domain::{PatchId, RunId, UnixMillis};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Input / report
// ---------------------------------------------------------------------------

/// Bounded input for one sweep pass.
///
/// `operated_at` is the caller-injected time applied to every mutated row;
/// `limit` is validated `1..=64`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupReconciliationSweepInput {
    /// Time applied to every mutated row.
    pub operated_at: UnixMillis,
    /// Maximum candidates to sweep; validated `1..=64`.
    pub limit: usize,
}

impl StartupReconciliationSweepInput {
    /// Creates a validated sweep input.
    ///
    /// # Errors
    ///
    /// Returns [`StartupReconciliationSweepError::InvalidLimit`] when `limit`
    /// is outside `1..=64`.
    #[allow(clippy::result_large_err)]
    pub fn new(
        operated_at: UnixMillis,
        limit: usize,
    ) -> Result<Self, StartupReconciliationSweepError> {
        if !(1..=64).contains(&limit) {
            return Err(StartupReconciliationSweepError::InvalidLimit { limit });
        }
        Ok(Self { operated_at, limit })
    }
}

/// Typed report of one bounded pass.
///
/// Counts use checked arithmetic; the sweep never silently wraps.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StartupReconciliationSweepReport {
    /// Number of candidates returned by discovery.
    pub discovered: usize,
    /// Number of candidates for which disposition was attempted and produced an
    /// outcome (`Interrupted` / `AlreadyInterrupted` / `SkippedMoved`).
    pub attempted: usize,
    /// Candidates sealed on this pass.
    pub interrupted: usize,
    /// Candidates already sealed by an identical earlier pass.
    pub already_interrupted: usize,
    /// Candidates whose fence failed without proven replay.
    pub skipped_moved: usize,
}

// ---------------------------------------------------------------------------
// Patch identity source
// ---------------------------------------------------------------------------

/// Caller-minted patch identities for one candidate.
///
/// `item_patch_id` must be `Some` exactly when the candidate carries an
/// assistant item; the coordinator validates that agreement before any mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupReconciliationPatches {
    /// Minted `turn_lifecycle` patch identity.
    pub turn_patch_id: PatchId,
    /// Minted `item_lifecycle` patch identity, required iff the candidate has
    /// an assistant item.
    pub item_patch_id: Option<PatchId>,
}

impl StartupReconciliationPatches {
    /// Creates patch identities, preserving caller-minted values.
    #[must_use]
    pub fn new(turn_patch_id: PatchId, item_patch_id: Option<PatchId>) -> Self {
        Self {
            turn_patch_id,
            item_patch_id,
        }
    }
}

/// Synchronous, caller-injected source of per-candidate patch identities.
///
/// The source must perform no I/O, mint no randomness, and return identities
/// in discovery order. The coordinator calls it exactly once per candidate,
/// before any mutation of that candidate.
pub trait StartupReconciliationPatchSource {
    /// Returns patch identities for `candidate` in discovery order.
    ///
    /// # Errors
    ///
    /// Any error stops the sweep before mutation of this candidate and is
    /// reported as [`StartupReconciliationSweepError::PatchSource`].
    fn patch_ids_for(
        &mut self,
        candidate: &StartupReconciliationCandidate,
    ) -> Result<StartupReconciliationPatches, PatchSourceError>;
}

/// Bounded, content-free patch-source failure.
///
/// The value carries no patch material and is bounded: its display is a fixed
/// string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchSourceError;

impl std::fmt::Display for PatchSourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("patch source failed")
    }
}

impl std::error::Error for PatchSourceError {}

// ---------------------------------------------------------------------------
// Typed sweep error
// ---------------------------------------------------------------------------

/// Typed failures of the bounded sweep.
///
/// Every variant's display is content-free and bounded: it never prints patch
/// identities, provider material, or secret bytes. The already-committed prefix
/// report, failing candidate index, and run identity are carried as typed
/// fields for programmatic use without leaking into the display string.
#[derive(Debug, Error)]
pub enum StartupReconciliationSweepError {
    /// The supplied limit is outside `1..=64`.
    #[error("startup reconciliation sweep limit {limit} must be between 1 and 64")]
    InvalidLimit {
        /// Offending limit.
        limit: usize,
    },

    /// Discovery failed before any candidate was processed.
    #[error("startup reconciliation sweep discovery failed")]
    Discovery {
        /// Discovery failure.
        #[source]
        source: StartupReconciliationError,
    },

    /// The patch source failed for the candidate at `failing_index`.
    #[error("startup reconciliation sweep patch source failed at candidate {failing_index}")]
    PatchSource {
        /// Already-committed prefix report.
        report: StartupReconciliationSweepReport,
        /// Index of the candidate whose source failed.
        failing_index: usize,
        /// Run identity of the failing candidate.
        failing_run_id: RunId,
        /// Bounded source failure (never patch material).
        #[source]
        source: PatchSourceError,
    },

    /// Patch-shape disagreement for the candidate at `failing_index`.
    #[error(
        "startup reconciliation sweep patch shape disagreement at candidate {failing_index}: {reason}"
    )]
    PatchShape {
        /// Already-committed prefix report.
        report: StartupReconciliationSweepReport,
        /// Index of the candidate whose shape disagreed.
        failing_index: usize,
        /// Run identity of the failing candidate.
        failing_run_id: RunId,
        /// Bounded reason label.
        reason: &'static str,
    },

    /// Counter overflow while advancing the report.
    #[error(
        "startup reconciliation sweep counter overflow at candidate {failing_index}: {counter}"
    )]
    CounterOverflow {
        /// Already-committed prefix report.
        report: StartupReconciliationSweepReport,
        /// Index of the candidate whose counter overflowed.
        failing_index: usize,
        /// Run identity of the failing candidate.
        failing_run_id: RunId,
        /// Counter that overflowed.
        counter: &'static str,
        /// Value at the boundary.
        value: usize,
    },

    /// Disposition failed for the candidate at `failing_index`.
    #[error("startup reconciliation sweep disposition failed at candidate {failing_index}")]
    Disposition {
        /// Already-committed prefix report.
        report: StartupReconciliationSweepReport,
        /// Index of the candidate whose disposition failed.
        failing_index: usize,
        /// Run identity of the failing candidate.
        failing_run_id: RunId,
        /// Underlying disposition failure.
        #[source]
        source: StartupReconciliationDispositionError,
    },
}

// ---------------------------------------------------------------------------
// Coordinator
// ---------------------------------------------------------------------------

#[allow(clippy::result_large_err)]
// The coordinator is intrinsically sequential and bounded: it must handle
// discovery, patch-source validation, and three disposition outcomes in one
// pass without changing behavior or public API. Splitting would obscure the
// single-pass invariant.
#[allow(clippy::too_many_lines)]
async fn sweep_impl<S>(
    repository: &Repository,
    input: StartupReconciliationSweepInput,
    patch_source: &mut S,
    observer: &mut dyn FnMut(&StartupReconciliationCandidate),
) -> Result<StartupReconciliationSweepReport, StartupReconciliationSweepError>
where
    S: StartupReconciliationPatchSource,
{
    if !(1..=64).contains(&input.limit) {
        return Err(StartupReconciliationSweepError::InvalidLimit { limit: input.limit });
    }

    let query = StartupReconciliationQuery::new(input.operated_at, input.limit).map_err(
        |error| match error {
            StartupReconciliationError::InvalidLimit { limit } => {
                StartupReconciliationSweepError::InvalidLimit { limit }
            }
            err @ StartupReconciliationError::Repository(_) => {
                StartupReconciliationSweepError::Discovery { source: err }
            }
        },
    )?;

    let candidates = repository
        .list_startup_reconciliation_candidates(query)
        .await
        .map_err(|source| StartupReconciliationSweepError::Discovery { source })?;

    let discovered = candidates.len();
    let mut report = StartupReconciliationSweepReport {
        discovered,
        attempted: 0,
        interrupted: 0,
        already_interrupted: 0,
        skipped_moved: 0,
    };

    if discovered == 0 {
        return Ok(report);
    }

    let candidates_vec = candidates.into_vec();

    for (index, candidate) in candidates_vec.iter().enumerate() {
        let patches = match patch_source.patch_ids_for(candidate) {
            Ok(value) => value,
            Err(source) => {
                return Err(StartupReconciliationSweepError::PatchSource {
                    report,
                    failing_index: index,
                    failing_run_id: candidate.run_id.clone(),
                    source,
                });
            }
        };

        // Validate shape agreement before any mutation of this candidate.
        let has_item = candidate.assistant_item_id.is_some();
        let has_patch = patches.item_patch_id.is_some();
        if has_item != has_patch {
            return Err(StartupReconciliationSweepError::PatchShape {
                report,
                failing_index: index,
                failing_run_id: candidate.run_id.clone(),
                reason: if has_item {
                    "candidate has an assistant item but no item patch was supplied"
                } else {
                    "candidate has no assistant item but an item patch was supplied"
                },
            });
        }
        if let Some(item_patch) = &patches.item_patch_id
            && item_patch.as_str() == patches.turn_patch_id.as_str()
        {
            return Err(StartupReconciliationSweepError::PatchShape {
                report,
                failing_index: index,
                failing_run_id: candidate.run_id.clone(),
                reason: "turn and item patch identities collide",
            });
        }

        let outcome = repository
            .dispose_expired_startup_candidate(StartupReconciliationDisposition {
                candidate,
                operated_at: input.operated_at,
                turn_patch_id: &patches.turn_patch_id,
                item_patch_id: patches.item_patch_id.as_ref(),
            })
            .await;

        match outcome {
            Ok(value) => match value {
                artisan_database::StartupReconciliationDispositionOutcome::Interrupted(_) => {
                    report.attempted = report.attempted.checked_add(1).ok_or(
                        StartupReconciliationSweepError::CounterOverflow {
                            report: report.clone(),
                            failing_index: index,
                            failing_run_id: candidate.run_id.clone(),
                            counter: "attempted",
                            value: report.attempted,
                        },
                    )?;
                    report.interrupted = report.interrupted.checked_add(1).ok_or(
                        StartupReconciliationSweepError::CounterOverflow {
                            report: report.clone(),
                            failing_index: index,
                            failing_run_id: candidate.run_id.clone(),
                            counter: "interrupted",
                            value: report.interrupted,
                        },
                    )?;
                    observer(candidate);
                }
                artisan_database::StartupReconciliationDispositionOutcome::AlreadyInterrupted(
                    _,
                ) => {
                    report.attempted = report.attempted.checked_add(1).ok_or(
                        StartupReconciliationSweepError::CounterOverflow {
                            report: report.clone(),
                            failing_index: index,
                            failing_run_id: candidate.run_id.clone(),
                            counter: "attempted",
                            value: report.attempted,
                        },
                    )?;
                    report.already_interrupted = report.already_interrupted.checked_add(1).ok_or(
                        StartupReconciliationSweepError::CounterOverflow {
                            report: report.clone(),
                            failing_index: index,
                            failing_run_id: candidate.run_id.clone(),
                            counter: "already_interrupted",
                            value: report.already_interrupted,
                        },
                    )?;
                    observer(candidate);
                }
                artisan_database::StartupReconciliationDispositionOutcome::SkippedMoved => {
                    report.attempted = report.attempted.checked_add(1).ok_or(
                        StartupReconciliationSweepError::CounterOverflow {
                            report: report.clone(),
                            failing_index: index,
                            failing_run_id: candidate.run_id.clone(),
                            counter: "attempted",
                            value: report.attempted,
                        },
                    )?;
                    report.skipped_moved = report.skipped_moved.checked_add(1).ok_or(
                        StartupReconciliationSweepError::CounterOverflow {
                            report: report.clone(),
                            failing_index: index,
                            failing_run_id: candidate.run_id.clone(),
                            counter: "skipped_moved",
                            value: report.skipped_moved,
                        },
                    )?;
                }
            },
            Err(source) => {
                return Err(StartupReconciliationSweepError::Disposition {
                    report,
                    failing_index: index,
                    failing_run_id: candidate.run_id.clone(),
                    source,
                });
            }
        }
    }

    Ok(report)
}

/// Bounded single-pass sweep over an injected repository.
///
/// The function calls discovery once, then disposes candidates sequentially in
/// discovery order. An empty discovery is success and never consults the patch
/// source. Source or patch-shape failure for candidate `N` occurs before any
/// mutation of `N`. A moved candidate is counted as `skipped_moved` and the
/// pass continues. An identical durable replay is counted as
/// `already_interrupted` and continues. The first source or disposition error
/// stops the pass and returns the already-committed prefix report with the
/// failing candidate's index and run identity; earlier per-candidate
/// transactions may already be durable and are never claimed rolled back.
///
/// One pass never loops, sleeps, schedules a timer, opens storage, acquires
/// custody, contacts a provider, requeues, retries a prompt, or wires
/// `ForgeApp::start`.
///
/// # Errors
///
/// Returns [`StartupReconciliationSweepError`] for invalid limits, discovery
/// failures, patch-source or patch-shape failures, counter overflow, and
/// disposition failures. Each error that carries a `report` has committed only
/// its prefix.
pub async fn sweep_startup_reconciliation<S>(
    repository: &Repository,
    input: StartupReconciliationSweepInput,
    patch_source: &mut S,
) -> Result<StartupReconciliationSweepReport, StartupReconciliationSweepError>
where
    S: StartupReconciliationPatchSource,
{
    let mut noop = |_: &StartupReconciliationCandidate| {};
    sweep_impl(repository, input, patch_source, &mut noop).await
}

pub(crate) async fn sweep_startup_reconciliation_observed<S, F>(
    repository: &Repository,
    input: StartupReconciliationSweepInput,
    patch_source: &mut S,
    mut observer: F,
) -> Result<StartupReconciliationSweepReport, StartupReconciliationSweepError>
where
    S: StartupReconciliationPatchSource,
    F: FnMut(&StartupReconciliationCandidate),
{
    sweep_impl(repository, input, patch_source, &mut observer).await
}
