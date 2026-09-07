//! Provider binding of one launched assistant run.
//!
//! `Repository::bind_run_provider` is the second transactional boundary after
//! `launch_claimed_run`. It moves a `launching` run to `running` while
//! advancing its owning `running` dispatch to the binding time in a single
//! all-or-nothing transaction. The first statement is the dispatch fence; the
//! second is the run fence. Either zero-row fence rolls back the entire
//! transaction with a typed diagnosis. This module never requeues, resurrects,
//! or proves external delivery: the returned receipts are durable facts only.
//!
//! The erased claim-token after binding is deliberately unverifiable. Replay
//! classification requires every remaining credential component to match exactly
//! and documents the limitation rather than pretending to verify it. An exact
//! replay answers `AlreadyBound` with the original `bound_at` without asserting
//! the lease is still live at retry wall time. When `bound_at ==
//! expected_launch_at` a replay can pass the dispatch fence and fail only the
//! run fence; that tentative dispatch write is still rolled back.

use artisan_domain::{MessageId, RunId, ThreadId, UnixMillis};
use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, Statement, TransactionTrait};
use thiserror::Error;
use zeroize::Zeroize;

use crate::entities::{self, AssistantRunLifecycle, DispatchState};

use super::message_dispatch::DispatchLeaseOwner;
use super::run_launch::{LaunchedRunReceipt, RunLaunchCredentials, RunStartKey};
use super::{
    ClaimedMessageDispatch, Repository, RepositoryError, corrupt_data, database_error, millis,
};

use super::run_launch::stored_bytes_match;

const PROVIDER_BINDING_MIN_BYTES: usize = 1;
const PROVIDER_BINDING_MAX_BYTES: usize = 262_144;

const BIND_DISPATCH_SQL: &str = r"
UPDATE message_dispatches
SET updated_at_ms = ?
WHERE message_id = ?
  AND correlation_id = ?
  AND attempt_count = ?
  AND queued_at_ms = ?
  AND available_at_ms = ?
  AND state = 'running'
  AND lease_owner = ?
  AND lease_expires_at_ms = ?
  AND updated_at_ms = ?
  AND lease_expires_at_ms > ?
RETURNING message_id
";

const BIND_RUN_SQL: &str = r"
UPDATE assistant_runs
SET lifecycle = 'running',
    claim_token = NULL,
    provider_binding_version = ?,
    provider_binding = ?,
    provider_bound_at_ms = ?,
    updated_at_ms = ?
WHERE run_id = ?
  AND thread_id = ?
  AND origin_message_id = ?
  AND origin_turn_id = ?
  AND lifecycle = 'launching'
  AND generation = ?
  AND run_start_key = ?
  AND owner = ?
  AND lease = ?
  AND claim_token = ?
  AND created_at_ms = ?
  AND updated_at_ms = ?
  AND provider_binding_version IS NULL
  AND provider_binding IS NULL
  AND provider_bound_at_ms IS NULL
  AND error_code IS NULL
  AND terminal_at_ms IS NULL
RETURNING run_id
";

/// Opaque provider binding bytes persisted through `OpaqueBytes`.
///
/// The wrapper enforces 1..=262144 bytes, exposes no `Debug`/`Display`/`Clone`
/// and no raw-byte accessor, and zeroizes on drop. Persisted form uses the
/// redacted `OpaqueBytes` model type so bytes do not appear in public API
/// results or model `Debug` output.
pub struct ProviderBindingBytes(Vec<u8>);

impl ProviderBindingBytes {
    /// Creates validated binding bytes without truncation.
    ///
    /// # Errors
    ///
    /// Returns `RunBindingError::InvalidBindingLength` when `bytes` is empty
    /// or exceeds 262144.
    pub fn new(bytes: Vec<u8>) -> Result<Self, RunBindingError> {
        let len = bytes.len();
        if !(PROVIDER_BINDING_MIN_BYTES..=PROVIDER_BINDING_MAX_BYTES).contains(&len) {
            return Err(RunBindingError::InvalidBindingLength { length: len });
        }
        Ok(Self(bytes))
    }

    fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for ProviderBindingBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Borrowed inputs of one atomic provider-bind.
///
/// All identities and capabilities are borrowed so an unknown transaction
/// outcome can be retried verbatim. `expected_launch_at` is the exact
/// `operated_at` used for the prior `launch_claimed_run`; `bound_at` is the
/// new binding time. `binding_version` must be positive. Generation is taken
/// from the `LaunchedRunReceipt` and must equal the persisted run generation,
/// never the dispatch attempt count.
pub struct BindRunProvider<'a> {
    /// The original live claim snapshot returned by `claim_next_message_dispatch`.
    pub claimed: &'a ClaimedMessageDispatch,
    /// Durable receipt returned by `launch_claimed_run`.
    pub receipt: &'a LaunchedRunReceipt,
    /// Exact 32-byte deduplication key of the launched run.
    pub run_start_key: &'a RunStartKey,
    /// Named owner/lease/claim capabilities of the launched run.
    pub credentials: &'a RunLaunchCredentials,
    /// Exact launch `operated_at` (`assistant_runs.created_at_ms`).
    pub expected_launch_at: UnixMillis,
    /// Binding time to write to both dispatch and run.
    pub bound_at: UnixMillis,
    /// Positive binding version.
    pub binding_version: i64,
    /// Opaque binding payload.
    pub binding_bytes: &'a ProviderBindingBytes,
}

/// Payload-free durable receipt of one provider binding.
///
/// Carries identities, generation, binding version and bound time only — never
/// the binding payload, never a capability, and never authority to resubmit a
/// prompt or proof of external delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundRunReceipt {
    /// Bound run identity.
    pub run_id: RunId,
    /// Thread owning the run.
    pub thread_id: ThreadId,
    /// Origin message identity.
    pub message_id: MessageId,
    /// Generation recorded on the run (must equal receipt generation).
    pub generation: i64,
    /// Persisted binding version.
    pub binding_version: i64,
    /// Time the provider binding was recorded.
    pub bound_at: UnixMillis,
}

/// Typed outcome of one bind call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindRunProviderOutcome {
    /// This transaction committed the binding.
    Bound(BoundRunReceipt),
    /// An earlier identical transaction already bound exactly this payload;
    /// durable receipt information only, never provider authority. The erased
    /// claim-token bytes cannot be compared and are deliberately ignored; a
    /// different supplied claim token after erasure is unverifiable, not
    /// misrepresented as verified.
    AlreadyBound(BoundRunReceipt),
}

/// Capability-specific failures of `Repository::bind_run_provider`.
///
/// Existing repository-layer rejections surface through
/// `RunBindingError::Repository` with their original typed source; nothing
/// here leaks capability or binding bytes.
#[derive(Debug, Error)]
pub enum RunBindingError {
    #[error("run `{run_id}` does not exist")]
    RunNotFound { run_id: RunId },
    #[error("run `{run_id}` is not in launching state")]
    RunNotLaunchable { run_id: RunId },
    #[error("provider binding payload is {length} bytes; must be 1..=262144")]
    InvalidBindingLength { length: usize },
    #[error("provider binding version {version} must be positive")]
    InvalidBindingVersion { version: i64 },
    #[error("run generation {generation} must be positive")]
    InvalidGeneration { generation: i64 },
    #[error("claimed dispatch snapshot for `{message_id}` no longer matches")]
    SnapshotMismatch { message_id: MessageId },
    #[error("run `{run_id}` launch start key or tokens did not match")]
    CredentialMismatch { run_id: RunId },
    #[error("binding identity conflict: {reason}")]
    IdentityConflict { reason: &'static str },
    #[error(transparent)]
    Repository(#[from] RepositoryError),
}

impl Repository {
    /// Atomically binds provider state to one launched run.
    ///
    /// After local bounds/generation/chronology checks
    /// (`claimed.updated_at <= expected_launch_at <= bound_at`, claimed lease
    /// expiry `> bound_at`), one transaction fences the RUNNING dispatch then
    /// the launching run. The dispatch fence requires exact
    /// message/correlation/attempt/queued/available/hex-owner/lease-expiry,
    /// `state='running'`, `updated_at_ms = expected_launch_at`, and
    /// `lease_expires_at > bound_at`, tentatively moving `updated_at_ms` to
    /// `bound_at`. The run fence requires exact run id, `lifecycle='launching'`,
    /// exact generation (positive, equal to receipt generation, never the
    /// dispatch attempt count), start key, thread/origin message/origin turn,
    /// owner/lease/claim token, `created_at_ms = updated_at_ms =
    /// expected_launch_at`, and absent binding/error/terminal fields, then
    /// sets `lifecycle='running'`, clears ONLY `claim_token`, writes the
    /// binding version/blob and `provider_bound_at_ms = bound_at`. Dispatch
    /// retains owner/expiry and stays RUNNING. Any zero-row fence rolls back
    /// the entire transaction with a typed diagnosis. An exact durable replay
    /// (dispatch RUNNING at `bound_at`, run `running` with exact binding
    /// tuple, matching owner/lease/start key/generation/origins,
    /// `created_at = expected_launch_at`, `updated_at = bound_at`, `claim_token`
    /// NULL) answers `AlreadyBound` without asserting the lease is live at
    /// retry wall time. The erased claim token is unverifiable after binding.
    ///
    /// # Errors
    ///
    /// Returns `RunBindingError::Repository` for existing typed rejections,
    /// `SnapshotMismatch` for stale snapshots, `CredentialMismatch` for
    /// mismatching start keys/tokens/generation, `InvalidBindingLength` /
    /// `InvalidBindingVersion`, `IdentityConflict` for colliding origins, and
    /// repository chronology/lease errors where appropriate.
    pub async fn bind_run_provider(
        &self,
        command: BindRunProvider<'_>,
    ) -> Result<BindRunProviderOutcome, RunBindingError> {
        validate_bind_inputs(&command)?;
        let expected_launch_at_ms = millis(command.expected_launch_at);
        let bound_at_ms = millis(command.bound_at);
        let claimed = command.claimed;
        let receipt = command.receipt;

        let encoded_owner = claimed.owner.to_storage();
        let transaction = self.database.begin().await.map_err(|source| {
            RunBindingError::Repository(database_error("begin run binding", source))
        })?;

        let dispatch_stmt = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            BIND_DISPATCH_SQL,
            [
                bound_at_ms.into(),
                claimed.message_id.as_str().into(),
                claimed.correlation_id.as_str().into(),
                i64::from(claimed.attempt_count).into(),
                millis(claimed.queued_at).into(),
                millis(claimed.available_at).into(),
                encoded_owner.into(),
                millis(claimed.lease_expires_at).into(),
                expected_launch_at_ms.into(),
                bound_at_ms.into(),
            ],
        );
        let fenced_dispatch = transaction
            .query_one_raw(dispatch_stmt)
            .await
            .map_err(|source| {
                RunBindingError::Repository(database_error("fence run binding dispatch", source))
            });
        let fenced_dispatch = match fenced_dispatch {
            Ok(v) => v,
            Err(error) => return rollback_bind(transaction, error).await,
        };
        if fenced_dispatch.is_none() {
            let diagnosed =
                classify_unfenced_bind(&transaction, &command, expected_launch_at_ms, bound_at_ms)
                    .await;
            return finish_diagnosed_bind(transaction, diagnosed).await;
        }

        let (owner_cap, lease_cap, claim_cap) = command.credentials.parts();
        let run_stmt = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            BIND_RUN_SQL,
            [
                command.binding_version.into(),
                command.binding_bytes.as_slice().to_vec().into(),
                bound_at_ms.into(),
                bound_at_ms.into(),
                command.receipt.run_id.as_str().into(),
                command.receipt.thread_id.as_str().into(),
                command.receipt.message_id.as_str().into(),
                command.receipt.turn_id.as_str().into(),
                receipt.generation.into(),
                command.run_start_key.expose().to_vec().into(),
                owner_cap.expose().to_vec().into(),
                lease_cap.expose().to_vec().into(),
                claim_cap.expose().to_vec().into(),
                expected_launch_at_ms.into(),
                expected_launch_at_ms.into(),
            ],
        );
        let fenced_run = transaction.query_one_raw(run_stmt).await.map_err(|source| {
            RunBindingError::Repository(database_error("fence run binding run", source))
        });
        let fenced_run = match fenced_run {
            Ok(v) => v,
            Err(error) => return rollback_bind(transaction, error).await,
        };
        if fenced_run.is_none() {
            let diagnosed =
                classify_unfenced_bind(&transaction, &command, expected_launch_at_ms, bound_at_ms)
                    .await;
            return finish_diagnosed_bind(transaction, diagnosed).await;
        }

        transaction.commit().await.map_err(|source| {
            RunBindingError::Repository(database_error("commit run binding", source))
        })?;

        Ok(BindRunProviderOutcome::Bound(BoundRunReceipt {
            run_id: receipt.run_id.clone(),
            thread_id: receipt.thread_id.clone(),
            message_id: receipt.message_id.clone(),
            generation: receipt.generation,
            binding_version: command.binding_version,
            bound_at: command.bound_at,
        }))
    }
}

fn validate_bind_inputs(command: &BindRunProvider<'_>) -> Result<(), RunBindingError> {
    if command.binding_version <= 0 {
        return Err(RunBindingError::InvalidBindingVersion {
            version: command.binding_version,
        });
    }
    if command.receipt.message_id != command.claimed.message_id {
        return Err(RunBindingError::SnapshotMismatch {
            message_id: command.claimed.message_id.clone(),
        });
    }
    if command.receipt.generation <= 0 {
        return Err(RunBindingError::InvalidGeneration {
            generation: command.receipt.generation,
        });
    }
    if command.claimed.attempt_count == 0 || i32::try_from(command.claimed.attempt_count).is_err() {
        return Err(RunBindingError::SnapshotMismatch {
            message_id: command.claimed.message_id.clone(),
        });
    }
    let expected_launch_at_ms = millis(command.expected_launch_at);
    let bound_at_ms = millis(command.bound_at);
    if millis(command.claimed.updated_at) > expected_launch_at_ms
        || expected_launch_at_ms > bound_at_ms
    {
        return Err(RunBindingError::Repository(
            RepositoryError::InvalidChronology {
                earlier_field: "claimed dispatch updated_at",
                later_field: "bind expected_launch_at/bound_at",
            },
        ));
    }
    if millis(command.claimed.lease_expires_at) <= bound_at_ms {
        return Err(RunBindingError::Repository(
            RepositoryError::DispatchLeaseExpired {
                message_id: command.claimed.message_id.clone(),
                lease_expires_at_ms: millis(command.claimed.lease_expires_at),
                operated_at_ms: bound_at_ms,
            },
        ));
    }
    Ok(())
}

async fn classify_unfenced_bind(
    transaction: &sea_orm::DatabaseTransaction,
    command: &BindRunProvider<'_>,
    expected_launch_at_ms: i64,
    bound_at_ms: i64,
) -> Result<BindRunProviderOutcome, RunBindingError> {
    let dispatch = load_dispatch_for_classify(transaction, command).await?;
    let dispatch_owner_matches = dispatch_owner_matches(&dispatch, command.claimed);
    let is_dispatch_replay = is_dispatch_replay(&dispatch, command.claimed, bound_at_ms);
    if let Some(replay) = check_exact_replay(
        transaction,
        command,
        expected_launch_at_ms,
        bound_at_ms,
        is_dispatch_replay,
        dispatch_owner_matches,
    )
    .await?
    {
        return Ok(replay);
    }
    if let Some(err) = diagnose_non_running_dispatch(
        &dispatch,
        command.claimed,
        expected_launch_at_ms,
        bound_at_ms,
        dispatch_owner_matches,
    ) {
        return Err(err);
    }
    diagnose_dispatch_snapshot(
        &dispatch,
        command.claimed,
        expected_launch_at_ms,
        bound_at_ms,
        dispatch_owner_matches,
    )?;
    diagnose_run_fence(transaction, command, expected_launch_at_ms).await
}

async fn load_dispatch_for_classify(
    transaction: &sea_orm::DatabaseTransaction,
    command: &BindRunProvider<'_>,
) -> Result<entities::MessageDispatch, RunBindingError> {
    entities::message_dispatch::Entity::find_by_id(command.claimed.message_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            RunBindingError::Repository(database_error("classify unfenced bind dispatch", source))
        })?
        .ok_or(RunBindingError::Repository(
            RepositoryError::DispatchNotFound {
                message_id: command.claimed.message_id.clone(),
            },
        ))
}

fn dispatch_owner_matches(
    dispatch: &entities::MessageDispatch,
    claimed: &ClaimedMessageDispatch,
) -> bool {
    dispatch.lease_owner.as_deref().is_some_and(|owner| {
        DispatchLeaseOwner::from_storage(owner).is_ok_and(|po| po.constant_time_eq(&claimed.owner))
    })
}

fn is_dispatch_replay(
    dispatch: &entities::MessageDispatch,
    claimed: &ClaimedMessageDispatch,
    bound_at_ms: i64,
) -> bool {
    dispatch.state == DispatchState::Running
        && dispatch.correlation_id == claimed.correlation_id.as_str()
        && i64::from(dispatch.attempt_count) == i64::from(claimed.attempt_count)
        && dispatch.queued_at_ms == millis(claimed.queued_at)
        && dispatch.available_at_ms == millis(claimed.available_at)
        && dispatch.lease_expires_at_ms == Some(millis(claimed.lease_expires_at))
        && dispatch.updated_at_ms == bound_at_ms
}

fn is_run_replay(
    run: &entities::AssistantRun,
    command: &BindRunProvider<'_>,
    expected_launch_at_ms: i64,
    bound_at_ms: i64,
) -> bool {
    run.lifecycle == AssistantRunLifecycle::Running
        && run.generation == command.receipt.generation
        && run.thread_id == command.receipt.thread_id.as_str()
        && run.origin_message_id == command.receipt.message_id.as_str()
        && run.origin_turn_id == command.receipt.turn_id.as_str()
        && run.created_at_ms == expected_launch_at_ms
        && run.updated_at_ms == bound_at_ms
        && run.provider_binding_version == Some(command.binding_version)
        && run.provider_bound_at_ms == Some(bound_at_ms)
        && run
            .provider_binding
            .as_ref()
            .is_some_and(|b| b.as_slice() == command.binding_bytes.as_slice())
        && run.claim_token.is_none()
        && run.error_code.is_none()
        && run.terminal_at_ms.is_none()
        && stored_bytes_match(&run.run_start_key, command.run_start_key.expose())
        && {
            let (owner_cap, lease_cap, _) = command.credentials.parts();
            owner_cap.matches_stored(run.owner.as_ref())
                && lease_cap.matches_stored(run.lease.as_ref())
        }
}

async fn check_exact_replay(
    transaction: &sea_orm::DatabaseTransaction,
    command: &BindRunProvider<'_>,
    expected_launch_at_ms: i64,
    bound_at_ms: i64,
    is_dispatch_replay: bool,
    dispatch_owner_matches: bool,
) -> Result<Option<BindRunProviderOutcome>, RunBindingError> {
    let run = entities::assistant_run::Entity::find_by_id(command.receipt.run_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            RunBindingError::Repository(database_error("classify unfenced bind run", source))
        })?;
    if let Some(run) = run
        && is_dispatch_replay
        && dispatch_owner_matches
        && is_run_replay(&run, command, expected_launch_at_ms, bound_at_ms)
    {
        return Ok(Some(BindRunProviderOutcome::AlreadyBound(
            BoundRunReceipt {
                run_id: command.receipt.run_id.clone(),
                thread_id: command.receipt.thread_id.clone(),
                message_id: command.receipt.message_id.clone(),
                generation: command.receipt.generation,
                binding_version: command.binding_version,
                bound_at: command.bound_at,
            },
        )));
    }
    Ok(None)
}

fn diagnose_non_running_dispatch(
    dispatch: &entities::MessageDispatch,
    claimed: &ClaimedMessageDispatch,
    expected_launch_at_ms: i64,
    bound_at_ms: i64,
    dispatch_owner_matches: bool,
) -> Option<RunBindingError> {
    if dispatch.state == DispatchState::Running {
        return None;
    }
    if dispatch.state == DispatchState::Leased {
        if dispatch.updated_at_ms > expected_launch_at_ms {
            return Some(RunBindingError::Repository(
                RepositoryError::InvalidChronology {
                    earlier_field: "message_dispatches.updated_at_ms",
                    later_field: "bind expected_launch_at",
                },
            ));
        }
        let Some(persisted_expiry) = dispatch.lease_expires_at_ms else {
            return Some(RunBindingError::Repository(corrupt_data(
                "message_dispatches",
                "lease_expires_at_ms",
                "required value is null",
            )));
        };
        if persisted_expiry <= bound_at_ms {
            return Some(RunBindingError::Repository(
                RepositoryError::DispatchLeaseExpired {
                    message_id: claimed.message_id.clone(),
                    lease_expires_at_ms: persisted_expiry,
                    operated_at_ms: bound_at_ms,
                },
            ));
        }
        if !dispatch_owner_matches {
            return Some(RunBindingError::Repository(
                RepositoryError::DispatchOwnerMismatch {
                    message_id: claimed.message_id.clone(),
                },
            ));
        }
    }
    Some(RunBindingError::SnapshotMismatch {
        message_id: claimed.message_id.clone(),
    })
}

fn diagnose_dispatch_snapshot(
    dispatch: &entities::MessageDispatch,
    claimed: &ClaimedMessageDispatch,
    expected_launch_at_ms: i64,
    bound_at_ms: i64,
    dispatch_owner_matches: bool,
) -> Result<(), RunBindingError> {
    if !dispatch_owner_matches
        || dispatch.correlation_id != claimed.correlation_id.as_str()
        || i64::from(dispatch.attempt_count) != i64::from(claimed.attempt_count)
        || dispatch.queued_at_ms != millis(claimed.queued_at)
        || dispatch.available_at_ms != millis(claimed.available_at)
        || dispatch.lease_expires_at_ms != Some(millis(claimed.lease_expires_at))
    {
        if let Some(owner) = dispatch.lease_owner.as_deref() {
            if let Ok(po) = DispatchLeaseOwner::from_storage(owner) {
                if !po.constant_time_eq(&claimed.owner) {
                    return Err(RunBindingError::Repository(
                        RepositoryError::DispatchOwnerMismatch {
                            message_id: claimed.message_id.clone(),
                        },
                    ));
                }
            } else {
                return Err(RunBindingError::Repository(corrupt_data(
                    "message_dispatches",
                    "lease_owner",
                    "corrupt",
                )));
            }
        }
        return Err(RunBindingError::SnapshotMismatch {
            message_id: claimed.message_id.clone(),
        });
    }
    if dispatch.updated_at_ms != expected_launch_at_ms && dispatch.updated_at_ms != bound_at_ms {
        return Err(RunBindingError::SnapshotMismatch {
            message_id: claimed.message_id.clone(),
        });
    }
    if dispatch
        .lease_expires_at_ms
        .is_some_and(|exp| exp <= bound_at_ms)
    {
        return Err(RunBindingError::Repository(
            RepositoryError::DispatchLeaseExpired {
                message_id: claimed.message_id.clone(),
                lease_expires_at_ms: dispatch.lease_expires_at_ms.unwrap_or(0),
                operated_at_ms: bound_at_ms,
            },
        ));
    }
    Ok(())
}

async fn diagnose_run_fence(
    transaction: &sea_orm::DatabaseTransaction,
    command: &BindRunProvider<'_>,
    expected_launch_at_ms: i64,
) -> Result<BindRunProviderOutcome, RunBindingError> {
    let run = entities::assistant_run::Entity::find_by_id(command.receipt.run_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            RunBindingError::Repository(database_error(
                "classify unfenced bind run second load",
                source,
            ))
        })?
        .ok_or(RunBindingError::RunNotFound {
            run_id: command.receipt.run_id.clone(),
        })?;
    if run.lifecycle != AssistantRunLifecycle::Launching
        && run.lifecycle != AssistantRunLifecycle::Running
    {
        return Err(RunBindingError::RunNotLaunchable {
            run_id: command.receipt.run_id.clone(),
        });
    }
    if run.lifecycle == AssistantRunLifecycle::Running {
        return Err(RunBindingError::CredentialMismatch {
            run_id: command.receipt.run_id.clone(),
        });
    }
    if run.generation != command.receipt.generation {
        return Err(RunBindingError::CredentialMismatch {
            run_id: command.receipt.run_id.clone(),
        });
    }
    if run.thread_id != command.receipt.thread_id.as_str()
        || run.origin_message_id != command.claimed.message_id.as_str()
        || run.origin_turn_id != command.receipt.turn_id.as_str()
    {
        return Err(RunBindingError::IdentityConflict {
            reason: "stored run originates from another message or turn",
        });
    }
    if run.created_at_ms != expected_launch_at_ms || run.updated_at_ms != expected_launch_at_ms {
        return Err(RunBindingError::SnapshotMismatch {
            message_id: command.claimed.message_id.clone(),
        });
    }
    if !stored_bytes_match(&run.run_start_key, command.run_start_key.expose()) {
        return Err(RunBindingError::CredentialMismatch {
            run_id: command.receipt.run_id.clone(),
        });
    }
    {
        let (owner_cap, lease_cap, claim_cap) = command.credentials.parts();
        if !owner_cap.matches_stored(run.owner.as_ref())
            || !lease_cap.matches_stored(run.lease.as_ref())
            || !claim_cap.matches_stored(run.claim_token.as_ref())
        {
            return Err(RunBindingError::CredentialMismatch {
                run_id: command.receipt.run_id.clone(),
            });
        }
    }
    if run.provider_binding_version.is_some()
        || run.provider_binding.is_some()
        || run.provider_bound_at_ms.is_some()
    {
        return Err(RunBindingError::CredentialMismatch {
            run_id: command.receipt.run_id.clone(),
        });
    }
    Err(RunBindingError::SnapshotMismatch {
        message_id: command.claimed.message_id.clone(),
    })
}

async fn rollback_bind<T>(
    transaction: sea_orm::DatabaseTransaction,
    error: RunBindingError,
) -> Result<T, RunBindingError> {
    transaction.rollback().await.map_err(|source| {
        RunBindingError::Repository(database_error("roll back run binding", source))
    })?;
    Err(error)
}

async fn finish_diagnosed_bind(
    transaction: sea_orm::DatabaseTransaction,
    diagnosed: Result<BindRunProviderOutcome, RunBindingError>,
) -> Result<BindRunProviderOutcome, RunBindingError> {
    transaction.rollback().await.map_err(|source| {
        RunBindingError::Repository(database_error("roll back run binding diagnosis", source))
    })?;
    diagnosed
}
