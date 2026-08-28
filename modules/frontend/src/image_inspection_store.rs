//! Shared state for full-screen image inspection.
//!
//! The store counts open viewers rather than storing a boolean. A retain from
//! each viewer must eventually be paired with its release, but viewers may
//! overlap while one closes and another opens. During that overlap the count
//! remains positive, so visibility cannot flicker when the first viewer
//! releases its lease.

/// Shared count of the viewers currently inspecting images.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct ImageInspectionStore {
    count: usize,
}

impl ImageInspectionStore {
    /// Creates an empty store with no open viewers.
    #[must_use]
    pub const fn new() -> Self {
        Self { count: 0 }
    }

    /// Retains one open-viewer lease.
    ///
    /// Each concurrent viewer owns one lease. Callers should pair this with
    /// exactly one [`Self::release`] when that viewer closes or unmounts.
    pub fn retain(&mut self) {
        self.count = self.count.saturating_add(1);
    }

    /// Releases one open-viewer lease, never taking the count below zero.
    ///
    /// An unmatched release is harmless. This makes cleanup idempotent at the
    /// shared boundary while preserving other viewers' leases.
    pub fn release(&mut self) {
        self.count = self.count.saturating_sub(1);
    }

    /// Returns the number of currently retained open-viewer leases.
    #[must_use]
    pub const fn count(&self) -> usize {
        self.count
    }

    /// Returns whether at least one viewer currently holds an inspection lease.
    ///
    /// Visibility is intentionally exactly `count > 0`, matching the shared
    /// inspection policy rather than the state of any one viewer.
    #[must_use]
    pub const fn is_visible(&self) -> bool {
        self.count > 0
    }
}
