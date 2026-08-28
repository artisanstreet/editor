//! Adds the conversation execution ledger: shared renderer ordinal slots,
//! turn and item projections, assistant run authority, replay patches,
//! engine checkpoints, and per-run batch receipts.
//!
//! The database enforces column shapes, enumerations, byte bounds,
//! uniqueness, the same-thread foreign-key graph, and the exact per-row
//! state tuples below. Everything else remains a future repository
//! invariant and is never implied by this schema:
//!
//! - Atomic allocation of state counters, ordinal slots, and patch
//!   sequences happens in repository transactions.
//! - Patch contiguity up to `conversation_state.last_patch_sequence` is a
//!   repository obligation; the database rejects duplicates only.
//! - Which run-to-turn origins are permitted (not merely well-formed) is a
//!   repository decision layered above these foreign keys.
//! - Lifecycle graph edges and terminal first-writer CAS semantics live in
//!   the repository; the database validates individual row shapes.
//! - The generation plus state plus owner fence is applied by writers.
//! - Exact batch sequence and digest classification is a repository duty;
//!   the database enforces per-run receipt identity only.
//! - Provider binding is written only after the consumer is ready; the
//!   database records the resulting tuple verbatim.
//! - A completed run requires a co-committed final assistant item in the
//!   completed lifecycle; that pairing is a repository transaction.
//! - Cancel confirmation is observed by the repository, not derived here.
//!
//! At batch sequence zero the checkpoint tuple may legitimately be absent;
//! later sequences classify it in the repository. Engine binding and
//! checkpoint payloads are intentionally opaque to this schema.

use sea_orm_migration::prelude::*;

const ENTITY_LIFECYCLES: &str =
    "('pending','streaming','active','waiting','completed','failed','interrupted','cancelled')";
const RUN_LIFECYCLE_STATES: &str = "('queued','launching','running','waiting',\
     'cancel_requested','interrupted','completed','failed','cancelled')";
const ITEM_KINDS: &str = "('user_message','assistant_message')";
const RENDER_PHASES: &str = "('commentary','final','unspecified')";
const PATCH_KINDS: &str =
    "('turn_upsert','item_upsert','item_append','item_lifecycle','turn_lifecycle')";

/// Creates the conversation execution objects after the receipts migration.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Parent identity for same-thread composite references to messages.
        manager
            .create_index(
                Index::create()
                    .name("uq_messages_message_id_thread_id")
                    .table(Messages::Table)
                    .col(Messages::MessageId)
                    .col(Messages::ThreadId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        create_conversation_state(manager).await?;
        create_conversation_ordinals(manager).await?;
        create_conversation_turns(manager).await?;
        create_assistant_runs(manager).await?;
        create_conversation_items(manager).await?;
        create_conversation_patches(manager).await?;
        create_run_checkpoints(manager).await?;
        create_run_batch_receipts(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RunBatchReceipts::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(RunCheckpoints::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationPatches::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationItems::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(AssistantRuns::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationTurns::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationOrdinals::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationState::Table).to_owned())
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("uq_messages_message_id_thread_id")
                    .to_owned(),
            )
            .await
    }
}

async fn create_conversation_state(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(ConversationState::Table)
                .col(
                    ColumnDef::new(ConversationState::ThreadId)
                        .string()
                        .not_null()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(ConversationState::NextRendererOrdinal)
                        .big_integer()
                        .not_null()
                        .default(0),
                )
                .col(
                    ColumnDef::new(ConversationState::LastPatchSequence)
                        .big_integer()
                        .not_null()
                        .default(0),
                )
                .col(
                    ColumnDef::new(ConversationState::UpdatedAtMs)
                        .big_integer()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_conversation_state_thread_id_threads")
                        .from(ConversationState::Table, ConversationState::ThreadId)
                        .to(Threads::Table, Threads::ThreadId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .check((
                    "ck_conversation_state_next_renderer_ordinal_nonnegative",
                    Expr::col(ConversationState::NextRendererOrdinal).gte(0),
                ))
                .check((
                    "ck_conversation_state_last_patch_sequence_nonnegative",
                    Expr::col(ConversationState::LastPatchSequence).gte(0),
                ))
                .to_owned(),
        )
        .await
}

async fn create_conversation_ordinals(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(ConversationOrdinals::Table)
                .col(
                    ColumnDef::new(ConversationOrdinals::ThreadId)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ConversationOrdinals::Ordinal)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ConversationOrdinals::Kind)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ConversationOrdinals::EntityId)
                        .string()
                        .not_null(),
                )
                .primary_key(
                    Index::create()
                        .name("pk_conversation_ordinals")
                        .col(ConversationOrdinals::ThreadId)
                        .col(ConversationOrdinals::Ordinal),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_conversation_ordinals_thread_id_threads")
                        .from(ConversationOrdinals::Table, ConversationOrdinals::ThreadId)
                        .to(Threads::Table, Threads::ThreadId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .check((
                    "ck_conversation_ordinals_ordinal_nonnegative",
                    Expr::col(ConversationOrdinals::Ordinal).gte(0),
                ))
                .check((
                    "ck_conversation_ordinals_kind",
                    Expr::cust("kind IN ('turn', 'item')"),
                ))
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("uq_conversation_ordinals_entity_id")
                .table(ConversationOrdinals::Table)
                .col(ConversationOrdinals::EntityId)
                .unique()
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("uq_conversation_ordinals_thread_id_ordinal_entity_id_kind")
                .table(ConversationOrdinals::Table)
                .col(ConversationOrdinals::ThreadId)
                .col(ConversationOrdinals::Ordinal)
                .col(ConversationOrdinals::EntityId)
                .col(ConversationOrdinals::Kind)
                .unique()
                .to_owned(),
        )
        .await
}

async fn create_conversation_turns(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(ConversationTurns::Table)
                .col(
                    ColumnDef::new(ConversationTurns::TurnId)
                        .string()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(ConversationTurns::ThreadId).string().not_null())
                .col(ColumnDef::new(ConversationTurns::Ordinal).big_integer().not_null())
                .col(ColumnDef::new(ConversationTurns::Kind).string().not_null())
                .col(ColumnDef::new(ConversationTurns::Revision).big_integer().not_null())
                .col(ColumnDef::new(ConversationTurns::Lifecycle).string().not_null())
                .col(ColumnDef::new(ConversationTurns::CreatedAtMs).big_integer().not_null())
                .col(ColumnDef::new(ConversationTurns::UpdatedAtMs).big_integer().not_null())
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_conversation_turns_thread_id_ordinal_turn_id_kind_conversation_ordinals")
                        .from_tbl(ConversationTurns::Table)
                        .from_col(ConversationTurns::ThreadId)
                        .from_col(ConversationTurns::Ordinal)
                        .from_col(ConversationTurns::TurnId)
                        .from_col(ConversationTurns::Kind)
                        .to_tbl(ConversationOrdinals::Table)
                        .to_col(ConversationOrdinals::ThreadId)
                        .to_col(ConversationOrdinals::Ordinal)
                        .to_col(ConversationOrdinals::EntityId)
                        .to_col(ConversationOrdinals::Kind)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .check((
                    "ck_conversation_turns_kind_fixed",
                    Expr::cust("kind = 'turn'"),
                ))
                .check((
                    "ck_conversation_turns_revision_nonnegative",
                    Expr::col(ConversationTurns::Revision).gte(0),
                ))
                .check((
                    "ck_conversation_turns_lifecycle",
                    Expr::cust(format!("lifecycle IN {ENTITY_LIFECYCLES}")),
                ))
                .check((
                    "ck_conversation_turns_updated_not_before_created",
                    Expr::col(ConversationTurns::UpdatedAtMs)
                        .gte(Expr::col(ConversationTurns::CreatedAtMs)),
                ))
                .to_owned(),
        )
        .await?;

    // Parent identity for same-thread composite references from items,
    // patches, and runs.
    manager
        .create_index(
            Index::create()
                .name("uq_conversation_turns_turn_id_thread_id")
                .table(ConversationTurns::Table)
                .col(ConversationTurns::TurnId)
                .col(ConversationTurns::ThreadId)
                .unique()
                .to_owned(),
        )
        .await
}

#[allow(
    clippy::too_many_lines,
    reason = "keeping each table definition contiguous makes its constraints auditable"
)]
async fn create_assistant_runs(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(AssistantRuns::Table)
                .col(
                    ColumnDef::new(AssistantRuns::RunId)
                        .string()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(AssistantRuns::ThreadId).string().not_null())
                .col(
                    ColumnDef::new(AssistantRuns::RunStartKey)
                        .binary()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(AssistantRuns::OriginMessageId)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(AssistantRuns::OriginTurnId)
                        .string()
                        .not_null(),
                )
                .col(ColumnDef::new(AssistantRuns::Lifecycle).string().not_null())
                .col(
                    ColumnDef::new(AssistantRuns::Generation)
                        .big_integer()
                        .not_null()
                        .default(0),
                )
                .col(ColumnDef::new(AssistantRuns::Owner).binary().null())
                .col(ColumnDef::new(AssistantRuns::Lease).binary().null())
                .col(ColumnDef::new(AssistantRuns::ClaimToken).binary().null())
                .col(
                    ColumnDef::new(AssistantRuns::ProviderBindingVersion)
                        .big_integer()
                        .null(),
                )
                .col(
                    ColumnDef::new(AssistantRuns::ProviderBinding)
                        .binary()
                        .null(),
                )
                .col(
                    ColumnDef::new(AssistantRuns::ProviderBoundAtMs)
                        .big_integer()
                        .null(),
                )
                .col(ColumnDef::new(AssistantRuns::ErrorCode).string().null())
                .col(ColumnDef::new(AssistantRuns::ErrorMessage).text().null())
                .col(
                    ColumnDef::new(AssistantRuns::CreatedAtMs)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(AssistantRuns::UpdatedAtMs)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(AssistantRuns::TerminalAtMs)
                        .big_integer()
                        .null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_assistant_runs_origin_message_id_thread_id_messages")
                        .from_tbl(AssistantRuns::Table)
                        .from_col(AssistantRuns::OriginMessageId)
                        .from_col(AssistantRuns::ThreadId)
                        .to_tbl(Messages::Table)
                        .to_col(Messages::MessageId)
                        .to_col(Messages::ThreadId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_assistant_runs_origin_turn_id_thread_id_conversation_turns")
                        .from_tbl(AssistantRuns::Table)
                        .from_col(AssistantRuns::OriginTurnId)
                        .from_col(AssistantRuns::ThreadId)
                        .to_tbl(ConversationTurns::Table)
                        .to_col(ConversationTurns::TurnId)
                        .to_col(ConversationTurns::ThreadId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .check((
                    "ck_assistant_runs_generation_nonnegative",
                    Expr::col(AssistantRuns::Generation).gte(0),
                ))
                .check((
                    "ck_assistant_runs_lifecycle",
                    Expr::cust(format!("lifecycle IN {RUN_LIFECYCLE_STATES}")),
                ))
                .check((
                    "ck_assistant_runs_run_start_key_exact_bytes",
                    Expr::cust("length(run_start_key) = 32"),
                ))
                .check((
                    "ck_assistant_runs_fence_keys_exact_bytes",
                    Expr::cust(
                        "(owner IS NULL OR length(owner) = 32) \
                         AND (lease IS NULL OR length(lease) = 32) \
                         AND (claim_token IS NULL OR length(claim_token) = 32)",
                    ),
                ))
                .check((
                    "ck_assistant_runs_provider_binding_tuple",
                    Expr::cust(
                        "(provider_binding_version IS NULL AND provider_binding IS NULL \
                         AND provider_bound_at_ms IS NULL) \
                         OR (provider_binding_version IS NOT NULL AND provider_binding_version > 0 \
                         AND provider_binding IS NOT NULL \
                         AND length(provider_binding) BETWEEN 1 AND 262144 \
                         AND provider_bound_at_ms IS NOT NULL)",
                    ),
                ))
                .check((
                    "ck_assistant_runs_error_pair_shape",
                    Expr::cust(
                        "(error_code IS NULL AND error_message IS NULL) \
                         OR (error_code IS NOT NULL \
                         AND length(cast(error_code AS BLOB)) BETWEEN 1 AND 128 \
                         AND error_message IS NOT NULL \
                         AND length(cast(error_message AS BLOB)) BETWEEN 1 AND 1024)",
                    ),
                ))
                .check((
                    "ck_assistant_runs_error_lifecycle_pairing",
                    Expr::cust(
                        "(lifecycle IN ('interrupted', 'failed') AND error_code IS NOT NULL) \
                         OR (lifecycle NOT IN ('interrupted', 'failed') AND error_code IS NULL)",
                    ),
                ))
                .check((
                    "ck_assistant_runs_terminal_at",
                    Expr::cust(
                        "(lifecycle IN ('completed', 'failed', 'cancelled') \
                         AND terminal_at_ms IS NOT NULL \
                         AND terminal_at_ms >= created_at_ms \
                         AND terminal_at_ms <= updated_at_ms) \
                         OR (lifecycle NOT IN ('completed', 'failed', 'cancelled') \
                         AND terminal_at_ms IS NULL)",
                    ),
                ))
                .check((
                    "ck_assistant_runs_state_shape",
                    Expr::cust(
                        "(lifecycle = 'queued' AND generation = 0 AND owner IS NULL \
                         AND lease IS NULL AND claim_token IS NULL \
                         AND provider_binding_version IS NULL AND provider_binding IS NULL \
                         AND provider_bound_at_ms IS NULL) \
                         OR (lifecycle = 'launching' AND generation > 0 \
                         AND owner IS NOT NULL AND lease IS NOT NULL \
                         AND claim_token IS NOT NULL AND provider_binding_version IS NULL \
                         AND provider_binding IS NULL AND provider_bound_at_ms IS NULL) \
                         OR (lifecycle IN ('running', 'waiting', 'cancel_requested') \
                         AND generation > 0 AND owner IS NOT NULL \
                         AND lease IS NOT NULL AND claim_token IS NULL \
                         AND provider_binding_version IS NOT NULL \
                         AND provider_binding IS NOT NULL AND provider_bound_at_ms IS NOT NULL) \
                         OR (lifecycle IN ('interrupted', 'completed', 'failed', 'cancelled') \
                         AND owner IS NULL AND lease IS NULL \
                         AND claim_token IS NULL)",
                    ),
                ))
                .check((
                    "ck_assistant_runs_updated_not_before_created",
                    Expr::col(AssistantRuns::UpdatedAtMs)
                        .gte(Expr::col(AssistantRuns::CreatedAtMs)),
                ))
                .to_owned(),
        )
        .await?;

    for (name, column) in [
        (
            "uq_assistant_runs_run_start_key",
            AssistantRuns::RunStartKey,
        ),
        (
            "uq_assistant_runs_origin_message_id",
            AssistantRuns::OriginMessageId,
        ),
        (
            "uq_assistant_runs_origin_turn_id",
            AssistantRuns::OriginTurnId,
        ),
    ] {
        manager
            .create_index(
                Index::create()
                    .name(name)
                    .table(AssistantRuns::Table)
                    .col(column)
                    .unique()
                    .to_owned(),
            )
            .await?;
    }

    // Parent identity for same-thread composite references from items,
    // patches, and checkpoints.
    manager
        .create_index(
            Index::create()
                .name("uq_assistant_runs_run_id_thread_id")
                .table(AssistantRuns::Table)
                .col(AssistantRuns::RunId)
                .col(AssistantRuns::ThreadId)
                .unique()
                .to_owned(),
        )
        .await
}

#[allow(
    clippy::too_many_lines,
    reason = "keeping each table definition contiguous makes its constraints auditable"
)]
async fn create_conversation_items(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(ConversationItems::Table)
                .col(
                    ColumnDef::new(ConversationItems::ItemId)
                        .string()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(ConversationItems::ThreadId).string().not_null())
                .col(ColumnDef::new(ConversationItems::TurnId).string().not_null())
                .col(ColumnDef::new(ConversationItems::Ordinal).big_integer().not_null())
                .col(ColumnDef::new(ConversationItems::Kind).string().not_null())
                .col(ColumnDef::new(ConversationItems::Revision).big_integer().not_null())
                .col(ColumnDef::new(ConversationItems::Lifecycle).string().not_null())
                .col(ColumnDef::new(ConversationItems::ItemKind).string().not_null())
                .col(
                    ColumnDef::new(ConversationItems::SourceMessageId)
                        .string()
                        .null(),
                )
                .col(ColumnDef::new(ConversationItems::RunId).string().null())
                .col(
                    ColumnDef::new(ConversationItems::NativeItemKey)
                        .text()
                        .null(),
                )
                .col(ColumnDef::new(ConversationItems::Phase).string().null())
                .col(ColumnDef::new(ConversationItems::Body).text().not_null())
                .col(ColumnDef::new(ConversationItems::CreatedAtMs).big_integer().not_null())
                .col(ColumnDef::new(ConversationItems::UpdatedAtMs).big_integer().not_null())
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_conversation_items_thread_id_ordinal_item_id_kind_conversation_ordinals")
                        .from_tbl(ConversationItems::Table)
                        .from_col(ConversationItems::ThreadId)
                        .from_col(ConversationItems::Ordinal)
                        .from_col(ConversationItems::ItemId)
                        .from_col(ConversationItems::Kind)
                        .to_tbl(ConversationOrdinals::Table)
                        .to_col(ConversationOrdinals::ThreadId)
                        .to_col(ConversationOrdinals::Ordinal)
                        .to_col(ConversationOrdinals::EntityId)
                        .to_col(ConversationOrdinals::Kind)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_conversation_items_turn_id_thread_id_conversation_turns")
                        .from_tbl(ConversationItems::Table)
                        .from_col(ConversationItems::TurnId)
                        .from_col(ConversationItems::ThreadId)
                        .to_tbl(ConversationTurns::Table)
                        .to_col(ConversationTurns::TurnId)
                        .to_col(ConversationTurns::ThreadId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_conversation_items_source_message_id_thread_id_messages")
                        .from_tbl(ConversationItems::Table)
                        .from_col(ConversationItems::SourceMessageId)
                        .from_col(ConversationItems::ThreadId)
                        .to_tbl(Messages::Table)
                        .to_col(Messages::MessageId)
                        .to_col(Messages::ThreadId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_conversation_items_run_id_thread_id_assistant_runs")
                        .from_tbl(ConversationItems::Table)
                        .from_col(ConversationItems::RunId)
                        .from_col(ConversationItems::ThreadId)
                        .to_tbl(AssistantRuns::Table)
                        .to_col(AssistantRuns::RunId)
                        .to_col(AssistantRuns::ThreadId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .check((
                    "ck_conversation_items_kind_fixed",
                    Expr::cust("kind = 'item'"),
                ))
                .check((
                    "ck_conversation_items_revision_nonnegative",
                    Expr::col(ConversationItems::Revision).gte(0),
                ))
                .check((
                    "ck_conversation_items_lifecycle",
                    Expr::cust(format!("lifecycle IN {ENTITY_LIFECYCLES}")),
                ))
                .check((
                    "ck_conversation_items_item_kind",
                    Expr::cust(format!("item_kind IN {ITEM_KINDS}")),
                ))
                .check((
                    "ck_conversation_items_phase",
                    Expr::cust(format!(
                        "phase IS NULL OR phase IN {RENDER_PHASES}"
                    )),
                ))
                .check((
                    "ck_conversation_items_body_bytes",
                    Expr::cust("length(cast(body AS BLOB)) <= 65536"),
                ))
                .check((
                    "ck_conversation_items_shape",
                    Expr::cust(
                        "(item_kind = 'user_message' AND source_message_id IS NOT NULL \
                         AND run_id IS NULL AND native_item_key IS NULL AND phase IS NULL) \
                         OR (item_kind = 'assistant_message' AND source_message_id IS NULL \
                         AND run_id IS NOT NULL AND phase IS NOT NULL \
                         AND (native_item_key IS NULL OR length(native_item_key) > 0))",
                    ),
                ))
                .check((
                    "ck_conversation_items_updated_not_before_created",
                    Expr::col(ConversationItems::UpdatedAtMs)
                        .gte(Expr::col(ConversationItems::CreatedAtMs)),
                ))
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("uq_conversation_items_source_message_id")
                .table(ConversationItems::Table)
                .col(ConversationItems::SourceMessageId)
                .unique()
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("uq_conversation_items_run_id_native_item_key")
                .table(ConversationItems::Table)
                .col(ConversationItems::RunId)
                .col(ConversationItems::NativeItemKey)
                .unique()
                .to_owned(),
        )
        .await?;

    // Parent identity for same-thread composite references from patches.
    manager
        .create_index(
            Index::create()
                .name("uq_conversation_items_item_id_thread_id")
                .table(ConversationItems::Table)
                .col(ConversationItems::ItemId)
                .col(ConversationItems::ThreadId)
                .unique()
                .to_owned(),
        )
        .await
}

#[allow(
    clippy::too_many_lines,
    reason = "keeping each table definition contiguous makes its constraints auditable"
)]
async fn create_conversation_patches(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(ConversationPatches::Table)
                .col(
                    ColumnDef::new(ConversationPatches::PatchId)
                        .string()
                        .not_null()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(ConversationPatches::ThreadId)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ConversationPatches::Sequence)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ConversationPatches::Kind)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ConversationPatches::Revision)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ConversationPatches::RecordedAtMs)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(ConversationPatches::TurnId).string().null())
                .col(ColumnDef::new(ConversationPatches::ItemId).string().null())
                .col(
                    ColumnDef::new(ConversationPatches::Ordinal)
                        .big_integer()
                        .null(),
                )
                .col(
                    ColumnDef::new(ConversationPatches::Lifecycle)
                        .string()
                        .null(),
                )
                .col(
                    ColumnDef::new(ConversationPatches::ItemKind)
                        .string()
                        .null(),
                )
                .col(ColumnDef::new(ConversationPatches::RunId).string().null())
                .col(ColumnDef::new(ConversationPatches::Phase).string().null())
                .col(ColumnDef::new(ConversationPatches::Body).text().null())
                .col(ColumnDef::new(ConversationPatches::Fragment).text().null())
                .col(
                    ColumnDef::new(ConversationPatches::EntityCreatedAtMs)
                        .big_integer()
                        .null(),
                )
                .col(
                    ColumnDef::new(ConversationPatches::EntityUpdatedAtMs)
                        .big_integer()
                        .null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_conversation_patches_turn_id_thread_id_conversation_turns")
                        .from_tbl(ConversationPatches::Table)
                        .from_col(ConversationPatches::TurnId)
                        .from_col(ConversationPatches::ThreadId)
                        .to_tbl(ConversationTurns::Table)
                        .to_col(ConversationTurns::TurnId)
                        .to_col(ConversationTurns::ThreadId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_conversation_patches_item_id_thread_id_conversation_items")
                        .from_tbl(ConversationPatches::Table)
                        .from_col(ConversationPatches::ItemId)
                        .from_col(ConversationPatches::ThreadId)
                        .to_tbl(ConversationItems::Table)
                        .to_col(ConversationItems::ItemId)
                        .to_col(ConversationItems::ThreadId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_conversation_patches_run_id_thread_id_assistant_runs")
                        .from_tbl(ConversationPatches::Table)
                        .from_col(ConversationPatches::RunId)
                        .from_col(ConversationPatches::ThreadId)
                        .to_tbl(AssistantRuns::Table)
                        .to_col(AssistantRuns::RunId)
                        .to_col(AssistantRuns::ThreadId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .check((
                    "ck_conversation_patches_sequence_positive",
                    Expr::col(ConversationPatches::Sequence).gt(0),
                ))
                .check((
                    "ck_conversation_patches_revision_nonnegative",
                    Expr::col(ConversationPatches::Revision).gte(0),
                ))
                .check((
                    "ck_conversation_patches_kind",
                    Expr::cust(format!("kind IN {PATCH_KINDS}")),
                ))
                .check((
                    "ck_conversation_patches_phase_enum",
                    Expr::cust(format!("phase IS NULL OR phase IN {RENDER_PHASES}")),
                ))
                .check((
                    "ck_conversation_patches_ordinal_nonnegative",
                    Expr::cust("ordinal IS NULL OR ordinal >= 0"),
                ))
                .check((
                    "ck_conversation_patches_lifecycle_enum",
                    Expr::cust(format!(
                        "lifecycle IS NULL OR lifecycle IN {ENTITY_LIFECYCLES}"
                    )),
                ))
                .check((
                    "ck_conversation_patches_item_kind_enum",
                    Expr::cust(format!("item_kind IS NULL OR item_kind IN {ITEM_KINDS}")),
                ))
                .check((
                    "ck_conversation_patches_body_bytes",
                    Expr::cust("body IS NULL OR length(cast(body AS BLOB)) <= 65536"),
                ))
                .check((
                    "ck_conversation_patches_fragment_bytes",
                    Expr::cust("fragment IS NULL OR length(cast(fragment AS BLOB)) <= 4096"),
                ))
                .check((
                    "ck_conversation_patches_entity_times_ordered",
                    Expr::cust(
                        "entity_created_at_ms IS NULL OR entity_updated_at_ms IS NULL \
                         OR entity_updated_at_ms >= entity_created_at_ms",
                    ),
                ))
                .check((
                    "ck_conversation_patches_payload",
                    Expr::cust(
                        "(kind = 'turn_upsert' \
                         AND item_id IS NULL AND item_kind IS NULL AND run_id IS NULL \
                         AND phase IS NULL AND body IS NULL AND fragment IS NULL \
                         AND turn_id IS NOT NULL AND ordinal IS NOT NULL \
                         AND lifecycle IS NOT NULL AND entity_created_at_ms IS NOT NULL \
                         AND entity_updated_at_ms IS NOT NULL) \
                         OR (kind = 'item_upsert' AND fragment IS NULL \
                         AND turn_id IS NOT NULL AND item_id IS NOT NULL \
                         AND ordinal IS NOT NULL AND lifecycle IS NOT NULL \
                         AND item_kind IS NOT NULL AND body IS NOT NULL \
                         AND entity_created_at_ms IS NOT NULL \
                         AND entity_updated_at_ms IS NOT NULL \
                         AND ((item_kind = 'user_message' AND run_id IS NULL AND phase IS NULL) \
                         OR (item_kind = 'assistant_message' AND run_id IS NOT NULL \
                         AND phase IS NOT NULL))) \
                         OR (kind = 'item_append' AND item_id IS NOT NULL \
                         AND fragment IS NOT NULL AND turn_id IS NULL AND ordinal IS NULL \
                         AND lifecycle IS NULL AND item_kind IS NULL AND run_id IS NULL \
                         AND phase IS NULL AND body IS NULL \
                         AND entity_created_at_ms IS NULL AND entity_updated_at_ms IS NULL) \
                         OR (kind = 'item_lifecycle' AND item_id IS NOT NULL \
                         AND lifecycle IS NOT NULL AND turn_id IS NULL AND ordinal IS NULL \
                         AND item_kind IS NULL AND run_id IS NULL AND phase IS NULL \
                         AND body IS NULL AND fragment IS NULL \
                         AND entity_created_at_ms IS NULL AND entity_updated_at_ms IS NULL) \
                         OR (kind = 'turn_lifecycle' AND turn_id IS NOT NULL \
                         AND lifecycle IS NOT NULL AND item_id IS NULL AND ordinal IS NULL \
                         AND item_kind IS NULL AND run_id IS NULL AND phase IS NULL \
                         AND body IS NULL AND fragment IS NULL \
                         AND entity_created_at_ms IS NULL AND entity_updated_at_ms IS NULL)",
                    ),
                ))
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("uq_conversation_patches_thread_id_sequence")
                .table(ConversationPatches::Table)
                .col(ConversationPatches::ThreadId)
                .col(ConversationPatches::Sequence)
                .unique()
                .to_owned(),
        )
        .await
}

async fn create_run_checkpoints(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(RunCheckpoints::Table)
                .col(
                    ColumnDef::new(RunCheckpoints::RunId)
                        .string()
                        .not_null()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(RunCheckpoints::Generation)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(RunCheckpoints::LastBatchSequence)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(RunCheckpoints::EngineCheckpointVersion)
                        .big_integer()
                        .null(),
                )
                .col(
                    ColumnDef::new(RunCheckpoints::EngineCheckpointBlob)
                        .binary()
                        .null(),
                )
                .col(
                    ColumnDef::new(RunCheckpoints::UpdatedAtMs)
                        .big_integer()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_run_checkpoints_run_id_assistant_runs")
                        .from(RunCheckpoints::Table, RunCheckpoints::RunId)
                        .to(AssistantRuns::Table, AssistantRuns::RunId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .check((
                    "ck_run_checkpoints_generation_nonnegative",
                    Expr::col(RunCheckpoints::Generation).gte(0),
                ))
                .check((
                    "ck_run_checkpoints_last_batch_sequence_nonnegative",
                    Expr::col(RunCheckpoints::LastBatchSequence).gte(0),
                ))
                .check((
                    "ck_run_checkpoints_engine_checkpoint_tuple",
                    Expr::cust(
                        "(engine_checkpoint_version IS NULL \
                         AND engine_checkpoint_blob IS NULL) \
                         OR (engine_checkpoint_version IS NOT NULL \
                         AND engine_checkpoint_version > 0 \
                         AND engine_checkpoint_blob IS NOT NULL \
                         AND length(engine_checkpoint_blob) BETWEEN 1 AND 262144)",
                    ),
                ))
                .to_owned(),
        )
        .await
}

async fn create_run_batch_receipts(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(RunBatchReceipts::Table)
                .col(ColumnDef::new(RunBatchReceipts::RunId).string().not_null())
                .col(
                    ColumnDef::new(RunBatchReceipts::BatchSequence)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(RunBatchReceipts::Generation)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(RunBatchReceipts::Digest).binary().not_null())
                .col(
                    ColumnDef::new(RunBatchReceipts::Committed)
                        .boolean()
                        .not_null(),
                )
                .primary_key(
                    Index::create()
                        .name("pk_run_batch_receipts")
                        .col(RunBatchReceipts::RunId)
                        .col(RunBatchReceipts::BatchSequence),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_run_batch_receipts_run_id_assistant_runs")
                        .from(RunBatchReceipts::Table, RunBatchReceipts::RunId)
                        .to(AssistantRuns::Table, AssistantRuns::RunId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .check((
                    "ck_run_batch_receipts_batch_sequence_positive",
                    Expr::col(RunBatchReceipts::BatchSequence).gt(0),
                ))
                .check((
                    "ck_run_batch_receipts_generation_nonnegative",
                    Expr::col(RunBatchReceipts::Generation).gte(0),
                ))
                .check((
                    "ck_run_batch_receipts_digest_exact_bytes",
                    Expr::cust("length(digest) = 32"),
                ))
                .check((
                    "ck_run_batch_receipts_committed_flag",
                    Expr::cust("committed IN (0, 1)"),
                ))
                .to_owned(),
        )
        .await
}

#[derive(DeriveIden)]
enum Messages {
    Table,
    MessageId,
    ThreadId,
}

#[derive(DeriveIden)]
enum Threads {
    Table,
    ThreadId,
}

#[derive(DeriveIden)]
enum ConversationState {
    Table,
    ThreadId,
    NextRendererOrdinal,
    LastPatchSequence,
    UpdatedAtMs,
}

#[derive(DeriveIden)]
enum ConversationOrdinals {
    Table,
    ThreadId,
    Ordinal,
    Kind,
    EntityId,
}

#[derive(DeriveIden)]
enum ConversationTurns {
    Table,
    TurnId,
    ThreadId,
    Ordinal,
    Kind,
    Revision,
    Lifecycle,
    CreatedAtMs,
    UpdatedAtMs,
}

#[derive(DeriveIden)]
enum AssistantRuns {
    Table,
    RunId,
    ThreadId,
    RunStartKey,
    OriginMessageId,
    OriginTurnId,
    Lifecycle,
    Generation,
    Owner,
    Lease,
    ClaimToken,
    ProviderBindingVersion,
    ProviderBinding,
    ProviderBoundAtMs,
    ErrorCode,
    ErrorMessage,
    CreatedAtMs,
    UpdatedAtMs,
    TerminalAtMs,
}

#[derive(DeriveIden)]
enum ConversationItems {
    Table,
    ItemId,
    ThreadId,
    TurnId,
    Ordinal,
    Kind,
    Revision,
    Lifecycle,
    ItemKind,
    SourceMessageId,
    RunId,
    NativeItemKey,
    Phase,
    Body,
    CreatedAtMs,
    UpdatedAtMs,
}

#[derive(DeriveIden)]
enum ConversationPatches {
    Table,
    PatchId,
    ThreadId,
    Sequence,
    Kind,
    Revision,
    RecordedAtMs,
    TurnId,
    ItemId,
    Ordinal,
    Lifecycle,
    ItemKind,
    RunId,
    Phase,
    Body,
    Fragment,
    EntityCreatedAtMs,
    EntityUpdatedAtMs,
}

#[derive(DeriveIden)]
enum RunCheckpoints {
    Table,
    RunId,
    Generation,
    LastBatchSequence,
    EngineCheckpointVersion,
    EngineCheckpointBlob,
    UpdatedAtMs,
}

#[derive(DeriveIden)]
enum RunBatchReceipts {
    Table,
    RunId,
    BatchSequence,
    Generation,
    Digest,
    Committed,
}
