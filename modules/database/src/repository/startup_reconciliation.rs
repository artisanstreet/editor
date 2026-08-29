//! Read-only discovery of expired, non-terminal run/dispatch pairs that a
//! later atomic startup-reconciliation disposition can fence.
//!
//! The query is bounded in time and cardinality, selects only
//! `message_dispatches.state = 'running'` paired with
//! `assistant_runs.lifecycle IN ('launching','running','waiting','cancel_requested')`
//! through the exact origin message identity, and only those whose
//! `lease_expires_at_ms <= operated_at`. No writes are performed, no provider
//! is contacted, and no lease expiry is interpreted as process death.

use artisan_domain::{ItemId, MessageId, RunId, ThreadId, TurnId, UnixMillis};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use thiserror::Error;

use super::{Repository, RepositoryError, corrupt_data, database_error};

const MIN_LIMIT: usize = 1;
const MAX_LIMIT: usize = 64;

const DISCOVERY_SQL: &str = r"
SELECT
  ar.run_id AS run_id,
  ar.thread_id AS thread_id,
  ar.origin_message_id AS origin_message_id,
  ar.origin_turn_id AS origin_turn_id,
  ar.generation AS generation,
  ar.lifecycle AS lifecycle,
  ar.updated_at_ms AS run_updated_at_ms,
  md.lease_expires_at_ms AS lease_expires_at_ms,
  md.updated_at_ms AS dispatch_updated_at_ms,
  (SELECT COUNT(*) FROM conversation_items ci2 WHERE ci2.run_id = ar.run_id AND ci2.item_kind = 'assistant_message') AS item_count,
  (SELECT ci3.item_id FROM conversation_items ci3 WHERE ci3.run_id = ar.run_id AND ci3.item_kind = 'assistant_message' LIMIT 1) AS assistant_item_id,
  (SELECT ci3.thread_id FROM conversation_items ci3 WHERE ci3.run_id = ar.run_id AND ci3.item_kind = 'assistant_message' LIMIT 1) AS assistant_item_thread_id,
  (SELECT ci3.turn_id FROM conversation_items ci3 WHERE ci3.run_id = ar.run_id AND ci3.item_kind = 'assistant_message' LIMIT 1) AS assistant_item_turn_id
FROM assistant_runs ar
JOIN message_dispatches md ON md.message_id = ar.origin_message_id
WHERE md.state = 'running'
  AND ar.lifecycle IN ('launching','running','waiting','cancel_requested')
  AND md.lease_expires_at_ms IS NOT NULL
  AND md.lease_expires_at_ms <= ?
ORDER BY md.lease_expires_at_ms ASC, ar.run_id ASC
LIMIT ?
";

/// Bounded query for startup reconciliation candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupReconciliationQuery {
    /// Time against which `lease_expires_at_ms <= operated_at` is evaluated.
    pub operated_at: UnixMillis,
    /// Maximum rows to return; validated `1..=64`.
    pub limit: usize,
}

impl StartupReconciliationQuery {
    /// Creates a validated query.
    ///
    /// # Errors
    ///
    /// Returns [`StartupReconciliationError::InvalidLimit`] when `limit` is
    /// outside `1..=64`.
    pub fn new(operated_at: UnixMillis, limit: usize) -> Result<Self, StartupReconciliationError> {
        if !(MIN_LIMIT..=MAX_LIMIT).contains(&limit) {
            return Err(StartupReconciliationError::InvalidLimit { limit });
        }
        Ok(Self { operated_at, limit })
    }
}

/// Non-terminal lifecycle values eligible for startup reconciliation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StartupRunLifecycle {
    /// `launching` — spawn outcome unknown, no assistant item expected.
    Launching,
    /// `running` — provider bound, streaming may have started.
    Running,
    /// `waiting` — provider waiting on tool.
    Waiting,
    /// `cancel_requested` — cancellation requested but not yet settled.
    CancelRequested,
}

impl StartupRunLifecycle {
    fn as_str(self) -> &'static str {
        match self {
            Self::Launching => "launching",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::CancelRequested => "cancel_requested",
        }
    }

    fn parse(value: &str) -> Result<Self, StartupReconciliationError> {
        match value {
            "launching" => Ok(Self::Launching),
            "running" => Ok(Self::Running),
            "waiting" => Ok(Self::Waiting),
            "cancel_requested" => Ok(Self::CancelRequested),
            _ => Err(StartupReconciliationError::Repository(corrupt_data(
                "assistant_runs",
                "lifecycle",
                &format!("unexpected lifecycle `{value}`"),
            ))),
        }
    }
}

impl std::fmt::Display for StartupRunLifecycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Durable facts of one expired non-terminal run/dispatch pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupReconciliationCandidate {
    /// Assistant run identity.
    pub run_id: RunId,
    /// Thread owning the run.
    pub thread_id: ThreadId,
    /// Origin message identity.
    pub message_id: MessageId,
    /// Origin turn identity.
    pub turn_id: TurnId,
    /// Positive generation of the run.
    pub generation: i64,
    /// Non-terminal lifecycle at discovery time.
    pub lifecycle: StartupRunLifecycle,
    /// Dispatch lease expiry at discovery time.
    pub lease_expires_at: UnixMillis,
    /// Current `assistant_runs.updated_at_ms` at discovery time.
    pub run_updated_at: UnixMillis,
    /// Current `message_dispatches.updated_at_ms` at discovery time.
    pub dispatch_updated_at: UnixMillis,
    /// At most one assistant message item for the run; `None` is valid for
    /// launching / before first batch.
    pub assistant_item_id: Option<ItemId>,
}

/// Bounded, ordered result collection.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StartupReconciliationCandidates(Vec<StartupReconciliationCandidate>);

impl StartupReconciliationCandidates {
    /// Returns the candidates in discovery order.
    #[must_use]
    pub fn into_vec(self) -> Vec<StartupReconciliationCandidate> {
        self.0
    }

    /// Returns the number of candidates.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` when no candidates were found.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterates over the candidates.
    pub fn iter(&self) -> std::slice::Iter<'_, StartupReconciliationCandidate> {
        self.0.iter()
    }
}

impl std::ops::Deref for StartupReconciliationCandidates {
    type Target = [StartupReconciliationCandidate];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl IntoIterator for StartupReconciliationCandidates {
    type Item = StartupReconciliationCandidate;
    type IntoIter = std::vec::IntoIter<StartupReconciliationCandidate>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a StartupReconciliationCandidates {
    type Item = &'a StartupReconciliationCandidate;
    type IntoIter = std::slice::Iter<'a, StartupReconciliationCandidate>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Typed failures of candidate discovery.
///
/// No variant carries raw SQL, owner/lease/claim bytes, provider-binding
/// blobs, prompt text, or credential material.
#[derive(Debug, Error)]
pub enum StartupReconciliationError {
    /// The supplied `limit` is outside `1..=64`.
    #[error("startup reconciliation limit {limit} must be between 1 and 64")]
    InvalidLimit {
        /// Offending limit.
        limit: usize,
    },

    /// An existing repository failure surfaced.
    #[error(transparent)]
    Repository(#[from] RepositoryError),
}

fn parse_identifier<T, F>(
    table: &'static str,
    field: &'static str,
    value: &str,
    parser: F,
) -> Result<T, StartupReconciliationError>
where
    F: FnOnce(&str) -> Result<T, artisan_domain::IdentifierError>,
{
    parser(value)
        .map_err(|error| StartupReconciliationError::Repository(corrupt_data(table, field, &error)))
}

impl Repository {
    /// Lists expired, non-terminal run/dispatch pairs that a later atomic
    /// startup-reconciliation disposition can fence. The call is read-only and
    /// never mutates any row, counter, patch, or receipt.
    ///
    /// Only `message_dispatches.state = 'running'` joined to
    /// `assistant_runs.lifecycle IN ('launching','running','waiting','cancel_requested')`
    /// via `origin_message_id = message_id` is considered. Only rows whose
    /// non-null `lease_expires_at_ms <= operated_at` are returned, in
    /// deterministic `lease_expires_at ASC, run_id ASC` order, limited by the
    /// validated query limit. The per-run assistant-message count is evaluated
    /// before the candidate limit is applied; more than one assistant message
    /// for a run is a typed corruption error even at limit `1`. When one item
    /// exists its `thread_id`/`turn_id` must agree with the run's
    /// `thread_id`/`origin_turn_id`. Any persisted identity string that cannot
    /// be parsed into its domain type, or a non-positive generation, fails
    /// closed with a typed corruption error.
    ///
    /// # Errors
    ///
    /// Returns [`StartupReconciliationError::InvalidLimit`] for an out-of-range
    /// limit and [`StartupReconciliationError::Repository`] for `CorruptData`,
    /// `Invariant`, or `Database` failures. No raw SQL or secret material is
    /// exposed.
    #[allow(clippy::too_many_lines)]
    pub async fn list_startup_reconciliation_candidates(
        &self,
        query: StartupReconciliationQuery,
    ) -> Result<StartupReconciliationCandidates, StartupReconciliationError> {
        if !(MIN_LIMIT..=MAX_LIMIT).contains(&query.limit) {
            return Err(StartupReconciliationError::InvalidLimit { limit: query.limit });
        }

        let operated_at_ms = query.operated_at.as_millis();
        let limit_i64 = i64::try_from(query.limit).unwrap_or(64);

        let statement = Statement::from_sql_and_values(
            DbBackend::Sqlite,
            DISCOVERY_SQL,
            [operated_at_ms.into(), limit_i64.into()],
        );

        let rows = self
            .database
            .query_all_raw(statement)
            .await
            .map_err(|source| {
                StartupReconciliationError::Repository(database_error(
                    "list startup reconciliation candidates",
                    source,
                ))
            })?;

        let mut candidates: Vec<StartupReconciliationCandidate> = Vec::with_capacity(rows.len());

        for row in &rows {
            let run_id_raw: String = row.try_get_by_index::<String>(0).map_err(|e| {
                StartupReconciliationError::Repository(corrupt_data("assistant_runs", "run_id", &e))
            })?;
            let thread_id_raw: String = row.try_get_by_index::<String>(1).map_err(|e| {
                StartupReconciliationError::Repository(corrupt_data(
                    "assistant_runs",
                    "thread_id",
                    &e,
                ))
            })?;
            let message_id_raw: String = row.try_get_by_index::<String>(2).map_err(|e| {
                StartupReconciliationError::Repository(corrupt_data(
                    "assistant_runs",
                    "origin_message_id",
                    &e,
                ))
            })?;
            let turn_id_raw: String = row.try_get_by_index::<String>(3).map_err(|e| {
                StartupReconciliationError::Repository(corrupt_data(
                    "assistant_runs",
                    "origin_turn_id",
                    &e,
                ))
            })?;
            let generation: i64 = row.try_get_by_index::<i64>(4).map_err(|e| {
                StartupReconciliationError::Repository(corrupt_data(
                    "assistant_runs",
                    "generation",
                    &e,
                ))
            })?;
            let lifecycle_raw: String = row.try_get_by_index::<String>(5).map_err(|e| {
                StartupReconciliationError::Repository(corrupt_data(
                    "assistant_runs",
                    "lifecycle",
                    &e,
                ))
            })?;
            let run_updated_at_ms: i64 = row.try_get_by_index::<i64>(6).map_err(|e| {
                StartupReconciliationError::Repository(corrupt_data(
                    "assistant_runs",
                    "updated_at_ms",
                    &e,
                ))
            })?;
            let lease_expires_at_ms: i64 = row.try_get_by_index::<i64>(7).map_err(|e| {
                StartupReconciliationError::Repository(corrupt_data(
                    "message_dispatches",
                    "lease_expires_at_ms",
                    &e,
                ))
            })?;
            let dispatch_updated_at_ms: i64 = row.try_get_by_index::<i64>(8).map_err(|e| {
                StartupReconciliationError::Repository(corrupt_data(
                    "message_dispatches",
                    "updated_at_ms",
                    &e,
                ))
            })?;
            let item_count: i64 = row.try_get_by_index::<i64>(9).map_err(|e| {
                StartupReconciliationError::Repository(corrupt_data(
                    "conversation_items",
                    "run_id",
                    &e,
                ))
            })?;
            let assistant_item_raw: Option<String> =
                row.try_get_by_index::<Option<String>>(10).map_err(|e| {
                    StartupReconciliationError::Repository(corrupt_data(
                        "conversation_items",
                        "item_id",
                        &e,
                    ))
                })?;
            let assistant_item_thread_raw: Option<String> =
                row.try_get_by_index::<Option<String>>(11).map_err(|e| {
                    StartupReconciliationError::Repository(corrupt_data(
                        "conversation_items",
                        "thread_id",
                        &e,
                    ))
                })?;
            let assistant_item_turn_raw: Option<String> =
                row.try_get_by_index::<Option<String>>(12).map_err(|e| {
                    StartupReconciliationError::Repository(corrupt_data(
                        "conversation_items",
                        "turn_id",
                        &e,
                    ))
                })?;

            // Per-run assistant-message count is evaluated before the candidate limit;
            // more than one item is corruption even at limit 1. Never select an arbitrary item.
            if item_count > 1 {
                return Err(StartupReconciliationError::Repository(corrupt_data(
                    "conversation_items",
                    "run_id",
                    "duplicate assistant message item for one run",
                )));
            }
            if item_count < 0 {
                return Err(StartupReconciliationError::Repository(corrupt_data(
                    "conversation_items",
                    "run_id",
                    "negative assistant item count",
                )));
            }

            // Fail closed when selected persisted values cannot form valid domain types.
            let run_id = parse_identifier("assistant_runs", "run_id", &run_id_raw, |v| {
                RunId::parse(v.to_owned())
            })?;
            let thread_id = parse_identifier("assistant_runs", "thread_id", &thread_id_raw, |v| {
                ThreadId::parse(v.to_owned())
            })?;
            let message_id = parse_identifier(
                "assistant_runs",
                "origin_message_id",
                &message_id_raw,
                |v| MessageId::parse(v.to_owned()),
            )?;
            let turn_id =
                parse_identifier("assistant_runs", "origin_turn_id", &turn_id_raw, |v| {
                    TurnId::parse(v.to_owned())
                })?;
            if generation <= 0 {
                return Err(StartupReconciliationError::Repository(corrupt_data(
                    "assistant_runs",
                    "generation",
                    &format!("generation {generation} must be positive"),
                )));
            }
            let lifecycle = StartupRunLifecycle::parse(&lifecycle_raw)?;

            let assistant_item_id = if item_count == 0 {
                if assistant_item_raw.is_some()
                    || assistant_item_thread_raw.is_some()
                    || assistant_item_turn_raw.is_some()
                {
                    return Err(StartupReconciliationError::Repository(corrupt_data(
                        "conversation_items",
                        "item_id",
                        "assistant item count 0 but item present",
                    )));
                }
                None
            } else {
                // item_count == 1
                let raw = assistant_item_raw.ok_or_else(|| {
                    StartupReconciliationError::Repository(corrupt_data(
                        "conversation_items",
                        "item_id",
                        "assistant item count 1 but item_id is null",
                    ))
                })?;
                let item_thread_raw = assistant_item_thread_raw.ok_or_else(|| {
                    StartupReconciliationError::Repository(corrupt_data(
                        "conversation_items",
                        "thread_id",
                        "assistant item thread_id is null",
                    ))
                })?;
                let item_turn_raw = assistant_item_turn_raw.ok_or_else(|| {
                    StartupReconciliationError::Repository(corrupt_data(
                        "conversation_items",
                        "turn_id",
                        "assistant item turn_id is null",
                    ))
                })?;
                // Validate that the item's thread/turn agree with the run's identities.
                // Do not hide a mismatched item by returning None.
                if item_thread_raw != thread_id_raw {
                    return Err(StartupReconciliationError::Repository(corrupt_data(
                        "conversation_items",
                        "thread_id",
                        &format!(
                            "assistant item thread `{item_thread_raw}` disagrees with run thread `{thread_id_raw}`"
                        ),
                    )));
                }
                if item_turn_raw != turn_id_raw {
                    return Err(StartupReconciliationError::Repository(corrupt_data(
                        "conversation_items",
                        "turn_id",
                        &format!(
                            "assistant item turn `{item_turn_raw}` disagrees with run origin turn `{turn_id_raw}`"
                        ),
                    )));
                }
                Some(parse_identifier(
                    "conversation_items",
                    "item_id",
                    &raw,
                    |v| ItemId::parse(v.to_owned()),
                )?)
            };

            candidates.push(StartupReconciliationCandidate {
                run_id,
                thread_id,
                message_id,
                turn_id,
                generation,
                lifecycle,
                lease_expires_at: UnixMillis::from_millis(lease_expires_at_ms),
                run_updated_at: UnixMillis::from_millis(run_updated_at_ms),
                dispatch_updated_at: UnixMillis::from_millis(dispatch_updated_at_ms),
                assistant_item_id,
            });
        }

        Ok(StartupReconciliationCandidates(candidates))
    }
}
