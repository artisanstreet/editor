//! Paired terminal settlement of a running bound run.
//!
//! `Repository::complete_run` and `Repository::fail_run` co-commit the
//! running dispatch, the running assistant run, the final assistant item, the
//! origin turn, the conversation-state counters, and the two lifecycle patches
//! in one `SeaORM` transaction. Every mutable row is fenced on its full snapshot;
//! a zero-row update rolls the whole transaction back with a typed conflict.
//! An exact replay of an identical command is harmless and answers an explicit
//! `Already*` outcome without mutation.

use artisan_domain::{AssistantMessagePhase, UnixMillis};
use sea_orm::ActiveModelTrait;

use crate::entities::{self, ConversationPatchKind, DispatchState, EntityLifecycle, RenderPhase};

use super::RunBatchScope;
use crate::repository::{RepositoryError, database_error};

mod auxiliary;
mod complete;
mod fail;

pub use self::auxiliary::{
    AuxiliaryTerminalError, CancelRun, CancelRunError, CancelRunOutcome, InterruptRun,
    InterruptRunError, InterruptRunOutcome, InterruptedRunReceipt,
};
pub use self::complete::{CompleteRun, CompleteRunError, CompleteRunOutcome};
pub use self::fail::{FailRun, FailRunError, FailRunOutcome};

// ---------------------------------------------------------------------------
// Bounded run-error wrappers
// ---------------------------------------------------------------------------

const RUN_ERROR_CODE_MIN_BYTES: usize = 1;
const RUN_ERROR_CODE_MAX_BYTES: usize = 128;
const RUN_ERROR_MESSAGE_MIN_BYTES: usize = 1;
const RUN_ERROR_MESSAGE_MAX_BYTES: usize = 1024;

/// Bounded non-empty run error code persisted as `assistant_runs.error_code`.
///
/// Validates `1..=128` UTF-8 bytes without truncation and exposes no
/// formatting that would leak the contained text through `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct RunErrorCode(String);

impl RunErrorCode {
    /// Creates a validated error code.
    ///
    /// # Errors
    ///
    /// Returns a static reason when `value` is empty or exceeds 128 UTF-8 bytes.
    pub fn parse(value: String) -> Result<Self, &'static str> {
        let len = value.len();
        if !(RUN_ERROR_CODE_MIN_BYTES..=RUN_ERROR_CODE_MAX_BYTES).contains(&len) {
            return Err("run error code must be 1..=128 bytes");
        }
        if value.is_empty() {
            return Err("run error code must not be empty");
        }
        Ok(Self(value))
    }

    /// Validated code as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for RunErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunErrorCode")
            .field("len", &self.0.len())
            .finish()
    }
}

/// Bounded non-empty run error message persisted as `assistant_runs.error_message`.
#[derive(Clone, PartialEq, Eq)]
pub struct RunErrorMessage(String);

impl RunErrorMessage {
    /// Creates a validated error message.
    ///
    /// # Errors
    ///
    /// Returns a static reason when `value` is empty or exceeds 1024 UTF-8 bytes.
    pub fn parse(value: String) -> Result<Self, &'static str> {
        let len = value.len();
        if !(RUN_ERROR_MESSAGE_MIN_BYTES..=RUN_ERROR_MESSAGE_MAX_BYTES).contains(&len) {
            return Err("run error message must be 1..=1024 bytes");
        }
        if value.is_empty() {
            return Err("run error message must not be empty");
        }
        Ok(Self(value))
    }

    /// Validated message as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for RunErrorMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunErrorMessage")
            .field("len", &self.0.len())
            .finish()
    }
}

/// Payload-free durable receipt of one terminal settlement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalRunReceipt {
    /// Settled run identity.
    pub run_id: artisan_domain::RunId,
    /// Generation recorded on the run.
    pub generation: i64,
    /// Terminal time recorded on the run and dispatch.
    pub terminal_at: UnixMillis,
}

// ---------------------------------------------------------------------------
// Shared patch input
// ---------------------------------------------------------------------------

struct TerminalPatchInput<'a> {
    thread_id: &'a str,
    patch_id: &'a str,
    sequence: i64,
    kind: ConversationPatchKind,
    revision: i64,
    recorded_at_ms: i64,
    item_id: Option<&'a str>,
    turn_id: Option<&'a str>,
    lifecycle: Option<EntityLifecycle>,
}

async fn insert_terminal_patch(
    transaction: &sea_orm::DatabaseTransaction,
    input: TerminalPatchInput<'_>,
) -> Result<(), RepositoryError> {
    let patch = entities::conversation_patch::ActiveModel {
        patch_id: sea_orm::ActiveValue::Set(input.patch_id.to_owned()),
        thread_id: sea_orm::ActiveValue::Set(input.thread_id.to_owned()),
        sequence: sea_orm::ActiveValue::Set(input.sequence),
        kind: sea_orm::ActiveValue::Set(input.kind),
        revision: sea_orm::ActiveValue::Set(input.revision),
        recorded_at_ms: sea_orm::ActiveValue::Set(input.recorded_at_ms),
        turn_id: sea_orm::ActiveValue::Set(input.turn_id.map(str::to_owned)),
        item_id: sea_orm::ActiveValue::Set(input.item_id.map(str::to_owned)),
        ordinal: sea_orm::ActiveValue::Set(None),
        lifecycle: sea_orm::ActiveValue::Set(input.lifecycle),
        item_kind: sea_orm::ActiveValue::Set(None),
        run_id: sea_orm::ActiveValue::Set(None),
        phase: sea_orm::ActiveValue::Set(None),
        body: sea_orm::ActiveValue::Set(None),
        fragment: sea_orm::ActiveValue::Set(None),
        entity_created_at_ms: sea_orm::ActiveValue::Set(None),
        entity_updated_at_ms: sea_orm::ActiveValue::Set(None),
    };
    patch
        .insert(transaction)
        .await
        .map_err(|source| database_error("insert terminal patch", source))?;
    Ok(())
}

fn validate_terminal_chronology(
    scope: &RunBatchScope<'_>,
    operated_at: UnixMillis,
) -> Result<(), RepositoryError> {
    let relations = [
        (
            scope.claimed.updated_at,
            scope.expected_launch_at,
            "claimed dispatch updated_at",
            "terminal expected_launch_at",
        ),
        (
            scope.expected_launch_at,
            scope.bound.bound_at,
            "terminal expected_launch_at",
            "provider bound_at",
        ),
        (
            scope.bound.bound_at,
            scope.expected_updated_at,
            "provider bound_at",
            "terminal expected_updated_at",
        ),
        (
            scope.expected_updated_at,
            operated_at,
            "terminal expected_updated_at",
            "terminal operated_at",
        ),
    ];
    for (earlier, later, earlier_field, later_field) in relations {
        if earlier.as_millis() > later.as_millis() {
            return Err(RepositoryError::InvalidChronology {
                earlier_field,
                later_field,
            });
        }
    }
    if scope.claimed.lease_expires_at.as_millis() <= operated_at.as_millis() {
        return Err(RepositoryError::DispatchLeaseExpired {
            message_id: scope.claimed.message_id.clone(),
            lease_expires_at_ms: scope.claimed.lease_expires_at.as_millis(),
            operated_at_ms: operated_at.as_millis(),
        });
    }
    Ok(())
}

fn map_phase(phase: AssistantMessagePhase) -> RenderPhase {
    match phase {
        AssistantMessagePhase::Unspecified => RenderPhase::Unspecified,
        AssistantMessagePhase::Commentary => RenderPhase::Commentary,
        AssistantMessagePhase::Final => RenderPhase::Final,
    }
}

fn render_phase_label(phase: &RenderPhase) -> &'static str {
    match phase {
        RenderPhase::Unspecified => "unspecified",
        RenderPhase::Commentary => "commentary",
        RenderPhase::Final => "final",
    }
}

const fn dispatch_state_label(state: &DispatchState) -> &'static str {
    match state {
        DispatchState::Queued => "queued",
        DispatchState::Leased => "leased",
        DispatchState::Running => "running",
        DispatchState::Completed => "completed",
        DispatchState::Failed => "failed",
    }
}

#[cfg(test)]
#[path = "../../../../../tests/database/run_terminal.rs"]
mod run_terminal;
