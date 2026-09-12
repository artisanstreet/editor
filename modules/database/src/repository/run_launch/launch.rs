//! Atomic claimed-dispatch launch transaction and durable graph construction.
//!
//! Owns [`LaunchClaimedRun`] and the fenced `launch_claimed_run` transaction:
//! one dispatch transition plus the conversation-state counters, shared
//! ordinals, initial pending turn, completed user item, two replay patches,
//! and the `launching` assistant-run row committed together.

use artisan_domain::{
    AuthoredText, ConversationCursor, ItemId, MessageId, PatchId, QueueMessagePayload, RequestId,
    RunId, ThreadId, TurnId, UnixMillis,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait,
    QueryFilter, Statement,
};

use crate::entities;
use crate::entities::{
    AssistantRunLifecycle, ConversationItemKind, ConversationPatchKind, EntityLifecycle,
    OpaqueBytes, OrdinalKind,
};

use crate::repository::queue_message::read_image_attachments;
use crate::repository::thread_engine_config;
use crate::repository::{
    ClaimedMessageDispatch, Repository, RepositoryError, ThreadEngineSettings, corrupt_data,
    database_error, millis,
};

use super::replay::classify_unfenced_launch;
use super::{
    INITIAL_RUN_GENERATION, InitialPatchProjection, LaunchedProjections, RunLaunchCredentials,
    RunLaunchError, RunStartKey, counter_overflow, insert_initial_patch, insert_item_projection,
    insert_ordinal, obtain_conversation_state,
};

/// Shared renderer ordinals consumed by one launch (one turn, one item).
const LAUNCH_ORDINALS: i64 = 2;

/// Durable replay patches written by one launch.
const LAUNCH_PATCHES: i64 = 2;

const LAUNCH_FENCE_SQL: &str = r"
UPDATE message_dispatches
SET state = 'running',
    updated_at_ms = ?
WHERE message_id = ?
  AND correlation_id = ?
  AND attempt_count = ?
  AND queued_at_ms = ?
  AND available_at_ms = ?
  AND state = 'leased'
  AND lease_owner = ?
  AND lease_expires_at_ms = ?
  AND updated_at_ms = ?
  AND updated_at_ms <= ?
  AND lease_expires_at_ms > ?
RETURNING message_id
";

/// Borrowed inputs of one atomic claimed-dispatch launch.
///
/// Nothing here is cloned away from the caller: an unknown transaction
/// outcome can be retried verbatim with the same values.
pub struct LaunchClaimedRun<'a> {
    /// The exact live claim returned by the atomic dispatch claim.
    pub claimed: &'a ClaimedMessageDispatch,
    /// Caller-minted identity of the assistant run to launch.
    pub run_id: &'a RunId,
    /// Caller-minted identity of the launched turn.
    pub turn_id: &'a TurnId,
    /// Caller-minted identity of the completed user item.
    pub item_id: &'a ItemId,
    /// Caller-minted identity of the initial `turn_upsert` patch.
    pub first_patch_id: &'a PatchId,
    /// Caller-minted identity of the initial `item_upsert` patch.
    pub second_patch_id: &'a PatchId,
    /// Caller-owned monotonic operation time stamped on every effect.
    pub operated_at: UnixMillis,
    /// Exact 32-byte deduplication key for this launch.
    pub run_start_key: &'a RunStartKey,
    /// Named owner/lease/claim capabilities persisted as raw BLOBs.
    pub credentials: &'a RunLaunchCredentials,
    /// Exact immutable engine settings read for this launch.
    pub engine_settings: &'a ThreadEngineSettings,
}

/// Payload-free durable receipt of one launch attempt.
///
/// It carries identities and counters only — never the accepted body, never
/// a capability, and never any provider authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchedRunReceipt {
    /// Launched run identity.
    pub run_id: RunId,
    /// Thread owning the launched conversation graph.
    pub thread_id: ThreadId,
    /// Accepted message the run originates from.
    pub message_id: MessageId,
    /// Launched turn identity.
    pub turn_id: TurnId,
    /// Completed user item identity.
    pub item_id: ItemId,
    /// Generation recorded on the launching run.
    pub generation: i64,
    /// Patch cursor produced by the launch's own sequence allocation.
    pub resulting_cursor: ConversationCursor,
}

/// Typed outcome of one launch call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchClaimedRunOutcome {
    /// This transaction created the graph and moved the dispatch to running.
    Started(LaunchedRunReceipt),
    /// An earlier identical transaction already launched exactly this run;
    /// durable receipt information only, never provider authority.
    AlreadyStarted(LaunchedRunReceipt),
}

impl Repository {
    /// Launches one claimed dispatch into its durable conversation graph.
    ///
    /// The transaction's first statement is one conditional UPDATE fencing
    /// the exact claimed snapshot — message id, correlation id, attempt
    /// count, queued/available times, hex-encoded owner, exact lease expiry,
    /// exact claimed update stamp, `updated_at <= operated_at`, and
    /// `lease_expiry > operated_at` — from `leased` to `running`. RUNNING
    /// keeps its owner and expiry; only the update stamp moves to
    /// `operated_at`. When the fence matches no row, the same transaction
    /// diagnoses an exact replay against the originally persisted graph,
    /// requiring the identical run identity, start key, origin tuple,
    /// projections, patch identities, launch-time stamps, still-launching
    /// lifecycle, generation 1, and all three matching tokens on a
    /// still-RUNNING dispatch with the matching claim identity, and answers
    /// [`LaunchClaimedRunOutcome::AlreadyStarted`]. Every other divergence is
    /// a typed conflict, and any failure rolls back the fenced write together
    /// with all graph effects.
    ///
    /// The accepted message's stored thread and body are loaded and used
    /// verbatim; chronology is checked against the message acceptance, thread
    /// creation, and existing conversation-state stamp. Counters advance by
    /// checked arithmetic only. This operation claims nothing about provider
    /// acceptance or UI visibility, and it offers no failure, requeue, or
    /// recovery path: unknown external outcomes stay a later explicit design.
    ///
    /// # Errors
    ///
    /// Returns [`RunLaunchError::Repository`] for existing typed repository
    /// rejections (unknown dispatch, wrong owner, expired lease, chronology,
    /// corrupt data), [`RunLaunchError::SnapshotMismatch`] for stale claim
    /// snapshots, [`RunLaunchError::CredentialMismatch`] for mismatching
    /// start keys or tokens, [`RunLaunchError::IdentityConflict`] for
    /// colliding durable identities, [`RunLaunchError::RunNotFound`] and
    /// [`RunLaunchError::RunNotLaunchable`] for unknown or already-advanced
    /// runs, and [`RunLaunchError::CounterOverflow`] when either shared
    /// counter lacks remaining range.
    pub async fn launch_claimed_run(
        &self,
        command: LaunchClaimedRun<'_>,
    ) -> Result<LaunchClaimedRunOutcome, RunLaunchError> {
        let operated_at_ms = millis(command.operated_at);
        let claimed = command.claimed;

        if claimed.attempt_count == 0 || i32::try_from(claimed.attempt_count).is_err() {
            return Err(RunLaunchError::SnapshotMismatch {
                message_id: claimed.message_id.clone(),
            });
        }
        if operated_at_ms < millis(claimed.updated_at) {
            return Err(RunLaunchError::Repository(
                RepositoryError::InvalidChronology {
                    earlier_field: "claimed dispatch updated_at",
                    later_field: "launch operated_at",
                },
            ));
        }
        // Same-call identity collisions are invisible to per-identity
        // vacancy queries: two individually vacant identities can still
        // collide with each other inside one launch. They are rejected
        // before any statement runs.
        if command.turn_id.as_str() == command.item_id.as_str() {
            return Err(RunLaunchError::IdentityConflict {
                reason: "turn and item identities collide within one launch",
            });
        }
        if command.first_patch_id.as_str() == command.second_patch_id.as_str() {
            return Err(RunLaunchError::IdentityConflict {
                reason: "initial patch identities collide within one launch",
            });
        }

        let encoded_owner = claimed.owner.to_storage();
        let transaction = self.begin_write().await.map_err(|source| {
            RunLaunchError::Repository(database_error("begin run launch", source))
        })?;

        let statement = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            LAUNCH_FENCE_SQL,
            [
                operated_at_ms.into(),
                claimed.message_id.as_str().into(),
                claimed.correlation_id.as_str().into(),
                i64::from(claimed.attempt_count).into(),
                millis(claimed.queued_at).into(),
                millis(claimed.available_at).into(),
                encoded_owner.into(),
                millis(claimed.lease_expires_at).into(),
                millis(claimed.updated_at).into(),
                operated_at_ms.into(),
                operated_at_ms.into(),
            ],
        );
        let fenced = transaction
            .query_one_raw(statement)
            .await
            .map_err(|source| {
                RunLaunchError::Repository(database_error("fence run launch", source))
            });
        let fenced = match fenced {
            Ok(fenced) => fenced,
            Err(error) => return rollback_launch(transaction, error).await,
        };

        if fenced.is_none() {
            let diagnosed = classify_unfenced_launch(&transaction, &command, operated_at_ms).await;
            return finish_diagnosed_launch(transaction, diagnosed).await;
        }

        let launched = build_launched_graph(&transaction, &command, operated_at_ms).await;
        match launched {
            Ok(receipt) => {
                transaction.commit().await.map_err(|source| {
                    RunLaunchError::Repository(database_error("commit run launch", source))
                })?;
                Ok(LaunchClaimedRunOutcome::Started(receipt))
            }
            Err(error) => rollback_launch(transaction, error).await,
        }
    }
}

/// The accepted message's validated thread and body, loaded verbatim.
struct AcceptedMessageContext {
    thread_id: ThreadId,
    payload: QueueMessagePayload,
}

/// Loads and revalidates the immutable accepted message for one launch.
///
/// Chronology against the message acceptance is checked here so no state is
/// mutated before the operation time is provably ordered.
async fn load_accepted_message(
    transaction: &sea_orm::DatabaseTransaction,
    message_id: &MessageId,
    operated_at_ms: i64,
) -> Result<AcceptedMessageContext, RunLaunchError> {
    let message = entities::message::Entity::find_by_id(message_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            RunLaunchError::Repository(database_error("load launched message", source))
        })?
        .ok_or(RunLaunchError::Repository(RepositoryError::Invariant {
            reason: "fenced launch lost its accepted message",
        }))?;
    if message.message_id != message_id.as_str() {
        return Err(RunLaunchError::Repository(RepositoryError::Invariant {
            reason: "fenced launch loaded a different message",
        }));
    }
    if operated_at_ms < message.accepted_at_ms {
        return Err(RunLaunchError::Repository(
            RepositoryError::InvalidChronology {
                earlier_field: "messages.accepted_at_ms",
                later_field: "launch operated_at",
            },
        ));
    }
    let thread_id = ThreadId::parse(message.thread_id.clone()).map_err(|error| {
        RunLaunchError::Repository(corrupt_data("messages", "thread_id", error))
    })?;
    let text =
        if message.body.is_empty() {
            None
        } else {
            Some(AuthoredText::parse(message.body.clone()).map_err(|error| {
                RunLaunchError::Repository(corrupt_data("messages", "body", error))
            })?)
        };
    let attachments = read_image_attachments(transaction, message_id)
        .await
        .map_err(RunLaunchError::Repository)?;
    let payload = QueueMessagePayload::new(text, attachments)
        .map_err(|error| RunLaunchError::Repository(corrupt_data("messages", "body", &error)))?;
    Ok(AcceptedMessageContext { thread_id, payload })
}

/// Confirms the owning thread exists and predates the launch operation,
/// then matches the supplied settings against the accepted receipt
/// snapshot for this message.
///
/// The snapshot recorded at accept is authoritative: a selection change
/// between accept and launch must NOT fail the launch, and the launch
/// runs the captured configuration. Rows accepted before snapshots
/// existed keep the legacy fence against current thread settings.
/// Thread existence and chronology always apply; run replay snapshot
/// validation elsewhere is untouched.
async fn ensure_thread_admits_launch(
    transaction: &sea_orm::DatabaseTransaction,
    thread_id: &ThreadId,
    message_id: &MessageId,
    correlation_id: &RequestId,
    operated_at_ms: i64,
    engine_settings: &ThreadEngineSettings,
) -> Result<(), RunLaunchError> {
    let thread = entities::thread::Entity::find_by_id(thread_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            RunLaunchError::Repository(database_error("load launched thread", source))
        })?
        .ok_or(RunLaunchError::Repository(
            RepositoryError::ThreadNotFound {
                thread_id: thread_id.clone(),
            },
        ))?;
    if operated_at_ms < thread.created_at_ms {
        return Err(RunLaunchError::Repository(
            RepositoryError::InvalidChronology {
                earlier_field: "threads.created_at_ms",
                later_field: "launch operated_at",
            },
        ));
    }
    if let Some(snapshot) =
        thread_engine_config::read_receipt_settings_in(transaction, correlation_id).await?
    {
        if snapshot != *engine_settings {
            return Err(RunLaunchError::SnapshotMismatch {
                message_id: message_id.clone(),
            });
        }
    } else {
        let actual = thread_engine_config::settings_from_thread(thread)?;
        if actual.as_ref() != Some(engine_settings) {
            return Err(RunLaunchError::SnapshotMismatch {
                message_id: message_id.clone(),
            });
        }
    }
    Ok(())
}

async fn build_launched_graph(
    transaction: &sea_orm::DatabaseTransaction,
    command: &LaunchClaimedRun<'_>,
    operated_at_ms: i64,
) -> Result<LaunchedRunReceipt, RunLaunchError> {
    let claimed = command.claimed;

    let AcceptedMessageContext { thread_id, payload } =
        load_accepted_message(transaction, &claimed.message_id.clone(), operated_at_ms).await?;
    ensure_thread_admits_launch(
        transaction,
        &thread_id,
        &claimed.message_id,
        &claimed.correlation_id,
        operated_at_ms,
        command.engine_settings,
    )
    .await?;

    let (next_renderer_ordinal, last_patch_sequence) =
        obtain_conversation_state(transaction, thread_id.as_str(), operated_at_ms).await?;

    let turn_ordinal = next_renderer_ordinal;
    let item_ordinal = turn_ordinal
        .checked_add(1)
        .ok_or_else(|| counter_overflow("renderer ordinal", turn_ordinal))?;
    let final_renderer_ordinal = turn_ordinal
        .checked_add(LAUNCH_ORDINALS)
        .ok_or_else(|| counter_overflow("renderer ordinal", turn_ordinal))?;
    let first_patch_sequence = last_patch_sequence
        .checked_add(1)
        .ok_or_else(|| counter_overflow("patch sequence", last_patch_sequence))?;
    let second_patch_sequence = first_patch_sequence
        .checked_add(1)
        .ok_or_else(|| counter_overflow("patch sequence", first_patch_sequence))?;
    let final_patch_sequence = last_patch_sequence
        .checked_add(LAUNCH_PATCHES)
        .ok_or_else(|| counter_overflow("patch sequence", last_patch_sequence))?;

    ensure_launch_identity_vacancy(transaction, command).await?;

    insert_ordinal(
        transaction,
        thread_id.as_str(),
        turn_ordinal,
        OrdinalKind::Turn,
        command.turn_id.as_str(),
    )
    .await?;
    insert_ordinal(
        transaction,
        thread_id.as_str(),
        item_ordinal,
        OrdinalKind::Item,
        command.item_id.as_str(),
    )
    .await?;

    let body = payload.text().map_or("", |text| text.as_str());
    let projections = LaunchedProjections {
        thread_id: thread_id.as_str(),
        turn_id: command.turn_id.as_str(),
        item_id: command.item_id.as_str(),
        message_id: claimed.message_id.as_str(),
        body,
        turn_ordinal,
        item_ordinal,
    };
    insert_turn_projection(transaction, &projections, operated_at_ms).await?;
    insert_item_projection(transaction, &projections, operated_at_ms).await?;
    insert_launched_run(transaction, command, thread_id.as_str(), operated_at_ms).await?;
    insert_initial_launch_patches(
        transaction,
        command,
        &projections,
        first_patch_sequence,
        second_patch_sequence,
        operated_at_ms,
    )
    .await?;

    entities::conversation_state::ActiveModel {
        thread_id: Set(thread_id.as_str().to_owned()),
        next_renderer_ordinal: Set(final_renderer_ordinal),
        last_patch_sequence: Set(final_patch_sequence),
        updated_at_ms: Set(operated_at_ms),
    }
    .update(transaction)
    .await
    .map_err(|source| {
        RunLaunchError::Repository(database_error("advance conversation counters", source))
    })?;

    let cursor_value = u64::try_from(second_patch_sequence)
        .map_err(|_| counter_overflow("patch sequence", second_patch_sequence))?;

    Ok(LaunchedRunReceipt {
        run_id: command.run_id.clone(),
        thread_id,
        message_id: claimed.message_id.clone(),
        turn_id: command.turn_id.clone(),
        item_id: command.item_id.clone(),
        generation: INITIAL_RUN_GENERATION,
        resulting_cursor: ConversationCursor::new(cursor_value),
    })
}

async fn insert_turn_projection(
    transaction: &sea_orm::DatabaseTransaction,
    projections: &LaunchedProjections<'_>,
    operated_at_ms: i64,
) -> Result<(), RunLaunchError> {
    entities::conversation_turn::Entity::insert(entities::conversation_turn::ActiveModel {
        turn_id: Set(projections.turn_id.to_owned()),
        thread_id: Set(projections.thread_id.to_owned()),
        ordinal: Set(projections.turn_ordinal),
        kind: Set(OrdinalKind::Turn),
        revision: Set(0),
        lifecycle: Set(EntityLifecycle::Pending),
        created_at_ms: Set(operated_at_ms),
        updated_at_ms: Set(operated_at_ms),
    })
    .exec(transaction)
    .await
    .map_err(|source| RunLaunchError::Repository(database_error("insert launched turn", source)))?;
    Ok(())
}

async fn insert_launched_run(
    transaction: &sea_orm::DatabaseTransaction,
    command: &LaunchClaimedRun<'_>,
    thread_id: &str,
    operated_at_ms: i64,
) -> Result<(), RunLaunchError> {
    let claimed = command.claimed;
    let (run_owner, run_lease, run_claim) = command.credentials.parts();
    entities::assistant_run::Entity::insert(entities::assistant_run::ActiveModel {
        run_id: Set(command.run_id.as_str().to_owned()),
        thread_id: Set(thread_id.to_owned()),
        run_start_key: Set(OpaqueBytes::new(command.run_start_key.expose().to_vec())),
        origin_message_id: Set(claimed.message_id.as_str().to_owned()),
        origin_turn_id: Set(command.turn_id.as_str().to_owned()),
        lifecycle: Set(AssistantRunLifecycle::Launching),
        generation: Set(INITIAL_RUN_GENERATION),
        owner: Set(Some(OpaqueBytes::new(run_owner.expose().to_vec()))),
        lease: Set(Some(OpaqueBytes::new(run_lease.expose().to_vec()))),
        claim_token: Set(Some(OpaqueBytes::new(run_claim.expose().to_vec()))),
        provider_binding_version: Set(None),
        provider_binding: Set(None),
        provider_bound_at_ms: Set(None),
        error_code: Set(None),
        error_message: Set(None),
        created_at_ms: Set(operated_at_ms),
        updated_at_ms: Set(operated_at_ms),
        terminal_at_ms: Set(None),
        engine_run_config_version: Set(Some(i64::from(
            command.engine_settings.config().storage_codec_version(),
        ))),
        engine_run_config_revision: Set(Some(command.engine_settings.revision().as_i64())),
        engine_run_config: Set(Some(OpaqueBytes::new(
            crate::engine_run_config::encode(command.engine_settings.config()).map_err(
                |error| {
                    RunLaunchError::Repository(corrupt_data(
                        "assistant_runs",
                        "engine_run_config",
                        &error,
                    ))
                },
            )?,
        ))),
    })
    .exec(transaction)
    .await
    .map_err(|source| RunLaunchError::Repository(database_error("insert launching run", source)))?;
    Ok(())
}

async fn insert_initial_launch_patches(
    transaction: &sea_orm::DatabaseTransaction,
    command: &LaunchClaimedRun<'_>,
    projections: &LaunchedProjections<'_>,
    first_patch_sequence: i64,
    second_patch_sequence: i64,
    operated_at_ms: i64,
) -> Result<(), RunLaunchError> {
    insert_initial_patch(
        transaction,
        projections.thread_id,
        first_patch_sequence,
        ConversationPatchKind::TurnUpsert,
        operated_at_ms,
        InitialPatchProjection {
            patch_id: command.first_patch_id,
            turn_id: projections.turn_id,
            item_id: None,
            ordinal: projections.turn_ordinal,
            lifecycle: EntityLifecycle::Pending,
            item_kind: None,
            body: None,
        },
    )
    .await?;
    insert_initial_patch(
        transaction,
        projections.thread_id,
        second_patch_sequence,
        ConversationPatchKind::ItemUpsert,
        operated_at_ms,
        InitialPatchProjection {
            patch_id: command.second_patch_id,
            turn_id: projections.turn_id,
            item_id: Some(projections.item_id),
            ordinal: projections.item_ordinal,
            lifecycle: EntityLifecycle::Completed,
            item_kind: Some(ConversationItemKind::UserMessage),
            body: Some(projections.body.to_owned()),
        },
    )
    .await?;
    Ok(())
}

/// Rejects fresh launches whose identities already exist durably.
///
/// The writer position is already held, so these lookups see a stable view:
/// any hit means the supplied identities contradict a genuinely fresh
/// launch and the whole transaction rolls back afterwards. Every uniqueness
/// surface this operation can touch is probed explicitly — run identity,
/// start key, both origins, user-item source message, turn and item
/// identities, both patch identities, and the globally unique ordinal
/// entity ids — so no database error string ever needs interpreting.
async fn ensure_launch_identity_vacancy(
    transaction: &sea_orm::DatabaseTransaction,
    command: &LaunchClaimedRun<'_>,
) -> Result<(), RunLaunchError> {
    let claimed_message = command.claimed.message_id.as_str().to_owned();
    let run_start_key = command.run_start_key.expose().to_vec();

    macro_rules! reject_if_present {
        ($reason:expr, $query:expr) => {
            let existing = $query.await.map_err(|source| {
                RunLaunchError::Repository(database_error("check launch identity vacancy", source))
            })?;
            if existing.is_some() {
                return Err(RunLaunchError::IdentityConflict { reason: $reason });
            }
        };
    }

    reject_if_present!(
        "origin message already owns a run",
        entities::assistant_run::Entity::find()
            .filter(entities::assistant_run::Column::OriginMessageId.eq(claimed_message.clone()))
            .one(transaction)
    );
    reject_if_present!(
        "run identity already exists",
        entities::assistant_run::Entity::find_by_id(command.run_id.as_str()).one(transaction)
    );
    reject_if_present!(
        "run start key already exists",
        entities::assistant_run::Entity::find()
            .filter(entities::assistant_run::Column::RunStartKey.eq(run_start_key))
            .one(transaction)
    );
    reject_if_present!(
        "origin turn already owns a run",
        entities::assistant_run::Entity::find()
            .filter(entities::assistant_run::Column::OriginTurnId.eq(command.turn_id.as_str()))
            .one(transaction)
    );
    reject_if_present!(
        "turn identity already exists",
        entities::conversation_turn::Entity::find_by_id(command.turn_id.as_str()).one(transaction)
    );
    reject_if_present!(
        "user item identity already exists",
        entities::conversation_item::Entity::find_by_id(command.item_id.as_str()).one(transaction)
    );
    reject_if_present!(
        "accepted message already has a conversation item",
        entities::conversation_item::Entity::find()
            .filter(
                entities::conversation_item::Column::SourceMessageId.eq(claimed_message.clone()),
            )
            .one(transaction)
    );
    reject_if_present!(
        "turn ordinal entity already exists",
        entities::conversation_ordinal::Entity::find()
            .filter(entities::conversation_ordinal::Column::EntityId.eq(command.turn_id.as_str()),)
            .one(transaction)
    );
    reject_if_present!(
        "item ordinal entity already exists",
        entities::conversation_ordinal::Entity::find()
            .filter(entities::conversation_ordinal::Column::EntityId.eq(command.item_id.as_str()),)
            .one(transaction)
    );
    reject_if_present!(
        "first patch identity already exists",
        entities::conversation_patch::Entity::find_by_id(command.first_patch_id.as_str())
            .one(transaction)
    );
    reject_if_present!(
        "second patch identity already exists",
        entities::conversation_patch::Entity::find_by_id(command.second_patch_id.as_str())
            .one(transaction)
    );

    Ok(())
}

async fn rollback_launch<T>(
    transaction: sea_orm::DatabaseTransaction,
    error: RunLaunchError,
) -> Result<T, RunLaunchError> {
    transaction.rollback().await.map_err(|source| {
        RunLaunchError::Repository(database_error("roll back run launch", source))
    })?;
    Err(error)
}

/// Rolls back the read-only diagnosis and returns its outcome unchanged.
async fn finish_diagnosed_launch(
    transaction: sea_orm::DatabaseTransaction,
    diagnosed: Result<LaunchClaimedRunOutcome, RunLaunchError>,
) -> Result<LaunchClaimedRunOutcome, RunLaunchError> {
    transaction.rollback().await.map_err(|source| {
        RunLaunchError::Repository(database_error("roll back run launch diagnosis", source))
    })?;
    diagnosed
}
