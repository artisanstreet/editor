//! Durable observation-history reads over the append-only ledger.
//!
//! [`Repository::read_observation_history`] replays one thread's committed
//! observations in delivery-sequence order after a subscriber cursor. Rows
//! are written atomically by the batch commit path in the sibling
//! `run_observation` module; the transaction helpers below (`LedgerInsert`,
//! [`allocate_delivery_base`], [`insert_rows`]) are that path's only write
//! seam. Payloads decode through the existing canonical batch codec, so no
//! JSON vocabulary is duplicated here.

use artisan_domain::{
    EngineObservationAttribution, EngineObservationEvent, RunId, ThreadId, TurnId, UnixMillis,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set};

use crate::entities::{OpaqueBytes, observation_ledger};

use super::Repository;
use super::RepositoryError;
use super::corrupt_data;
use super::database_error;
use super::run_observation::{
    OBSERVATION_BATCH_MAX_OBSERVATIONS, RunObservationError, decode_observation_checkpoint,
};

/// One validated ledger row staged for the open batch transaction.
///
/// The producing run, Forge turn, and commit instant come from the batch
/// command scope; the delivery sequence is allocated in-transaction by
/// [`allocate_delivery_base`]; the payload is the canonical
/// single-observation envelope produced by the existing batch codec.
pub(super) struct LedgerInsert {
    pub(super) thread_id: String,
    pub(super) delivery_sequence: i64,
    pub(super) run_id: String,
    pub(super) observation_sequence: i64,
    pub(super) turn_id: String,
    pub(super) committed_at_ms: i64,
    pub(super) engine: String,
    pub(super) binding_version: i64,
    pub(super) observation_version: i64,
    pub(super) observation_bytes: Vec<u8>,
}

/// Allocates the first free delivery sequence for `count` rows on one thread.
///
/// Returns `MAX(delivery_sequence) + 1` read through the caller's open
/// transaction, so concurrent commit transactions serialize on the write
/// lock and sequences stay strictly increasing across runs. The first
/// sequence on a thread is `1`.
///
/// # Errors
///
/// Returns [`RunObservationError`] when the persisted maximum is negative
/// or the allocation overflows the signed counter range.
pub(super) async fn allocate_delivery_base(
    transaction: &sea_orm::DatabaseTransaction,
    thread_id: &str,
    count: usize,
) -> Result<i64, RunObservationError> {
    let additional = i64::try_from(count).map_err(|_| RunObservationError::CounterOverflow {
        counter: "delivery sequence",
        value: i64::MAX,
    })?;
    let latest = observation_ledger::Entity::find()
        .filter(observation_ledger::Column::ThreadId.eq(thread_id))
        .order_by_desc(observation_ledger::Column::DeliverySequence)
        .one(transaction)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("allocate delivery sequence", source))
        })?;
    let maximum = match latest {
        None => 0,
        Some(row) => {
            if row.delivery_sequence < 0 {
                return Err(RunObservationError::Repository(corrupt_data(
                    "observation_ledger",
                    "delivery_sequence",
                    "counter is negative",
                )));
            }
            row.delivery_sequence
        }
    };
    maximum
        .checked_add(additional)
        .ok_or(RunObservationError::CounterOverflow {
            counter: "delivery sequence",
            value: maximum,
        })?;
    maximum
        .checked_add(1)
        .ok_or(RunObservationError::CounterOverflow {
            counter: "delivery sequence",
            value: maximum,
        })
}

/// Inserts every staged ledger row through the caller's open transaction.
///
/// The caller commits once or rolls everything back; a duplicate
/// `(run_id, observation_sequence)` fails here and rolls back the batch
/// checkpoint and receipt with it.
///
/// # Errors
///
/// Returns [`RunObservationError`] when a row violates the ledger schema.
pub(super) async fn insert_rows(
    transaction: &sea_orm::DatabaseTransaction,
    rows: &[LedgerInsert],
) -> Result<(), RunObservationError> {
    for row in rows {
        observation_ledger::Entity::insert(observation_ledger::ActiveModel {
            thread_id: Set(row.thread_id.clone()),
            delivery_sequence: Set(row.delivery_sequence),
            run_id: Set(row.run_id.clone()),
            observation_sequence: Set(row.observation_sequence),
            turn_id: Set(row.turn_id.clone()),
            committed_at_ms: Set(row.committed_at_ms),
            engine: Set(row.engine.clone()),
            binding_version: Set(row.binding_version),
            observation_version: Set(row.observation_version),
            observation_bytes: Set(OpaqueBytes::new(row.observation_bytes.clone())),
        })
        .exec(transaction)
        .await
        .map_err(|source| {
            RunObservationError::Repository(database_error("insert observation ledger row", source))
        })?;
    }
    Ok(())
}

impl Repository {
    /// Reads one thread's committed observations after a delivery cursor.
    ///
    /// Returns at most `limit` attributed events with delivery sequences
    /// strictly greater than `after_sequence`, in ascending delivery order.
    /// One page never exceeds [`OBSERVATION_BATCH_MAX_OBSERVATIONS`]
    /// events: an oversized `limit` is capped, so an unbounded request can
    /// never pin unbounded history in memory; callers paginate with the last
    /// returned delivery sequence. A `limit` of zero returns no rows; a
    /// cursor beyond the signed range matches nothing. Every returned event
    /// carries `Some` attribution with the producing run, its Forge turn,
    /// the caller-injected commit instant, and the row's delivery sequence.
    /// Payloads are the immutable original typed observations decoded
    /// through the canonical batch codec.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError`] when a row fails domain validation, a
    /// delivery sequence is not positive, a payload is not its canonical
    /// single-observation envelope, or a database query fails.
    pub async fn read_observation_history(
        &self,
        thread_id: &ThreadId,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<EngineObservationEvent>, RepositoryError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Ok(after) = i64::try_from(after_sequence) else {
            return Ok(Vec::new());
        };
        let bounded = limit.min(OBSERVATION_BATCH_MAX_OBSERVATIONS);
        let take = u64::try_from(bounded).map_err(|_| RepositoryError::Invariant {
            reason: "observation history limit is not representable",
        })?;
        let rows = observation_ledger::Entity::find()
            .filter(observation_ledger::Column::ThreadId.eq(thread_id.as_str()))
            .filter(observation_ledger::Column::DeliverySequence.gt(after))
            .order_by_asc(observation_ledger::Column::DeliverySequence)
            .limit(take)
            .all(&self.database)
            .await
            .map_err(|source| database_error("read observation history", source))?;
        rows.iter().map(ledger_event).collect()
    }
}

/// Rebuilds one attributed delivery event from its immutable ledger row.
fn ledger_event(row: &observation_ledger::Model) -> Result<EngineObservationEvent, RepositoryError> {
    let thread_id = ThreadId::parse(row.thread_id.clone())
        .map_err(|error| corrupt_data("observation_ledger", "thread_id", error))?;
    let run_id = RunId::parse(row.run_id.clone())
        .map_err(|error| corrupt_data("observation_ledger", "run_id", error))?;
    let turn_id = TurnId::parse(row.turn_id.clone())
        .map_err(|error| corrupt_data("observation_ledger", "turn_id", error))?;
    if row.delivery_sequence <= 0 {
        return Err(corrupt_data(
            "observation_ledger",
            "delivery_sequence",
            "delivery sequence must be positive",
        ));
    }
    let delivery_sequence =
        u64::try_from(row.delivery_sequence).map_err(|_| RepositoryError::Invariant {
            reason: "observation delivery sequence is not representable",
        })?;
    let decoded =
        decode_observation_checkpoint(row.observation_version, row.observation_bytes.as_slice())
            .map_err(|source| {
                corrupt_data("observation_ledger", "observation_bytes", source)
            })?;
    if decoded.engine().as_str() != row.engine
        || decoded.binding_version() != row.binding_version
    {
        return Err(corrupt_data(
            "observation_ledger",
            "observation_bytes",
            "payload envelope disagrees with its ledger columns",
        ));
    }
    let [observation] = decoded.observations().as_slice() else {
        return Err(corrupt_data(
            "observation_ledger",
            "observation_bytes",
            "ledger row must carry exactly one observation",
        ));
    };
    let persisted_sequence =
        i64::try_from(observation.sequence().get()).map_err(|_| RepositoryError::Invariant {
            reason: "observation sequence is not representable",
        })?;
    if persisted_sequence != row.observation_sequence {
        return Err(corrupt_data(
            "observation_ledger",
            "observation_sequence",
            "payload sequence disagrees with its ledger column",
        ));
    }
    Ok(EngineObservationEvent {
        thread_id,
        observation: observation.clone(),
        attribution: Some(EngineObservationAttribution {
            run_id,
            turn_id,
            committed_at: UnixMillis::from_millis(row.committed_at_ms),
            delivery_sequence,
        }),
    })
}
