//! Mid-turn interaction delivery vocabulary for the engine owner.
//!
//! While a configured turn is live, the owning dispatch loop may deliver
//! explicit approval/question answers onto its [`AcceptedTurn`](super::operation::AcceptedTurn).
//! This module owns the delivery endpoint state: idempotent command ids and
//! per-target resolution tracking with [`CommandTargetError`] on unknown or
//! already-resolved targets, mirroring TypeScript `EngineCommandTargetError`
//! (`artisan_run_id`, `command_id`, `target_id`, `target`). A reused command
//! id with a changed intent is [`CommandIdConflict`], mirroring
//! `EngineCommandIdConflictError`.
//!
//! The ledger is deliberately provider-agnostic: it records that the owner
//! accepted the decision, never how a provider session applied it. Forwarding
//! a decision into the live provider session needs a child-protocol endpoint
//! that does not exist yet, so delivery today records the decision durably
//! (through the dispatch loop's resolve transaction) plus here on the
//! accepted turn, and never disturbs control flow: answering never cancels,
//! interrupts, or steers the run.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use artisan_domain::{ObservationId, RunId};
use thiserror::Error;

/// Which pending provider request a delivered response answers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InteractionTarget {
    /// A pending approval request.
    Approval,
    /// A pending question.
    Question,
    /// A steered follow-up text for the live turn. Noted by the dispatch
    /// steer arm (never seeded from durable state); redelivery under the
    /// same command id answers Duplicate with no second provider write.
    Steer,
}

impl InteractionTarget {
    /// Returns the stable target spelling shared with the engine errors.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Question => "question",
            Self::Steer => "steer",
        }
    }
}

/// Why a delivered response missed its target.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TargetFailure {
    /// No pending request carries this target id on the live turn.
    Unknown,
    /// The target was already resolved by an earlier delivered response.
    Resolved,
}

/// A delivered command whose provider request target is absent or already
/// resolved.
///
/// Mirrors TypeScript `EngineCommandTargetError`: the run, the delivered
/// command id, the named target, and which request kind it named, plus the
/// precise miss reason so unknown and resolved targets stay distinguishable.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error(
    "command `{command_id}` targets a {reason} {target} `{target_id}` on run `{artisan_run_id}`"
)]
pub struct CommandTargetError {
    artisan_run_id: RunId,
    command_id: String,
    target_id: String,
    target: InteractionTarget,
    reason: TargetFailure,
}

impl CommandTargetError {
    /// Creates an unknown-target error over already validated identities.
    #[must_use]
    pub const fn unknown(
        artisan_run_id: RunId,
        command_id: String,
        target_id: String,
        target: InteractionTarget,
    ) -> Self {
        Self {
            artisan_run_id,
            command_id,
            target_id,
            target,
            reason: TargetFailure::Unknown,
        }
    }

    /// Creates an already-resolved error over already validated identities.
    #[must_use]
    pub const fn resolved(
        artisan_run_id: RunId,
        command_id: String,
        target_id: String,
        target: InteractionTarget,
    ) -> Self {
        Self {
            artisan_run_id,
            command_id,
            target_id,
            target,
            reason: TargetFailure::Resolved,
        }
    }

    /// Returns the run the command targeted.
    #[allow(dead_code)]
    #[must_use]
    pub const fn run_id(&self) -> &RunId {
        &self.artisan_run_id
    }

    /// Returns the delivered command identity.
    #[allow(dead_code)]
    #[must_use]
    pub fn command_id(&self) -> &str {
        &self.command_id
    }

    /// Returns the named provider request identity.
    #[allow(dead_code)]
    #[must_use]
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    /// Returns which request kind was named.
    #[allow(dead_code)]
    #[must_use]
    pub const fn target(&self) -> InteractionTarget {
        self.target
    }

    /// Returns why the target missed.
    #[allow(dead_code)]
    #[must_use]
    pub const fn reason(&self) -> TargetFailure {
        self.reason
    }
}

impl std::fmt::Display for InteractionTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::fmt::Display for TargetFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => formatter.write_str("unknown"),
            Self::Resolved => formatter.write_str("already-resolved"),
        }
    }
}

/// Reuse of an accepted command identifier with a changed intent.
///
/// Mirrors TypeScript `EngineCommandIdConflictError`: only the run and the
/// reused command id travel; intent bytes never enter the error.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("command `{command_id}` was already accepted with a different intent on run `{run_id}`")]
pub struct CommandIdConflict {
    run_id: RunId,
    command_id: String,
}

impl CommandIdConflict {
    /// Creates a conflict over already validated identities.
    #[must_use]
    pub const fn new(run_id: RunId, command_id: String) -> Self {
        Self { run_id, command_id }
    }

    /// Returns the run the command targeted.
    #[allow(dead_code)]
    #[must_use]
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Returns the reused command identity.
    #[allow(dead_code)]
    #[must_use]
    pub fn command_id(&self) -> &str {
        &self.command_id
    }
}

/// Failure delivering one mid-turn response onto its accepted turn.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum InteractionDeliveryError {
    /// The named target is unknown or already resolved.
    #[error(transparent)]
    Target(#[from] CommandTargetError),
    /// The command id was already accepted with a different intent.
    #[error(transparent)]
    IdConflict(#[from] CommandIdConflict),
}

/// How one delivered response settled on its accepted turn.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TurnInteractionOutcome {
    /// The decision was recorded on the turn for the first time.
    Applied,
    /// The identical command was already delivered; no second effect.
    Duplicate,
}

/// Owner-side delivery ledger for one live accepted turn.
///
/// Tracks which provider targets are still pending, which resolved, and
/// which command ids were accepted with which intent. The dispatch loop
/// seeds pending targets from durable state and delivers each applied
/// response here after its resolve transaction commits.
#[derive(Debug)]
pub struct TurnInteractionLedger {
    run_id: RunId,
    pending: HashMap<String, InteractionTarget>,
    resolved: HashSet<String>,
    seen_commands: HashMap<String, String>,
}

impl TurnInteractionLedger {
    /// Creates an empty ledger for one live run.
    #[must_use]
    pub fn new(run_id: RunId) -> Self {
        Self {
            run_id,
            pending: HashMap::new(),
            resolved: HashSet::new(),
            seen_commands: HashMap::new(),
        }
    }

    /// Returns the run this ledger delivers for.
    #[allow(dead_code)]
    #[must_use]
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// Notes one pending provider target.
    ///
    /// Re-noting a pending target is a no-op; noting a target that already
    /// resolved never regresses it back to pending. Returns whether the
    /// target became newly pending.
    pub fn note_requested(&mut self, target_id: &ObservationId, target: InteractionTarget) -> bool {
        if self.resolved.contains(target_id.as_str()) {
            return false;
        }
        match self.pending.get(target_id.as_str()) {
            Some(known) if *known == target => false,
            _ => {
                self.pending.insert(target_id.as_str().to_owned(), target);
                true
            }
        }
    }

    /// Nonmutating eligibility of one delivery, sharing [`Self::deliver`]
    /// validation without recording anything.
    ///
    /// Returns `Ok(Applied)` when the command would record now,
    /// `Ok(Duplicate)` when the identical command already recorded, or the
    /// same typed error `deliver` would return. The steer arm preflights
    /// here before any provider contact and records via `deliver` only
    /// after the actual provider ack, so an interruption before the ack
    /// never writes a resolution the durable row cannot see, while a
    /// projection retry observes `Duplicate` (known acked).
    pub fn preflight(
        &self,
        command_id: &str,
        target_id: &ObservationId,
        target: InteractionTarget,
        intent: &str,
    ) -> Result<TurnInteractionOutcome, InteractionDeliveryError> {
        self.validate(command_id, target_id, target, intent)
    }

    /// Delivers one validated response onto the turn.
    ///
    /// A seen command id with the identical intent answers `Duplicate` with
    /// no second effect; with a different intent it is a conflict. An unseen
    /// command resolves its pending target, and misses its target with a
    /// typed error on unknown or already-resolved ids. Delivery never
    /// disturbs control flow: resolving records the decision only.
    ///
    /// Validation is shared with [`Self::preflight`]: approval and question
    /// paths keep calling this unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`InteractionDeliveryError::Target`] for unknown or resolved
    /// targets and [`InteractionDeliveryError::IdConflict`] for a reused
    /// command id with a changed intent.
    pub fn deliver(
        &mut self,
        command_id: &str,
        target_id: &ObservationId,
        target: InteractionTarget,
        intent: &str,
    ) -> Result<TurnInteractionOutcome, InteractionDeliveryError> {
        let outcome = self.validate(command_id, target_id, target, intent)?;
        if outcome == TurnInteractionOutcome::Applied {
            self.pending.remove(target_id.as_str());
            self.resolved.insert(target_id.as_str().to_owned());
            self.seen_commands
                .insert(command_id.to_owned(), intent.to_owned());
        }
        Ok(outcome)
    }

    /// Shared validation for [`Self::preflight`] and [`Self::deliver`].
    /// Reads `seen_commands`, `resolved`, then `pending` in that order and
    /// mutates nothing.
    fn validate(
        &self,
        command_id: &str,
        target_id: &ObservationId,
        target: InteractionTarget,
        intent: &str,
    ) -> Result<TurnInteractionOutcome, InteractionDeliveryError> {
        if let Some(seen) = self.seen_commands.get(command_id) {
            if seen == intent {
                return Ok(TurnInteractionOutcome::Duplicate);
            }
            return Err(CommandIdConflict::new(self.run_id.clone(), command_id.to_owned()).into());
        }
        if self.resolved.contains(target_id.as_str()) {
            return Err(CommandTargetError::resolved(
                self.run_id.clone(),
                command_id.to_owned(),
                target_id.as_str().to_owned(),
                target,
            )
            .into());
        }
        match self.pending.get(target_id.as_str()) {
            Some(known) if *known == target => Ok(TurnInteractionOutcome::Applied),
            _ => Err(CommandTargetError::unknown(
                self.run_id.clone(),
                command_id.to_owned(),
                target_id.as_str().to_owned(),
                target,
            )
            .into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use artisan_domain::{ObservationId, RunId};

    use super::{InteractionTarget, TargetFailure, TurnInteractionLedger, TurnInteractionOutcome};

    fn run_id() -> RunId {
        RunId::parse("run-ledger").expect("fixture run id should be valid")
    }

    fn target_id(value: &str) -> ObservationId {
        ObservationId::parse(value).expect("fixture target id should be valid")
    }

    #[test]
    fn deliver_resolves_a_noted_target_exactly_once() {
        let mut ledger = TurnInteractionLedger::new(run_id());
        assert!(ledger.note_requested(&target_id("approval-1"), InteractionTarget::Approval));
        assert_eq!(
            ledger.deliver(
                "cmd-1",
                &target_id("approval-1"),
                InteractionTarget::Approval,
                "allow"
            ),
            Ok(TurnInteractionOutcome::Applied)
        );
        assert_eq!(
            ledger.deliver(
                "cmd-1",
                &target_id("approval-1"),
                InteractionTarget::Approval,
                "allow"
            ),
            Ok(TurnInteractionOutcome::Duplicate)
        );
    }

    #[test]
    fn deliver_rejects_unknown_and_resolved_targets_distinctly() {
        let mut ledger = TurnInteractionLedger::new(run_id());
        let unknown = ledger
            .deliver(
                "cmd-x",
                &target_id("ghost"),
                InteractionTarget::Approval,
                "deny",
            )
            .expect_err("unknown target must fail");
        let super::InteractionDeliveryError::Target(error) = unknown else {
            panic!("unknown target must report a target error")
        };
        assert_eq!(error.reason(), TargetFailure::Unknown);

        assert!(ledger.note_requested(&target_id("approval-2"), InteractionTarget::Approval));
        assert_eq!(
            ledger.deliver(
                "cmd-2",
                &target_id("approval-2"),
                InteractionTarget::Approval,
                "deny"
            ),
            Ok(TurnInteractionOutcome::Applied)
        );
        let resolved = ledger
            .deliver(
                "cmd-3",
                &target_id("approval-2"),
                InteractionTarget::Approval,
                "deny",
            )
            .expect_err("resolved target must fail");
        let super::InteractionDeliveryError::Target(error) = resolved else {
            panic!("resolved target must report a target error")
        };
        assert_eq!(error.reason(), TargetFailure::Resolved);
    }

    #[test]
    fn reused_command_id_with_changed_intent_conflicts() {
        let mut ledger = TurnInteractionLedger::new(run_id());
        assert!(ledger.note_requested(&target_id("approval-3"), InteractionTarget::Approval));
        assert_eq!(
            ledger.deliver(
                "cmd-9",
                &target_id("approval-3"),
                InteractionTarget::Approval,
                "allow"
            ),
            Ok(TurnInteractionOutcome::Applied)
        );
        let conflict = ledger
            .deliver(
                "cmd-9",
                &target_id("approval-3"),
                InteractionTarget::Approval,
                "deny",
            )
            .expect_err("changed intent must conflict");
        assert!(matches!(
            conflict,
            super::InteractionDeliveryError::IdConflict(_)
        ));
    }

    #[test]
    fn noting_a_resolved_target_never_regresses_it() {
        let mut ledger = TurnInteractionLedger::new(run_id());
        assert!(ledger.note_requested(&target_id("approval-4"), InteractionTarget::Approval));
        assert_eq!(
            ledger.deliver(
                "cmd-4",
                &target_id("approval-4"),
                InteractionTarget::Approval,
                "deny"
            ),
            Ok(TurnInteractionOutcome::Applied)
        );
        assert!(!ledger.note_requested(&target_id("approval-4"), InteractionTarget::Approval));
    }

    #[test]
    fn kind_mismatch_is_an_unknown_target() {
        let mut ledger = TurnInteractionLedger::new(run_id());
        assert!(ledger.note_requested(&target_id("q-1"), InteractionTarget::Question));
        let missed = ledger
            .deliver(
                "cmd-5",
                &target_id("q-1"),
                InteractionTarget::Approval,
                "allow",
            )
            .expect_err("kind mismatch must miss");
        let super::InteractionDeliveryError::Target(error) = missed else {
            panic!("kind mismatch must report a target error")
        };
        assert_eq!(error.reason(), TargetFailure::Unknown);
    }
}
