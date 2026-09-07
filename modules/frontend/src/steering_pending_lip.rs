//! Immutable-value state for the stack of pending steering acknowledgement
//! lips.
//!
//! This is the native counterpart of
//! `lib/thread-interaction/steering-pending-lip.ts`. A lip remains visible
//! until the receipt that owns its generation releases it. New lips are
//! prepended, so the pending stack is always newest first. Every transition
//! borrows its input and returns a new value; a caller can therefore keep an
//! older snapshot while an asynchronous settlement works on a later one.
//!
//! The TypeScript state is assembled by the composer and borrowed by the
//! steering stages. The Rust value is likewise deliberately unaware of
//! timers, effects, rendering, or the composer payload's shape. It only owns
//! generation, the caller's millisecond stamp, and a generic submission value.
//!
//! Generations start at one. `u64::MAX` is a valid final generation, but no
//! generation after it exists: allocation uses checked addition and returns
//! [`SteeringPendingLipError::GenerationExhausted`] instead of wrapping to
//! zero. An exhausted begin leaves the input state untouched. `from_parts`
//! can represent a restored or deliberately malformed value, so release
//! retains the TypeScript filter semantics even for duplicate generation
//! entries: every entry with the addressed generation is removed, and entries
//! with any other generation remain in their original order.

use std::fmt;

/// One submitted steer that is still represented by a pending lip.
///
/// The generation is the sole ownership key used by
/// [`release_pending_steering_lip`]. The timestamp is an opaque millisecond
/// value supplied by the caller; this module never reads the clock or
/// interprets the value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingSteeringLip<Submission> {
    generation: u64,
    started_at: u64,
    submission: Submission,
}

impl<Submission> PendingSteeringLip<Submission> {
    /// Creates one pending lip from its caller-owned values.
    #[must_use]
    pub const fn new(generation: u64, started_at: u64, submission: Submission) -> Self {
        Self {
            generation,
            started_at,
            submission,
        }
    }

    /// Returns the generation whose settlement owns this lip.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the caller-provided millisecond stamp.
    #[must_use]
    pub const fn started_at(&self) -> u64 {
        self.started_at
    }

    /// Borrows the submitted payload without changing the lip.
    #[must_use]
    pub const fn submission(&self) -> &Submission {
        &self.submission
    }

    /// Recovers the submitted payload when the caller no longer needs the
    /// lip record itself.
    #[must_use]
    pub fn into_submission(self) -> Submission {
        self.submission
    }
}

/// Local ownership state for the newest-first pending-lip stack.
///
/// The fields stay private so the only normal transitions are the
/// immutable-value operations in this module. [`Self::from_parts`] exists for
/// callers that need to restore a value or inspect how the release operation
/// behaves when handed an impossible state; it intentionally does not sort,
/// deduplicate, or otherwise rewrite the supplied stack.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteeringPendingLipState<Submission> {
    next_generation: u64,
    pending: Vec<PendingSteeringLip<Submission>>,
}

impl<Submission> SteeringPendingLipState<Submission> {
    /// Creates the empty composer state used before the first steer.
    ///
    /// The first successful begin consequently receives generation `1`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_generation: 0,
            pending: Vec::new(),
        }
    }

    /// Creates a state from its serialized/value parts without normalization.
    ///
    /// Normal operation should use [`Self::new`] and
    /// [`begin_pending_steering_lip`]. This constructor is still useful for a
    /// caller restoring an already-owned value and makes the state machine's
    /// behavior explicit for duplicate or otherwise impossible entries.
    #[must_use]
    pub fn from_parts(next_generation: u64, pending: Vec<PendingSteeringLip<Submission>>) -> Self {
        Self {
            next_generation,
            pending,
        }
    }

    /// Returns the last generation allocated by a successful begin.
    #[must_use]
    pub const fn next_generation(&self) -> u64 {
        self.next_generation
    }

    /// Returns the pending lips in render order: newest first.
    #[must_use]
    pub fn pending(&self) -> &[PendingSteeringLip<Submission>] {
        &self.pending
    }

    /// Returns whether no pending lip is currently visible.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Returns the number of pending lips currently visible.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }
}

impl<Submission> Default for SteeringPendingLipState<Submission> {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a pending-lip transition could not allocate a new generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SteeringPendingLipError {
    /// The last representable generation is already recorded in the state.
    /// No generation is reused and no pending entry is added.
    GenerationExhausted,
}

impl fmt::Display for SteeringPendingLipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GenerationExhausted => {
                formatter.write_str("steering pending lip generation space is exhausted")
            }
        }
    }
}

impl std::error::Error for SteeringPendingLipError {}

/// The value returned when a pending lip is successfully begun.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BeginPendingSteeringLip<Submission> {
    /// The newly allocated lip, also present at the front of [`Self::state`].
    pub begun: PendingSteeringLip<Submission>,
    /// The post-begin value; the input state was not modified.
    pub state: SteeringPendingLipState<Submission>,
}

/// The value returned when a pending lip release is attempted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasePendingSteeringLip<Submission> {
    /// Whether at least one entry owned by `generation` was removed.
    pub released: bool,
    /// The post-release value; the input state was not modified.
    pub state: SteeringPendingLipState<Submission>,
}

/// Prepends one pending lip and allocates its next generation.
///
/// This is the value-oriented port of `begin_pending_steering_lip`. The
/// existing state is borrowed and never changed. Because the returned
/// [`BeginPendingSteeringLip::begun`] and the returned stack both own the
/// payload, `Submission` is cloned once for the stack's copy. The caller's
/// original input state remains an independent value from its perspective.
///
/// # Errors
///
/// Returns [`SteeringPendingLipError::GenerationExhausted`] when
/// `state.next_generation` is `u64::MAX`. The state is unchanged and the
/// counter never wraps.
pub fn begin_pending_steering_lip<Submission: Clone>(
    state: &SteeringPendingLipState<Submission>,
    submission: Submission,
    started_at: u64,
) -> Result<BeginPendingSteeringLip<Submission>, SteeringPendingLipError> {
    let generation = state
        .next_generation
        .checked_add(1)
        .ok_or(SteeringPendingLipError::GenerationExhausted)?;
    let begun = PendingSteeringLip::new(generation, started_at, submission);
    let mut pending = Vec::with_capacity(state.pending.len() + 1);
    pending.push(begun.clone());
    pending.extend(state.pending.iter().cloned());

    Ok(BeginPendingSteeringLip {
        begun,
        state: SteeringPendingLipState::from_parts(generation, pending),
    })
}

/// Releases only entries owned by the supplied generation.
///
/// This is the value-oriented port of `release_pending_steering_lip`. A
/// missing generation returns `released == false` and an equal state. In a
/// valid stack generations are unique; if a restored/impossible state has
/// duplicates, all entries with that exact generation are filtered just as
/// the TypeScript implementation's `filter` does. A settlement for an older
/// generation therefore cannot remove a newer entry.
#[must_use]
pub fn release_pending_steering_lip<Submission: Clone>(
    state: &SteeringPendingLipState<Submission>,
    generation: u64,
) -> ReleasePendingSteeringLip<Submission> {
    let released = state.pending.iter().any(|lip| lip.generation == generation);
    if !released {
        return ReleasePendingSteeringLip {
            released: false,
            state: state.clone(),
        };
    }

    let pending = state
        .pending
        .iter()
        .filter(|lip| lip.generation != generation)
        .cloned()
        .collect();
    ReleasePendingSteeringLip {
        released: true,
        state: SteeringPendingLipState::from_parts(state.next_generation, pending),
    }
}
