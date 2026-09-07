//! Bounded input validation and the private canonical v1 receipt digest.
//!
//! This module is not a generic serialization framework. It validates one
//! `CommitRunBatch` command before any SQL is opened and computes the
//! SHA-256 receipt digest over the frozen v1 layout. Encoding v1 is part of
//! the persisted contract: the encoder stays private here and future formats
//! must preserve old digest classification.
//!
//! The digest deliberately EXCLUDES every secret (start key, owner, lease,
//! claim, and binding bytes) so a receipt can never become a second
//! credential verifier, and EXCLUDES allocated current counters and mutable
//! current rows so an older batch still classifies as an exact replay after
//! later legitimate writes.

use std::collections::HashSet;

use artisan_domain::{
    AssistantBody, AssistantMessagePhase, CONVERSATION_PATCH_BATCH_MAX_PATCHES,
    CONVERSATION_TEXT_FRAGMENT_MAX_BYTES,
};
use sha2::{Digest, Sha256};

use super::{AssistantChange, CheckpointUpdate, CommitRunBatch, RunObservationError};
use crate::repository::RepositoryError;

/// ASCII domain prefix of canonical encoding v1; followed by one NUL byte.
const DIGEST_DOMAIN_PREFIX: &[u8] = b"artisan.run-batch.v1";

/// Operation tag of a progress batch (terminal operations use other tags).
const PROGRESS_OPERATION_TAG: u8 = 0;

/// Pure pre-SQL validation returning the canonical v1 digest.
///
/// Checks bounds, signed-counter representations, duplicate inputs, scope
/// coherence, and chronology, then computes the digest from the validated
/// command. Nothing here opens SQL; all effects stay tentative until the
/// sole transaction commit.
pub(super) fn validate_and_digest(
    command: &CommitRunBatch<'_>,
) -> Result<[u8; 32], RunObservationError> {
    if command.batch_sequence <= 0 {
        return Err(RunObservationError::InvalidBatchSequence {
            sequence: command.batch_sequence,
        });
    }
    if command.changes.is_empty() {
        return Err(RunObservationError::EmptyBatch);
    }
    let activation_patches = usize::from(command.activate_turn_patch_id.is_some());
    let emitted_patches = command.changes.len() + activation_patches;
    if emitted_patches > CONVERSATION_PATCH_BATCH_MAX_PATCHES {
        return Err(RunObservationError::PatchBudgetExceeded {
            count: emitted_patches,
            maximum: CONVERSATION_PATCH_BATCH_MAX_PATCHES,
        });
    }
    validate_scope(command)?;
    validate_chronology(command)?;
    validate_changes(command)?;
    Ok(canonical_digest(command))
}

/// Validates scope coherence: generations, versions, and identity agreement
/// between the claimed dispatch, launch receipt, and binding receipt.
fn validate_scope(command: &CommitRunBatch<'_>) -> Result<(), RunObservationError> {
    let scope = &command.scope;
    if scope.launched.generation <= 0
        || scope.launched.generation != scope.bound.generation
        || scope.bound.binding_version <= 0
    {
        return Err(RunObservationError::CredentialMismatch {
            run_id: scope.launched.run_id.clone(),
        });
    }
    let attempt = scope.claimed.attempt_count;
    if attempt == 0 || i32::try_from(attempt).is_err() {
        return Err(RunObservationError::SnapshotMismatch {
            message_id: scope.claimed.message_id.clone(),
        });
    }
    if scope.claimed.message_id != scope.launched.message_id
        || scope.claimed.message_id != scope.bound.message_id
    {
        return Err(RunObservationError::SnapshotMismatch {
            message_id: scope.claimed.message_id.clone(),
        });
    }
    if scope.launched.run_id != scope.bound.run_id {
        return Err(RunObservationError::IdentityConflict {
            reason: "launched and bound run identities differ",
        });
    }
    if scope.launched.thread_id != scope.bound.thread_id {
        return Err(RunObservationError::IdentityConflict {
            reason: "launched and bound thread identities differ",
        });
    }
    Ok(())
}

/// Validates the frozen chronology chain and the strict lease-expiry bound.
fn validate_chronology(command: &CommitRunBatch<'_>) -> Result<(), RunObservationError> {
    let scope = &command.scope;
    let relations = [
        (
            scope.claimed.updated_at,
            scope.expected_launch_at,
            "claimed dispatch updated_at",
            "batch expected_launch_at",
        ),
        (
            scope.expected_launch_at,
            scope.bound.bound_at,
            "batch expected_launch_at",
            "provider bound_at",
        ),
        (
            scope.bound.bound_at,
            scope.expected_updated_at,
            "provider bound_at",
            "batch expected_updated_at",
        ),
        (
            scope.expected_updated_at,
            command.operated_at,
            "batch expected_updated_at",
            "batch operated_at",
        ),
    ];
    for (earlier, later, earlier_field, later_field) in relations {
        if earlier.as_millis() > later.as_millis() {
            return Err(RunObservationError::Repository(
                RepositoryError::InvalidChronology {
                    earlier_field,
                    later_field,
                },
            ));
        }
    }
    if scope.claimed.lease_expires_at.as_millis() <= command.operated_at.as_millis() {
        return Err(RunObservationError::Repository(
            RepositoryError::DispatchLeaseExpired {
                message_id: scope.claimed.message_id.clone(),
                lease_expires_at_ms: scope.claimed.lease_expires_at.as_millis(),
                operated_at_ms: command.operated_at.as_millis(),
            },
        ));
    }
    Ok(())
}

/// Validates duplicate identities, byte bounds, and signed-representation
/// fit for every declared change.
fn validate_changes(command: &CommitRunBatch<'_>) -> Result<(), RunObservationError> {
    let mut patch_ids: HashSet<&str> = HashSet::new();
    let mut item_ids: HashSet<&str> = HashSet::new();
    if let Some(patch_id) = command.activate_turn_patch_id {
        patch_ids.insert(patch_id.as_str());
    }
    for change in command.changes {
        let (item_id, patch_id) = change_identities(change);
        if !patch_ids.insert(patch_id) {
            return Err(RunObservationError::PatchConflict {
                reason: "duplicate patch identity within one batch",
            });
        }
        if !item_ids.insert(item_id) {
            return Err(RunObservationError::TargetConflict {
                reason: "duplicate item target within one batch",
            });
        }
        match change {
            AssistantChange::Start { body, .. } | AssistantChange::Replace { body, .. } => {
                let length = body.as_str().len();
                if length > AssistantBody::MAX_BYTES {
                    return Err(RunObservationError::BodyTooLong {
                        length,
                        maximum: AssistantBody::MAX_BYTES,
                    });
                }
            }
            AssistantChange::Append { text, .. } => {
                let length = text.as_str().len();
                if length > CONVERSATION_TEXT_FRAGMENT_MAX_BYTES {
                    return Err(RunObservationError::FragmentTooLong {
                        length,
                        maximum: CONVERSATION_TEXT_FRAGMENT_MAX_BYTES,
                    });
                }
            }
        }
        if let AssistantChange::Append {
            expected_revision, ..
        }
        | AssistantChange::Replace {
            expected_revision, ..
        } = change
            && i64::try_from(expected_revision.get()).is_err()
        {
            return Err(RunObservationError::CounterOverflow {
                counter: "revision",
                value: i64::MAX,
            });
        }
    }
    Ok(())
}

fn change_identities<'a>(change: &'a AssistantChange<'a>) -> (&'a str, &'a str) {
    match change {
        AssistantChange::Start {
            item_id, patch_id, ..
        }
        | AssistantChange::Append {
            item_id, patch_id, ..
        }
        | AssistantChange::Replace {
            item_id, patch_id, ..
        } => (item_id.as_str(), patch_id.as_str()),
    }
}

/// Canonical v1 digest of one validated command.
///
/// Layout: the ASCII domain prefix plus one NUL; the progress operation tag;
/// all integers as fixed-width little-endian `i64`; all collection and
/// byte/string lengths as little-endian `u32` prefixes followed by exact
/// UTF-8/raw bytes with no normalization; `Option` tags `0`/`1`; mutation
/// tags Start=0/Append=1/Replace=2; phase tags Unspecified=0/Commentary=1/
/// Final=2; checkpoint tags Keep=0/Replace=1.
fn canonical_digest(command: &CommitRunBatch<'_>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(DIGEST_DOMAIN_PREFIX);
    hasher.update([0u8]);
    hasher.update([PROGRESS_OPERATION_TAG]);

    let scope = &command.scope;
    let launched = scope.launched;
    let claimed = scope.claimed;

    write_str(&mut hasher, launched.run_id.as_str());
    write_str(&mut hasher, launched.thread_id.as_str());
    write_str(&mut hasher, launched.message_id.as_str());
    write_str(&mut hasher, launched.turn_id.as_str());
    write_i64(&mut hasher, launched.generation);
    write_i64(&mut hasher, scope.bound.binding_version);
    write_i64(&mut hasher, scope.bound.bound_at.as_millis());
    write_i64(&mut hasher, scope.expected_launch_at.as_millis());
    write_i64(&mut hasher, scope.expected_updated_at.as_millis());
    write_i64(&mut hasher, command.operated_at.as_millis());
    write_i64(&mut hasher, command.batch_sequence);

    write_str(&mut hasher, claimed.message_id.as_str());
    write_str(&mut hasher, claimed.correlation_id.as_str());
    write_i64(&mut hasher, i64::from(claimed.attempt_count));
    write_i64(&mut hasher, claimed.queued_at.as_millis());
    write_i64(&mut hasher, claimed.available_at.as_millis());
    write_i64(&mut hasher, claimed.lease_expires_at.as_millis());
    write_i64(&mut hasher, claimed.updated_at.as_millis());

    match command.activate_turn_patch_id {
        None => hasher.update([0u8]),
        Some(patch_id) => {
            hasher.update([1u8]);
            write_str(&mut hasher, patch_id.as_str());
        }
    }

    write_length(&mut hasher, command.changes.len());
    for change in command.changes {
        write_change(&mut hasher, change);
    }

    match command.checkpoint {
        CheckpointUpdate::Keep => hasher.update([0u8]),
        CheckpointUpdate::Replace(checkpoint) => {
            hasher.update([1u8]);
            write_i64(&mut hasher, checkpoint.version());
            write_bytes(&mut hasher, checkpoint.as_slice());
        }
    }

    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn write_change(hasher: &mut Sha256, change: &AssistantChange<'_>) {
    match change {
        AssistantChange::Start {
            item_id,
            phase,
            body,
            patch_id,
        } => {
            hasher.update([0u8]);
            write_str(hasher, item_id.as_str());
            hasher.update([phase_tag(*phase)]);
            write_bytes(hasher, body.as_str().as_bytes());
            write_str(hasher, patch_id.as_str());
        }
        AssistantChange::Append {
            item_id,
            expected_revision,
            text,
            patch_id,
        } => {
            hasher.update([1u8]);
            write_str(hasher, item_id.as_str());
            write_i64(hasher, revision_digest_value(expected_revision.get()));
            write_bytes(hasher, text.as_str().as_bytes());
            write_str(hasher, patch_id.as_str());
        }
        AssistantChange::Replace {
            item_id,
            expected_revision,
            body,
            phase,
            patch_id,
        } => {
            hasher.update([2u8]);
            write_str(hasher, item_id.as_str());
            write_i64(hasher, revision_digest_value(expected_revision.get()));
            write_bytes(hasher, body.as_str().as_bytes());
            hasher.update([phase_tag(*phase)]);
            write_str(hasher, patch_id.as_str());
        }
    }
}

fn write_i64(hasher: &mut Sha256, value: i64) {
    hasher.update(value.to_le_bytes());
}

fn write_length(hasher: &mut Sha256, length: usize) {
    // Every hashed length is validated far below `u32::MAX` before the
    // digest is computed; the fallback keeps the encoder total.
    let prefix = u32::try_from(length).unwrap_or(u32::MAX);
    hasher.update(prefix.to_le_bytes());
}

fn write_bytes(hasher: &mut Sha256, value: &[u8]) {
    write_length(hasher, value.len());
    hasher.update(value);
}

fn write_str(hasher: &mut Sha256, value: &str) {
    write_bytes(hasher, value.as_bytes());
}

/// Digest representation of a pre-validated domain revision.
fn revision_digest_value(revision: u64) -> i64 {
    // Validation already rejected revisions that do not fit `i64`.
    i64::try_from(revision).unwrap_or(i64::MAX)
}

const fn phase_tag(phase: AssistantMessagePhase) -> u8 {
    match phase {
        AssistantMessagePhase::Unspecified => 0,
        AssistantMessagePhase::Commentary => 1,
        AssistantMessagePhase::Final => 2,
    }
}
