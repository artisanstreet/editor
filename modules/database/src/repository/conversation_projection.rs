//! Bounded, transactionally consistent conversation snapshot reads.

use std::collections::HashSet;

use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, QueryResult, Statement, TransactionTrait,
    Value,
};

use artisan_domain::{
    AssistantBody, AssistantMessageItem, AssistantMessagePhase, ConversationCursor,
    ConversationItem, ConversationLifecycle, ConversationQuery, ConversationQueryBounds,
    ConversationSnapshot, ConversationSnapshotError, ConversationTurn, ItemId, ItemOrdinal,
    MessageBody, MessageId, Revision, RunId, ThreadId, TurnId, TurnOrdinal, UnixMillis,
    UserMessageItem,
};

use super::{Repository, RepositoryError, corrupt_data, database_error};

const THREAD_QUERY: &str = "SELECT thread_id, CAST(created_at_ms AS TEXT), CAST(updated_at_ms AS TEXT) \
     FROM threads WHERE thread_id = ? LIMIT 1";
const STATE_QUERY: &str = "SELECT thread_id, CAST(next_renderer_ordinal AS TEXT), \
            CAST(last_patch_sequence AS TEXT), CAST(updated_at_ms AS TEXT) \
     FROM conversation_state WHERE thread_id = ? LIMIT 1";
const TURN_COLUMNS: &str = "SELECT turn_id, thread_id, CAST(ordinal AS TEXT), kind, \
            CAST(revision AS TEXT), lifecycle, CAST(created_at_ms AS TEXT), \
            CAST(updated_at_ms AS TEXT) FROM conversation_turns";
const ITEM_COLUMNS: &str = "SELECT item_id, thread_id, turn_id, CAST(ordinal AS TEXT), kind, \
            CAST(revision AS TEXT), lifecycle, item_kind, source_message_id, \
            run_id, native_item_key, phase, body, CAST(created_at_ms AS TEXT), \
            CAST(updated_at_ms AS TEXT) FROM conversation_items";

impl Repository {
    /// Reads one bounded canonical conversation snapshot from one database
    /// snapshot.
    ///
    /// The transaction is deliberately read-only. It keeps the thread,
    /// conversation state, selected turns, and selected items at one
    /// consistent SQLite snapshot, even when another connection commits while
    /// this read is in progress. Window and range limits are applied by the
    /// turn query before rows are materialized.
    ///
    /// # Errors
    ///
    /// Returns `ThreadNotFound` when the thread is absent, `CorruptData` for
    /// malformed persisted projection values, or `Database` when SQLite cannot
    /// complete the read or transaction.
    pub async fn read_conversation_snapshot(
        &self,
        query: &ConversationQuery,
    ) -> Result<ConversationSnapshot, RepositoryError> {
        let transaction = self
            .database
            .begin()
            .await
            .map_err(|source| database_error("begin conversation snapshot read", source))?;
        match read_snapshot(&transaction, query).await {
            Ok(snapshot) => {
                transaction.commit().await.map_err(|source| {
                    database_error("commit conversation snapshot read", source)
                })?;
                Ok(snapshot)
            }
            Err(error) => {
                transaction.rollback().await.map_err(|source| {
                    database_error("roll back conversation snapshot read", source)
                })?;
                Err(error)
            }
        }
    }
}

async fn read_snapshot(
    transaction: &DatabaseTransaction,
    query: &ConversationQuery,
) -> Result<ConversationSnapshot, RepositoryError> {
    let thread = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            THREAD_QUERY,
            [query.thread_id.as_str().to_owned().into()],
        ))
        .await
        .map_err(|source| database_error("load conversation thread", source))?;
    let Some(thread) = thread else {
        return Err(RepositoryError::ThreadNotFound {
            thread_id: query.thread_id.clone(),
        });
    };

    let persisted_thread_id =
        ThreadId::parse(raw_value::<String>(&thread, 0, "threads", "thread_id")?)
            .map_err(|error| corrupt_data("threads", "thread_id", &error))?;
    ensure_expected_thread(
        &persisted_thread_id,
        &query.thread_id,
        "threads",
        "thread_id",
    )?;
    let thread_created_at_ms = raw_signed_integer(&thread, 1, "threads", "created_at_ms")?;
    let thread_updated_at_ms = raw_signed_integer(&thread, 2, "threads", "updated_at_ms")?;
    validate_entity_times("threads", thread_created_at_ms, thread_updated_at_ms)?;

    let state = load_state(transaction, &query.thread_id).await?;
    let Some((cursor, updated_at)) = state else {
        ensure_no_projection_rows(transaction, &query.thread_id).await?;
        return ConversationSnapshot::new(
            query.thread_id.clone(),
            ConversationCursor::default(),
            Vec::new(),
            Vec::new(),
            UnixMillis::from_millis(thread_updated_at_ms),
        )
        .map_err(|error| snapshot_error(&error));
    };

    let turns = load_turns(transaction, query).await?;
    let selected_turn_ids = turns
        .iter()
        .map(|turn| turn.turn_id.as_str().to_owned())
        .collect::<HashSet<_>>();
    let items = load_items(transaction, query, &selected_turn_ids).await?;

    ConversationSnapshot::new(query.thread_id.clone(), cursor, turns, items, updated_at)
        .map_err(|error| snapshot_error(&error))
}

async fn load_state(
    transaction: &DatabaseTransaction,
    expected_thread_id: &ThreadId,
) -> Result<Option<(ConversationCursor, UnixMillis)>, RepositoryError> {
    let state = transaction
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            STATE_QUERY,
            [expected_thread_id.as_str().to_owned().into()],
        ))
        .await
        .map_err(|source| database_error("load conversation state", source))?;
    let Some(state) = state else {
        return Ok(None);
    };

    let persisted_thread_id = ThreadId::parse(raw_value::<String>(
        &state,
        0,
        "conversation_state",
        "thread_id",
    )?)
    .map_err(|error| corrupt_data("conversation_state", "thread_id", &error))?;
    ensure_expected_thread(
        &persisted_thread_id,
        expected_thread_id,
        "conversation_state",
        "thread_id",
    )?;
    let next_renderer_ordinal = nonnegative_counter(
        raw_signed_integer(&state, 1, "conversation_state", "next_renderer_ordinal")?,
        "conversation_state",
        "next_renderer_ordinal",
    )?;
    let _ = next_renderer_ordinal;
    let last_patch_sequence = nonnegative_counter(
        raw_signed_integer(&state, 2, "conversation_state", "last_patch_sequence")?,
        "conversation_state",
        "last_patch_sequence",
    )?;
    let updated_at_ms = raw_signed_integer(&state, 3, "conversation_state", "updated_at_ms")?;

    Ok(Some((
        ConversationCursor::new(last_patch_sequence),
        UnixMillis::from_millis(updated_at_ms),
    )))
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
            .map_err(|source| database_error("check conversation state absence", source))?;
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

async fn load_turns(
    transaction: &DatabaseTransaction,
    query: &ConversationQuery,
) -> Result<Vec<ConversationTurn>, RepositoryError> {
    let maximum_turn_count = query_maximum(query);
    match query.bounds {
        ConversationQueryBounds::Window { .. } => {
            transaction
                .query_all_raw(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    format!("{TURN_COLUMNS} WHERE thread_id = ? ORDER BY ordinal DESC LIMIT ?"),
                    [
                        query.thread_id.as_str().to_owned().into(),
                        maximum_turn_count.into(),
                    ],
                ))
                .await
        }
        ConversationQueryBounds::Range {
            before_turn_ordinal,
            minimum_turn_ordinal,
            ..
        } => {
            // SQLite's durable ordinal column is signed. An upper bound above
            // i64::MAX includes every representable persisted ordinal, while
            // a lower bound above it includes none. Both cases are handled
            // without narrowing the domain's u64 value.
            let before = i64::try_from(before_turn_ordinal.get()).ok();
            let minimum =
                minimum_turn_ordinal.and_then(|ordinal| i64::try_from(ordinal.get()).ok());
            if minimum_turn_ordinal.is_some() && minimum.is_none() {
                return Ok(Vec::new());
            }
            match (before, minimum) {
                (Some(before), Some(minimum)) => {
                    transaction
                        .query_all_raw(Statement::from_sql_and_values(
                            DbBackend::Sqlite,
                            format!(
                                "{TURN_COLUMNS} WHERE thread_id = ? AND ordinal < ? \
                                 AND ordinal >= ? ORDER BY ordinal DESC LIMIT ?"
                            ),
                            [
                                query.thread_id.as_str().to_owned().into(),
                                before.into(),
                                minimum.into(),
                                maximum_turn_count.into(),
                            ],
                        ))
                        .await
                }
                (Some(before), None) => {
                    transaction
                        .query_all_raw(Statement::from_sql_and_values(
                            DbBackend::Sqlite,
                            format!(
                                "{TURN_COLUMNS} WHERE thread_id = ? AND ordinal < ? \
                                 ORDER BY ordinal DESC LIMIT ?"
                            ),
                            [
                                query.thread_id.as_str().to_owned().into(),
                                before.into(),
                                maximum_turn_count.into(),
                            ],
                        ))
                        .await
                }
                (None, Some(minimum)) => {
                    transaction
                        .query_all_raw(Statement::from_sql_and_values(
                            DbBackend::Sqlite,
                            format!(
                                "{TURN_COLUMNS} WHERE thread_id = ? AND ordinal >= ? \
                                 ORDER BY ordinal DESC LIMIT ?"
                            ),
                            [
                                query.thread_id.as_str().to_owned().into(),
                                minimum.into(),
                                maximum_turn_count.into(),
                            ],
                        ))
                        .await
                }
                (None, None) => {
                    transaction
                        .query_all_raw(Statement::from_sql_and_values(
                            DbBackend::Sqlite,
                            format!(
                                "{TURN_COLUMNS} WHERE thread_id = ? ORDER BY ordinal DESC LIMIT ?"
                            ),
                            [
                                query.thread_id.as_str().to_owned().into(),
                                maximum_turn_count.into(),
                            ],
                        ))
                        .await
                }
            }
        }
    }
    .map_err(|source| database_error("load bounded conversation turns", source))?
    .into_iter()
    .map(|row| turn_from_row(&row, &query.thread_id))
    .collect()
}

fn query_maximum(query: &ConversationQuery) -> i64 {
    match query.bounds {
        ConversationQueryBounds::Window { maximum_turn_count }
        | ConversationQueryBounds::Range {
            maximum_turn_count, ..
        } => i64::from(maximum_turn_count.get()),
    }
}

async fn load_items(
    transaction: &DatabaseTransaction,
    query: &ConversationQuery,
    selected_turn_ids: &HashSet<String>,
) -> Result<Vec<ConversationItem>, RepositoryError> {
    if selected_turn_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = (0..selected_turn_ids.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "{ITEM_COLUMNS} WHERE thread_id = ? AND turn_id IN ({placeholders}) \
         ORDER BY ordinal ASC"
    );
    let mut values: Vec<Value> = Vec::with_capacity(selected_turn_ids.len() + 1);
    values.push(query.thread_id.as_str().to_owned().into());
    for turn_id in selected_turn_ids {
        values.push(Value::from(turn_id.clone()));
    }

    transaction
        .query_all_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            sql,
            values,
        ))
        .await
        .map_err(|source| database_error("load selected conversation items", source))?
        .into_iter()
        .map(|row| item_from_row(&row, &query.thread_id, selected_turn_ids))
        .collect()
}

fn turn_from_row(
    row: &QueryResult,
    expected_thread_id: &ThreadId,
) -> Result<ConversationTurn, RepositoryError> {
    let turn_id = TurnId::parse(raw_value::<String>(
        row,
        0,
        "conversation_turns",
        "turn_id",
    )?)
    .map_err(|error| corrupt_data("conversation_turns", "turn_id", &error))?;
    let persisted_thread_id = ThreadId::parse(raw_value::<String>(
        row,
        1,
        "conversation_turns",
        "thread_id",
    )?)
    .map_err(|error| corrupt_data("conversation_turns", "thread_id", &error))?;
    ensure_expected_thread(
        &persisted_thread_id,
        expected_thread_id,
        "conversation_turns",
        "thread_id",
    )?;
    let ordinal = TurnOrdinal::new(nonnegative_counter(
        raw_signed_integer(row, 2, "conversation_turns", "ordinal")?,
        "conversation_turns",
        "ordinal",
    )?);
    require_literal(
        &raw_value::<String>(row, 3, "conversation_turns", "kind")?,
        "turn",
        "conversation_turns",
        "kind",
    )?;
    let revision = Revision::new(nonnegative_counter(
        raw_signed_integer(row, 4, "conversation_turns", "revision")?,
        "conversation_turns",
        "revision",
    )?);
    let lifecycle = parse_lifecycle(
        &raw_value::<String>(row, 5, "conversation_turns", "lifecycle")?,
        "conversation_turns",
    )?;
    let created_at_ms = raw_signed_integer(row, 6, "conversation_turns", "created_at_ms")?;
    let updated_at_ms = raw_signed_integer(row, 7, "conversation_turns", "updated_at_ms")?;
    validate_entity_times("conversation_turns", created_at_ms, updated_at_ms)?;

    Ok(ConversationTurn {
        turn_id,
        ordinal,
        revision,
        lifecycle,
        created_at: UnixMillis::from_millis(created_at_ms),
        updated_at: UnixMillis::from_millis(updated_at_ms),
    })
}

#[allow(
    clippy::too_many_lines,
    reason = "the durable item shape is decoded and validated in one place"
)]
fn item_from_row(
    row: &QueryResult,
    expected_thread_id: &ThreadId,
    selected_turn_ids: &HashSet<String>,
) -> Result<ConversationItem, RepositoryError> {
    let item_id = ItemId::parse(raw_value::<String>(
        row,
        0,
        "conversation_items",
        "item_id",
    )?)
    .map_err(|error| corrupt_data("conversation_items", "item_id", &error))?;
    let persisted_thread_id = ThreadId::parse(raw_value::<String>(
        row,
        1,
        "conversation_items",
        "thread_id",
    )?)
    .map_err(|error| corrupt_data("conversation_items", "thread_id", &error))?;
    ensure_expected_thread(
        &persisted_thread_id,
        expected_thread_id,
        "conversation_items",
        "thread_id",
    )?;
    let turn_id = TurnId::parse(raw_value::<String>(
        row,
        2,
        "conversation_items",
        "turn_id",
    )?)
    .map_err(|error| corrupt_data("conversation_items", "turn_id", &error))?;
    if !selected_turn_ids.contains(turn_id.as_str()) {
        return Err(corrupt_data(
            "conversation_items",
            "turn_id",
            "item query returned a row outside the selected turn set",
        ));
    }
    let ordinal = ItemOrdinal::new(nonnegative_counter(
        raw_signed_integer(row, 3, "conversation_items", "ordinal")?,
        "conversation_items",
        "ordinal",
    )?);
    require_literal(
        &raw_value::<String>(row, 4, "conversation_items", "kind")?,
        "item",
        "conversation_items",
        "kind",
    )?;
    let revision = Revision::new(nonnegative_counter(
        raw_signed_integer(row, 5, "conversation_items", "revision")?,
        "conversation_items",
        "revision",
    )?);
    let lifecycle = parse_lifecycle(
        &raw_value::<String>(row, 6, "conversation_items", "lifecycle")?,
        "conversation_items",
    )?;
    let item_kind = raw_value::<String>(row, 7, "conversation_items", "item_kind")?;
    let source_message_id =
        raw_value::<Option<String>>(row, 8, "conversation_items", "source_message_id")?;
    let run_id = raw_value::<Option<String>>(row, 9, "conversation_items", "run_id")?;
    let native_item_key =
        raw_value::<Option<String>>(row, 10, "conversation_items", "native_item_key")?;
    if native_item_key.as_deref() == Some("") {
        return Err(corrupt_data(
            "conversation_items",
            "native_item_key",
            "optional native item key must not be empty",
        ));
    }
    let phase = raw_value::<Option<String>>(row, 11, "conversation_items", "phase")?;
    let body = raw_value::<String>(row, 12, "conversation_items", "body")?;
    let created_at_ms = raw_signed_integer(row, 13, "conversation_items", "created_at_ms")?;
    let updated_at_ms = raw_signed_integer(row, 14, "conversation_items", "updated_at_ms")?;
    validate_entity_times("conversation_items", created_at_ms, updated_at_ms)?;

    match item_kind.as_str() {
        "user_message" => {
            if run_id.is_some() || native_item_key.is_some() || phase.is_some() {
                return Err(corrupt_data(
                    "conversation_items",
                    "item_kind",
                    "user-message item has assistant-only fields",
                ));
            }
            let source_message_id = source_message_id.ok_or_else(|| {
                corrupt_data(
                    "conversation_items",
                    "source_message_id",
                    "user-message item is missing its source message",
                )
            })?;
            MessageId::parse(source_message_id)
                .map_err(|error| corrupt_data("conversation_items", "source_message_id", &error))?;
            let body = MessageBody::parse(body)
                .map_err(|error| corrupt_data("conversation_items", "body", &error))?;
            Ok(ConversationItem::UserMessage(UserMessageItem {
                item_id,
                turn_id,
                ordinal,
                revision,
                lifecycle,
                body,
                created_at: UnixMillis::from_millis(created_at_ms),
                updated_at: UnixMillis::from_millis(updated_at_ms),
            }))
        }
        "assistant_message" => {
            if source_message_id.is_some() {
                return Err(corrupt_data(
                    "conversation_items",
                    "item_kind",
                    "assistant-message item has a user-only source message",
                ));
            }
            let run_id = run_id.ok_or_else(|| {
                corrupt_data(
                    "conversation_items",
                    "run_id",
                    "assistant-message item is missing its run",
                )
            })?;
            let run_id = RunId::parse(run_id)
                .map_err(|error| corrupt_data("conversation_items", "run_id", &error))?;
            let phase = parse_phase(phase)?;
            let body = AssistantBody::parse(body)
                .map_err(|error| corrupt_data("conversation_items", "body", &error))?;
            Ok(ConversationItem::AssistantMessage(AssistantMessageItem {
                item_id,
                turn_id,
                run_id,
                ordinal,
                revision,
                lifecycle,
                body,
                phase,
                created_at: UnixMillis::from_millis(created_at_ms),
                updated_at: UnixMillis::from_millis(updated_at_ms),
            }))
        }
        _ => Err(corrupt_data(
            "conversation_items",
            "item_kind",
            &format!("unknown conversation item kind {item_kind}"),
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
        _ => Err(corrupt_data(
            table,
            "lifecycle",
            &format!("unknown conversation lifecycle {value}"),
        )),
    }
}

fn parse_phase(value: Option<String>) -> Result<AssistantMessagePhase, RepositoryError> {
    let value = value.ok_or_else(|| {
        corrupt_data(
            "conversation_items",
            "phase",
            "assistant-message item is missing its phase",
        )
    })?;
    match value.as_str() {
        "commentary" => Ok(AssistantMessagePhase::Commentary),
        "final" => Ok(AssistantMessagePhase::Final),
        "unspecified" => Ok(AssistantMessagePhase::Unspecified),
        _ => Err(corrupt_data(
            "conversation_items",
            "phase",
            &format!("unknown assistant message phase {value}"),
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
    Err(corrupt_data(
        table,
        field,
        &format!("expected thread {expected}, found {actual}"),
    ))
}

fn require_literal(
    actual: &str,
    expected: &str,
    table: &'static str,
    field: &'static str,
) -> Result<(), RepositoryError> {
    if actual == expected {
        return Ok(());
    }
    Err(corrupt_data(
        table,
        field,
        &format!("expected {expected}, found {actual}"),
    ))
}

fn nonnegative_counter(
    value: i64,
    table: &'static str,
    field: &'static str,
) -> Result<u64, RepositoryError> {
    u64::try_from(value).map_err(|_| {
        corrupt_data(
            table,
            field,
            &format!("persisted counter value {value} is negative"),
        )
    })
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
        &format!("updated timestamp {updated_at_ms} precedes created timestamp {created_at_ms}"),
    ))
}

fn raw_signed_integer(
    row: &QueryResult,
    index: usize,
    table: &'static str,
    field: &'static str,
) -> Result<i64, RepositoryError> {
    let value = raw_value::<String>(row, index, table, field)?;
    value
        .parse::<i64>()
        .map_err(|error| corrupt_data(table, field, &error))
}

fn raw_value<T>(
    row: &QueryResult,
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

fn snapshot_error(error: &ConversationSnapshotError) -> RepositoryError {
    corrupt_data("conversation_snapshot", "invariants", error)
}
