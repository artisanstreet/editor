//! Pure freshness and storage-repair policy for the frontend engine-usage cache.
//!
//! This module deliberately stops at already-decoded observations. It does
//! not acquire browser storage, provide an Effect service, decode a schema, or
//! parse timestamps. A caller maps a finite parsed fetched-at timestamp to
//! `Some` and maps a missing or invalid timestamp to `None` before calling the
//! freshness policy.

/// The exact browser-storage key used by the engine-usage cache.
pub const ENGINE_USAGE_CACHE_STORAGE_KEY: &str = "artisan.engine-usage-cache";

/// The age at which a cached engine-usage snapshot must be refreshed.
pub const ENGINE_USAGE_REFRESH_WINDOW_MS: i64 = 180_000;

/// Freshness classification for an already-parsed fetched-at observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineUsageFreshness {
    /// No usable fetched-at value was available to the policy.
    MissingOrInvalid,
    /// The snapshot is younger than the refresh window.
    Fresh,
    /// The snapshot is exactly at or older than the refresh window.
    Due,
}

impl EngineUsageFreshness {
    /// Returns whether the caller should fetch a new snapshot.
    #[must_use]
    pub const fn is_due(self) -> bool {
        !matches!(self, Self::Fresh)
    }
}

/// Classifies cache freshness using a parsed signed millisecond timestamp.
///
/// `None` represents either a missing snapshot timestamp or a timestamp that
/// the caller's parser rejected. The subtraction is widened to `i128` before
/// it is performed, preserving the TypeScript comparison for every pair of
/// signed `i64` millisecond values without wrapping at an integer boundary.
#[must_use]
pub fn engine_usage_cache_freshness(
    fetched_at_ms: Option<i64>,
    now_ms: i64,
) -> EngineUsageFreshness {
    let Some(fetched_at_ms) = fetched_at_ms else {
        return EngineUsageFreshness::MissingOrInvalid;
    };

    let age_ms = i128::from(now_ms) - i128::from(fetched_at_ms);
    if age_ms >= i128::from(ENGINE_USAGE_REFRESH_WINDOW_MS) {
        EngineUsageFreshness::Due
    } else {
        EngineUsageFreshness::Fresh
    }
}

/// Returns the boolean freshness policy used by the TypeScript call site.
#[must_use]
pub fn engine_usage_refresh_is_due(fetched_at_ms: Option<i64>, now_ms: i64) -> bool {
    engine_usage_cache_freshness(fetched_at_ms, now_ms).is_due()
}

/// Identifies which dependency rejected a cache read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineUsageCacheReadFailure {
    /// The underlying storage read failed.
    Storage,
    /// Schema decoding or validation failed after the storage read.
    Schema,
}

/// One observation produced by a storage/schema cache load attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineUsageCacheLoadObservation<T> {
    /// A read failure makes the persisted entry unsafe to retain.
    ReadFailure(EngineUsageCacheReadFailure),
    /// The cache key was absent.
    Missing,
    /// The stored value decoded and validated successfully.
    Valid(T),
}

/// Pure policy output for one cache load observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineUsageCacheLoadOutput<T> {
    /// Remove the persisted entry and return no cached value.
    RemoveCorruptEntry,
    /// Return no value without requesting removal.
    Missing,
    /// Return the decoded value unchanged.
    Valid(T),
}

impl<T> EngineUsageCacheLoadOutput<T> {
    /// Returns whether the storage layer must remove the cache entry.
    #[must_use]
    pub const fn requests_corrupt_entry_removal(&self) -> bool {
        matches!(self, Self::RemoveCorruptEntry)
    }

    /// Borrows the cached value, if this output contains one.
    #[must_use]
    pub fn as_value(&self) -> Option<&T> {
        match self {
            Self::Valid(value) => Some(value),
            Self::RemoveCorruptEntry | Self::Missing => None,
        }
    }

    /// Takes ownership of the cached value, if this output contains one.
    #[must_use]
    pub fn into_value(self) -> Option<T> {
        match self {
            Self::Valid(value) => Some(value),
            Self::RemoveCorruptEntry | Self::Missing => None,
        }
    }
}

/// Applies the load repair policy without performing storage or schema I/O.
#[must_use]
pub fn engine_usage_cache_load<T>(
    observation: EngineUsageCacheLoadObservation<T>,
) -> EngineUsageCacheLoadOutput<T> {
    match observation {
        EngineUsageCacheLoadObservation::ReadFailure(_) => {
            EngineUsageCacheLoadOutput::RemoveCorruptEntry
        }
        EngineUsageCacheLoadObservation::Missing => EngineUsageCacheLoadOutput::Missing,
        EngineUsageCacheLoadObservation::Valid(value) => EngineUsageCacheLoadOutput::Valid(value),
    }
}

/// One observation from a cache save attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineUsageCacheSaveObservation {
    /// The underlying save completed successfully.
    Succeeded,
    /// The underlying save failed and must not escape this policy boundary.
    Failed,
}

/// Pure policy output for one cache save observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineUsageCacheSaveOutput {
    /// The value was persisted.
    Saved,
    /// The save failure was absorbed, matching the TypeScript service.
    FailureAbsorbed,
}

impl EngineUsageCacheSaveOutput {
    /// Returns whether the save failure was intentionally swallowed.
    #[must_use]
    pub const fn is_failure_absorbed(self) -> bool {
        matches!(self, Self::FailureAbsorbed)
    }
}

/// Applies the save policy without performing storage I/O.
#[must_use]
pub const fn engine_usage_cache_save(
    observation: EngineUsageCacheSaveObservation,
) -> EngineUsageCacheSaveOutput {
    match observation {
        EngineUsageCacheSaveObservation::Succeeded => EngineUsageCacheSaveOutput::Saved,
        EngineUsageCacheSaveObservation::Failed => EngineUsageCacheSaveOutput::FailureAbsorbed,
    }
}
