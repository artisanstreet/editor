//! Private transaction-borrowing row, ordinal, revision, and patch effects.
//!
//! Every function here borrows the caller's open transaction. Loads feed the
//! validation in the parent module; [`persist_plan`] writes one complete
//! tentative plan — ordinal ledger slots, fresh item rows, checked
//! existing-item post-images, the optional turn activation, ordered patches,
//! the conversation-state counters, the run-checkpoint upsert, and the
//! committed receipt — leaving the single commit to the caller.

use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};

use crate::entities::{
    self, ConversationItemKind, ConversationPatchKind, EntityLifecycle, OpaqueBytes, OrdinalKind,
    RenderPhase,
};
use crate::repository::database_error;

use super::{CheckpointUpdate, RunObservationError};

/// Loaded per-thread conversation counters.
pub(super) struct LoadedState {
    pub next_renderer_ordinal: i64,
    pub last_patch_sequence: i64,
    pub updated_at_ms: i64,
}

/// Loaded origin-turn row.
pub(super) struct LoadedTurn {
    pub turn_id: String,
    pub thread_id: String,
    pub ordinal: i64,
    pub revision: i64,
    pub lifecycle: EntityLifecycle,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Complete post-image of one assistant item row to insert or update.
pub(super) struct ItemRow {
    pub item_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub ordinal: i64,
    pub revision: i64,
    pub lifecycle: EntityLifecycle,
    pub phase: RenderPhase,
    pub body: String,
    pub run_id: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Complete post-image of the activated origin turn.
pub(super) struct TurnRow {
    pub turn_id: String,
    pub thread_id: String,
    pub ordinal: i64,
    pub revision: i64,
    pub lifecycle: EntityLifecycle,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// One durable replay patch row to insert.
pub(super) struct PatchToInsert {
    pub patch_id: String,
    pub sequence: i64,
    pub kind: ConversationPatchKind,
    pub revision: i64,
    pub recorded_at_ms: i64,
    pub turn_id: Option<String>,
    pub item_id: Option<String>,
    pub ordinal: Option<i64>,
    pub lifecycle: Option<EntityLifecycle>,
    pub item_kind: Option<ConversationItemKind>,
    pub run_id: Option<String>,
    pub phase: Option<RenderPhase>,
    pub body: Option<String>,
    pub fragment: Option<String>,
    pub entity_created_at_ms: Option<i64>,
    pub entity_updated_at_ms: Option<i64>,
}

/// Advanced conversation-state counters to stamp.
pub(super) struct StateRow {
    pub next_renderer_ordinal: i64,
    pub last_patch_sequence: i64,
    pub updated_at_ms: i64,
}

/// Run-checkpoint upsert instruction with the previously loaded row.
pub(super) struct CheckpointRow {
    pub existing: Option<entities::RunCheckpoint>,
    pub run_id: String,
    pub generation: i64,
    pub last_batch_sequence: i64,
    pub updated_at_ms: i64,
}

/// The `committed = true` receipt row carrying the canonical digest.
pub(super) struct ReceiptRow {
    pub run_id: String,
    pub generation: i64,
    pub batch_sequence: i64,
    pub digest: [u8; 32],
}

/// Complete tentative effect plan of one validated batch.
pub(super) struct PersistencePlan {
    pub thread_id: String,
    pub fresh_ordinals: Vec<(i64, String)>,
    pub items_to_insert: Vec<ItemRow>,
    pub items_to_update: Vec<ItemRow>,
    pub turn_update: Option<TurnRow>,
    pub patches: Vec<PatchToInsert>,
    pub state: StateRow,
    pub checkpoint: CheckpointRow,
    pub receipt: ReceiptRow,
}

pub(super) async fn load_conversation_state(
    transaction: &sea_orm::DatabaseTransaction,
    thread_id: &str,
) -> Result<Option<LoadedState>, RunObservationError> {
    let row = entities::conversation_state::Entity::find_by_id(thread_id)
        .one(transaction)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("load batch conversation state", source))
        })?;
    Ok(row.map(|state| LoadedState {
        next_renderer_ordinal: state.next_renderer_ordinal,
        last_patch_sequence: state.last_patch_sequence,
        updated_at_ms: state.updated_at_ms,
    }))
}

pub(super) async fn load_run_checkpoint(
    transaction: &sea_orm::DatabaseTransaction,
    run_id: &str,
) -> Result<Option<entities::RunCheckpoint>, RunObservationError> {
    entities::run_checkpoint::Entity::find_by_id(run_id)
        .one(transaction)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("load run checkpoint", source))
        })
}

pub(super) async fn receipts_exist_for_run(
    transaction: &sea_orm::DatabaseTransaction,
    run_id: &str,
) -> Result<bool, RunObservationError> {
    let receipt = entities::run_batch_receipt::Entity::find()
        .filter(entities::run_batch_receipt::Column::RunId.eq(run_id))
        .one(transaction)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("probe run batch receipts", source))
        })?;
    Ok(receipt.is_some())
}

pub(super) async fn load_turn(
    transaction: &sea_orm::DatabaseTransaction,
    turn_id: &str,
) -> Result<Option<LoadedTurn>, RunObservationError> {
    let row = entities::conversation_turn::Entity::find_by_id(turn_id)
        .one(transaction)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("load batch origin turn", source))
        })?;
    Ok(row.map(|turn| LoadedTurn {
        turn_id: turn.turn_id,
        thread_id: turn.thread_id,
        ordinal: turn.ordinal,
        revision: turn.revision,
        lifecycle: turn.lifecycle,
        created_at_ms: turn.created_at_ms,
        updated_at_ms: turn.updated_at_ms,
    }))
}

pub(super) async fn load_item(
    transaction: &sea_orm::DatabaseTransaction,
    item_id: &str,
) -> Result<Option<entities::ConversationItem>, RunObservationError> {
    entities::conversation_item::Entity::find_by_id(item_id)
        .one(transaction)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("load batch target item", source))
        })
}

pub(super) async fn patch_exists(
    transaction: &sea_orm::DatabaseTransaction,
    patch_id: &str,
) -> Result<bool, RunObservationError> {
    let patch = entities::conversation_patch::Entity::find_by_id(patch_id)
        .one(transaction)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("probe batch patch identity", source))
        })?;
    Ok(patch.is_some())
}

pub(super) async fn ordinal_entity_exists(
    transaction: &sea_orm::DatabaseTransaction,
    entity_id: &str,
) -> Result<bool, RunObservationError> {
    let ordinal = entities::conversation_ordinal::Entity::find()
        .filter(entities::conversation_ordinal::Column::EntityId.eq(entity_id))
        .one(transaction)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("probe batch ordinal identity", source))
        })?;
    Ok(ordinal.is_some())
}

pub(super) const fn map_phase(phase: artisan_domain::AssistantMessagePhase) -> RenderPhase {
    match phase {
        artisan_domain::AssistantMessagePhase::Unspecified => RenderPhase::Unspecified,
        artisan_domain::AssistantMessagePhase::Commentary => RenderPhase::Commentary,
        artisan_domain::AssistantMessagePhase::Final => RenderPhase::Final,
    }
}

/// Writes every tentative effect of one validated plan through the borrowed
/// transaction; the caller commits once or rolls everything back.
pub(super) async fn persist_plan(
    transaction: &sea_orm::DatabaseTransaction,
    plan: PersistencePlan,
    checkpoint_update: CheckpointUpdate<'_>,
) -> Result<(), RunObservationError> {
    for (ordinal, entity_id) in plan.fresh_ordinals {
        entities::conversation_ordinal::Entity::insert(
            entities::conversation_ordinal::ActiveModel {
                thread_id: Set(plan.thread_id.clone()),
                ordinal: Set(ordinal),
                kind: Set(OrdinalKind::Item),
                entity_id: Set(entity_id),
            },
        )
        .exec(transaction)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("insert batch ordinal", source))
        })?;
    }
    for row in plan.items_to_insert {
        entities::conversation_item::Entity::insert(item_active_model(row))
            .exec(transaction)
            .await
            .map_err(|source| {
                RunObservationError::Repository(database_error(
                    "insert batch assistant item",
                    source,
                ))
            })?;
    }
    for row in plan.items_to_update {
        item_active_model(row)
            .update(transaction)
            .await
            .map_err(|source| {
                RunObservationError::Repository(database_error(
                    "update batch assistant item",
                    source,
                ))
            })?;
    }
    if let Some(turn) = plan.turn_update {
        entities::conversation_turn::ActiveModel {
            turn_id: Set(turn.turn_id),
            thread_id: Set(turn.thread_id),
            ordinal: Set(turn.ordinal),
            kind: Set(OrdinalKind::Turn),
            revision: Set(turn.revision),
            lifecycle: Set(turn.lifecycle),
            created_at_ms: Set(turn.created_at_ms),
            updated_at_ms: Set(turn.updated_at_ms),
        }
        .update(transaction)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("activate batch turn", source))
        })?;
    }
    for patch in plan.patches {
        insert_patch(transaction, &plan.thread_id, patch).await?;
    }
    entities::conversation_state::ActiveModel {
        thread_id: Set(plan.thread_id.clone()),
        next_renderer_ordinal: Set(plan.state.next_renderer_ordinal),
        last_patch_sequence: Set(plan.state.last_patch_sequence),
        updated_at_ms: Set(plan.state.updated_at_ms),
    }
    .update(transaction)
    .await
    .map_err(|source| {
        RunObservationError::Repository(database_error("advance batch conversation state", source))
    })?;
    upsert_checkpoint(transaction, plan.checkpoint, checkpoint_update).await?;
    entities::run_batch_receipt::Entity::insert(entities::run_batch_receipt::ActiveModel {
        run_id: Set(plan.receipt.run_id),
        batch_sequence: Set(plan.receipt.batch_sequence),
        generation: Set(plan.receipt.generation),
        digest: Set(OpaqueBytes::new(plan.receipt.digest.to_vec())),
        committed: Set(true),
    })
    .exec(transaction)
    .await
    .map_err(|source| {
        RunObservationError::Repository(database_error("insert batch receipt", source))
    })?;
    Ok(())
}

fn item_active_model(row: ItemRow) -> entities::conversation_item::ActiveModel {
    entities::conversation_item::ActiveModel {
        item_id: Set(row.item_id),
        thread_id: Set(row.thread_id),
        turn_id: Set(row.turn_id),
        ordinal: Set(row.ordinal),
        kind: Set(OrdinalKind::Item),
        revision: Set(row.revision),
        lifecycle: Set(row.lifecycle),
        item_kind: Set(ConversationItemKind::AssistantMessage),
        source_message_id: Set(None),
        run_id: Set(Some(row.run_id)),
        native_item_key: Set(None),
        phase: Set(Some(row.phase)),
        body: Set(row.body),
        created_at_ms: Set(row.created_at_ms),
        updated_at_ms: Set(row.updated_at_ms),
    }
}

async fn insert_patch(
    transaction: &sea_orm::DatabaseTransaction,
    thread_id: &str,
    patch: PatchToInsert,
) -> Result<(), RunObservationError> {
    entities::conversation_patch::Entity::insert(entities::conversation_patch::ActiveModel {
        patch_id: Set(patch.patch_id),
        thread_id: Set(thread_id.to_owned()),
        sequence: Set(patch.sequence),
        kind: Set(patch.kind),
        revision: Set(patch.revision),
        recorded_at_ms: Set(patch.recorded_at_ms),
        turn_id: Set(patch.turn_id),
        item_id: Set(patch.item_id),
        ordinal: Set(patch.ordinal),
        lifecycle: Set(patch.lifecycle),
        item_kind: Set(patch.item_kind),
        run_id: Set(patch.run_id),
        phase: Set(patch.phase),
        body: Set(patch.body),
        fragment: Set(patch.fragment),
        entity_created_at_ms: Set(patch.entity_created_at_ms),
        entity_updated_at_ms: Set(patch.entity_updated_at_ms),
    })
    .exec(transaction)
    .await
    .map_err(|source| {
        RunObservationError::Repository(database_error("insert batch patch", source))
    })?;
    Ok(())
}

async fn upsert_checkpoint(
    transaction: &sea_orm::DatabaseTransaction,
    checkpoint: CheckpointRow,
    update: CheckpointUpdate<'_>,
) -> Result<(), RunObservationError> {
    if let Some(existing) = checkpoint.existing {
        let mut active: entities::run_checkpoint::ActiveModel = existing.into();
        active.last_batch_sequence = Set(checkpoint.last_batch_sequence);
        active.updated_at_ms = Set(checkpoint.updated_at_ms);
        if let CheckpointUpdate::Replace(engine) = update {
            active.engine_checkpoint_version = Set(Some(engine.version()));
            active.engine_checkpoint_blob = Set(Some(OpaqueBytes::new(engine.as_slice().to_vec())));
        }
        active.update(transaction).await.map_err(|source| {
            RunObservationError::Repository(database_error("update run checkpoint", source))
        })?;
    } else {
        let (version, blob) = match update {
            CheckpointUpdate::Keep => (None, None),
            CheckpointUpdate::Replace(engine) => (
                Some(engine.version()),
                Some(OpaqueBytes::new(engine.as_slice().to_vec())),
            ),
        };
        entities::run_checkpoint::Entity::insert(entities::run_checkpoint::ActiveModel {
            run_id: Set(checkpoint.run_id),
            generation: Set(checkpoint.generation),
            last_batch_sequence: Set(checkpoint.last_batch_sequence),
            engine_checkpoint_version: Set(version),
            engine_checkpoint_blob: Set(blob),
            updated_at_ms: Set(checkpoint.updated_at_ms),
        })
        .exec(transaction)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("insert run checkpoint", source))
        })?;
    }
    Ok(())
}
