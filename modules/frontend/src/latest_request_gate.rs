//! Monotonic freshness fencing for component-local asynchronous work.
//!
//! [`LatestRequestGate`] is the native equivalent of the editor route's
//! `MakeLatestRequestGate`: every successful [`LatestRequestGate::begin`]
//! obtains a newer generation, and [`LatestRequestGate::is_current`] accepts
//! only the generation currently held by the gate. A caller can therefore
//! ignore a completion that belongs to an older request without coupling this
//! small state type to a transport or executor.
//!
//! The counter is shared atomically by clones, so the gate can be handed to
//! request producers and completion handlers on different threads. A check is
//! a snapshot, not a lock around the caller's eventual publication: another
//! `begin` may supersede it immediately afterward. The caller owns the work
//! and must perform its final freshness check at the point where it would
//! publish a result. This type deliberately does not cancel or interrupt that
//! work.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Why a new request generation could not be allocated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LatestRequestGateError {
    /// The representable generation space is exhausted.
    ///
    /// The gate remains at `u64::MAX`; it never wraps to zero or any earlier
    /// generation. Callers should treat this as a permanent refusal for the
    /// lifetime of this gate.
    GenerationExhausted,
}

impl fmt::Display for LatestRequestGateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GenerationExhausted => {
                formatter.write_str("latest request generation is exhausted")
            }
        }
    }
}

impl std::error::Error for LatestRequestGateError {}

/// Shared monotonic generation state for one component's latest-request gate.
///
/// Cloning a gate shares its counter rather than copying it. Successful
/// [`Self::begin`] calls are serialized by the atomic read-modify-write and
/// receive distinct, strictly increasing `u64` generations in that atomic
/// order. [`Self::is_current`] is an exact atomic snapshot comparison.
///
/// This type fences result publication but owns no request task, future, or
/// cancellation signal. The owner of an asynchronous operation decides when
/// to stop it and must check the returned generation before applying a result.
#[derive(Clone, Debug)]
pub struct LatestRequestGate {
    generation: Arc<AtomicU64>,
}

impl Default for LatestRequestGate {
    fn default() -> Self {
        Self::from_current_generation(0)
    }
}

impl LatestRequestGate {
    /// Creates a gate whose initial current generation is zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a gate starting at an explicitly supplied current generation.
    ///
    /// Ordinary component-local gates should use [`Self::new`]. This
    /// constructor is useful when an embedding owner already has a generation
    /// epoch to adopt, and it also makes the finite overflow policy testable
    /// without attempting billions of requests. The same checked monotonic
    /// rule applies from the supplied value; supplying `u64::MAX` creates an
    /// already-exhausted gate.
    #[must_use]
    pub fn from_current_generation(generation: u64) -> Self {
        Self {
            generation: Arc::new(AtomicU64::new(generation)),
        }
    }

    /// Begins a new request and returns its strictly newer generation.
    ///
    /// The atomic increment is checked. If the current generation is already
    /// `u64::MAX`, this returns [`LatestRequestGateError::GenerationExhausted`]
    /// and leaves the gate at `u64::MAX`; no old generation can become current
    /// again through wraparound. A successful call linearizes at its atomic
    /// increment, which is the ordering to use when concurrent callers begin
    /// requests.
    ///
    /// # Errors
    ///
    /// Returns [`LatestRequestGateError::GenerationExhausted`] after the
    /// counter reaches the end of its `u64` representation.
    pub fn begin(&self) -> Result<u64, LatestRequestGateError> {
        let previous = self
            .generation
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                current.checked_add(1)
            })
            .map_err(|_| LatestRequestGateError::GenerationExhausted)?;

        previous
            .checked_add(1)
            .ok_or(LatestRequestGateError::GenerationExhausted)
    }

    /// Returns whether `candidate` is the gate's current generation.
    ///
    /// This is an exact equality check against one atomic snapshot. It does
    /// not reserve the generation or keep a later [`Self::begin`] from
    /// superseding it, so callers should make this check immediately before
    /// the side effect they want to fence.
    #[must_use]
    pub fn is_current(&self, candidate: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == candidate
    }
}
