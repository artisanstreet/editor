//! Launch replay classification and receipt reconstruction.
//!
//! Owns the unfenced-launch diagnosis, exact-replay validation of the
//! persisted graph, engine snapshot, ordinals, and patches, and the durable
//! receipt rebuilt from the originally persisted rows.

use artisan_domain::{AuthoredText, ConversationCursor, MessageId, QueueMessagePayload, ThreadId};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::entities;
use crate::entities::{
    AssistantRunLifecycle, ConversationItemKind, ConversationPatchKind, DispatchState,
    EntityLifecycle, OrdinalKind,
};

use crate::repository::message_dispatch::{ClaimedMessageDispatch, DispatchLeaseOwner};
use crate::repository::queue_message::read_image_attachments;
use crate::repository::{
    RepositoryError, ThreadEngineSettings, corrupt_data, database_error, millis,
};

use super::{
    INITIAL_RUN_GENERATION, LaunchClaimedRun, LaunchClaimedRunOutcome, LaunchedRunReceipt,
    RunLaunchError, counter_overflow, stored_bytes_match,
};

/// Diagnoses a launch whose fence matched no row inside its transaction.
///
/// An exact replay of the original launch answers `AlreadyStarted` derived
/// from the persisted rows; every other divergence is typed without writing
/// anything.
pub(super) async fn classify_unfenced_launch(
    transaction: &sea_orm::DatabaseTransaction,
    command: &LaunchClaimedRun<'_>,
    operated_at_ms: i64,
) -> Result<LaunchClaimedRunOutcome, RunLaunchError> {
    let claimed = command.claimed;

    let dispatch = entities::message_dispatch::Entity::find_by_id(claimed.message_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            RunLaunchError::Repository(database_error("classify unfenced launch", source))
        })?
        .ok_or(RunLaunchError::Repository(
            RepositoryError::DispatchNotFound {
                message_id: claimed.message_id.clone(),
            },
        ))?;

    match dispatch.state {
        DispatchState::Running => {
            classify_running_replay(transaction, command, &dispatch, operated_at_ms)
                .await
                .map(LaunchClaimedRunOutcome::AlreadyStarted)
        }
        DispatchState::Leased => {
            if dispatch.updated_at_ms > operated_at_ms {
                return Err(RunLaunchError::Repository(
                    RepositoryError::InvalidChronology {
                        earlier_field: "message_dispatches.updated_at_ms",
                        later_field: "launch operated_at",
                    },
                ));
            }
            let persisted_expiry = dispatch.lease_expires_at_ms.ok_or_else(|| {
                RunLaunchError::Repository(corrupt_data(
                    "message_dispatches",
                    "lease_expires_at_ms",
                    "required value is null",
                ))
            })?;
            if persisted_expiry <= operated_at_ms {
                return Err(RunLaunchError::Repository(
                    RepositoryError::DispatchLeaseExpired {
                        message_id: claimed.message_id.clone(),
                        lease_expires_at_ms: persisted_expiry,
                        operated_at_ms,
                    },
                ));
            }
            let persisted_owner = dispatch
                .lease_owner
                .as_deref()
                .ok_or_else(|| {
                    RunLaunchError::Repository(corrupt_data(
                        "message_dispatches",
                        "lease_owner",
                        "required value is null",
                    ))
                })?
                .to_owned();
            let persisted_owner =
                DispatchLeaseOwner::from_storage(&persisted_owner).map_err(|error| {
                    RunLaunchError::Repository(corrupt_data(
                        "message_dispatches",
                        "lease_owner",
                        error,
                    ))
                })?;
            if !persisted_owner.constant_time_eq(&claimed.owner) {
                return Err(RunLaunchError::Repository(
                    RepositoryError::DispatchOwnerMismatch {
                        message_id: claimed.message_id.clone(),
                    },
                ));
            }
            Err(RunLaunchError::SnapshotMismatch {
                message_id: claimed.message_id.clone(),
            })
        }
        other => Err(RunLaunchError::Repository(
            RepositoryError::InvalidDispatchState {
                message_id: claimed.message_id.clone(),
                state: dispatch_state_label(&other),
            },
        )),
    }
}

/// Validates one exact replay against the persisted launching graph.
///
/// Every originally persisted projection value — including timestamps,
/// revisions, lifecycles, bodies, NULL columns, the shared ordinal ledger,
/// and ordinal adjacency — must equal the launch's recorded shape, and the
/// receipt's cursor derives from the ORIGINAL second patch sequence rather
/// than any current counter.
/// Fences an exact replay's dispatch row against the claimed snapshot.
fn validate_running_dispatch_snapshot(
    dispatch: &entities::MessageDispatch,
    claimed: &ClaimedMessageDispatch,
    operated_at_ms: i64,
) -> Result<(), RunLaunchError> {
    if dispatch.correlation_id != claimed.correlation_id.as_str()
        || i64::from(dispatch.attempt_count) != i64::from(claimed.attempt_count)
        || dispatch.queued_at_ms != millis(claimed.queued_at)
        || dispatch.available_at_ms != millis(claimed.available_at)
        || dispatch.lease_expires_at_ms != Some(millis(claimed.lease_expires_at))
        || dispatch.updated_at_ms != operated_at_ms
    {
        return Err(RunLaunchError::SnapshotMismatch {
            message_id: claimed.message_id.clone(),
        });
    }
    let persisted_owner = dispatch
        .lease_owner
        .as_deref()
        .ok_or_else(|| {
            RunLaunchError::Repository(corrupt_data(
                "message_dispatches",
                "lease_owner",
                "required value is null",
            ))
        })?
        .to_owned();
    let persisted_owner = DispatchLeaseOwner::from_storage(&persisted_owner).map_err(|error| {
        RunLaunchError::Repository(corrupt_data("message_dispatches", "lease_owner", error))
    })?;
    if !persisted_owner.constant_time_eq(&claimed.owner) {
        return Err(RunLaunchError::Repository(
            RepositoryError::DispatchOwnerMismatch {
                message_id: claimed.message_id.clone(),
            },
        ));
    }
    Ok(())
}

/// Loads the replayed run and validates its launch-state fences.
///
/// A missing run identity whose origin message already owns another run is
/// a typed identity conflict, never a fresh or missing run.
async fn load_validated_replayed_run(
    transaction: &sea_orm::DatabaseTransaction,
    command: &LaunchClaimedRun<'_>,
    operated_at_ms: i64,
) -> Result<entities::AssistantRun, RunLaunchError> {
    let claimed = command.claimed;

    let found = entities::assistant_run::Entity::find_by_id(command.run_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            RunLaunchError::Repository(database_error("load replayed run", source))
        })?;
    let Some(run) = found else {
        let origin_conflict = entities::assistant_run::Entity::find()
            .filter(
                entities::assistant_run::Column::OriginMessageId.eq(claimed.message_id.as_str()),
            )
            .one(transaction)
            .await
            .map_err(|source| {
                RunLaunchError::Repository(database_error("check replayed run origin", source))
            })?;
        return if origin_conflict.is_some() {
            Err(RunLaunchError::IdentityConflict {
                reason: "origin message already owns a different run identity",
            })
        } else {
            Err(RunLaunchError::RunNotFound {
                run_id: command.run_id.clone(),
            })
        };
    };

    if run.origin_message_id != claimed.message_id.as_str()
        || run.origin_turn_id != command.turn_id.as_str()
    {
        return Err(RunLaunchError::IdentityConflict {
            reason: "stored run originates from another message or turn",
        });
    }
    if run.lifecycle != AssistantRunLifecycle::Launching {
        return Err(RunLaunchError::RunNotLaunchable {
            run_id: command.run_id.clone(),
        });
    }
    if run.generation != INITIAL_RUN_GENERATION
        || run.created_at_ms != operated_at_ms
        || run.updated_at_ms != operated_at_ms
        || run.provider_binding_version.is_some()
        || run.provider_binding.is_some()
        || run.provider_bound_at_ms.is_some()
        || !stored_bytes_match(&run.run_start_key, command.run_start_key.expose())
        || !command.credentials.matches_stored(
            run.owner.as_ref(),
            run.lease.as_ref(),
            run.claim_token.as_ref(),
        )
    {
        return Err(RunLaunchError::CredentialMismatch {
            run_id: command.run_id.clone(),
        });
    }
    validate_engine_snapshot(&run, command.engine_settings, &claimed.message_id)?;
    Ok(run)
}

fn validate_engine_snapshot(
    run: &entities::AssistantRun,
    settings: &ThreadEngineSettings,
    message_id: &MessageId,
) -> Result<(), RunLaunchError> {
    if run.engine_run_config_version != Some(i64::from(settings.config().storage_codec_version())) {
        return Err(RunLaunchError::Repository(corrupt_data(
            "assistant_runs",
            "engine_run_config_version",
            "configured run snapshot must use its codec version",
        )));
    }
    let revision = run
        .engine_run_config_revision
        .and_then(|value| u64::try_from(value).ok())
        .and_then(|value| artisan_domain::EngineConfigRevision::new(value).ok())
        .ok_or_else(|| {
            RunLaunchError::Repository(corrupt_data(
                "assistant_runs",
                "engine_run_config_revision",
                "run snapshot revision is outside its domain range",
            ))
        })?;
    let blob = run.engine_run_config.as_ref().ok_or_else(|| {
        RunLaunchError::Repository(corrupt_data(
            "assistant_runs",
            "engine_run_config",
            "required run snapshot is null",
        ))
    })?;
    let stored = crate::engine_run_config::decode(blob.as_slice()).map_err(|error| {
        RunLaunchError::Repository(corrupt_data("assistant_runs", "engine_run_config", &error))
    })?;
    let canonical = crate::engine_run_config::encode(settings.config()).map_err(|error| {
        RunLaunchError::Repository(corrupt_data("assistant_runs", "engine_run_config", &error))
    })?;
    if revision != settings.revision()
        || stored != *settings.config()
        || blob.as_slice() != canonical.as_slice()
    {
        return Err(RunLaunchError::SnapshotMismatch {
            message_id: message_id.clone(),
        });
    }
    Ok(())
}

/// Validates the replayed turn/item projections and their ordinal ledger.
async fn validate_replayed_turn_item(
    transaction: &sea_orm::DatabaseTransaction,
    command: &LaunchClaimedRun<'_>,
    run_thread_id: &str,
    persisted_body: &str,
    operated_at_ms: i64,
) -> Result<(entities::ConversationTurn, entities::ConversationItem), RunLaunchError> {
    let claimed = command.claimed;

    let turn = entities::conversation_turn::Entity::find_by_id(command.turn_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| RunLaunchError::Repository(database_error("load replayed turn", source)))?
        .ok_or(RunLaunchError::IdentityConflict {
            reason: "launched turn projection is missing",
        })?;
    let item = entities::conversation_item::Entity::find_by_id(command.item_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| RunLaunchError::Repository(database_error("load replayed item", source)))?
        .ok_or(RunLaunchError::IdentityConflict {
            reason: "launched user item projection is missing",
        })?;

    if turn.thread_id != run_thread_id
        || item.thread_id != run_thread_id
        || item.turn_id != turn.turn_id
        || turn.ordinal.checked_add(1) != Some(item.ordinal)
        || turn.kind != OrdinalKind::Turn
        || turn.revision != 0
        || turn.lifecycle != EntityLifecycle::Pending
        || turn.created_at_ms != operated_at_ms
        || turn.updated_at_ms != operated_at_ms
        || item.kind != OrdinalKind::Item
        || item.revision != 0
        || item.lifecycle != EntityLifecycle::Completed
        || item.item_kind != ConversationItemKind::UserMessage
        || item.source_message_id.as_deref() != Some(claimed.message_id.as_str())
        || item.run_id.is_some()
        || item.native_item_key.is_some()
        || item.phase.is_some()
        || item.body != persisted_body
        || item.created_at_ms != operated_at_ms
        || item.updated_at_ms != operated_at_ms
    {
        return Err(RunLaunchError::IdentityConflict {
            reason: "stored turn and user item projections contradict the launch",
        });
    }

    let turn_ledger =
        entities::conversation_ordinal::Entity::find_by_id((turn.thread_id.clone(), turn.ordinal))
            .one(transaction)
            .await
            .map_err(|source| {
                RunLaunchError::Repository(database_error("load replayed turn ordinal", source))
            })?
            .ok_or(RunLaunchError::IdentityConflict {
                reason: "turn ordinal ledger entry is missing",
            })?;
    let item_ledger =
        entities::conversation_ordinal::Entity::find_by_id((item.thread_id.clone(), item.ordinal))
            .one(transaction)
            .await
            .map_err(|source| {
                RunLaunchError::Repository(database_error("load replayed item ordinal", source))
            })?
            .ok_or(RunLaunchError::IdentityConflict {
                reason: "item ordinal ledger entry is missing",
            })?;
    if turn_ledger.entity_id != turn.turn_id
        || turn_ledger.kind != OrdinalKind::Turn
        || item_ledger.entity_id != item.item_id
        || item_ledger.kind != OrdinalKind::Item
        || turn_ledger.ordinal.checked_add(1) != Some(item_ledger.ordinal)
    {
        return Err(RunLaunchError::IdentityConflict {
            reason: "shared ordinal ledger contradicts the launch",
        });
    }

    Ok((turn, item))
}

/// Validates the two initial replay patches against their recorded shape.
///
/// Returns the ORIGINAL second patch sequence, from which the replay
/// receipt's cursor is derived.
async fn validate_replayed_initial_patches(
    transaction: &sea_orm::DatabaseTransaction,
    command: &LaunchClaimedRun<'_>,
    turn: &entities::ConversationTurn,
    item: &entities::ConversationItem,
    persisted_body: &str,
    operated_at_ms: i64,
) -> Result<i64, RunLaunchError> {
    let first_patch =
        entities::conversation_patch::Entity::find_by_id(command.first_patch_id.as_str())
            .one(transaction)
            .await
            .map_err(|source| {
                RunLaunchError::Repository(database_error("load replayed first patch", source))
            })?
            .ok_or(RunLaunchError::IdentityConflict {
                reason: "initial turn-upsert patch is missing",
            })?;
    let second_patch =
        entities::conversation_patch::Entity::find_by_id(command.second_patch_id.as_str())
            .one(transaction)
            .await
            .map_err(|source| {
                RunLaunchError::Repository(database_error("load replayed second patch", source))
            })?
            .ok_or(RunLaunchError::IdentityConflict {
                reason: "initial item-upsert patch is missing",
            })?;

    if first_patch.thread_id != turn.thread_id
        || first_patch.kind != ConversationPatchKind::TurnUpsert
        || first_patch.turn_id.as_deref() != Some(turn.turn_id.as_str())
        || first_patch.item_id.is_some()
        || first_patch.ordinal != Some(turn.ordinal)
        || first_patch.lifecycle != Some(EntityLifecycle::Pending)
        || first_patch.item_kind.is_some()
        || first_patch.run_id.is_some()
        || first_patch.phase.is_some()
        || first_patch.body.is_some()
        || first_patch.fragment.is_some()
        || first_patch.revision != 0
        || first_patch.recorded_at_ms != operated_at_ms
        || first_patch.entity_created_at_ms != Some(operated_at_ms)
        || first_patch.entity_updated_at_ms != Some(operated_at_ms)
        || second_patch.thread_id != turn.thread_id
        || second_patch.kind != ConversationPatchKind::ItemUpsert
        || second_patch.turn_id.as_deref() != Some(turn.turn_id.as_str())
        || second_patch.item_id.as_deref() != Some(item.item_id.as_str())
        || second_patch.ordinal != Some(item.ordinal)
        || second_patch.lifecycle != Some(EntityLifecycle::Completed)
        || second_patch.item_kind != Some(ConversationItemKind::UserMessage)
        || second_patch.run_id.is_some()
        || second_patch.phase.is_some()
        || second_patch.fragment.is_some()
        || second_patch.body.as_deref() != Some(persisted_body)
        || second_patch.revision != 0
        || second_patch.recorded_at_ms != operated_at_ms
        || second_patch.entity_created_at_ms != Some(operated_at_ms)
        || second_patch.entity_updated_at_ms != Some(operated_at_ms)
        || first_patch.sequence.checked_add(1) != Some(second_patch.sequence)
    {
        return Err(RunLaunchError::IdentityConflict {
            reason: "stored initial patches contradict the launch",
        });
    }
    Ok(second_patch.sequence)
}

/// Validates one exact replay against the persisted launching graph.
///
/// Every originally persisted projection value — including timestamps,
/// revisions, lifecycles, bodies, NULL columns, the shared ordinal ledger,
/// and ordinal adjacency — must equal the launch's recorded shape, and the
/// receipt's cursor derives from the ORIGINAL second patch sequence rather
/// than any current counter.
async fn classify_running_replay(
    transaction: &sea_orm::DatabaseTransaction,
    command: &LaunchClaimedRun<'_>,
    dispatch: &entities::MessageDispatch,
    operated_at_ms: i64,
) -> Result<LaunchedRunReceipt, RunLaunchError> {
    validate_running_dispatch_snapshot(dispatch, command.claimed, operated_at_ms)?;
    let run = load_validated_replayed_run(transaction, command, operated_at_ms).await?;
    let persisted_body = message_body_of_replay(transaction, command.claimed).await?;
    let (turn, item) = validate_replayed_turn_item(
        transaction,
        command,
        run.thread_id.as_str(),
        persisted_body.as_str(),
        operated_at_ms,
    )
    .await?;

    let second_sequence = validate_replayed_initial_patches(
        transaction,
        command,
        &turn,
        &item,
        persisted_body.as_str(),
        operated_at_ms,
    )
    .await?;

    let cursor_value = u64::try_from(second_sequence)
        .map_err(|_| counter_overflow("patch sequence", second_sequence))?;

    Ok(LaunchedRunReceipt {
        run_id: command.run_id.clone(),
        thread_id: ThreadId::parse(run.thread_id.clone()).map_err(|error| {
            RunLaunchError::Repository(corrupt_data("assistant_runs", "thread_id", error))
        })?,
        message_id: command.claimed.message_id.clone(),
        turn_id: command.turn_id.clone(),
        item_id: command.item_id.clone(),
        generation: INITIAL_RUN_GENERATION,
        resulting_cursor: ConversationCursor::new(cursor_value),
    })
}

async fn message_body_of_replay(
    transaction: &sea_orm::DatabaseTransaction,
    claimed: &ClaimedMessageDispatch,
) -> Result<String, RunLaunchError> {
    let message = entities::message::Entity::find_by_id(claimed.message_id.as_str())
        .one(transaction)
        .await
        .map_err(|source| {
            RunLaunchError::Repository(database_error("load replayed message", source))
        })?
        .ok_or(RunLaunchError::Repository(RepositoryError::Invariant {
            reason: "replayed launch lost its accepted message",
        }))?;
    let text =
        if message.body.is_empty() {
            None
        } else {
            Some(AuthoredText::parse(message.body.clone()).map_err(|error| {
                RunLaunchError::Repository(corrupt_data("messages", "body", error))
            })?)
        };
    let attachments = read_image_attachments(transaction, &claimed.message_id)
        .await
        .map_err(RunLaunchError::Repository)?;
    QueueMessagePayload::new(text, attachments)
        .map_err(|error| RunLaunchError::Repository(corrupt_data("messages", "body", &error)))?;
    Ok(message.body)
}

fn dispatch_state_label(state: &DispatchState) -> &'static str {
    match state {
        DispatchState::Queued => "queued",
        DispatchState::Leased => "leased",
        DispatchState::Running => "running",
        DispatchState::Completed => "completed",
        DispatchState::Failed => "failed",
    }
}
