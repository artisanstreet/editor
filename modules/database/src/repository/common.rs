//! Shared persistence-boundary constructors used across repository slices.
//!
//! Each slice keeps its own typed error enum. The helpers below are generic
//! over [`RepositoryFailure`] so every slice builds its own corrupt-data and
//! database-operation variants without duplicating the mapping.

use artisan_domain::{AssistantMessagePhase, ConversationLifecycle, ThreadId};
use sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, ExecResult, QueryResult,
    SqliteTransactionMode, Statement, TransactionOptions, TransactionTrait, TryGetable,
};

use crate::sqlite_write_retry::retry_write_operation;

use super::write_gate::WriteGateGuard;
use super::{Repository, RepositoryError};

/// Begins one read-then-write transaction with SQLite `BEGIN IMMEDIATE`.
///
/// Taking the writer fence before the first statement means a concurrent
/// commit cannot invalidate a read snapshot the transaction later upgrades,
/// so the `SQLITE_BUSY_SNAPSHOT` class cannot surface from a repository
/// transaction. Writer-fence contention that still reaches the driver after
/// `busy_timeout` is retried on the shared bounded schedule; every other
/// failure is returned unchanged for the caller's typed boundary.
///
/// The transaction first waits its turn at the repository's write gate, so
/// writers queue in-process in arrival order without holding a pool
/// connection: reads keep the rest of the pool however slowly the disk
/// commits, and writers of this process never contend for SQLite's lock.
/// `operation` names the write in the gate's stall reports.
impl Repository {
    pub(crate) async fn begin_write(
        &self,
        operation: &'static str,
    ) -> Result<WriteTransaction, DbErr> {
        let gate = self.acquire_write_gate(operation).await;
        let transaction = retry_write_operation(|| {
            self.database.begin_with_options(TransactionOptions {
                sqlite_transaction_mode: Some(SqliteTransactionMode::Immediate),
                ..Default::default()
            })
        })
        .await?;
        Ok(WriteTransaction {
            transaction,
            _gate: gate,
        })
    }

    /// Waits for this process's turn to write.
    ///
    /// Single-statement writes outside [`Self::begin_write`] hold the guard
    /// across their statement for the same ordering. A wait or hold longer
    /// than the gate's stall threshold is reported under `operation`.
    pub(crate) async fn acquire_write_gate(&self, operation: &'static str) -> WriteGateGuard {
        self.write_gate.acquire(operation).await
    }
}

/// A `BEGIN IMMEDIATE` transaction holding the repository's write gate until
/// it commits, rolls back, or drops.
pub(crate) struct WriteTransaction {
    // Declared first so it drops (and rolls back) before the gate opens.
    transaction: DatabaseTransaction,
    _gate: WriteGateGuard,
}

impl WriteTransaction {
    /// Commits the transaction, then opens the gate for the next writer.
    pub(crate) async fn commit(self) -> Result<(), DbErr> {
        self.transaction.commit().await
    }

    /// Rolls the transaction back, then opens the gate for the next writer.
    pub(crate) async fn rollback(self) -> Result<(), DbErr> {
        self.transaction.rollback().await
    }
}

impl std::ops::Deref for WriteTransaction {
    type Target = DatabaseTransaction;

    fn deref(&self) -> &DatabaseTransaction {
        &self.transaction
    }
}

#[async_trait::async_trait]
impl ConnectionTrait for WriteTransaction {
    fn get_database_backend(&self) -> DbBackend {
        self.transaction.get_database_backend()
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        self.transaction.execute_raw(stmt).await
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        self.transaction.execute_unprepared(sql).await
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        self.transaction.query_one_raw(stmt).await
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        self.transaction.query_all_raw(stmt).await
    }
}

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

#[cfg(test)]
mod write_gate_tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use sea_orm::{ConnectionTrait, DbBackend, Statement};

    use super::Repository;
    use crate::{SqliteConfig, connect};

    async fn count_rows(repository: &Repository) -> i64 {
        repository
            .database
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT count(*) FROM gate_rows",
            ))
            .await
            .expect("count query runs")
            .expect("count query returns a row")
            .try_get_by_index::<i64>(0)
            .expect("count is an integer")
    }

    /// A writer waiting its turn holds no pool connection, so a read still
    /// gets one while another write is open. Two connections make the
    /// difference observable: without the gate the waiting writer would sit
    /// on the second connection inside SQLite's `busy_timeout`.
    #[tokio::test]
    async fn a_held_write_queues_writers_without_starving_reads() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("artisan-write-gate-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let database = connect(
            SqliteConfig::file(dir.join("gate.sqlite3"))
                .max_connections(2)
                .sqlx_logging(false),
        )
        .await
        .expect("file database opens");
        database
            .execute_unprepared("CREATE TABLE gate_rows (value INTEGER NOT NULL)")
            .await
            .expect("table");
        let repository = Repository::new(database);

        let held = repository
            .begin_write("test write")
            .await
            .expect("first writer");
        held.execute_unprepared("INSERT INTO gate_rows VALUES (1)")
            .await
            .expect("first insert");
        let second = {
            let repository = repository.clone();
            tokio::spawn(async move {
                let transaction = repository.begin_write("test write").await?;
                transaction
                    .execute_unprepared("INSERT INTO gate_rows VALUES (2)")
                    .await?;
                transaction.commit().await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!second.is_finished(), "the second writer waits its turn");

        let during = tokio::time::timeout(Duration::from_secs(2), count_rows(&repository))
            .await
            .expect("a read is never queued behind writers");
        assert_eq!(during, 0, "the open write is not visible yet");

        held.commit().await.expect("first commit");
        second
            .await
            .expect("second writer joins")
            .expect("second writer commits after the first");
        assert_eq!(count_rows(&repository).await, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The gate names the write holding it, so a writer stuck behind a slow
    /// disk can report what it waits for, and forgets it once it lets go.
    #[tokio::test]
    async fn the_gate_names_its_holder_until_it_lets_go() {
        let repository = Repository::new(
            connect(SqliteConfig::in_memory().sqlx_logging(false))
                .await
                .expect("memory database opens"),
        );
        assert_eq!(repository.write_gate.holder(), None);
        let held = repository.acquire_write_gate("first write").await;
        assert_eq!(repository.write_gate.holder(), Some("first write"));
        let waiting = {
            let repository = repository.clone();
            tokio::spawn(async move {
                let _turn = repository.acquire_write_gate("second write").await;
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "the second write waits its turn");
        assert_eq!(repository.write_gate.holder(), Some("first write"));
        drop(held);
        waiting.await.expect("second write takes its turn");
        assert_eq!(repository.write_gate.holder(), None);
    }
}
