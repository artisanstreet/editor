//! Shared persistence-boundary constructors used across repository slices.
//!
//! Each slice keeps its own typed error enum. The helpers below are generic
//! over [`RepositoryFailure`] so every slice builds its own corrupt-data and
//! database-operation variants without duplicating the mapping.

use artisan_domain::{AssistantMessagePhase, ConversationLifecycle, ThreadId};
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, QueryResult, Statement, TryGetable,
};

use super::RepositoryError;

/// Thread projection shared by the conversation snapshot and patch replay reads.
pub(crate) const THREAD_QUERY: &str = "SELECT thread_id, CAST(created_at_ms AS TEXT), CAST(updated_at_ms AS TEXT) \
     FROM threads WHERE thread_id = ? LIMIT 1";

/// Conversation-state projection shared by the conversation snapshot and patch
/// replay reads.
pub(crate) const STATE_QUERY: &str = "SELECT thread_id, CAST(next_renderer_ordinal AS TEXT), \
            CAST(last_patch_sequence AS TEXT), CAST(updated_at_ms AS TEXT) \
     FROM conversation_state WHERE thread_id = ? LIMIT 1";

/// A repository error type that carries the shared corrupt-data and database
/// operation failures.
pub(crate) trait RepositoryFailure: Sized {
    /// Builds this type's persisted-data corruption variant.
    fn corrupt_data(table: &'static str, field: &'static str, reason: String) -> Self;

    /// Builds this type's database operation failure variant.
    fn database_error(operation: &'static str, source: DbErr) -> Self;
}

impl RepositoryFailure for RepositoryError {
    fn corrupt_data(table: &'static str, field: &'static str, reason: String) -> Self {
        Self::CorruptData {
            table,
            field,
            reason,
        }
    }

    fn database_error(operation: &'static str, source: DbErr) -> Self {
        Self::Database { operation, source }
    }
}

/// Builds one persisted-data corruption rejection for `E`.
#[expect(
    clippy::needless_pass_by_value,
    reason = "call sites pass owned error values and string literals alike; taking the generic \
              by value keeps one uniform call shape at every rejection site"
)]
pub(crate) fn corrupt_data<E: RepositoryFailure>(
    table: &'static str,
    field: &'static str,
    reason: impl ToString,
) -> E {
    E::corrupt_data(table, field, reason.to_string())
}

/// Builds one database operation failure for `E`.
pub(crate) fn database_error<E: RepositoryFailure>(operation: &'static str, source: DbErr) -> E {
    E::database_error(operation, source)
}

/// Reads one typed column from a raw query row for `E`.
///
/// # Errors
///
/// Returns `E`'s corruption variant when the column cannot be extracted.
pub(crate) fn raw_value<T, E: RepositoryFailure>(
    row: &QueryResult,
    index: usize,
    table: &'static str,
    field: &'static str,
) -> Result<T, E>
where
    T: TryGetable,
{
    row.try_get_by_index(index)
        .map_err(|error| corrupt_data(table, field, &error))
}

/// Reads one typed column from a raw query row for `E`, with the field name
/// preceding the table name.
///
/// # Errors
///
/// Returns `E`'s corruption variant when the column cannot be extracted.
pub(crate) fn row_value<T, E: RepositoryFailure>(
    row: &QueryResult,
    index: usize,
    field: &'static str,
    table: &'static str,
) -> Result<T, E>
where
    T: TryGetable,
{
    raw_value(row, index, table, field)
}

/// Reads one signed integer column through its persisted text encoding.
///
/// # Errors
///
/// Returns `E`'s corruption variant when the column cannot be extracted or
/// does not parse as a signed integer.
pub(crate) fn raw_signed_integer<E: RepositoryFailure>(
    row: &QueryResult,
    index: usize,
    table: &'static str,
    field: &'static str,
) -> Result<i64, E> {
    let value = raw_value::<String, E>(row, index, table, field)?;
    value
        .parse::<i64>()
        .map_err(|error| corrupt_data(table, field, &error))
}

/// Builds the corruption rejection for a negative conversation counter.
pub(crate) fn negative_counter<E: RepositoryFailure>(column: &'static str) -> E {
    corrupt_data("conversation_state", column, "counter is negative")
}

/// Converts one persisted counter, rejecting negative storage with the
/// caller's reason text.
///
/// # Errors
///
/// Returns `E`'s corruption variant when `value` is negative.
pub(crate) fn nonnegative_counter<E: RepositoryFailure>(
    value: i64,
    table: &'static str,
    field: &'static str,
    negative_reason: impl FnOnce(i64) -> String,
) -> Result<u64, E> {
    u64::try_from(value).map_err(|_| corrupt_data(table, field, negative_reason(value)))
}

/// Parses one persisted conversation lifecycle value.
///
/// # Errors
///
/// Returns `E`'s corruption variant when `value` is not a known lifecycle.
pub(crate) fn parse_lifecycle<E: RepositoryFailure>(
    value: &str,
    table: &'static str,
    unknown_reason: impl FnOnce(&str) -> String,
) -> Result<ConversationLifecycle, E> {
    match value {
        "pending" => Ok(ConversationLifecycle::Pending),
        "streaming" => Ok(ConversationLifecycle::Streaming),
        "active" => Ok(ConversationLifecycle::Active),
        "waiting" => Ok(ConversationLifecycle::Waiting),
        "completed" => Ok(ConversationLifecycle::Completed),
        "failed" => Ok(ConversationLifecycle::Failed),
        "interrupted" => Ok(ConversationLifecycle::Interrupted),
        "cancelled" => Ok(ConversationLifecycle::Cancelled),
        _ => Err(corrupt_data(table, "lifecycle", unknown_reason(value))),
    }
}

/// Parses one persisted assistant message phase, including the absent case.
///
/// # Errors
///
/// Returns `E`'s corruption variant when `value` is absent or unknown.
pub(crate) fn parse_phase<E: RepositoryFailure>(
    value: Option<String>,
    table: &'static str,
    missing_reason: &'static str,
    unknown_reason: impl FnOnce(&str) -> String,
) -> Result<AssistantMessagePhase, E> {
    let value = value.ok_or_else(|| corrupt_data(table, "phase", missing_reason))?;
    match value.as_str() {
        "commentary" => Ok(AssistantMessagePhase::Commentary),
        "final" => Ok(AssistantMessagePhase::Final),
        "unspecified" => Ok(AssistantMessagePhase::Unspecified),
        _ => Err(corrupt_data(table, "phase", unknown_reason(&value))),
    }
}

/// Rejects a persisted row whose thread id differs from the expected one.
///
/// # Errors
///
/// Returns `E`'s corruption variant when `actual` differs from `expected`.
pub(crate) fn ensure_expected_thread<E: RepositoryFailure>(
    actual: &ThreadId,
    expected: &ThreadId,
    table: &'static str,
    field: &'static str,
    mismatch_reason: impl FnOnce() -> String,
) -> Result<(), E> {
    if actual == expected {
        return Ok(());
    }
    Err(corrupt_data(table, field, mismatch_reason()))
}

/// Validates that an entity's update timestamp does not precede its creation.
///
/// # Errors
///
/// Returns `E`'s corruption variant when `updated_at_ms` precedes
/// `created_at_ms`.
pub(crate) fn validate_entity_times<E: RepositoryFailure>(
    table: &'static str,
    created_at_ms: i64,
    updated_at_ms: i64,
    invalid_reason: impl FnOnce(i64, i64) -> String,
) -> Result<(), E> {
    if updated_at_ms >= created_at_ms {
        return Ok(());
    }
    Err(corrupt_data(
        table,
        "updated_at_ms",
        invalid_reason(updated_at_ms, created_at_ms),
    ))
}

/// Confirms no conversation projection rows exist while conversation state is
/// absent.
///
/// # Errors
///
/// Returns `E`'s corruption variant when any projection table holds rows for
/// the thread, or `E`'s database operation variant when a probe query fails.
pub(crate) async fn ensure_no_projection_rows<E: RepositoryFailure>(
    transaction: &DatabaseTransaction,
    thread_id: &ThreadId,
    operation: &'static str,
) -> Result<(), E> {
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
            .map_err(|source| database_error(operation, source))?;
        if row.is_some() {
            return Err(corrupt_data(
                "conversation_state",
                "thread_id",
                format!("{table} contains rows without conversation state"),
            ));
        }
    }
    Ok(())
}
