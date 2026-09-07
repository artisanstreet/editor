//! Dependency-free ownership state for overlapping engine usage refreshes.
//!
//! The legacy controller serializes these transitions with an Effect
//! `SubscriptionRef`. This native slice keeps only the state machine: it
//! does not execute refreshes, schedule work, expose streams, or perform any
//! provider I/O. Callers own the surrounding execution and use the returned
//! generation-bearing claims for cleanup.

use std::fmt;

/// The finite identifier space used for refresh generations.
pub type EngineUsageRefreshClaimId = u64;

/// Why a refresh claim could not be allocated.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EngineUsageRefreshError {
    /// Every representable claim identifier has been allocated.
    ///
    /// The state never wraps from [`u64::MAX`] back to zero. A request that
    /// contains only engines that are already active still returns an empty
    /// claim successfully, because it does not need a new identifier.
    GenerationExhausted,
}

impl fmt::Display for EngineUsageRefreshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GenerationExhausted => {
                formatter.write_str("engine usage refresh generation space is exhausted")
            }
        }
    }
}

impl std::error::Error for EngineUsageRefreshError {}

/// Ownership token for one engine refresh.
///
/// The claim ID is a generation, not merely an engine lookup key. A caller
/// must release the exact token it received: an old cleanup token cannot
/// release a newer claim for the same engine after reacquisition.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineUsageRefreshClaim {
    claim_id: EngineUsageRefreshClaimId,
    engine_id: String,
}

impl EngineUsageRefreshClaim {
    /// Returns this claim's monotonic generation identifier.
    #[must_use]
    pub const fn claim_id(&self) -> EngineUsageRefreshClaimId {
        self.claim_id
    }

    /// Returns the engine owned by this claim.
    #[must_use]
    pub fn engine_id(&self) -> &str {
        &self.engine_id
    }

    /// Returns the claim ID as a generation token.
    #[must_use]
    pub const fn generation(&self) -> EngineUsageRefreshClaimId {
        self.claim_id()
    }
}

/// Ordered claim state for one component-scoped refresh coordinator.
///
/// Active claims are retained in insertion order, matching the legacy
/// JavaScript `Map`: returned claims and [`Self::active_engine_ids`] preserve
/// the first requested order, and a released engine is appended at the end
/// when it is later reacquired. The state is deliberately synchronous and
/// owns no task, lock, executor, stream, or provider operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineUsageRefreshState {
    active_claims: Vec<EngineUsageRefreshClaim>,
    next_claim_id: Option<EngineUsageRefreshClaimId>,
}

impl EngineUsageRefreshState {
    /// Creates an empty state whose first claim receives generation zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            active_claims: Vec::new(),
            next_claim_id: Some(0),
        }
    }

    /// Creates empty state starting at `next_claim_id`.
    ///
    /// Normal callers should use [`Self::new`]. This constructor makes the
    /// finite overflow boundary directly testable and can also be used by an
    /// embedding owner that has already reserved an epoch. The supplied ID
    /// is the next ID to allocate, not the last ID already allocated.
    #[must_use]
    pub const fn from_next_claim_id(next_claim_id: EngineUsageRefreshClaimId) -> Self {
        Self {
            active_claims: Vec::new(),
            next_claim_id: Some(next_claim_id),
        }
    }

    /// Claims the unclaimed engines from `requested_engine_ids`.
    ///
    /// Duplicate requested IDs are removed while retaining their first
    /// occurrence. Active engines are skipped. Newly allocated claims and
    /// active-engine snapshots retain insertion order. A request that yields
    /// no new claims leaves every part of the state unchanged, including the
    /// next generation.
    ///
    /// Allocation is transactional at the exhaustion boundary: if the
    /// request needs more IDs than remain, this returns
    /// [`EngineUsageRefreshError::GenerationExhausted`] and does not add a
    /// partial prefix. The final `u64::MAX` ID is usable; after it is
    /// allocated, the state records that no next ID exists instead of
    /// wrapping to zero.
    ///
    /// # Errors
    ///
    /// Returns [`EngineUsageRefreshError::GenerationExhausted`] when at least
    /// one new engine needs a generation but the finite ID space cannot
    /// provide all generations required by this request.
    pub fn claim<I, S>(
        &mut self,
        requested_engine_ids: I,
    ) -> Result<Vec<EngineUsageRefreshClaim>, EngineUsageRefreshError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut unclaimed_engine_ids = Vec::new();
        for requested_engine_id in requested_engine_ids {
            let engine_id = requested_engine_id.as_ref();
            let already_requested = unclaimed_engine_ids
                .iter()
                .any(|existing: &String| existing == engine_id);
            let already_active = self
                .active_claims
                .iter()
                .any(|claim| claim.engine_id() == engine_id);
            if !already_requested && !already_active {
                unclaimed_engine_ids.push(engine_id.to_owned());
            }
        }

        if unclaimed_engine_ids.is_empty() {
            return Ok(Vec::new());
        }

        let Some(next_claim_id) = self.next_claim_id else {
            return Err(EngineUsageRefreshError::GenerationExhausted);
        };
        let claim_count = u64::try_from(unclaimed_engine_ids.len())
            .map_err(|_| EngineUsageRefreshError::GenerationExhausted)?;

        // Use u128 for the inclusive number of IDs from the next one through
        // u64::MAX, whose count is one larger when `next_claim_id` is zero.
        let available_ids = u128::from(u64::MAX) - u128::from(next_claim_id) + 1;
        if u128::from(claim_count) > available_ids {
            return Err(EngineUsageRefreshError::GenerationExhausted);
        }

        let mut claimed = Vec::with_capacity(unclaimed_engine_ids.len());
        for (offset, engine_id) in unclaimed_engine_ids.into_iter().enumerate() {
            let offset =
                u64::try_from(offset).map_err(|_| EngineUsageRefreshError::GenerationExhausted)?;
            let claim_id = next_claim_id
                .checked_add(offset)
                .ok_or(EngineUsageRefreshError::GenerationExhausted)?;
            claimed.push(EngineUsageRefreshClaim {
                claim_id,
                engine_id,
            });
        }

        // `None` is the explicit exhausted state. It occurs only when this
        // request consumed the final ID, because the capacity check above
        // rejected every larger request before any state was changed.
        self.next_claim_id = next_claim_id.checked_add(claim_count);
        self.active_claims.extend(claimed.iter().cloned());
        Ok(claimed)
    }

    /// Releases `claim` only if both its engine and generation are current.
    ///
    /// Returns `true` when the active claim was removed. A stale or repeated
    /// release returns `false` and leaves the state unchanged.
    #[must_use]
    pub fn release(&mut self, claim: &EngineUsageRefreshClaim) -> bool {
        let Some(index) = self.active_claims.iter().position(|current| {
            current.engine_id() == claim.engine_id() && current.generation() == claim.generation()
        }) else {
            return false;
        };

        self.active_claims.remove(index);
        true
    }

    /// Releases each matching claim and returns the number actually removed.
    ///
    /// Claims are processed in input order. Duplicate or stale tokens are
    /// harmless and are counted at most once.
    #[must_use]
    pub fn release_all(&mut self, claims: &[EngineUsageRefreshClaim]) -> usize {
        let mut released = 0;
        for claim in claims {
            if self.release(claim) {
                released += 1;
            }
        }
        released
    }

    /// Returns active engine IDs in the state-machine's insertion order.
    #[must_use]
    pub fn active_engine_ids(&self) -> Vec<String> {
        self.active_claims
            .iter()
            .map(|claim| claim.engine_id().to_owned())
            .collect()
    }

    /// Returns whether no engine currently has an active refresh claim.
    #[must_use]
    pub const fn is_idle(&self) -> bool {
        self.active_claims.is_empty()
    }
}

impl Default for EngineUsageRefreshState {
    fn default() -> Self {
        Self::new()
    }
}
