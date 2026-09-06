//! Bounded, read-only selection of a provider session for the next composer run.
//!
//! A provider binding is useful only when it is tied to the exact immutable
//! run configuration that the caller is about to use.  This module therefore
//! reads the immutable `assistant_runs.engine_run_config` snapshot and the
//! binding in one transaction, newest first.  It deliberately does not
//! repair rows, create a side table, or expose provider binding bytes.

use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder,
    TransactionTrait,
};
use serde::Deserialize;

use artisan_domain::{EngineId, EngineProfileId, RunId, ThreadId, UnixMillis};

use crate::engine_run_config;
use crate::entities::{self, AssistantRunLifecycle};

use super::{Repository, RepositoryError, corrupt_data, database_error};

const PROVIDER_BINDING_VERSION: i64 = 1;
const SESSION_ID_MAX_BYTES: usize = 256;

/// Exact scope used to select a continuation.  The caller supplies the
/// current run id only when it has already persisted a new run and wants that
/// row excluded from historical selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionContinuationQuery {
    /// The exact conversation thread whose runs may be considered.
    pub thread_id: ThreadId,
    /// The engine implementation the new run will use.
    pub engine_id: EngineId,
    /// The managed profile the new run will use.
    pub profile_id: EngineProfileId,
    /// A newly persisted current run that must not select itself.
    pub exclude_run_id: Option<RunId>,
}

/// The explicit result of a continuation lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionContinuationLookup {
    /// There are no historical run rows after applying the exclusion.
    NoHistory,
    /// A settled, compatible provider binding and its durable facts are safe
    /// for the caller to offer to the HTTP resume leaf.
    Usable(SessionContinuation),
    /// A newer row prevents falling back to an older provider conversation.
    Unavailable(SessionContinuationUnavailable),
    /// A newer row is settled but cannot be used with the requested scope.
    Incompatible(SessionContinuationIncompatible),
}

/// Why a newer row prevents safe historical selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionContinuationUnavailableReason {
    /// A queued, launching, running, waiting, or cancellation-requested run
    /// may still produce external effects.
    ActiveRun,
    /// A settled run has no complete provider binding tuple.
    UnboundSettledRun,
    /// An interrupted run has an ambiguous external outcome.
    AmbiguousRun,
    /// More rows existed than this bounded read is allowed to inspect.
    CandidateLimit,
}

/// A safe reference to the row that caused an unavailable result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionContinuationUnavailable {
    /// The newer run that blocks fallback.
    pub run_id: RunId,
    /// The bounded reason for the block.
    pub reason: SessionContinuationUnavailableReason,
}

/// Why a settled row is not compatible with the requested engine/profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionContinuationIncompatibility {
    /// The immutable run configuration selected another engine.
    Engine,
    /// The immutable run configuration selected another profile.
    Profile,
    /// The provider binding column uses an unsupported version.
    ProviderBindingVersion,
    /// The provider binding names another engine.
    ProviderBindingEngine,
    /// The provider binding names another profile.
    ProviderBindingProfile,
}

/// A safe reference to the row that caused an incompatible result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionContinuationIncompatible {
    /// The newer run that prevents fallback.
    pub run_id: RunId,
    /// The bounded incompatibility category.
    pub reason: SessionContinuationIncompatibility,
}

/// Validated provider session identity.  The value is available to the
/// engine-owner caller only through [`Self::as_str`]; formatting is redacted.
#[derive(Clone, Eq, PartialEq)]
pub struct ProviderSessionId(String);

impl ProviderSessionId {
    fn parse(value: String) -> Result<Self, RepositoryError> {
        if value.is_empty()
            || value.len() > SESSION_ID_MAX_BYTES
            || value.contains('/')
            || value.contains('?')
            || value.contains('#')
            || value.contains('%')
            || value.contains('\r')
            || value.contains('\n')
            || value
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(corrupt_data(
                "assistant_runs",
                "provider_binding",
                "session id is outside its bounded route-segment grammar",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the validated route segment for the engine-owner HTTP leaf.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ProviderSessionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProviderSessionId { <redacted> }")
    }
}

/// Redacted immutable facts about the historical run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriorRunFacts {
    /// The selected historical run.
    pub run_id: RunId,
    /// The run's exact thread identity.
    pub thread_id: ThreadId,
    /// The persisted lifecycle at selection time.
    pub lifecycle: AssistantRunLifecycle,
    /// The fenced generation used by its durable checkpoint.
    pub generation: i64,
    /// Immutable run creation instant.
    pub created_at: UnixMillis,
    /// Last durable row update instant.
    pub updated_at: UnixMillis,
    /// Terminal instant, when the lifecycle has one.
    pub terminal_at: Option<UnixMillis>,
}

/// Redacted facts about the durable checkpoint tuple.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionContinuationCheckpoint {
    /// Checkpoint generation, which must equal [`PriorRunFacts::generation`].
    pub generation: i64,
    /// Last durably committed batch sequence represented by the checkpoint.
    pub last_batch_sequence: i64,
    /// Provider checkpoint schema version, if a payload exists.
    pub engine_checkpoint_version: Option<i64>,
    /// Whether a provider checkpoint payload exists.  The payload itself is
    /// intentionally never returned.
    pub has_engine_checkpoint: bool,
    /// Checkpoint update instant.
    pub updated_at: UnixMillis,
}

/// Durable sequence facts needed to resume the log tail without replaying
/// prompt text as a fake continuation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionContinuationSequence {
    /// Last batch sequence from the checkpoint, or zero without one.
    pub last_batch_sequence: i64,
    /// Last persisted receipt sequence, when one exists.
    pub last_committed_batch_sequence: Option<i64>,
    /// Last conversation patch sequence, when the thread state row exists.
    pub last_patch_sequence: Option<i64>,
}

/// A provider session and the nonsecret durable facts that authorize a
/// caller to attempt resumption.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionContinuation {
    /// The exact thread selected by the query.
    pub thread_id: ThreadId,
    /// The immutable engine implementation recorded for the run.
    pub engine_id: EngineId,
    /// The immutable profile recorded for the run.
    pub profile_id: EngineProfileId,
    /// The validated provider session route segment.
    pub session_id: ProviderSessionId,
    /// The persisted provider-binding column version.
    pub binding_version: i64,
    /// Historical run facts.
    pub prior_run: PriorRunFacts,
    /// Historical checkpoint facts, if a checkpoint row exists.
    pub checkpoint: Option<SessionContinuationCheckpoint>,
    /// Historical sequence facts.
    pub sequence: SessionContinuationSequence,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderBinding {
    engine: String,
    profile_id: String,
    session_id: String,
}

impl Repository {
    /// Reads the newest eligible provider binding for an exact thread and
    /// engine/profile scope.
    ///
    /// Rows are ordered by immutable creation time descending and then by run
    /// id descending.  A newer active, unbound, ambiguous, or incompatible
    /// row is returned as an explicit disposition and never skipped in favour
    /// of an older conversation.  The only intentional skip is
    /// `exclude_run_id`, which is the caller's current newly-launched run.
    ///
    /// The read is transactionally consistent across the run, checkpoint,
    /// receipts, and thread sequence row.  It never mutates or repairs any
    /// persisted data.
    pub async fn read_session_continuation(
        &self,
        query: SessionContinuationQuery,
    ) -> Result<SessionContinuationLookup, RepositoryError> {
        let transaction = self
            .database
            .begin()
            .await
            .map_err(|source| database_error("begin session-continuation read", source))?;

        let result = read_session_continuation(&transaction, &query).await;
        match result {
            Ok(value) => {
                transaction
                    .commit()
                    .await
                    .map_err(|source| database_error("commit session-continuation read", source))?;
                Ok(value)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }
}

async fn read_session_continuation<C: ConnectionTrait>(
    database: &C,
    query: &SessionContinuationQuery,
) -> Result<SessionContinuationLookup, RepositoryError> {
    let mut candidates = entities::assistant_run::Entity::find()
        .filter(entities::assistant_run::Column::ThreadId.eq(query.thread_id.as_str()));
    if let Some(exclude_run_id) = &query.exclude_run_id {
        candidates =
            candidates.filter(entities::assistant_run::Column::RunId.ne(exclude_run_id.as_str()));
    }
    // Only the newest non-excluded run may decide continuation. Reading
    // older rows adds no authority and must not cap the thread's lifetime.
    let candidate = candidates
        .order_by_desc(entities::assistant_run::Column::CreatedAtMs)
        .order_by_desc(entities::assistant_run::Column::RunId)
        .one(database)
        .await
        .map_err(|source| database_error("read session-continuation candidate", source))?;
    let Some(row) = candidate else {
        return Ok(SessionContinuationLookup::NoHistory);
    };
    let run_id = parse_run_id(&row.run_id)?;
    inspect_candidate(database, query, row, run_id).await
}

async fn inspect_candidate<C: ConnectionTrait>(
    database: &C,
    query: &SessionContinuationQuery,
    run: entities::assistant_run::Model,
    run_id: RunId,
) -> Result<SessionContinuationLookup, RepositoryError> {
    validate_run_row(&run, query)?;

    match &run.lifecycle {
        AssistantRunLifecycle::Queued
        | AssistantRunLifecycle::Launching
        | AssistantRunLifecycle::Running
        | AssistantRunLifecycle::Waiting
        | AssistantRunLifecycle::CancelRequested => {
            return Ok(SessionContinuationLookup::Unavailable(
                SessionContinuationUnavailable {
                    run_id: run_id.clone(),
                    reason: SessionContinuationUnavailableReason::ActiveRun,
                },
            ));
        }
        AssistantRunLifecycle::Interrupted => {
            return Ok(SessionContinuationLookup::Unavailable(
                SessionContinuationUnavailable {
                    run_id: run_id.clone(),
                    reason: SessionContinuationUnavailableReason::AmbiguousRun,
                },
            ));
        }
        AssistantRunLifecycle::Completed
        | AssistantRunLifecycle::Failed
        | AssistantRunLifecycle::Cancelled => {}
    }

    let config = load_run_config(&run)?;
    let config_engine = config.selection().engine_id();
    let config_profile = config.selection().profile_id();
    if config_engine != query.engine_id {
        return Ok(SessionContinuationLookup::Incompatible(
            SessionContinuationIncompatible {
                run_id: run_id.clone(),
                reason: SessionContinuationIncompatibility::Engine,
            },
        ));
    }
    if config_profile != &query.profile_id {
        return Ok(SessionContinuationLookup::Incompatible(
            SessionContinuationIncompatible {
                run_id: run_id.clone(),
                reason: SessionContinuationIncompatibility::Profile,
            },
        ));
    }

    let (binding_version, session_id, profile_id) = match decode_binding(&run, config_engine)? {
        BindingDisposition::Unbound => {
            return Ok(SessionContinuationLookup::Unavailable(
                SessionContinuationUnavailable {
                    run_id: run_id.clone(),
                    reason: SessionContinuationUnavailableReason::UnboundSettledRun,
                },
            ));
        }
        BindingDisposition::Incompatible(reason) => {
            return Ok(SessionContinuationLookup::Incompatible(
                SessionContinuationIncompatible {
                    run_id: run_id.clone(),
                    reason,
                },
            ));
        }
        BindingDisposition::Usable {
            version,
            session_id,
            profile_id,
        } => (version, session_id, profile_id),
    };

    if profile_id.as_str() != config_profile.as_str() {
        return Ok(SessionContinuationLookup::Incompatible(
            SessionContinuationIncompatible {
                run_id: run_id.clone(),
                reason: SessionContinuationIncompatibility::ProviderBindingProfile,
            },
        ));
    }

    let (checkpoint, sequence) = read_durable_facts(database, &run, &run_id).await?;
    let prior_run = PriorRunFacts {
        run_id,
        thread_id: query.thread_id.clone(),
        lifecycle: run.lifecycle,
        generation: run.generation,
        created_at: UnixMillis::from_millis(run.created_at_ms),
        updated_at: UnixMillis::from_millis(run.updated_at_ms),
        terminal_at: run.terminal_at_ms.map(UnixMillis::from_millis),
    };

    Ok(SessionContinuationLookup::Usable(SessionContinuation {
        thread_id: query.thread_id.clone(),
        engine_id: config_engine,
        profile_id: config_profile.clone(),
        session_id,
        binding_version,
        prior_run,
        checkpoint,
        sequence,
    }))
}

fn validate_run_row(
    run: &entities::assistant_run::Model,
    query: &SessionContinuationQuery,
) -> Result<(), RepositoryError> {
    if run.thread_id != query.thread_id.as_str() {
        return Err(corrupt_data(
            "assistant_runs",
            "thread_id",
            "candidate thread does not match the scoped thread",
        ));
    }
    if run.generation < 0 {
        return Err(corrupt_data(
            "assistant_runs",
            "generation",
            "run generation is negative",
        ));
    }
    if run.updated_at_ms < run.created_at_ms {
        return Err(corrupt_data(
            "assistant_runs",
            "updated_at_ms",
            "run update precedes creation",
        ));
    }
    match &run.lifecycle {
        AssistantRunLifecycle::Completed
        | AssistantRunLifecycle::Failed
        | AssistantRunLifecycle::Cancelled => {
            let Some(terminal_at_ms) = run.terminal_at_ms else {
                return Err(corrupt_data(
                    "assistant_runs",
                    "terminal_at_ms",
                    "settled run has no terminal timestamp",
                ));
            };
            if !(run.created_at_ms..=run.updated_at_ms).contains(&terminal_at_ms) {
                return Err(corrupt_data(
                    "assistant_runs",
                    "terminal_at_ms",
                    "terminal timestamp is outside the run interval",
                ));
            }
        }
        _ if run.terminal_at_ms.is_some() => {
            return Err(corrupt_data(
                "assistant_runs",
                "terminal_at_ms",
                "live or interrupted run has a terminal timestamp",
            ));
        }
        _ => {}
    }
    if run.provider_binding_version.is_some()
        || run.provider_binding.is_some()
        || run.provider_bound_at_ms.is_some()
    {
        let (Some(bound_at_ms), Some(_)) =
            (run.provider_bound_at_ms, run.provider_binding.as_ref())
        else {
            return Err(corrupt_data(
                "assistant_runs",
                "provider_binding",
                "provider binding tuple is incomplete",
            ));
        };
        if run.provider_binding_version.is_none()
            || bound_at_ms < run.created_at_ms
            || bound_at_ms > run.updated_at_ms
        {
            return Err(corrupt_data(
                "assistant_runs",
                "provider_bound_at_ms",
                "provider binding timestamp is outside the run interval",
            ));
        }
    }
    Ok(())
}

fn load_run_config(
    run: &entities::assistant_run::Model,
) -> Result<artisan_domain::EngineRunConfig, RepositoryError> {
    if run.engine_run_config_version != Some(1) && run.engine_run_config_version != Some(2)
        || run.engine_run_config_revision.is_none()
        || run
            .engine_run_config_revision
            .is_some_and(|revision| revision <= 0)
    {
        return Err(corrupt_data(
            "assistant_runs",
            "engine_run_config_version",
            "immutable engine configuration tuple is invalid",
        ));
    }
    let Some(config) = run.engine_run_config.as_ref() else {
        return Err(corrupt_data(
            "assistant_runs",
            "engine_run_config",
            "immutable engine configuration is missing",
        ));
    };
    engine_run_config::decode(config.as_slice()).map_err(|_| {
        corrupt_data(
            "assistant_runs",
            "engine_run_config",
            "immutable engine configuration is not canonical",
        )
    })
}

enum BindingDisposition {
    Unbound,
    Incompatible(SessionContinuationIncompatibility),
    Usable {
        version: i64,
        session_id: ProviderSessionId,
        profile_id: EngineProfileId,
    },
}

fn decode_binding(
    run: &entities::assistant_run::Model,
    expected_engine: EngineId,
) -> Result<BindingDisposition, RepositoryError> {
    let (Some(version), Some(binding), Some(_bound_at_ms)) = (
        run.provider_binding_version,
        run.provider_binding.as_ref(),
        run.provider_bound_at_ms,
    ) else {
        if run.provider_binding_version.is_none()
            && run.provider_binding.is_none()
            && run.provider_bound_at_ms.is_none()
        {
            return Ok(BindingDisposition::Unbound);
        }
        return Err(corrupt_data(
            "assistant_runs",
            "provider_binding",
            "provider binding tuple is incomplete",
        ));
    };
    if version != PROVIDER_BINDING_VERSION {
        return Ok(BindingDisposition::Incompatible(
            SessionContinuationIncompatibility::ProviderBindingVersion,
        ));
    }
    if binding.as_slice().is_empty() || binding.as_slice().len() > 262_144 {
        return Err(corrupt_data(
            "assistant_runs",
            "provider_binding",
            "provider binding exceeds its persisted bound",
        ));
    }
    let parsed: ProviderBinding = serde_json::from_slice(binding.as_slice()).map_err(|_| {
        corrupt_data(
            "assistant_runs",
            "provider_binding",
            "provider binding is not a valid bounded object",
        )
    })?;
    if EngineId::parse(&parsed.engine).ok() != Some(expected_engine) {
        return Ok(BindingDisposition::Incompatible(
            SessionContinuationIncompatibility::ProviderBindingEngine,
        ));
    }
    let profile_text = parsed.profile_id;
    let profile = EngineProfileId::parse(profile_text.clone()).map_err(|_| {
        corrupt_data(
            "assistant_runs",
            "provider_binding",
            "provider binding profile id is invalid",
        )
    })?;
    if profile.as_str().is_empty() {
        return Err(corrupt_data(
            "assistant_runs",
            "provider_binding",
            "provider binding profile id is empty",
        ));
    }
    if profile_text != profile.as_str() {
        return Err(corrupt_data(
            "assistant_runs",
            "provider_binding",
            "provider binding profile id changed during validation",
        ));
    }
    let session_id = ProviderSessionId::parse(parsed.session_id)?;
    Ok(BindingDisposition::Usable {
        version,
        session_id,
        profile_id: profile,
    })
}

async fn read_durable_facts<C: ConnectionTrait>(
    database: &C,
    run: &entities::assistant_run::Model,
    run_id: &RunId,
) -> Result<
    (
        Option<SessionContinuationCheckpoint>,
        SessionContinuationSequence,
    ),
    RepositoryError,
> {
    let checkpoint = entities::run_checkpoint::Entity::find_by_id(run_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("read continuation checkpoint", source))?;
    let receipt = entities::run_batch_receipt::Entity::find()
        .filter(entities::run_batch_receipt::Column::RunId.eq(run_id.as_str()))
        .order_by_desc(entities::run_batch_receipt::Column::BatchSequence)
        .one(database)
        .await
        .map_err(|source| database_error("read continuation batch receipt", source))?;
    let state = entities::conversation_state::Entity::find_by_id(run.thread_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("read continuation thread sequence", source))?;

    if let Some(state) = &state
        && state.last_patch_sequence < 0
    {
        return Err(corrupt_data(
            "conversation_state",
            "last_patch_sequence",
            "conversation patch sequence is negative",
        ));
    }
    if let Some(receipt) = &receipt {
        if !receipt.committed {
            return Err(corrupt_data(
                "run_batch_receipts",
                "committed",
                "uncommitted receipt is not a durable sequence fact",
            ));
        }
        if receipt.generation != run.generation || receipt.batch_sequence <= 0 {
            return Err(corrupt_data(
                "run_batch_receipts",
                "generation",
                "receipt generation or sequence is incompatible with its run",
            ));
        }
    }

    let checkpoint_facts = match checkpoint {
        Some(row) => {
            if row.generation != run.generation
                || row.last_batch_sequence < 0
                || row.updated_at_ms < run.created_at_ms
                || row.updated_at_ms > run.updated_at_ms
            {
                return Err(corrupt_data(
                    "run_checkpoints",
                    "generation",
                    "checkpoint facts are outside the run fence",
                ));
            }
            let has_version = row.engine_checkpoint_version.is_some();
            let has_blob = row.engine_checkpoint_blob.is_some();
            if has_version != has_blob
                || row
                    .engine_checkpoint_version
                    .is_some_and(|version| version <= 0)
                || row.engine_checkpoint_blob.as_ref().is_some_and(|blob| {
                    blob.as_slice().is_empty() || blob.as_slice().len() > 262_144
                })
            {
                return Err(corrupt_data(
                    "run_checkpoints",
                    "engine_checkpoint_blob",
                    "checkpoint payload tuple is invalid",
                ));
            }
            if let Some(receipt) = &receipt {
                if row.last_batch_sequence != receipt.batch_sequence {
                    return Err(corrupt_data(
                        "run_checkpoints",
                        "last_batch_sequence",
                        "checkpoint and receipt sequences disagree",
                    ));
                }
            } else if row.last_batch_sequence != 0 {
                return Err(corrupt_data(
                    "run_checkpoints",
                    "last_batch_sequence",
                    "checkpoint has no corresponding batch receipt",
                ));
            }
            Some(SessionContinuationCheckpoint {
                generation: row.generation,
                last_batch_sequence: row.last_batch_sequence,
                engine_checkpoint_version: row.engine_checkpoint_version,
                has_engine_checkpoint: has_blob,
                updated_at: UnixMillis::from_millis(row.updated_at_ms),
            })
        }
        None => {
            if receipt.is_some() {
                return Err(corrupt_data(
                    "run_checkpoints",
                    "run_id",
                    "batch receipt exists without a checkpoint row",
                ));
            }
            None
        }
    };

    let last_batch_sequence = checkpoint_facts
        .as_ref()
        .map_or(0, |checkpoint| checkpoint.last_batch_sequence);
    Ok((
        checkpoint_facts,
        SessionContinuationSequence {
            last_batch_sequence,
            last_committed_batch_sequence: receipt.map(|row| row.batch_sequence),
            last_patch_sequence: state.map(|row| row.last_patch_sequence),
        },
    ))
}

fn parse_run_id(value: &str) -> Result<RunId, RepositoryError> {
    RunId::parse(value.to_owned()).map_err(|_| {
        corrupt_data(
            "assistant_runs",
            "run_id",
            "candidate run id is not a valid bounded identifier",
        )
    })
}
