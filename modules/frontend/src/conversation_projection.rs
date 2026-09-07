//! Bounded client-side replay of authoritative conversation state.
//!
//! This leaf is the ONE frontend-owned pure state owner for already-decoded
//! domain [`ConversationSnapshot`] and [`PatchBatch`] values: canonical
//! visible-state replay with atomic whole-value and whole-batch publication.
//! It performs no network delivery, rendering, pacing, Markdown parsing,
//! persistence, or run execution, and it never fabricates timestamps,
//! snapshots, or cursors for an uninitialized projection.
//!
//! Replay limits that bounded retained state cannot overcome are deliberate:
//! zero patch history is retained, so any batch whose start differs from the
//! current cursor — an old/subsumed tail, a duplicate, or a gap — requests an
//! explicit resnapshot instead of claiming payload equivalence. Rows, cursor,
//! and watermark publish together only after an entire batch validates; a
//! failed same-thread batch preserves the last good snapshot and only flips
//! [`ProjectionStatus::ResnapshotRequired`], and the caller owns recovery
//! I/O. A snapshot's turn ceiling does not bound items or text by itself, so
//! explicit VP-approved client retention caps (2048 items, 8 MiB summed body
//! UTF-8 bytes) are checked on borrowed input before candidate rows are
//! cloned, and each staged mutation's prospective budget is checked before
//! its replacement text is allocated.
//! [`AssistantMessagePhase::Final`](artisan_domain::AssistantMessagePhase)
//! classifies renderer text only and never implies lifecycle completion, and
//! a turn or item lifecycle never proves provider/run completion either.
//! During any one mutation this owner retains at most the old text budget
//! plus one staged candidate copy of that budget, alongside the caller's
//! own input, bounded per-patch replacement temporaries, and identity and
//! index bookkeeping; decoder allocation and whole-application memory are
//! out of scope for that statement.

use std::collections::HashSet;

use artisan_domain::{
    AssistantBody, CONVERSATION_QUERY_MAX_TURNS, ConversationCursor, ConversationItem,
    ConversationLifecycle, ConversationPatch, ConversationSnapshot, ConversationSnapshotError,
    ConversationTurn, ItemId, ItemOrdinal, LifecycleTransitionError, MESSAGE_BODY_MAX_BYTES,
    MessageBody, PatchBatch, Revision, ThreadId, TurnId, UnixMillis,
};

use self::validation::{
    RawKind, Retention, created_not_after_updated, item_body_len, item_index, ordinal_taken,
    raw_identity_taken_by_other_kind, turn_index,
};

mod validation;

/// One frontend-owned projection of one thread's authoritative conversation.
///
/// State is private; construction is the only way to bind the fixed
/// [`ThreadId`]; the type intentionally cannot be cloned or compared, so the
/// state this value publishes cannot be forked or aliased through it. That
/// is a composition limit, not a provenance proof: a caller may still
/// construct another projection for the same thread, and no domain value
/// authenticates which connection or request generation produced a frame.
/// The materialized window reuses the domain snapshot value itself — there
/// are no parallel rows, text copies, or counters here.
#[derive(Debug)]
pub struct ConversationProjection {
    thread_id: ThreadId,
    state: Option<ConversationSnapshot>,
    status: ProjectionStatus,
}

/// Delivery health of the projection's materialized window.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProjectionStatus {
    /// No authoritative snapshot has been accepted yet.
    AwaitingSnapshot,
    /// The latest snapshot/batch flow is intact and current.
    Ready,
    /// Recovery is required; the last good snapshot stays preserved and
    /// visible, but no further batch applies until a valid snapshot clears
    /// this state.
    ResnapshotRequired,
}

/// Outcome of one successful authoritative snapshot installation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SnapshotDisposition {
    /// The snapshot established or advanced the materialized window.
    Applied,
    /// An equal-cursor, exactly equal snapshot was reinstalled: visible
    /// state did not change, and recovery was still cleared.
    Unchanged,
}

/// Outcome of one successfully applied patch batch.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BatchDisposition {
    /// The published cursor after the final patch of the batch.
    pub to_cursor: ConversationCursor,
}

/// Why a mutation was refused. Variants carry identities or domain errors —
/// never message bodies — and every refusal leaves prior visible rows and
/// cursor untouched; a failed same-thread batch only moves the status to
/// [`ProjectionStatus::ResnapshotRequired`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionError {
    /// A frame named a different thread than this owner's fixed identity.
    ThreadMismatch,
    /// A batch arrived before any authoritative snapshot established state.
    BaselineRequired,
    /// A batch arrived while recovery was already required.
    RecoveryRequired,
    /// A delivery operation did not match the current cursor: a patch batch
    /// failed exact continuation, or a resumed acknowledgement named a
    /// different cursor.
    CursorMismatch,
    /// A snapshot lowered the cursor or watermark, conflicted at an equal
    /// cursor, or violated a common-entity rule against the current window.
    SnapshotConflict,
    /// Identity rules failed: cross-kind identifier reuse, an occupied
    /// shared ordinal, kind/turn/ordinal/creation changes on a known
    /// entity, or an assistant run swap.
    IdentityConflict,
    /// A revision ladder failed, including checked-counter overflow at
    /// `u64::MAX`, which is rejected as a typed conflict instead of wrapping.
    RevisionConflict,
    /// A lifecycle transition violated the existing sealed-terminal rule.
    Lifecycle(LifecycleTransitionError),
    /// Creation followed update, or an entity/update instant regressed.
    /// Equal instants stay legal; all signed values stay legal.
    TimeOrdering,
    /// An append/lifecycle delta named an absent target, or an inserted
    /// item's turn was absent at that patch position.
    UnknownTarget,
    /// Concatenated body text crossed its domain UTF-8 byte contract.
    BodyBoundExceeded,
    /// A retained-state ceiling was crossed: the domain's 512-turn window or
    /// a VP-approved client retention ceiling (2048 items, 8 MiB summed body
    /// bytes); prior state is preserved untouched.
    RetentionExceeded,
    /// Defensive belt-and-braces: the domain snapshot constructor rejected
    /// staged structure the owner's own checks should have caught first.
    Structure(ConversationSnapshotError),
}

impl std::fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ThreadMismatch => {
                formatter.write_str("conversation frame names a different thread")
            }
            Self::BaselineRequired => {
                formatter.write_str("projection awaits its baseline snapshot")
            }
            Self::RecoveryRequired => {
                formatter.write_str("projection requires a fresh snapshot before further batches")
            }
            Self::CursorMismatch => {
                formatter.write_str("delivery does not continue the current cursor")
            }
            Self::SnapshotConflict => {
                formatter.write_str("snapshot conflicts with the materialized window")
            }
            Self::IdentityConflict => formatter.write_str("entity identity rules were violated"),
            Self::RevisionConflict => formatter.write_str("entity revision ladder was violated"),
            Self::Lifecycle(error) => {
                write!(formatter, "entity lifecycle transition was sealed: {error}")
            }
            Self::TimeOrdering => formatter.write_str("entity timestamp ordering was violated"),
            Self::UnknownTarget => {
                formatter.write_str("mutation targeted an entity absent at that position")
            }
            Self::BodyBoundExceeded => {
                formatter.write_str("body text exceeded its documented UTF-8 byte bound")
            }
            Self::RetentionExceeded => formatter.write_str("client retention budget was exceeded"),
            Self::Structure(error) => {
                write!(
                    formatter,
                    "staged structure failed domain validation: {error}"
                )
            }
        }
    }
}

impl std::error::Error for ProjectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Lifecycle(error) => Some(error),
            Self::Structure(error) => Some(error),
            _ => None,
        }
    }
}

impl ConversationProjection {
    /// Creates an empty projection bound to one thread. Nothing is
    /// fabricated: status starts at [`ProjectionStatus::AwaitingSnapshot`]
    /// with no snapshot and no cursor.
    #[must_use]
    pub fn new(thread_id: ThreadId) -> Self {
        Self {
            thread_id,
            state: None,
            status: ProjectionStatus::AwaitingSnapshot,
        }
    }

    /// The fixed thread identity this projection serves.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Current delivery health of the materialized window.
    #[must_use]
    pub const fn status(&self) -> ProjectionStatus {
        self.status
    }

    /// The materialized authoritative window, if a baseline snapshot has
    /// been accepted. While [`ProjectionStatus::ResnapshotRequired`] the
    /// last good snapshot remains visible here for the caller's recovery UX.
    #[must_use]
    pub const fn snapshot(&self) -> Option<&ConversationSnapshot> {
        self.state.as_ref()
    }

    /// Installs one authoritative subscription snapshot atomically.
    ///
    /// Budgets, semantic times, cross-kind identifier reuse, and the
    /// cursor/revision ladder are all checked on the borrowed value before
    /// anything is cloned; a refusal leaves every prior row, cursor, and the
    /// status byte-identical. A first valid snapshot may carry arbitrary
    /// revisions and cursor. Later snapshots must not lower the cursor or
    /// the watermark; an equal cursor demands exact full equality (including
    /// rows and watermark) and reports [`SnapshotDisposition::Unchanged`];
    /// a higher cursor replaces the bounded window atomically, keeping
    /// common entities immutable in identity/turn ownership/ordinal/
    /// creation/kind/run association, free of revision or time regression,
    /// and lifecycle-legal through the existing domain transition helper.
    /// Higher snapshot revisions may jump over multiple unseen revisions.
    /// Success always clears recovery.
    ///
    /// This API accepts an authoritative subscription snapshot only; it does
    /// not authenticate which connection or request generation produced it,
    /// and it never merges an arbitrary older-history query range. Recovery
    /// I/O belongs to the caller.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError`] as documented per variant; wrong-thread
    /// input never disturbs this thread's state or status.
    pub fn install_snapshot(
        &mut self,
        incoming: &ConversationSnapshot,
    ) -> Result<SnapshotDisposition, ProjectionError> {
        if incoming.thread_id() != &self.thread_id {
            return Err(ProjectionError::ThreadMismatch);
        }

        Retention::seed(incoming.items()).map_err(|_| ProjectionError::RetentionExceeded)?;
        validate_snapshot_semantics(incoming)?;

        let Some(previous) = self.state.as_ref() else {
            self.publish(incoming.clone());
            return Ok(SnapshotDisposition::Applied);
        };

        if incoming.cursor() < previous.cursor() {
            return Err(ProjectionError::SnapshotConflict);
        }
        if incoming.cursor() == previous.cursor() {
            if incoming == previous {
                self.status = ProjectionStatus::Ready;
                return Ok(SnapshotDisposition::Unchanged);
            }
            return Err(ProjectionError::SnapshotConflict);
        }
        if incoming.updated_at().as_millis() < previous.updated_at().as_millis() {
            return Err(ProjectionError::SnapshotConflict);
        }
        for previous_turn in previous.turns() {
            if let Some(next_turn) = incoming
                .turns()
                .iter()
                .find(|turn| turn.turn_id == previous_turn.turn_id)
            {
                validate_common_turn(previous_turn, next_turn)?;
            }
        }
        for previous_item in previous.items() {
            if let Some(next_item) = incoming
                .items()
                .iter()
                .find(|item| item.item_id() == previous_item.item_id())
            {
                validate_common_item(previous_item, next_item)?;
            }
        }
        // Cross-kind reincarnation: a higher window may omit a known entity
        // and reintroduce its raw identifier text as the other kind. Both
        // directions are checked on borrowed input before cloning.
        for turn in incoming.turns() {
            if previous
                .items()
                .iter()
                .any(|item| item.item_id().as_str() == turn.turn_id.as_str())
            {
                return Err(ProjectionError::IdentityConflict);
            }
        }
        for item in incoming.items() {
            if previous
                .turns()
                .iter()
                .any(|turn| turn.turn_id.as_str() == item.item_id().as_str())
            {
                return Err(ProjectionError::IdentityConflict);
            }
        }

        self.publish(incoming.clone());
        Ok(SnapshotDisposition::Applied)
    }

    /// Acknowledges an authoritative resumed subscription at the existing
    /// last-good cursor.
    ///
    /// This is deliberately a status-only operation. It validates the
    /// acknowledgement against this projection's fixed thread and existing
    /// snapshot, then clears recovery without cloning, replacing, or otherwise
    /// changing the snapshot. No baseline or cursor is fabricated.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::ThreadMismatch`] for a foreign thread,
    /// [`ProjectionError::BaselineRequired`] when no snapshot has been
    /// accepted, or [`ProjectionError::CursorMismatch`] when the acknowledgement
    /// does not name the existing last-good cursor. Every refusal preserves the
    /// snapshot, cursor, and prior status exactly.
    pub fn acknowledge_resumed(
        &mut self,
        thread_id: &ThreadId,
        cursor: ConversationCursor,
    ) -> Result<(), ProjectionError> {
        if thread_id != &self.thread_id {
            return Err(ProjectionError::ThreadMismatch);
        }
        let Some(previous) = self.state.as_ref() else {
            return Err(ProjectionError::BaselineRequired);
        };
        if previous.cursor() != cursor {
            return Err(ProjectionError::CursorMismatch);
        }
        self.status = ProjectionStatus::Ready;
        Ok(())
    }

    /// Applies one contiguous patch batch on top of the current window.
    ///
    /// The batch must name this thread, the projection must hold a baseline,
    /// and recovery must not be pending; otherwise typed refusals leave
    /// everything untouched. `batch.from_cursor()` must equal the current
    /// cursor (the batch's first patch then continues at cursor + 1); any
    /// other relationship — subsumed tail, exact repeat, overlap, forward
    /// gap — flips the status to [`ProjectionStatus::ResnapshotRequired`],
    /// because retained rows plus a cursor cannot prove payload equivalence
    /// against zero patch history. Valid application stages ONE candidate,
    /// enforces revision ladders, immutable identity, lifecycle legality,
    /// time ordering, and retention budgets per patch, recomputes the
    /// watermark as the maximum of the previous watermark and every delta's
    /// Forge-supplied time, then republishes through the domain snapshot
    /// constructor so structural proofs stay uniform. Full upsert values
    /// keep their own metadata; they are never restamped with the maximum.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError`] as documented per variant. Any invalid
    /// same-thread batch preserves the last good snapshot and only sets the
    /// recovery status; nothing from a refused batch ever partially lands.
    pub fn apply_batch(&mut self, batch: &PatchBatch) -> Result<BatchDisposition, ProjectionError> {
        if batch.thread_id() != &self.thread_id {
            return Err(ProjectionError::ThreadMismatch);
        }
        let Some(previous) = self.state.as_ref() else {
            return Err(ProjectionError::BaselineRequired);
        };
        if self.status == ProjectionStatus::ResnapshotRequired {
            return Err(ProjectionError::RecoveryRequired);
        }
        if batch.from_cursor() != previous.cursor() {
            self.status = ProjectionStatus::ResnapshotRequired;
            return Err(ProjectionError::CursorMismatch);
        }

        match self.stage(previous, batch) {
            Ok(candidate) => {
                self.publish(candidate);
                Ok(BatchDisposition {
                    to_cursor: batch.to_cursor(),
                })
            }
            Err(error) => {
                self.status = ProjectionStatus::ResnapshotRequired;
                Err(error)
            }
        }
    }

    /// Stages one full candidate for `batch` without touching published
    /// state; on success the caller publishes the returned snapshot
    /// atomically.
    fn stage(
        &self,
        previous: &ConversationSnapshot,
        batch: &PatchBatch,
    ) -> Result<ConversationSnapshot, ProjectionError> {
        let mut staged_turns: Vec<ConversationTurn> = previous.turns().to_vec();
        let mut staged_items: Vec<ConversationItem> = previous.items().to_vec();
        let mut retention =
            Retention::seed(&staged_items).map_err(|_| ProjectionError::RetentionExceeded)?;
        let mut watermark_millis = previous.updated_at().as_millis();

        for patch in batch.patches() {
            match patch {
                ConversationPatch::TurnUpsert { turn, .. } => {
                    apply_turn_upsert(&mut staged_turns, &staged_items, turn)?;
                }
                ConversationPatch::ItemUpsert { item, .. } => {
                    apply_item_upsert(&staged_turns, &mut staged_items, &mut retention, item)?;
                }
                ConversationPatch::ItemAppend {
                    item_id,
                    revision,
                    text,
                    updated_at,
                    ..
                } => {
                    apply_item_append(
                        &mut staged_items,
                        &mut retention,
                        item_id,
                        *revision,
                        text.as_str(),
                        *updated_at,
                    )?;
                }
                ConversationPatch::ItemLifecycle {
                    item_id,
                    revision,
                    lifecycle,
                    updated_at,
                    ..
                } => {
                    apply_item_lifecycle(
                        &mut staged_items,
                        item_id,
                        *revision,
                        *lifecycle,
                        *updated_at,
                    )?;
                }
                ConversationPatch::TurnLifecycle {
                    turn_id,
                    revision,
                    lifecycle,
                    updated_at,
                    ..
                } => {
                    apply_turn_lifecycle(
                        &mut staged_turns,
                        turn_id,
                        *revision,
                        *lifecycle,
                        *updated_at,
                    )?;
                }
            }
            // The watermark is the maximum of the previous watermark and each
            // patch's own Forge-supplied instant — full values through their
            // metadata, deltas through their explicit field — never a local
            // clock or arrival time.
            watermark_millis = watermark_millis.max(patch.updated_at().as_millis());
        }

        ConversationSnapshot::new(
            self.thread_id.clone(),
            batch.to_cursor(),
            staged_turns,
            staged_items,
            UnixMillis::from_millis(watermark_millis),
        )
        .map_err(ProjectionError::Structure)
    }

    /// Publishes an already-validated snapshot and clears recovery.
    fn publish(&mut self, candidate: ConversationSnapshot) {
        self.state = Some(candidate);
        self.status = ProjectionStatus::Ready;
    }
}

/// Applies one complete turn value: insert-at-revision-zero or the known
/// ladder with immutable ordinal/creation and lifecycle legality. The
/// prospective turn-window, ordinal, and raw-identity checks all run before
/// the value is cloned.
fn apply_turn_upsert(
    staged_turns: &mut Vec<ConversationTurn>,
    staged_items: &[ConversationItem],
    turn: &ConversationTurn,
) -> Result<(), ProjectionError> {
    if let Some(index) = turn_index(staged_turns, &turn.turn_id) {
        let previous = &staged_turns[index];
        if turn.revision == previous.revision {
            if turn != previous {
                return Err(ProjectionError::RevisionConflict);
            }
            return Ok(());
        }
        if turn.revision != checked_next_revision(previous.revision)? {
            return Err(ProjectionError::RevisionConflict);
        }
        if turn.ordinal != previous.ordinal || turn.created_at != previous.created_at {
            return Err(ProjectionError::IdentityConflict);
        }
        previous
            .lifecycle
            .validate_transition(turn.lifecycle)
            .map_err(ProjectionError::Lifecycle)?;
        ensure_nondecreasing(previous.updated_at, turn.updated_at)?;
        staged_turns[index] = turn.clone();
        Ok(())
    } else {
        if staged_turns.len() >= usize::from(CONVERSATION_QUERY_MAX_TURNS) {
            return Err(ProjectionError::RetentionExceeded);
        }
        if turn.revision != Revision::default() {
            return Err(ProjectionError::RevisionConflict);
        }
        if ordinal_taken(staged_turns, staged_items, turn.ordinal.get()) {
            return Err(ProjectionError::IdentityConflict);
        }
        if raw_identity_taken_by_other_kind(
            staged_turns,
            staged_items,
            RawKind::Turn(turn.turn_id.as_str()),
        ) {
            return Err(ProjectionError::IdentityConflict);
        }
        if !created_not_after_updated(turn.created_at, turn.updated_at) {
            return Err(ProjectionError::TimeOrdering);
        }
        staged_turns.push(turn.clone());
        Ok(())
    }
}

/// Applies one complete item value: insert-at-revision-zero with its turn
/// already present at this patch position, or the known ladder with every
/// immutable field enforced. Insertion budgets are charged before cloning.
fn apply_item_upsert(
    staged_turns: &[ConversationTurn],
    staged_items: &mut Vec<ConversationItem>,
    retention: &mut Retention,
    item: &ConversationItem,
) -> Result<(), ProjectionError> {
    if let Some(index) = item_index(staged_items, item.item_id()) {
        let previous_kind_is_user = matches!(staged_items[index], ConversationItem::UserMessage(_));
        let next_kind_is_user = matches!(item, ConversationItem::UserMessage(_));
        if previous_kind_is_user != next_kind_is_user {
            return Err(ProjectionError::IdentityConflict);
        }
        if let (
            ConversationItem::AssistantMessage(previous),
            ConversationItem::AssistantMessage(next),
        ) = (&staged_items[index], item)
            && previous.run_id != next.run_id
        {
            return Err(ProjectionError::IdentityConflict);
        }
        let previous_revision = entity_revision(&staged_items[index]);
        let next_revision = entity_revision(item);
        if next_revision == previous_revision {
            if staged_items[index] != *item {
                return Err(ProjectionError::RevisionConflict);
            }
            return Ok(());
        }
        if next_revision != checked_next_revision(previous_revision)? {
            return Err(ProjectionError::RevisionConflict);
        }
        let previous_lifecycle = entity_lifecycle(&staged_items[index]);
        previous_lifecycle
            .validate_transition(entity_lifecycle(item))
            .map_err(ProjectionError::Lifecycle)?;
        ensure_nondecreasing(
            entity_updated_at(&staged_items[index]),
            entity_updated_at(item),
        )?;
        let previous_identity = item_identity(&staged_items[index]);
        let next_identity = item_identity(item);
        if previous_identity.turn_id != next_identity.turn_id
            || previous_identity.ordinal != next_identity.ordinal
            || previous_identity.created_at != next_identity.created_at
        {
            return Err(ProjectionError::IdentityConflict);
        }
        retention
            .replace_body(item_body_len(&staged_items[index]), item_body_len(item))
            .map_err(|_| ProjectionError::RetentionExceeded)?;
        staged_items[index] = item.clone();
        Ok(())
    } else {
        if entity_revision(item) != Revision::default() {
            return Err(ProjectionError::RevisionConflict);
        }
        if turn_index(staged_turns, item.turn_id()).is_none() {
            return Err(ProjectionError::UnknownTarget);
        }
        if raw_identity_taken_by_other_kind(
            staged_turns,
            staged_items,
            RawKind::Item(item.item_id().as_str()),
        ) {
            return Err(ProjectionError::IdentityConflict);
        }
        if ordinal_taken(staged_turns, staged_items, item.ordinal().get()) {
            return Err(ProjectionError::IdentityConflict);
        }
        if !created_not_after_updated(entity_created_at(item), entity_updated_at(item)) {
            return Err(ProjectionError::TimeOrdering);
        }
        retention
            .insert_item(item_body_len(item))
            .map_err(|_| ProjectionError::RetentionExceeded)?;
        staged_items.push(item.clone());
        Ok(())
    }
}

/// Applies one append onto either text-bearing kind: the exact next
/// revision, nondecreasing time, the per-kind body byte bound, and the
/// retention budget are all proven before the concatenated body is
/// allocated, and the domain body constructor stays authoritative.
fn apply_item_append(
    staged_items: &mut [ConversationItem],
    retention: &mut Retention,
    item_id: &ItemId,
    revision: Revision,
    fragment: &str,
    updated_at: UnixMillis,
) -> Result<(), ProjectionError> {
    let index = item_index(staged_items, item_id).ok_or(ProjectionError::UnknownTarget)?;
    let next_revision = checked_next_revision(entity_revision(&staged_items[index]))?;
    if revision != next_revision {
        return Err(ProjectionError::RevisionConflict);
    }
    ensure_nondecreasing(entity_updated_at(&staged_items[index]), updated_at)?;
    let previous_len = item_body_len(&staged_items[index]);
    let joined_len = previous_len
        .checked_add(fragment.len())
        .ok_or(ProjectionError::BodyBoundExceeded)?;
    if joined_len > max_body_bytes(&staged_items[index]) {
        return Err(ProjectionError::BodyBoundExceeded);
    }
    retention
        .replace_body(previous_len, joined_len)
        .map_err(|_| ProjectionError::RetentionExceeded)?;
    match &mut staged_items[index] {
        ConversationItem::UserMessage(message) => {
            let mut joined = String::with_capacity(joined_len);
            joined.push_str(message.body.as_str());
            joined.push_str(fragment);
            message.body =
                MessageBody::parse(joined).map_err(|_| ProjectionError::BodyBoundExceeded)?;
            message.revision = next_revision;
            message.updated_at = updated_at;
        }
        ConversationItem::AssistantMessage(message) => {
            let mut joined = String::with_capacity(joined_len);
            joined.push_str(message.body.as_str());
            joined.push_str(fragment);
            message.body =
                AssistantBody::parse(joined).map_err(|_| ProjectionError::BodyBoundExceeded)?;
            message.revision = next_revision;
            message.updated_at = updated_at;
        }
    }
    Ok(())
}

/// Advances one item's lifecycle in place: exact next revision,
/// nondecreasing time, and the sealed-terminal transition rule.
fn apply_item_lifecycle(
    staged_items: &mut [ConversationItem],
    item_id: &ItemId,
    revision: Revision,
    lifecycle: ConversationLifecycle,
    updated_at: UnixMillis,
) -> Result<(), ProjectionError> {
    let index = item_index(staged_items, item_id).ok_or(ProjectionError::UnknownTarget)?;
    let next_revision = checked_next_revision(entity_revision(&staged_items[index]))?;
    if revision != next_revision {
        return Err(ProjectionError::RevisionConflict);
    }
    ensure_nondecreasing(entity_updated_at(&staged_items[index]), updated_at)?;
    entity_lifecycle(&staged_items[index])
        .validate_transition(lifecycle)
        .map_err(ProjectionError::Lifecycle)?;
    match &mut staged_items[index] {
        ConversationItem::UserMessage(message) => {
            message.revision = next_revision;
            message.lifecycle = lifecycle;
            message.updated_at = updated_at;
        }
        ConversationItem::AssistantMessage(message) => {
            message.revision = next_revision;
            message.lifecycle = lifecycle;
            message.updated_at = updated_at;
        }
    }
    Ok(())
}

/// Advances one turn's lifecycle in place under the same delta rules.
fn apply_turn_lifecycle(
    staged_turns: &mut [ConversationTurn],
    turn_id: &TurnId,
    revision: Revision,
    lifecycle: ConversationLifecycle,
    updated_at: UnixMillis,
) -> Result<(), ProjectionError> {
    let index = turn_index(staged_turns, turn_id).ok_or(ProjectionError::UnknownTarget)?;
    let next_revision = checked_next_revision(staged_turns[index].revision)?;
    if revision != next_revision {
        return Err(ProjectionError::RevisionConflict);
    }
    ensure_nondecreasing(staged_turns[index].updated_at, updated_at)?;
    staged_turns[index]
        .lifecycle
        .validate_transition(lifecycle)
        .map_err(ProjectionError::Lifecycle)?;
    staged_turns[index].revision = next_revision;
    staged_turns[index].lifecycle = lifecycle;
    staged_turns[index].updated_at = updated_at;
    Ok(())
}

/// Identity facets that may never change for a known conversation entity.
struct EntityIdentity<'a> {
    /// Owning turn of an item (turns carry their own immutable identity).
    turn_id: &'a TurnId,
    /// Shared ordinal position.
    ordinal: ItemOrdinal,
    /// Creation instant.
    created_at: UnixMillis,
}

const fn item_identity(item: &ConversationItem) -> EntityIdentity<'_> {
    EntityIdentity {
        turn_id: item.turn_id(),
        ordinal: item.ordinal(),
        created_at: entity_created_at(item),
    }
}

fn validate_common_turn(
    previous: &ConversationTurn,
    next: &ConversationTurn,
) -> Result<(), ProjectionError> {
    if next.ordinal != previous.ordinal || next.created_at != previous.created_at {
        return Err(ProjectionError::IdentityConflict);
    }
    if next.revision < previous.revision {
        return Err(ProjectionError::RevisionConflict);
    }
    if next.revision == previous.revision && next != previous {
        return Err(ProjectionError::RevisionConflict);
    }
    if next.revision > previous.revision {
        previous
            .lifecycle
            .validate_transition(next.lifecycle)
            .map_err(ProjectionError::Lifecycle)?;
        if previous.updated_at.as_millis() > next.updated_at.as_millis() {
            return Err(ProjectionError::TimeOrdering);
        }
    }
    Ok(())
}

fn validate_common_item(
    previous: &ConversationItem,
    next: &ConversationItem,
) -> Result<(), ProjectionError> {
    let previous_kind_is_user = matches!(previous, ConversationItem::UserMessage(_));
    let next_kind_is_user = matches!(next, ConversationItem::UserMessage(_));
    if previous_kind_is_user != next_kind_is_user {
        return Err(ProjectionError::IdentityConflict);
    }
    if let (
        ConversationItem::AssistantMessage(previous_message),
        ConversationItem::AssistantMessage(next_message),
    ) = (previous, next)
        && previous_message.run_id != next_message.run_id
    {
        return Err(ProjectionError::IdentityConflict);
    }
    if next.item_id() != previous.item_id()
        || next.turn_id() != previous.turn_id()
        || next.ordinal() != previous.ordinal()
    {
        return Err(ProjectionError::IdentityConflict);
    }
    let previous_created = entity_created_at(previous);
    let next_created = entity_created_at(next);
    if previous_created != next_created {
        return Err(ProjectionError::IdentityConflict);
    }
    let previous_revision = entity_revision(previous);
    let next_revision = entity_revision(next);
    if next_revision < previous_revision {
        return Err(ProjectionError::RevisionConflict);
    }
    if next_revision == previous_revision && previous != next {
        return Err(ProjectionError::RevisionConflict);
    }
    if next_revision > previous_revision {
        entity_lifecycle(previous)
            .validate_transition(entity_lifecycle(next))
            .map_err(ProjectionError::Lifecycle)?;
        if entity_updated_at(previous).as_millis() > entity_updated_at(next).as_millis() {
            return Err(ProjectionError::TimeOrdering);
        }
    }
    Ok(())
}

fn validate_snapshot_semantics(snapshot: &ConversationSnapshot) -> Result<(), ProjectionError> {
    let watermark = snapshot.updated_at();
    for turn in snapshot.turns() {
        if !created_not_after_updated(turn.created_at, turn.updated_at) {
            return Err(ProjectionError::TimeOrdering);
        }
        if turn.updated_at.as_millis() > watermark.as_millis() {
            return Err(ProjectionError::TimeOrdering);
        }
    }
    let mut item_texts: HashSet<&str> = HashSet::with_capacity(snapshot.items().len());
    for item in snapshot.items() {
        if !created_not_after_updated(entity_created_at(item), entity_updated_at(item)) {
            return Err(ProjectionError::TimeOrdering);
        }
        if entity_updated_at(item).as_millis() > watermark.as_millis() {
            return Err(ProjectionError::TimeOrdering);
        }
        item_texts.insert(item.item_id().as_str());
    }
    // The durable ledger mints turn and item identities from one globally
    // unique entity_id column, so identical raw text across the two kinds
    // can never name two real entities.
    for turn in snapshot.turns() {
        if item_texts.contains(turn.turn_id.as_str()) {
            return Err(ProjectionError::IdentityConflict);
        }
    }
    Ok(())
}

fn checked_next_revision(revision: Revision) -> Result<Revision, ProjectionError> {
    revision
        .checked_next()
        .map_err(|_| ProjectionError::RevisionConflict)
}

fn ensure_nondecreasing(previous: UnixMillis, next: UnixMillis) -> Result<(), ProjectionError> {
    if previous.as_millis() > next.as_millis() {
        return Err(ProjectionError::TimeOrdering);
    }
    Ok(())
}

const fn max_body_bytes(item: &ConversationItem) -> usize {
    match item {
        ConversationItem::UserMessage(_) => MESSAGE_BODY_MAX_BYTES,
        ConversationItem::AssistantMessage(_) => AssistantBody::MAX_BYTES,
    }
}

const fn entity_created_at(item: &ConversationItem) -> UnixMillis {
    match item {
        ConversationItem::UserMessage(message) => message.created_at,
        ConversationItem::AssistantMessage(message) => message.created_at,
    }
}

const fn entity_updated_at(item: &ConversationItem) -> UnixMillis {
    match item {
        ConversationItem::UserMessage(message) => message.updated_at,
        ConversationItem::AssistantMessage(message) => message.updated_at,
    }
}

const fn entity_revision(item: &ConversationItem) -> Revision {
    match item {
        ConversationItem::UserMessage(message) => message.revision,
        ConversationItem::AssistantMessage(message) => message.revision,
    }
}

const fn entity_lifecycle(item: &ConversationItem) -> ConversationLifecycle {
    match item {
        ConversationItem::UserMessage(message) => message.lifecycle,
        ConversationItem::AssistantMessage(message) => message.lifecycle,
    }
}
