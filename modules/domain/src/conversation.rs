//! Conversation snapshots and bounded replay values.
//!
//! This module carries only renderer-facing durable state. It intentionally
//! stops before runs, engines, providers, dispatch, and storage. Forge mints
//! entity and patch identities; counters express ordering without conflating
//! identities with positions.

use std::collections::HashSet;

use thiserror::Error;

use crate::bounds::{
    CONVERSATION_PATCH_BATCH_MAX_PATCHES, CONVERSATION_QUERY_MAX_TURNS,
    CONVERSATION_TEXT_FRAGMENT_MAX_BYTES,
};
use crate::identifiers::{ItemId, PatchId, ThreadId, TurnId};
use crate::text::MessageBody;
use crate::time::UnixMillis;

/// Failure while advancing a bounded conversation counter.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CounterError {
    /// Patch zero is reserved for the cursor before the first patch.
    #[error("patch sequence must be greater than zero")]
    ZeroPatchSequence,
    /// The counter cannot advance beyond its integer representation.
    #[error("{counter} counter overflowed at {value}")]
    Overflow {
        /// Name of the counter that could not advance.
        counter: &'static str,
        /// Value at the representation boundary.
        value: u64,
    },
}

macro_rules! zero_based_counter {
    ($(#[$docs:meta])* $name:ident) => {
        $(#[$docs])*
        #[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(u64);

        impl $name {
            /// Creates a counter. Zero is a valid first value.
            #[must_use]
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            /// Returns the integer representation.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }
    };
}

zero_based_counter! {
    /// Last patch sequence a subscriber has applied.
    ///
    /// Zero means no patch has been applied yet.
    ConversationCursor
}

zero_based_counter! {
    /// Stable zero-based position of one turn in a conversation.
    TurnOrdinal
}

zero_based_counter! {
    /// Stable zero-based position of one item in a conversation.
    ItemOrdinal
}

zero_based_counter! {
    /// Zero-based revision of one turn or item.
    Revision
}

impl Revision {
    /// Advances the revision without integer wraparound.
    ///
    /// # Errors
    ///
    /// Returns [`CounterError::Overflow`] at [`u64::MAX`].
    pub fn checked_next(self) -> Result<Self, CounterError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(CounterError::Overflow {
                counter: "revision",
                value: self.0,
            })
    }
}

/// One-based sequence of a durable replay patch.
///
/// Patch zero never exists. [`ConversationCursor::default`] is the explicit
/// zero sentinel before patch one.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PatchSequence(u64);

impl PatchSequence {
    /// Creates a one-based patch sequence.
    ///
    /// # Errors
    ///
    /// Returns [`CounterError::ZeroPatchSequence`] for zero.
    pub const fn new(value: u64) -> Result<Self, CounterError> {
        if value == 0 {
            Err(CounterError::ZeroPatchSequence)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the integer representation.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Advances the sequence without integer wraparound.
    ///
    /// # Errors
    ///
    /// Returns [`CounterError::Overflow`] at [`u64::MAX`].
    pub fn checked_next(self) -> Result<Self, CounterError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(CounterError::Overflow {
                counter: "patch sequence",
                value: self.0,
            })
    }
}

impl From<PatchSequence> for ConversationCursor {
    fn from(value: PatchSequence) -> Self {
        Self(value.get())
    }
}

impl ConversationCursor {
    /// Returns the first sequence after this cursor.
    ///
    /// # Errors
    ///
    /// Returns [`CounterError::Overflow`] when the cursor is [`u64::MAX`].
    pub fn checked_next_sequence(self) -> Result<PatchSequence, CounterError> {
        self.0
            .checked_add(1)
            .map(PatchSequence)
            .ok_or(CounterError::Overflow {
                counter: "conversation cursor",
                value: self.0,
            })
    }
}

/// Validation failure for one streamed text fragment.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum IncrementalTextError {
    /// The fragment exceeded its renderer-facing byte ceiling.
    #[error("incremental text is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// Offending length in UTF-8 bytes.
        length: usize,
        /// Documented fragment ceiling.
        maximum: usize,
    },
}

/// One bounded incremental text fragment.
///
/// Empty is valid because a stream may be opened before its first visible
/// token. Complete stored user messages use [`MessageBody`] instead.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct IncrementalText(String);

impl IncrementalText {
    /// Validates one external fragment using a UTF-8 byte bound.
    ///
    /// # Errors
    ///
    /// Returns [`IncrementalTextError::TooLong`] when the fragment exceeds
    /// [`CONVERSATION_TEXT_FRAGMENT_MAX_BYTES`].
    pub fn parse(value: impl Into<String>) -> Result<Self, IncrementalTextError> {
        let value = value.into();
        let length = value.len();
        if length > CONVERSATION_TEXT_FRAGMENT_MAX_BYTES {
            return Err(IncrementalTextError::TooLong {
                length,
                maximum: CONVERSATION_TEXT_FRAGMENT_MAX_BYTES,
            });
        }
        Ok(Self(value))
    }

    /// Returns the validated fragment exactly as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Renderer-visible lifecycle shared by conversation turns and items.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConversationLifecycle {
    /// Durable entity exists but work has not started.
    Pending,
    /// Text or reasoning is arriving incrementally.
    Streaming,
    /// Work is actively progressing.
    Active,
    /// Work is waiting for input or another dependency.
    Waiting,
    /// Work completed successfully.
    Completed,
    /// Work ended because of a failure.
    Failed,
    /// Work was externally stopped and may be resumed.
    Interrupted,
    /// Work was deliberately cancelled.
    Cancelled,
}

impl ConversationLifecycle {
    /// Whether the lifecycle is sealed against later transitions.
    ///
    /// Interrupted is deliberately not terminal: the same activity may resume.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Minimal durable representation of one canonical conversation turn.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationTurn {
    /// Forge-minted turn identity.
    pub turn_id: TurnId,
    /// Stable position in the containing conversation.
    pub ordinal: TurnOrdinal,
    /// Current entity revision; newly queued turns start at zero.
    pub revision: Revision,
    /// Renderer-visible lifecycle.
    pub lifecycle: ConversationLifecycle,
    /// Creation time as signed Unix epoch milliseconds.
    pub created_at: UnixMillis,
    /// Last update time as signed Unix epoch milliseconds.
    pub updated_at: UnixMillis,
}

/// One durably queued user-message item.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UserMessageItem {
    /// Forge-minted item identity.
    pub item_id: ItemId,
    /// Turn that owns the message.
    pub turn_id: TurnId,
    /// Stable position in the containing conversation.
    pub ordinal: ItemOrdinal,
    /// Current entity revision; newly queued items start at zero.
    pub revision: Revision,
    /// Renderer-visible lifecycle.
    pub lifecycle: ConversationLifecycle,
    /// Complete, bounded body stored durably by Forge.
    pub body: MessageBody,
    /// Creation time as signed Unix epoch milliseconds.
    pub created_at: UnixMillis,
    /// Last update time as signed Unix epoch milliseconds.
    pub updated_at: UnixMillis,
}

/// Renderer-visible conversation item vocabulary for this phase.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ConversationItem {
    /// Canonical user input durably queued before any engine dispatch.
    UserMessage(UserMessageItem),
}

impl ConversationItem {
    /// Returns the Forge-minted item identity.
    #[must_use]
    pub const fn item_id(&self) -> &ItemId {
        match self {
            Self::UserMessage(item) => &item.item_id,
        }
    }

    /// Returns the owning turn identity.
    #[must_use]
    pub const fn turn_id(&self) -> &TurnId {
        match self {
            Self::UserMessage(item) => &item.turn_id,
        }
    }

    /// Returns the stable item ordinal.
    #[must_use]
    pub const fn ordinal(&self) -> ItemOrdinal {
        match self {
            Self::UserMessage(item) => item.ordinal,
        }
    }
}

/// Structural failure in a conversation snapshot.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ConversationSnapshotError {
    /// Two turns reused the same Forge identity.
    #[error("conversation snapshot contains duplicate turn id {turn_id}")]
    DuplicateTurnId {
        /// Reused identity.
        turn_id: TurnId,
    },
    /// Two items reused the same Forge identity.
    #[error("conversation snapshot contains duplicate item id {item_id}")]
    DuplicateItemId {
        /// Reused identity.
        item_id: ItemId,
    },
    /// A turn and/or item reused a globally allocated ordinal.
    #[error("conversation snapshot contains duplicate ordinal {ordinal}")]
    DuplicateOrdinal {
        /// Reused zero-based ordinal.
        ordinal: u64,
    },
    /// An item referenced a turn absent from the snapshot.
    #[error("conversation item {item_id} references unknown turn {turn_id}")]
    UnknownTurn {
        /// Item carrying the invalid reference.
        item_id: ItemId,
        /// Missing turn identity.
        turn_id: TurnId,
    },
}

/// Canonical renderer snapshot at one per-thread replay cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationSnapshot {
    thread_id: ThreadId,
    cursor: ConversationCursor,
    turns: Vec<ConversationTurn>,
    items: Vec<ConversationItem>,
    updated_at: UnixMillis,
}

impl ConversationSnapshot {
    /// Builds a snapshot after validating entity identity, ordinal, and turn
    /// ownership invariants.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationSnapshotError`] for duplicate identities,
    /// duplicate globally allocated ordinals, or an item whose turn is absent.
    pub fn new(
        thread_id: ThreadId,
        cursor: ConversationCursor,
        mut turns: Vec<ConversationTurn>,
        mut items: Vec<ConversationItem>,
        updated_at: UnixMillis,
    ) -> Result<Self, ConversationSnapshotError> {
        let mut turn_ids = HashSet::with_capacity(turns.len());
        let mut ordinals = HashSet::with_capacity(turns.len() + items.len());
        for turn in &turns {
            if !turn_ids.insert(turn.turn_id.clone()) {
                return Err(ConversationSnapshotError::DuplicateTurnId {
                    turn_id: turn.turn_id.clone(),
                });
            }
            if !ordinals.insert(turn.ordinal.get()) {
                return Err(ConversationSnapshotError::DuplicateOrdinal {
                    ordinal: turn.ordinal.get(),
                });
            }
        }

        let mut item_ids = HashSet::with_capacity(items.len());
        for item in &items {
            if !item_ids.insert(item.item_id().clone()) {
                return Err(ConversationSnapshotError::DuplicateItemId {
                    item_id: item.item_id().clone(),
                });
            }
            if !turn_ids.contains(item.turn_id()) {
                return Err(ConversationSnapshotError::UnknownTurn {
                    item_id: item.item_id().clone(),
                    turn_id: item.turn_id().clone(),
                });
            }
            if !ordinals.insert(item.ordinal().get()) {
                return Err(ConversationSnapshotError::DuplicateOrdinal {
                    ordinal: item.ordinal().get(),
                });
            }
        }

        turns.sort_by_key(|turn| turn.ordinal);
        items.sort_by_key(ConversationItem::ordinal);

        Ok(Self {
            thread_id,
            cursor,
            turns,
            items,
            updated_at,
        })
    }

    /// Returns the thread this projection belongs to.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the last patch incorporated into the snapshot.
    #[must_use]
    pub const fn cursor(&self) -> ConversationCursor {
        self.cursor
    }

    /// Returns canonical turns in stable ordinal order.
    #[must_use]
    pub fn turns(&self) -> &[ConversationTurn] {
        &self.turns
    }

    /// Returns renderer-visible items in stable ordinal order.
    #[must_use]
    pub fn items(&self) -> &[ConversationItem] {
        &self.items
    }

    /// Returns the projection update time.
    #[must_use]
    pub const fn updated_at(&self) -> UnixMillis {
        self.updated_at
    }
}

/// One sequenced mutation against a conversation snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationPatch {
    /// Inserts or replaces one canonical turn.
    TurnUpsert {
        /// Forge-minted patch identity.
        patch_id: PatchId,
        /// Contiguous replay sequence.
        sequence: PatchSequence,
        /// Complete turn value at its current revision.
        turn: ConversationTurn,
    },
    /// Inserts or replaces one renderer-visible item.
    ItemUpsert {
        /// Forge-minted patch identity.
        patch_id: PatchId,
        /// Contiguous replay sequence.
        sequence: PatchSequence,
        /// Complete item value at its current revision.
        item: ConversationItem,
    },
    /// Appends a bounded fragment to a text-bearing item.
    ItemAppend {
        /// Forge-minted patch identity.
        patch_id: PatchId,
        /// Contiguous replay sequence.
        sequence: PatchSequence,
        /// Target item identity.
        item_id: ItemId,
        /// Revision after applying this append.
        revision: Revision,
        /// Exact incremental fragment; empty is permitted.
        text: IncrementalText,
    },
    /// Advances an item's renderer lifecycle.
    ItemLifecycle {
        /// Forge-minted patch identity.
        patch_id: PatchId,
        /// Contiguous replay sequence.
        sequence: PatchSequence,
        /// Target item identity.
        item_id: ItemId,
        /// Revision after applying this transition.
        revision: Revision,
        /// New lifecycle.
        lifecycle: ConversationLifecycle,
    },
    /// Advances a turn's renderer lifecycle.
    TurnLifecycle {
        /// Forge-minted patch identity.
        patch_id: PatchId,
        /// Contiguous replay sequence.
        sequence: PatchSequence,
        /// Target turn identity.
        turn_id: TurnId,
        /// Revision after applying this transition.
        revision: Revision,
        /// New lifecycle.
        lifecycle: ConversationLifecycle,
    },
}

impl ConversationPatch {
    /// Returns the unique Forge-minted patch identity.
    #[must_use]
    pub const fn patch_id(&self) -> &PatchId {
        match self {
            Self::TurnUpsert { patch_id, .. }
            | Self::ItemUpsert { patch_id, .. }
            | Self::ItemAppend { patch_id, .. }
            | Self::ItemLifecycle { patch_id, .. }
            | Self::TurnLifecycle { patch_id, .. } => patch_id,
        }
    }

    /// Returns the one-based replay sequence.
    #[must_use]
    pub const fn sequence(&self) -> PatchSequence {
        match self {
            Self::TurnUpsert { sequence, .. }
            | Self::ItemUpsert { sequence, .. }
            | Self::ItemAppend { sequence, .. }
            | Self::ItemLifecycle { sequence, .. }
            | Self::TurnLifecycle { sequence, .. } => *sequence,
        }
    }
}

/// Failure while constructing one contiguous replay batch.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PatchBatchError {
    /// Replay envelopes may not carry an empty batch.
    #[error("conversation patch batch must not be empty")]
    Empty,
    /// The batch exceeded the legacy replay-read ceiling.
    #[error("conversation patch batch holds {count} patches; the maximum is {maximum}")]
    TooManyPatches {
        /// Offending number of patches.
        count: usize,
        /// Documented batch ceiling.
        maximum: usize,
    },
    /// Two entries in the batch reused one patch identity.
    #[error("conversation patch batch contains duplicate patch id {patch_id}")]
    DuplicatePatchId {
        /// Reused identity.
        patch_id: PatchId,
    },
    /// Two entries reused one sequence.
    #[error("conversation patch batch contains duplicate sequence {sequence}")]
    DuplicateSequence {
        /// Reused one-based sequence.
        sequence: u64,
    },
    /// A sequence moved backwards rather than advancing.
    #[error("conversation patch sequence moved backwards from {previous} to {actual}")]
    OutOfOrder {
        /// Previous sequence or starting cursor.
        previous: u64,
        /// Backwards sequence received.
        actual: u64,
    },
    /// One or more expected sequences were absent.
    #[error("conversation patch gap: expected sequence {expected}, received {actual}")]
    Gap {
        /// Next contiguous sequence required.
        expected: u64,
        /// Later sequence that exposed the gap.
        actual: u64,
    },
    /// The declared end cursor did not match the last patch.
    #[error("conversation patch endpoint mismatch: declared {declared}, actual {actual}")]
    EndpointMismatch {
        /// Cursor declared by the envelope.
        declared: u64,
        /// Sequence of the final patch.
        actual: u64,
    },
    /// A cursor could not be advanced without wraparound.
    #[error(transparent)]
    Counter(#[from] CounterError),
}

/// One non-empty, bounded, contiguous patch replay after a known cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatchBatch {
    thread_id: ThreadId,
    from_cursor: ConversationCursor,
    to_cursor: ConversationCursor,
    patches: Vec<ConversationPatch>,
}

impl PatchBatch {
    /// Validates batch size, identity uniqueness, strict sequence contiguity,
    /// and the declared endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`PatchBatchError`] for an empty or oversized batch, duplicate
    /// identities or sequences, backwards ordering, a gap, cursor overflow,
    /// or an endpoint that disagrees with the final patch.
    pub fn new(
        thread_id: ThreadId,
        from_cursor: ConversationCursor,
        to_cursor: ConversationCursor,
        patches: Vec<ConversationPatch>,
    ) -> Result<Self, PatchBatchError> {
        if patches.is_empty() {
            return Err(PatchBatchError::Empty);
        }
        if patches.len() > CONVERSATION_PATCH_BATCH_MAX_PATCHES {
            return Err(PatchBatchError::TooManyPatches {
                count: patches.len(),
                maximum: CONVERSATION_PATCH_BATCH_MAX_PATCHES,
            });
        }

        let mut patch_ids = HashSet::with_capacity(patches.len());
        let mut sequences = HashSet::with_capacity(patches.len());
        let mut previous = from_cursor.get();
        let mut expected = from_cursor.checked_next_sequence()?.get();
        for (index, patch) in patches.iter().enumerate() {
            if !patch_ids.insert(patch.patch_id().clone()) {
                return Err(PatchBatchError::DuplicatePatchId {
                    patch_id: patch.patch_id().clone(),
                });
            }

            let actual = patch.sequence().get();
            if !sequences.insert(actual) {
                return Err(PatchBatchError::DuplicateSequence { sequence: actual });
            }
            if actual < expected {
                return Err(PatchBatchError::OutOfOrder { previous, actual });
            }
            if actual > expected {
                return Err(PatchBatchError::Gap { expected, actual });
            }

            previous = actual;
            if index + 1 < patches.len() {
                expected = patch.sequence().checked_next()?.get();
            }
        }

        if to_cursor.get() != previous {
            return Err(PatchBatchError::EndpointMismatch {
                declared: to_cursor.get(),
                actual: previous,
            });
        }

        Ok(Self {
            thread_id,
            from_cursor,
            to_cursor,
            patches,
        })
    }

    /// Returns the thread whose projection is advanced.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the subscriber cursor before this batch.
    #[must_use]
    pub const fn from_cursor(&self) -> ConversationCursor {
        self.from_cursor
    }

    /// Returns the cursor after this batch.
    #[must_use]
    pub const fn to_cursor(&self) -> ConversationCursor {
        self.to_cursor
    }

    /// Returns the bounded contiguous patches.
    #[must_use]
    pub fn patches(&self) -> &[ConversationPatch] {
        &self.patches
    }
}

/// Validation failure for a bounded conversation query turn count.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum QueryTurnCountError {
    /// A bounded query must request at least one turn.
    #[error("conversation query turn count must be greater than zero")]
    Zero,
    /// The requested count exceeded the protocol ceiling.
    #[error("conversation query requests {value} turns; the maximum is {maximum}")]
    TooLarge {
        /// Offending requested count.
        value: u64,
        /// Documented maximum.
        maximum: u16,
    },
}

/// Validated turn count for a windowed or ranged query.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct QueryTurnCount(u16);

impl QueryTurnCount {
    /// Creates a count in `1..=512`.
    ///
    /// # Errors
    ///
    /// Returns [`QueryTurnCountError::Zero`] for zero or
    /// [`QueryTurnCountError::TooLarge`] above
    /// [`CONVERSATION_QUERY_MAX_TURNS`].
    pub fn new(value: u64) -> Result<Self, QueryTurnCountError> {
        if value == 0 {
            return Err(QueryTurnCountError::Zero);
        }
        if value > u64::from(CONVERSATION_QUERY_MAX_TURNS) {
            return Err(QueryTurnCountError::TooLarge {
                value,
                maximum: CONVERSATION_QUERY_MAX_TURNS,
            });
        }
        match u16::try_from(value) {
            Ok(value) => Ok(Self(value)),
            Err(_) => Err(QueryTurnCountError::TooLarge {
                value,
                maximum: CONVERSATION_QUERY_MAX_TURNS,
            }),
        }
    }

    /// Returns the validated count.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Scope of one conversation snapshot query.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversationQueryBounds {
    /// Newest bounded turns.
    Window {
        /// Maximum turns to include.
        maximum_turn_count: QueryTurnCount,
    },
    /// Older bounded turns before a loaded ordinal.
    Range {
        /// Exclusive upper ordinal.
        before_turn_ordinal: TurnOrdinal,
        /// Optional inclusive floor for navigation toward one target.
        minimum_turn_ordinal: Option<TurnOrdinal>,
        /// Maximum turns to include.
        maximum_turn_count: QueryTurnCount,
    },
}

/// Request for one canonical conversation snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationQuery {
    /// Thread whose projection is requested.
    pub thread_id: ThreadId,
    /// Full, tail-window, or older-range read.
    pub bounds: ConversationQueryBounds,
}

/// Request to begin authoritative conversation delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationSubscribe {
    /// Thread to observe.
    pub thread_id: ThreadId,
    /// Cursor to resume after, or `None` for a fresh subscription that must
    /// begin with [`ConversationSubscriptionStart`].
    pub after: Option<ConversationCursor>,
}

impl ConversationSubscribe {
    /// Creates a fresh subscription whose first server value must be a snapshot.
    #[must_use]
    pub const fn fresh(thread_id: ThreadId) -> Self {
        Self {
            thread_id,
            after: None,
        }
    }

    /// Resumes delivery strictly after a previously applied cursor.
    #[must_use]
    pub const fn resume(thread_id: ThreadId, after: ConversationCursor) -> Self {
        Self {
            thread_id,
            after: Some(after),
        }
    }
}

/// Request to end authoritative conversation delivery for one thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationUnsubscribe {
    /// Thread no longer observed by the client.
    pub thread_id: ThreadId,
}

/// Client conversation read/subscription request vocabulary.
///
/// Query reads are always bounded; older history is hydrated with additional
/// range requests instead of admitting an unbounded snapshot frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationRequest {
    /// Read one bounded canonical snapshot.
    Query(ConversationQuery),
    /// Begin fresh snapshot-first delivery or resume after a cursor.
    Subscribe(ConversationSubscribe),
    /// End delivery for one thread.
    Unsubscribe(ConversationUnsubscribe),
}

/// Mandatory first value of a fresh conversation subscription.
///
/// Patch batches are follow-up values and cannot substitute for this type,
/// making snapshot-before-patch ordering explicit at the service boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationSubscriptionStart(ConversationSnapshot);

impl ConversationSubscriptionStart {
    /// Marks a validated snapshot as the first value of fresh delivery.
    #[must_use]
    pub const fn new(snapshot: ConversationSnapshot) -> Self {
        Self(snapshot)
    }

    /// Returns the initial authoritative snapshot.
    #[must_use]
    pub const fn snapshot(&self) -> &ConversationSnapshot {
        &self.0
    }
}
