//! Bounded queued and failed message listings and their row mapping.

#![forbid(unsafe_code)]

use sea_orm::{ConnectionTrait, DbBackend, QueryResult, Statement, TransactionTrait, Value};

use artisan_domain::{
    AuthoredText, DispatchError, FAILED_MESSAGE_LIST_MAX, FailedMessageListError,
    FailedMessageListing, FailedMessageSummary, ListFailedMessages, ListQueuedMessages,
    QUEUED_MESSAGE_LIST_MAX, QueuedMessageListError, QueuedMessageListOrder, QueuedMessageListing,
    QueuedMessageState, QueuedMessageSummary, ThreadId, UnixMillis,
};

use crate::repository::thread_engine_config::read_receipt_settings_in;
use crate::repository::{Repository, corrupt_data, database_error, row_value};

use super::QueuedMessageRepositoryError;
use super::rows::{
    authored_text, ensure_thread, parse_message_id, parse_request_id, parse_thread_id,
    read_image_refs,
};
const LIST_COUNT_SQL: &str = r"
SELECT COUNT(*)
FROM messages AS m
JOIN message_dispatches AS d ON d.message_id = m.message_id
JOIN command_receipts AS r ON r.request_id = d.correlation_id
WHERE m.thread_id = ?
  AND r.command_kind = 'queue_message'
  AND r.thread_id = m.thread_id
  AND r.message_id = m.message_id
  AND d.state IN ('queued', 'leased')
  AND NOT EXISTS (
      SELECT 1
      FROM queued_message_withdrawals AS w
      WHERE w.thread_id = m.thread_id
        AND w.message_id = d.message_id
        AND w.original_request_id = d.correlation_id
        AND w.outcome = 'withdrawn'
  )
";

const LIST_OLDEST_SQL: &str = r"
SELECT m.message_id,
       m.thread_id,
       m.body,
       d.correlation_id,
       r.body,
       m.accepted_at_ms,
       d.queued_at_ms,
       r.accepted_at_ms,
       d.last_error,
       d.state
FROM messages AS m
JOIN message_dispatches AS d ON d.message_id = m.message_id
JOIN command_receipts AS r ON r.request_id = d.correlation_id
WHERE m.thread_id = ?
  AND r.command_kind = 'queue_message'
  AND r.thread_id = m.thread_id
  AND r.message_id = m.message_id
  AND d.state IN ('queued', 'leased')
  AND NOT EXISTS (
       SELECT 1
       FROM queued_message_withdrawals AS w
       WHERE w.thread_id = m.thread_id
         AND w.message_id = d.message_id
         AND w.original_request_id = d.correlation_id
         AND w.outcome = 'withdrawn'
  )
ORDER BY d.queued_at_ms ASC, d.message_id ASC
LIMIT ?
";

const LIST_LATEST_SQL: &str = r"
SELECT m.message_id,
       m.thread_id,
       m.body,
       d.correlation_id,
       r.body,
       m.accepted_at_ms,
       d.queued_at_ms,
       r.accepted_at_ms,
       d.last_error,
       d.state
FROM messages AS m
JOIN message_dispatches AS d ON d.message_id = m.message_id
JOIN command_receipts AS r ON r.request_id = d.correlation_id
WHERE m.thread_id = ?
  AND r.command_kind = 'queue_message'
  AND r.thread_id = m.thread_id
  AND r.message_id = m.message_id
  AND d.state IN ('queued', 'leased')
  AND NOT EXISTS (
       SELECT 1
       FROM queued_message_withdrawals AS w
       WHERE w.thread_id = m.thread_id
         AND w.message_id = d.message_id
         AND w.original_request_id = d.correlation_id
         AND w.outcome = 'withdrawn'
  )
ORDER BY d.queued_at_ms DESC, d.message_id DESC
LIMIT ?
";

const FAILED_LIST_COUNT_SQL: &str = r"
SELECT COUNT(*)
FROM messages AS m
JOIN message_dispatches AS d ON d.message_id = m.message_id
JOIN command_receipts AS r ON r.request_id = d.correlation_id
WHERE m.thread_id = ?
  AND r.command_kind = 'queue_message'
  AND r.thread_id = m.thread_id
  AND r.message_id = m.message_id
  AND d.state = 'failed'
  AND d.last_error IS NOT NULL
  AND NOT EXISTS (
      SELECT 1
      FROM queued_message_withdrawals AS w
      WHERE w.thread_id = m.thread_id
        AND w.message_id = d.message_id
        AND w.original_request_id = d.correlation_id
        AND w.outcome = 'withdrawn'
  )
  AND NOT EXISTS (
      SELECT 1 FROM failed_message_recoveries AS fr WHERE fr.message_id = d.message_id
  )
  AND NOT EXISTS (
      SELECT 1
      FROM messages AS later
      JOIN message_dispatches AS ld ON ld.message_id = later.message_id
      WHERE later.thread_id = m.thread_id
        AND later.accepted_at_ms > m.accepted_at_ms
        AND ld.state IN ('running', 'completed')
  )
";

const FAILED_LIST_SQL: &str = r"
SELECT m.message_id,
       m.thread_id,
       m.body,
       d.correlation_id,
       r.body,
       m.accepted_at_ms,
       d.updated_at_ms,
       r.accepted_at_ms,
       d.last_error,
       NOT EXISTS (
           SELECT 1 FROM conversation_items AS i WHERE i.source_message_id = d.message_id
       )
FROM messages AS m
JOIN message_dispatches AS d ON d.message_id = m.message_id
JOIN command_receipts AS r ON r.request_id = d.correlation_id
WHERE m.thread_id = ?
  AND r.command_kind = 'queue_message'
  AND r.thread_id = m.thread_id
  AND r.message_id = m.message_id
  AND d.state = 'failed'
  AND d.last_error IS NOT NULL
  AND NOT EXISTS (
      SELECT 1
      FROM queued_message_withdrawals AS w
      WHERE w.thread_id = m.thread_id
        AND w.message_id = d.message_id
        AND w.original_request_id = d.correlation_id
        AND w.outcome = 'withdrawn'
  )
  AND NOT EXISTS (
      SELECT 1 FROM failed_message_recoveries AS fr WHERE fr.message_id = d.message_id
  )
  AND NOT EXISTS (
      SELECT 1
      FROM messages AS later
      JOIN message_dispatches AS ld ON ld.message_id = later.message_id
      WHERE later.thread_id = m.thread_id
        AND later.accepted_at_ms > m.accepted_at_ms
        AND ld.state IN ('running', 'completed')
  )
ORDER BY d.updated_at_ms DESC, d.message_id DESC
LIMIT ?
";

impl Repository {
    /// Reads one bounded page of accepted general messages that have not
    /// reached the transcript yet, for exactly one existing thread.
    ///
    /// Rows are ordered by `(queued_at_ms, message_id)` in the requested
    /// direction. Only `queue_message` receipts paired with a `queued` or
    /// `leased` dispatch are eligible, each reported with its Forge-owned
    /// state ([`QueuedMessageState::Queued`] or
    /// [`QueuedMessageState::Dispatching`]); running, completed, failed, and
    /// withdrawn rows are excluded. Launch moves a dispatch to `running` in
    /// the same transaction that projects its transcript item, so a message
    /// is always in exactly one of this listing, the transcript, or the
    /// failed listing. A dispatcher requeue returns its claim to `queued`
    /// with a persisted `last_error`, which is the reason the Forge is
    /// holding the row. The
    /// result contains authored text and byte-free image references, never
    /// image bytes. `total_count` and `has_more` are derived from the same
    /// eligibility predicate as the page query in one read transaction.
    ///
    /// # Errors
    ///
    /// Returns a typed missing-thread, limit, corruption, or database error.
    pub async fn read_queued_messages(
        &self,
        query: ListQueuedMessages,
    ) -> Result<QueuedMessageListing, QueuedMessageRepositoryError> {
        validate_limit(query.limit)?;
        let transaction = self
            .database
            .begin()
            .await
            .map_err(|source| database_error("begin queued-message listing", source))?;
        if let Err(error) = ensure_thread(&transaction, &query.thread_id).await {
            transaction
                .rollback()
                .await
                .map_err(|source| database_error("rollback queued-message listing", source))?;
            return Err(error);
        }

        let result = Self::read_queued_message_page(&transaction, &query).await;
        match result {
            Ok(listing) => {
                transaction
                    .commit()
                    .await
                    .map_err(|source| database_error("commit queued-message listing", source))?;
                Ok(listing)
            }
            Err(error) => {
                transaction
                    .rollback()
                    .await
                    .map_err(|source| database_error("rollback queued-message listing", source))?;
                Err(error)
            }
        }
    }

    /// Reads the count and page from one consistent SQLite read transaction.
    async fn read_queued_message_page(
        database: &impl ConnectionTrait,
        query: &ListQueuedMessages,
    ) -> Result<QueuedMessageListing, QueuedMessageRepositoryError> {
        let total_count = read_eligible_count(database, &query.thread_id).await?;
        let limit = sqlite_limit(query.limit)?;
        let sql = match query.order {
            QueuedMessageListOrder::OldestFirst => LIST_OLDEST_SQL,
            QueuedMessageListOrder::LatestFirst => LIST_LATEST_SQL,
        };
        let rows = database
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                sql,
                [
                    Value::String(Some(query.thread_id.as_str().to_owned())),
                    Value::BigInt(Some(limit)),
                ],
            ))
            .await
            .map_err(|source| database_error("read queued-message page", source))?;

        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            messages.push(summary_from_row(database, &query.thread_id, &row).await?);
        }

        QueuedMessageListing::new(
            query.thread_id.clone(),
            query.order,
            query.limit,
            total_count,
            messages,
        )
        .map_err(QueuedMessageRepositoryError::InvalidListing)
    }

    /// Alias with the verb used by callers that treat the result as a list.
    ///
    /// # Errors
    ///
    /// Returns the same bounded-list, ownership, corruption, and database
    /// errors as [`Self::read_queued_messages`].
    pub async fn list_queued_messages(
        &self,
        query: ListQueuedMessages,
    ) -> Result<QueuedMessageListing, QueuedMessageRepositoryError> {
        self.read_queued_messages(query).await
    }

    /// Reads one bounded page of terminally failed dispatches for exactly one
    /// existing thread, newest failures first.
    ///
    /// The dispatcher never claims a failed row by itself; only an explicit
    /// retry requeues it. Withdrawn and recovered rows are excluded, and so
    /// is a failure superseded by a later message of the thread that reached
    /// the transcript. `retryable` is true while the message never reached
    /// the transcript, so its stored payload can be dispatched again. Every
    /// returned row
    /// carries its persisted dispatcher reason verbatim, so the reader sees
    /// exactly why the send cannot proceed on this thread. The result
    /// contains authored text and byte-free image references, never image
    /// bytes; the full payload for recovery travels only through the exact
    /// [`Self::read_failed_message_payload`] seam.
    ///
    /// # Errors
    ///
    /// Returns a typed missing-thread, limit, corruption, or database error.
    pub async fn read_failed_messages(
        &self,
        query: ListFailedMessages,
    ) -> Result<FailedMessageListing, QueuedMessageRepositoryError> {
        validate_failed_limit(query.limit)?;
        let transaction = self
            .database
            .begin()
            .await
            .map_err(|source| database_error("begin failed-message listing", source))?;
        if let Err(error) = ensure_thread(&transaction, &query.thread_id).await {
            transaction
                .rollback()
                .await
                .map_err(|source| database_error("rollback failed-message listing", source))?;
            return Err(error);
        }

        let result = Self::read_failed_message_page(&transaction, &query).await;
        match result {
            Ok(listing) => {
                transaction
                    .commit()
                    .await
                    .map_err(|source| database_error("commit failed-message listing", source))?;
                Ok(listing)
            }
            Err(error) => {
                transaction
                    .rollback()
                    .await
                    .map_err(|source| database_error("rollback failed-message listing", source))?;
                Err(error)
            }
        }
    }

    /// Reads the count and page from one consistent SQLite read transaction.
    async fn read_failed_message_page(
        database: &impl ConnectionTrait,
        query: &ListFailedMessages,
    ) -> Result<FailedMessageListing, QueuedMessageRepositoryError> {
        let total_count = read_failed_count(database, &query.thread_id).await?;
        let limit = sqlite_limit(query.limit)?;
        let rows = database
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                FAILED_LIST_SQL,
                [
                    Value::String(Some(query.thread_id.as_str().to_owned())),
                    Value::BigInt(Some(limit)),
                ],
            ))
            .await
            .map_err(|source| database_error("read failed-message page", source))?;

        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            messages.push(failed_summary_from_row(database, &query.thread_id, &row).await?);
        }

        FailedMessageListing::new(query.thread_id.clone(), query.limit, total_count, messages)
            .map_err(QueuedMessageRepositoryError::InvalidFailedListing)
    }
}

async fn read_eligible_count(
    database: &impl ConnectionTrait,
    thread_id: &ThreadId,
) -> Result<u64, QueuedMessageRepositoryError> {
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            LIST_COUNT_SQL,
            [Value::String(Some(thread_id.as_str().to_owned()))],
        ))
        .await
        .map_err(|source| database_error("count queued messages", source))?
        .ok_or(QueuedMessageRepositoryError::Invariant {
            reason: "queued-message count query returned no row",
        })?;
    let count = row_value::<i64, _>(&row, 0, "count", "message_dispatches")?;
    u64::try_from(count).map_err(|_| {
        corrupt_data(
            "message_dispatches",
            "count",
            "queued-message count is negative",
        )
    })
}

async fn read_failed_count(
    database: &impl ConnectionTrait,
    thread_id: &ThreadId,
) -> Result<u64, QueuedMessageRepositoryError> {
    let row = database
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            FAILED_LIST_COUNT_SQL,
            [Value::String(Some(thread_id.as_str().to_owned()))],
        ))
        .await
        .map_err(|source| database_error("count failed messages", source))?
        .ok_or(QueuedMessageRepositoryError::Invariant {
            reason: "failed-message count query returned no row",
        })?;
    let count = row_value::<i64, _>(&row, 0, "count", "message_dispatches")?;
    u64::try_from(count).map_err(|_| {
        corrupt_data(
            "message_dispatches",
            "count",
            "failed-message count is negative",
        )
    })
}

async fn failed_summary_from_row(
    database: &impl ConnectionTrait,
    requested_thread_id: &ThreadId,
    row: &QueryResult,
) -> Result<FailedMessageSummary, QueuedMessageRepositoryError> {
    let message_id = parse_message_id(row_value(row, 0, "message_id", "messages")?)?;
    let thread_id = parse_thread_id(row_value(row, 1, "thread_id", "messages")?)?;
    if thread_id != *requested_thread_id {
        return Err(corrupt_data(
            "messages",
            "thread_id",
            "failed-message page returned another thread",
        ));
    }
    let message_body = row_value::<String, _>(row, 2, "body", "messages")?;
    let original_request_id =
        parse_request_id(row_value(row, 3, "correlation_id", "message_dispatches")?)?;
    let text = authored_text(row_value::<Option<String>, _>(
        row,
        4,
        "body",
        "command_receipts",
    )?)?;
    if message_body != text.as_ref().map_or("", AuthoredText::as_str) {
        return Err(corrupt_data(
            "command_receipts",
            "body",
            "queue receipt and immutable message text presence disagree",
        ));
    }
    let accepted_at_ms = row_value::<i64, _>(row, 5, "accepted_at_ms", "messages")?;
    let failed_at_ms = row_value::<i64, _>(row, 6, "updated_at_ms", "message_dispatches")?;
    let receipt_accepted_at_ms = row_value::<i64, _>(row, 7, "accepted_at_ms", "command_receipts")?;
    if receipt_accepted_at_ms != accepted_at_ms {
        return Err(corrupt_data(
            "message_dispatches",
            "updated_at_ms",
            "message, dispatch, and receipt acceptance times disagree",
        ));
    }
    if failed_at_ms < accepted_at_ms {
        return Err(corrupt_data(
            "message_dispatches",
            "updated_at_ms",
            "dispatch failure precedes message acceptance",
        ));
    }
    let reason = row_value::<Option<String>, _>(row, 8, "last_error", "message_dispatches")?
        .ok_or_else(|| {
            corrupt_data(
                "message_dispatches",
                "last_error",
                "terminal failure carries no diagnostic",
            )
        })
        .and_then(|reason| {
            DispatchError::parse(reason)
                .map_err(|error| corrupt_data("message_dispatches", "last_error", error))
        })?;
    let retryable = row_value::<bool, _>(row, 9, "retryable", "conversation_items")?;

    let attachments = read_image_refs(database, &message_id, &thread_id).await?;
    if text.as_ref().is_none_or(AuthoredText::is_blank) && attachments.is_empty() {
        return Err(corrupt_data(
            "messages",
            "body",
            "failed general message has neither authored text nor image attachments",
        ));
    }

    Ok(FailedMessageSummary {
        message_id,
        thread_id,
        original_request_id,
        text,
        attachments,
        accepted_at: UnixMillis::from_millis(accepted_at_ms),
        failed_at: UnixMillis::from_millis(failed_at_ms),
        reason,
        retryable,
    })
}

fn validate_failed_limit(limit: usize) -> Result<(), QueuedMessageRepositoryError> {
    if limit == 0 {
        return Err(QueuedMessageRepositoryError::InvalidFailedListLimit(
            FailedMessageListError::Empty,
        ));
    }
    if limit > FAILED_MESSAGE_LIST_MAX {
        return Err(QueuedMessageRepositoryError::InvalidFailedListLimit(
            FailedMessageListError::TooLarge {
                limit,
                maximum: FAILED_MESSAGE_LIST_MAX,
            },
        ));
    }
    Ok(())
}

async fn summary_from_row(
    database: &impl ConnectionTrait,
    requested_thread_id: &ThreadId,
    row: &QueryResult,
) -> Result<QueuedMessageSummary, QueuedMessageRepositoryError> {
    let message_id = parse_message_id(row_value(row, 0, "message_id", "messages")?)?;
    let thread_id = parse_thread_id(row_value(row, 1, "thread_id", "messages")?)?;
    if thread_id != *requested_thread_id {
        return Err(corrupt_data(
            "messages",
            "thread_id",
            "queued-message page returned another thread",
        ));
    }
    let message_body = row_value::<String, _>(row, 2, "body", "messages")?;
    let original_request_id =
        parse_request_id(row_value(row, 3, "correlation_id", "message_dispatches")?)?;
    let text = authored_text(row_value::<Option<String>, _>(
        row,
        4,
        "body",
        "command_receipts",
    )?)?;
    if message_body != text.as_ref().map_or("", AuthoredText::as_str) {
        return Err(corrupt_data(
            "command_receipts",
            "body",
            "queue receipt and immutable message text presence disagree",
        ));
    }
    let accepted_at_ms = row_value::<i64, _>(row, 5, "accepted_at_ms", "messages")?;
    let queued_at_ms = row_value::<i64, _>(row, 6, "queued_at_ms", "message_dispatches")?;
    let receipt_accepted_at_ms = row_value::<i64, _>(row, 7, "accepted_at_ms", "command_receipts")?;
    if queued_at_ms != accepted_at_ms || receipt_accepted_at_ms != accepted_at_ms {
        return Err(corrupt_data(
            "message_dispatches",
            "queued_at_ms",
            "message, dispatch, and receipt acceptance times disagree",
        ));
    }
    let last_error = row_value::<Option<String>, _>(row, 8, "last_error", "message_dispatches")?
        .map(DispatchError::parse)
        .transpose()
        .map_err(|error| corrupt_data("message_dispatches", "last_error", error))?;
    let state = match row_value::<String, _>(row, 9, "state", "message_dispatches")?.as_str() {
        "queued" => QueuedMessageState::Queued,
        "leased" => QueuedMessageState::Dispatching,
        _ => {
            return Err(corrupt_data(
                "message_dispatches",
                "state",
                "queued listing returned a settled dispatch",
            ));
        }
    };
    let engine = read_receipt_settings_in(database, &original_request_id)
        .await
        .map_err(|_| {
            corrupt_data(
                "command_receipts",
                "engine_run_config",
                "queued message configuration snapshot is unreadable",
            )
        })?
        .map(|settings| settings.config().selection().engine_id());

    let attachments = read_image_refs(database, &message_id, &thread_id).await?;
    if text.as_ref().is_none_or(AuthoredText::is_blank) && attachments.is_empty() {
        return Err(corrupt_data(
            "messages",
            "body",
            "queued general message has neither authored text nor image attachments",
        ));
    }

    Ok(QueuedMessageSummary {
        message_id,
        thread_id,
        original_request_id,
        text,
        attachments,
        accepted_at: UnixMillis::from_millis(accepted_at_ms),
        last_error,
        state,
        engine,
    })
}

fn validate_limit(limit: usize) -> Result<(), QueuedMessageRepositoryError> {
    if limit == 0 {
        return Err(QueuedMessageRepositoryError::InvalidListLimit(
            QueuedMessageListError::Empty,
        ));
    }
    if limit > QUEUED_MESSAGE_LIST_MAX {
        return Err(QueuedMessageRepositoryError::InvalidListLimit(
            QueuedMessageListError::TooLarge {
                limit,
                maximum: QUEUED_MESSAGE_LIST_MAX,
            },
        ));
    }
    Ok(())
}

fn sqlite_limit(limit: usize) -> Result<i64, QueuedMessageRepositoryError> {
    i64::try_from(limit).map_err(|_| QueuedMessageRepositoryError::Invariant {
        reason: "queued-message list limit does not fit SQLite",
    })
}
