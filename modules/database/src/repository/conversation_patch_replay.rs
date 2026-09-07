//! Bounded, transactionally consistent conversation patch replay reads.

use std::collections::HashSet;

use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};

use artisan_domain::bounds::CONVERSATION_PATCH_BATCH_MAX_PATCHES;
use artisan_domain::{
    AssistantBody, AssistantMessageItem, AssistantMessagePhase, ConversationCursor,
    ConversationItem, ConversationLifecycle, ConversationPatch, ConversationTurn, IncrementalText,
    ItemId, ItemOrdinal, MessageBody, PatchBatch, PatchId, PatchSequence, Revision, RunId,
    ThreadId, TurnId, TurnOrdinal, UnixMillis,
};

use super::{Repository, RepositoryError, corrupt_data, database_error};

const THREAD_QUERY: &str = "SELECT thread_id, CAST(created_at_ms AS TEXT), CAST(updated_at_ms AS TEXT) \
     FROM threads WHERE thread_id = ? LIMIT 1";
const STATE_QUERY: &str = "SELECT thread_id, CAST(next_renderer_ordinal AS TEXT), \
            CAST(last_patch_sequence AS TEXT), CAST(updated_at_ms AS TEXT) \
     FROM conversation_state WHERE thread_id = ? LIMIT 1";
const PATCH_SELECT: &str = "SELECT patch_id, thread_id, CAST(sequence AS TEXT), kind, \
            CAST(revision AS TEXT), CAST(recorded_at_ms AS TEXT), turn_id, item_id, \
            CAST(ordinal AS TEXT), lifecycle, item_kind, run_id, phase, body, fragment, \
            CAST(entity_created_at_ms AS TEXT), CAST(entity_updated_at_ms AS TEXT) \
     FROM conversation_patches WHERE thread_id = ? AND sequence > ? \
     ORDER BY sequence ASC LIMIT ?";

/// Bounded replay result for one per-thread cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversationPatchReplay {
    /// The requested cursor equals the durable tail.
    Current { cursor: ConversationCursor },
    /// The next contiguous batch after the cursor, capped at 64 patches.
    Batch(PatchBatch),
    /// The requested cursor is beyond the durable tail.
    ResnapshotRequired {
        requested_cursor: ConversationCursor,
        current_cursor: ConversationCursor,
    },
}

impl Repository {
    /// Reads one bounded patch batch after `after_cursor`.
    ///
    /// The transaction keeps the thread, conversation state, and selected
    /// patch rows at one SQLite snapshot. `Current` means the caller is
    /// already at the tail; `ResnapshotRequired` means the caller is beyond
    /// it; `Batch` is the next contiguous capped batch otherwise.
    ///
    /// # Errors
    ///
    /// Returns `ThreadNotFound` for an absent thread, `CorruptData` for
    /// malformed or gapped persisted state, or `Database` when SQLite cannot
    /// complete the read.
    pub async fn read_conversation_patch_replay(
        &self,
        thread_id: &ThreadId,
        after_cursor: ConversationCursor,
    ) -> Result<ConversationPatchReplay, RepositoryError> {
        let transaction = self
            .database
            .begin()
            .await
            .map_err(|source| database_error("begin patch replay read", source))?;
        match read_replay(&transaction, thread_id, after_cursor).await {
            Ok(result) => {
                transaction
                    .commit()
                    .await
                    .map_err(|source| database_error("commit patch replay read", source))?;
                Ok(result)
            }
            Err(error) => {
                transaction
                    .rollback()
                    .await
                    .map_err(|source| database_error("roll back patch replay read", source))?;
                Err(error)
            }
        }
    }
}

async fn read_replay(
    transaction: &DatabaseTransaction,
    thread_id: &ThreadId,
    after_cursor: ConversationCursor,
) -> Result<ConversationPatchReplay, RepositoryError> {
    ensure_thread_exists(transaction, thread_id).await?;
    let tail = load_tail(transaction, thread_id).await?;
    let after_value = after_cursor.get();
    let tail_value = tail.get();
    if after_value > tail_value {
        return Ok(ConversationPatchReplay::ResnapshotRequired {
            requested_cursor: after_cursor,
            current_cursor: tail,
        });
    }
    if after_value == tail_value {
        return Ok(ConversationPatchReplay::Current { cursor: tail });
    }
    // after < tail : there must be a contiguous batch.
    let patches = load_patches(transaction, thread_id, after_cursor, tail).await?;
    if patches.is_empty() {
        return Err(corrupt_data(
            "conversation_patches",
            "sequence",
            "missing patch rows before durable tail",
        ));
    }
    // Ensure first patch is exactly cursor+1 and endpoint does not exceed tail.
    let first_sequence = patches.first().expect("non-empty").sequence().get();
    let expected_first = after_cursor
        .checked_next_sequence()
        .map_err(|error| corrupt_data("conversation_patches", "sequence", &error))?
        .get();
    if first_sequence != expected_first {
        return Err(corrupt_data(
            "conversation_patches",
            "sequence",
            &format!("expected sequence {expected_first}, found {first_sequence}"),
        ));
    }
    let last_sequence = patches.last().expect("non-empty").sequence().get();
    if last_sequence > tail_value {
        return Err(corrupt_data(
            "conversation_patches",
            "sequence",
            "patch batch endpoint beyond durable tail",
        ));
    }
    let to_cursor = ConversationCursor::new(last_sequence);
    let batch = PatchBatch::new(thread_id.clone(), after_cursor, to_cursor, patches)
        .map_err(|error| corrupt_data("conversation_patches", "sequence", &error))?;
    Ok(ConversationPatchReplay::Batch(batch))
}

async fn ensure_thread_exists(
    transaction: &DatabaseTransaction,
    thread_id: &ThreadId,
) -> Result<(), RepositoryError> {
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            THREAD_QUERY,
            [thread_id.as_str().to_owned().into()],
        ))
        .await
        .map_err(|source| database_error("load patch replay thread", source))?;
    let Some(row) = row else {
        return Err(RepositoryError::ThreadNotFound {
            thread_id: thread_id.clone(),
        });
    };
    let persisted = ThreadId::parse(raw_value::<String>(&row, 0, "threads", "thread_id")?)
        .map_err(|error| corrupt_data("threads", "thread_id", &error))?;
    ensure_expected_thread(&persisted, thread_id, "threads", "thread_id")?;
    let created = raw_signed_integer(&row, 1, "threads", "created_at_ms")?;
    let updated = raw_signed_integer(&row, 2, "threads", "updated_at_ms")?;
    validate_entity_times("threads", created, updated)?;
    Ok(())
}

async fn load_tail(
    transaction: &DatabaseTransaction,
    thread_id: &ThreadId,
) -> Result<ConversationCursor, RepositoryError> {
    let row = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            STATE_QUERY,
            [thread_id.as_str().to_owned().into()],
        ))
        .await
        .map_err(|source| database_error("load patch replay state", source))?;
    let Some(row) = row else {
        ensure_no_projection_rows(transaction, thread_id).await?;
        return Ok(ConversationCursor::default());
    };
    let persisted = ThreadId::parse(raw_value::<String>(
        &row,
        0,
        "conversation_state",
        "thread_id",
    )?)
    .map_err(|error| corrupt_data("conversation_state", "thread_id", &error))?;
    ensure_expected_thread(&persisted, thread_id, "conversation_state", "thread_id")?;
    let next_ordinal = raw_signed_integer(&row, 1, "conversation_state", "next_renderer_ordinal")?;
    let _ = nonnegative_counter(next_ordinal, "conversation_state", "next_renderer_ordinal")?;
    let last_patch = raw_signed_integer(&row, 2, "conversation_state", "last_patch_sequence")?;
    let tail_u64 = nonnegative_counter(last_patch, "conversation_state", "last_patch_sequence")?;
    let updated = raw_signed_integer(&row, 3, "conversation_state", "updated_at_ms")?;
    let _ = updated;
    Ok(ConversationCursor::new(tail_u64))
}

async fn ensure_no_projection_rows(
    transaction: &DatabaseTransaction,
    thread_id: &ThreadId,
) -> Result<(), RepositoryError> {
    for table in [
        "conversation_ordinals",
        "conversation_turns",
        "conversation_items",
        "conversation_patches",
    ] {
        let sql = format!("SELECT 1 FROM {table} WHERE thread_id = ? LIMIT 1");
        let row = transaction
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                sql,
                [thread_id.as_str().to_owned().into()],
            ))
            .await
            .map_err(|source| database_error("check patch replay state absence", source))?;
        if row.is_some() {
            return Err(corrupt_data(
                "conversation_state",
                "thread_id",
                &format!("{table} contains rows without conversation state"),
            ));
        }
    }
    Ok(())
}

async fn load_patches(
    transaction: &DatabaseTransaction,
    thread_id: &ThreadId,
    after_cursor: ConversationCursor,
    tail: ConversationCursor,
) -> Result<Vec<ConversationPatch>, RepositoryError> {
    let after_i64 = i64::try_from(after_cursor.get()).unwrap_or(i64::MAX);
    let limit =
        i64::try_from(CONVERSATION_PATCH_BATCH_MAX_PATCHES).expect("patch batch maximum fits i64");
    let rows = transaction
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            PATCH_SELECT,
            [
                thread_id.as_str().to_owned().into(),
                after_i64.into(),
                limit.into(),
            ],
        ))
        .await
        .map_err(|source| database_error("load patch replay rows", source))?;
    let mut patches = Vec::with_capacity(rows.len());
    let mut seen_sequences = HashSet::with_capacity(rows.len());
    let mut seen_patch_ids = HashSet::with_capacity(rows.len());
    for row in rows {
        let patch = patch_from_row(
            &row,
            thread_id,
            &mut seen_sequences,
            &mut seen_patch_ids,
            tail,
        )?;
        patches.push(patch);
    }
    // Contiguity beyond first row is validated by PatchBatch::new, but
    // interior gaps should already be corruption. We rely on PatchBatch for
    // final validation after this function returns.
    Ok(patches)
}

#[allow(
    clippy::too_many_lines,
    reason = "patch row validation is intentionally contiguous"
)]
fn patch_from_row(
    row: &sea_orm::QueryResult,
    expected_thread_id: &ThreadId,
    seen_sequences: &mut HashSet<u64>,
    seen_patch_ids: &mut HashSet<String>,
    tail: ConversationCursor,
) -> Result<ConversationPatch, RepositoryError> {
    let patch_id_raw = raw_value::<String>(row, 0, "conversation_patches", "patch_id")?;
    let patch_id = PatchId::parse(patch_id_raw)
        .map_err(|error| corrupt_data("conversation_patches", "patch_id", &error))?;
    if !seen_patch_ids.insert(patch_id.as_str().to_owned()) {
        return Err(corrupt_data(
            "conversation_patches",
            "patch_id",
            "duplicate patch identity in batch",
        ));
    }
    let thread_raw = raw_value::<String>(row, 1, "conversation_patches", "thread_id")?;
    let persisted_thread = ThreadId::parse(thread_raw)
        .map_err(|error| corrupt_data("conversation_patches", "thread_id", &error))?;
    ensure_expected_thread(
        &persisted_thread,
        expected_thread_id,
        "conversation_patches",
        "thread_id",
    )?;
    let sequence_raw = raw_signed_integer(row, 2, "conversation_patches", "sequence")?;
    if sequence_raw <= 0 {
        return Err(corrupt_data(
            "conversation_patches",
            "sequence",
            "patch sequence must be positive",
        ));
    }
    let sequence_u64 = u64::try_from(sequence_raw).map_err(|_| {
        corrupt_data(
            "conversation_patches",
            "sequence",
            "patch sequence is negative",
        )
    })?;
    if sequence_u64 > tail.get() {
        return Err(corrupt_data(
            "conversation_patches",
            "sequence",
            "patch sequence beyond durable tail",
        ));
    }
    let sequence = PatchSequence::new(sequence_u64)
        .map_err(|error| corrupt_data("conversation_patches", "sequence", &error))?;
    if !seen_sequences.insert(sequence_u64) {
        return Err(corrupt_data(
            "conversation_patches",
            "sequence",
            "duplicate patch sequence",
        ));
    }
    let kind = raw_value::<String>(row, 3, "conversation_patches", "kind")?;
    let revision_raw = raw_signed_integer(row, 4, "conversation_patches", "revision")?;
    let revision_u64 = nonnegative_counter(revision_raw, "conversation_patches", "revision")?;
    let revision = Revision::new(revision_u64);
    let recorded_raw = raw_signed_integer(row, 5, "conversation_patches", "recorded_at_ms")?;
    let recorded_at = UnixMillis::from_millis(recorded_raw);
    let turn_id_opt = raw_value::<Option<String>>(row, 6, "conversation_patches", "turn_id")?;
    let item_id_opt = raw_value::<Option<String>>(row, 7, "conversation_patches", "item_id")?;
    let ordinal_opt = raw_opt_signed_integer(row, 8, "conversation_patches", "ordinal")?;
    let lifecycle_opt = raw_value::<Option<String>>(row, 9, "conversation_patches", "lifecycle")?;
    let item_kind_opt = raw_value::<Option<String>>(row, 10, "conversation_patches", "item_kind")?;
    let run_id_opt = raw_value::<Option<String>>(row, 11, "conversation_patches", "run_id")?;
    let phase_opt = raw_value::<Option<String>>(row, 12, "conversation_patches", "phase")?;
    let body_opt = raw_value::<Option<String>>(row, 13, "conversation_patches", "body")?;
    let fragment_opt = raw_value::<Option<String>>(row, 14, "conversation_patches", "fragment")?;
    let entity_created_opt =
        raw_opt_signed_integer(row, 15, "conversation_patches", "entity_created_at_ms")?;
    let entity_updated_opt =
        raw_opt_signed_integer(row, 16, "conversation_patches", "entity_updated_at_ms")?;

    match kind.as_str() {
        "turn_upsert" => {
            if item_id_opt.is_some()
                || item_kind_opt.is_some()
                || run_id_opt.is_some()
                || phase_opt.is_some()
                || body_opt.is_some()
                || fragment_opt.is_some()
            {
                return Err(corrupt_data(
                    "conversation_patches",
                    "kind",
                    "turn_upsert has unexpected columns",
                ));
            }
            let turn_id_str = turn_id_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "turn_id",
                    "turn_upsert missing turn",
                )
            })?;
            let turn_id = TurnId::parse(turn_id_str)
                .map_err(|error| corrupt_data("conversation_patches", "turn_id", &error))?;
            let ordinal_raw = ordinal_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "ordinal",
                    "turn_upsert missing ordinal",
                )
            })?;
            let ordinal_u64 = nonnegative_counter(ordinal_raw, "conversation_patches", "ordinal")?;
            let ordinal = TurnOrdinal::new(ordinal_u64);
            let lifecycle_str = lifecycle_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "lifecycle",
                    "turn_upsert missing lifecycle",
                )
            })?;
            let lifecycle = parse_lifecycle(&lifecycle_str, "conversation_patches")?;
            let created_raw = entity_created_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "entity_created_at_ms",
                    "turn_upsert missing entity stamps",
                )
            })?;
            let updated_raw = entity_updated_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "entity_updated_at_ms",
                    "turn_upsert missing entity stamps",
                )
            })?;
            validate_entity_times("conversation_patches", created_raw, updated_raw)?;
            let turn = ConversationTurn {
                turn_id,
                ordinal,
                revision,
                lifecycle,
                created_at: UnixMillis::from_millis(created_raw),
                updated_at: UnixMillis::from_millis(updated_raw),
            };
            Ok(ConversationPatch::TurnUpsert {
                patch_id,
                sequence,
                turn,
            })
        }
        "item_upsert" => {
            if fragment_opt.is_some() {
                return Err(corrupt_data(
                    "conversation_patches",
                    "fragment",
                    "item_upsert must not have fragment",
                ));
            }
            let turn_id_str = turn_id_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "turn_id",
                    "item_upsert missing turn",
                )
            })?;
            let item_id_str = item_id_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "item_id",
                    "item_upsert missing item",
                )
            })?;
            let turn_id = TurnId::parse(turn_id_str)
                .map_err(|error| corrupt_data("conversation_patches", "turn_id", &error))?;
            let item_id = ItemId::parse(item_id_str)
                .map_err(|error| corrupt_data("conversation_patches", "item_id", &error))?;
            let ordinal_raw = ordinal_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "ordinal",
                    "item_upsert missing ordinal",
                )
            })?;
            let ordinal_u64 = nonnegative_counter(ordinal_raw, "conversation_patches", "ordinal")?;
            let ordinal = ItemOrdinal::new(ordinal_u64);
            let lifecycle_str = lifecycle_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "lifecycle",
                    "item_upsert missing lifecycle",
                )
            })?;
            let lifecycle = parse_lifecycle(&lifecycle_str, "conversation_patches")?;
            let item_kind_str = item_kind_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "item_kind",
                    "item_upsert missing item kind",
                )
            })?;
            let body_str = body_opt.ok_or_else(|| {
                corrupt_data("conversation_patches", "body", "item_upsert missing body")
            })?;
            let created_raw = entity_created_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "entity_created_at_ms",
                    "item_upsert missing stamps",
                )
            })?;
            let updated_raw = entity_updated_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "entity_updated_at_ms",
                    "item_upsert missing stamps",
                )
            })?;
            validate_entity_times("conversation_patches", created_raw, updated_raw)?;
            let created_at = UnixMillis::from_millis(created_raw);
            let updated_at = UnixMillis::from_millis(updated_raw);
            let _ = recorded_raw;
            let item = match item_kind_str.as_str() {
                "user_message" => {
                    if run_id_opt.is_some() || phase_opt.is_some() {
                        return Err(corrupt_data(
                            "conversation_patches",
                            "item_kind",
                            "user item has assistant fields",
                        ));
                    }
                    let body = MessageBody::parse(body_str)
                        .map_err(|error| corrupt_data("conversation_patches", "body", &error))?;
                    ConversationItem::UserMessage(artisan_domain::UserMessageItem {
                        item_id,
                        turn_id,
                        ordinal,
                        revision,
                        lifecycle,
                        body,
                        created_at,
                        updated_at,
                    })
                }
                "assistant_message" => {
                    let run_id_str = run_id_opt.ok_or_else(|| {
                        corrupt_data(
                            "conversation_patches",
                            "run_id",
                            "assistant item missing run",
                        )
                    })?;
                    let run_id = RunId::parse(run_id_str)
                        .map_err(|error| corrupt_data("conversation_patches", "run_id", &error))?;
                    let phase_str = phase_opt.ok_or_else(|| {
                        corrupt_data(
                            "conversation_patches",
                            "phase",
                            "assistant item missing phase",
                        )
                    })?;
                    let phase = parse_phase(Some(phase_str))?;
                    let body = AssistantBody::parse(body_str)
                        .map_err(|error| corrupt_data("conversation_patches", "body", &error))?;
                    ConversationItem::AssistantMessage(AssistantMessageItem {
                        item_id,
                        turn_id,
                        run_id,
                        ordinal,
                        revision,
                        lifecycle,
                        body,
                        phase,
                        created_at,
                        updated_at,
                    })
                }
                _ => {
                    return Err(corrupt_data(
                        "conversation_patches",
                        "item_kind",
                        "unknown item kind",
                    ));
                }
            };
            Ok(ConversationPatch::ItemUpsert {
                patch_id,
                sequence,
                item,
            })
        }
        "item_append" => {
            if turn_id_opt.is_some()
                || ordinal_opt.is_some()
                || lifecycle_opt.is_some()
                || item_kind_opt.is_some()
                || run_id_opt.is_some()
                || phase_opt.is_some()
                || body_opt.is_some()
                || entity_created_opt.is_some()
                || entity_updated_opt.is_some()
            {
                return Err(corrupt_data(
                    "conversation_patches",
                    "kind",
                    "item_append has unexpected columns",
                ));
            }
            let item_id_str = item_id_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "item_id",
                    "item_append missing item",
                )
            })?;
            let item_id = ItemId::parse(item_id_str)
                .map_err(|error| corrupt_data("conversation_patches", "item_id", &error))?;
            let fragment_str = fragment_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "fragment",
                    "item_append missing fragment",
                )
            })?;
            let text = IncrementalText::parse(fragment_str)
                .map_err(|error| corrupt_data("conversation_patches", "fragment", &error))?;
            Ok(ConversationPatch::ItemAppend {
                patch_id,
                sequence,
                item_id,
                revision,
                text,
                updated_at: recorded_at,
            })
        }
        "item_lifecycle" => {
            if turn_id_opt.is_some()
                || ordinal_opt.is_some()
                || item_kind_opt.is_some()
                || run_id_opt.is_some()
                || phase_opt.is_some()
                || body_opt.is_some()
                || fragment_opt.is_some()
                || entity_created_opt.is_some()
                || entity_updated_opt.is_some()
            {
                return Err(corrupt_data(
                    "conversation_patches",
                    "kind",
                    "item_lifecycle has unexpected columns",
                ));
            }
            let item_id_str = item_id_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "item_id",
                    "item_lifecycle missing item",
                )
            })?;
            let item_id = ItemId::parse(item_id_str)
                .map_err(|error| corrupt_data("conversation_patches", "item_id", &error))?;
            let lifecycle_str = lifecycle_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "lifecycle",
                    "item_lifecycle missing lifecycle",
                )
            })?;
            let lifecycle = parse_lifecycle(&lifecycle_str, "conversation_patches")?;
            Ok(ConversationPatch::ItemLifecycle {
                patch_id,
                sequence,
                item_id,
                revision,
                lifecycle,
                updated_at: recorded_at,
            })
        }
        "turn_lifecycle" => {
            if item_id_opt.is_some()
                || ordinal_opt.is_some()
                || item_kind_opt.is_some()
                || run_id_opt.is_some()
                || phase_opt.is_some()
                || body_opt.is_some()
                || fragment_opt.is_some()
                || entity_created_opt.is_some()
                || entity_updated_opt.is_some()
            {
                return Err(corrupt_data(
                    "conversation_patches",
                    "kind",
                    "turn_lifecycle has unexpected columns",
                ));
            }
            let turn_id_str = turn_id_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "turn_id",
                    "turn_lifecycle missing turn",
                )
            })?;
            let turn_id = TurnId::parse(turn_id_str)
                .map_err(|error| corrupt_data("conversation_patches", "turn_id", &error))?;
            let lifecycle_str = lifecycle_opt.ok_or_else(|| {
                corrupt_data(
                    "conversation_patches",
                    "lifecycle",
                    "turn_lifecycle missing lifecycle",
                )
            })?;
            let lifecycle = parse_lifecycle(&lifecycle_str, "conversation_patches")?;
            Ok(ConversationPatch::TurnLifecycle {
                patch_id,
                sequence,
                turn_id,
                revision,
                lifecycle,
                updated_at: recorded_at,
            })
        }
        _ => Err(corrupt_data(
            "conversation_patches",
            "kind",
            "unknown patch kind",
        )),
    }
}

fn parse_lifecycle(
    value: &str,
    table: &'static str,
) -> Result<ConversationLifecycle, RepositoryError> {
    match value {
        "pending" => Ok(ConversationLifecycle::Pending),
        "streaming" => Ok(ConversationLifecycle::Streaming),
        "active" => Ok(ConversationLifecycle::Active),
        "waiting" => Ok(ConversationLifecycle::Waiting),
        "completed" => Ok(ConversationLifecycle::Completed),
        "failed" => Ok(ConversationLifecycle::Failed),
        "interrupted" => Ok(ConversationLifecycle::Interrupted),
        "cancelled" => Ok(ConversationLifecycle::Cancelled),
        _ => Err(corrupt_data(table, "lifecycle", "unknown lifecycle")),
    }
}

fn parse_phase(value: Option<String>) -> Result<AssistantMessagePhase, RepositoryError> {
    let value = value.ok_or_else(|| {
        corrupt_data(
            "conversation_patches",
            "phase",
            "assistant item missing phase",
        )
    })?;
    match value.as_str() {
        "commentary" => Ok(AssistantMessagePhase::Commentary),
        "final" => Ok(AssistantMessagePhase::Final),
        "unspecified" => Ok(AssistantMessagePhase::Unspecified),
        _ => Err(corrupt_data(
            "conversation_patches",
            "phase",
            "unknown phase",
        )),
    }
}

fn ensure_expected_thread(
    actual: &ThreadId,
    expected: &ThreadId,
    table: &'static str,
    field: &'static str,
) -> Result<(), RepositoryError> {
    if actual == expected {
        return Ok(());
    }
    Err(corrupt_data(table, field, "thread id mismatch"))
}

fn nonnegative_counter(
    value: i64,
    table: &'static str,
    field: &'static str,
) -> Result<u64, RepositoryError> {
    u64::try_from(value)
        .map_err(|_| corrupt_data(table, field, &format!("counter value {value} is negative")))
}

fn validate_entity_times(
    table: &'static str,
    created_at_ms: i64,
    updated_at_ms: i64,
) -> Result<(), RepositoryError> {
    if updated_at_ms >= created_at_ms {
        return Ok(());
    }
    Err(corrupt_data(
        table,
        "updated_at_ms",
        &format!("updated {updated_at_ms} precedes created {created_at_ms}"),
    ))
}

fn raw_signed_integer(
    row: &sea_orm::QueryResult,
    index: usize,
    table: &'static str,
    field: &'static str,
) -> Result<i64, RepositoryError> {
    let value = raw_value::<String>(row, index, table, field)?;
    value
        .parse::<i64>()
        .map_err(|error| corrupt_data(table, field, &error))
}

fn raw_opt_signed_integer(
    row: &sea_orm::QueryResult,
    index: usize,
    table: &'static str,
    field: &'static str,
) -> Result<Option<i64>, RepositoryError> {
    let value = raw_value::<Option<String>>(row, index, table, field)?;
    match value {
        None => Ok(None),
        Some(text) => text
            .parse::<i64>()
            .map(Some)
            .map_err(|error| corrupt_data(table, field, &error)),
    }
}

fn raw_value<T>(
    row: &sea_orm::QueryResult,
    index: usize,
    table: &'static str,
    field: &'static str,
) -> Result<T, RepositoryError>
where
    T: sea_orm::TryGetable,
{
    row.try_get_by_index::<T>(index)
        .map_err(|source| corrupt_data(table, field, &source))
}
