//! Synchronous, bounded retention for thread session snapshots.
//!
//! This is the pure state boundary behind the browser session projection. It
//! deliberately owns no stream, service, task, or runtime. A small insertion-
//! ordered vector is sufficient for the fixed 64-entry limit and makes the
//! eviction order explicit rather than depending on a map's iteration order.

use std::borrow::Borrow;

/// Maximum number of thread session snapshots retained by a projection.
pub const MAX_RETAINED_SESSIONS: usize = 64;

/// One snapshot and the thread key to which it belongs.
///
/// The key and value are kept in one record so a replacement or eviction
/// cannot move a value independently of its thread. Fields stay private so
/// callers construct the pair through [`Self::new`] and read it through the
/// accessors below.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadSessionSnapshot<K, V> {
    thread_id: K,
    value: V,
}

impl<K, V> ThreadSessionSnapshot<K, V> {
    /// Creates a snapshot associated with `thread_id`.
    #[must_use]
    pub fn new(thread_id: K, value: V) -> Self {
        Self { thread_id, value }
    }

    /// Returns the thread key carried by this snapshot.
    #[must_use]
    pub const fn thread_id(&self) -> &K {
        &self.thread_id
    }

    /// Returns the snapshot value.
    #[must_use]
    pub const fn value(&self) -> &V {
        &self.value
    }

    /// Splits the snapshot back into its coupled key and value.
    #[must_use]
    pub fn into_parts(self) -> (K, V) {
        (self.thread_id, self.value)
    }
}

/// An insertion-ordered, bounded projection of thread session snapshots.
///
/// The vector is ordered from oldest to newest. Publishing an existing thread
/// removes its old record before appending the replacement, which refreshes
/// that thread's recency. Once the limit is exceeded, records are removed from
/// the front until at most [`MAX_RETAINED_SESSIONS`] remain.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThreadSessionProjection<K, V> {
    snapshots: Vec<ThreadSessionSnapshot<K, V>>,
}

impl<K: Eq, V> ThreadSessionProjection<K, V> {
    /// Creates an empty projection.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            snapshots: Vec::new(),
        }
    }

    /// Returns the number of retained snapshots.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.snapshots.len()
    }

    /// Returns whether no snapshots are retained.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    /// Returns the current snapshot for `thread_id`, if it is retained.
    #[must_use]
    pub fn current<Q>(&self, thread_id: &Q) -> Option<&ThreadSessionSnapshot<K, V>>
    where
        K: Borrow<Q>,
        Q: Eq + ?Sized,
    {
        self.snapshots
            .iter()
            .find(|snapshot| snapshot.thread_id().borrow() == thread_id)
    }

    /// Returns only the current value for `thread_id`, if it is retained.
    #[must_use]
    pub fn current_value<Q>(&self, thread_id: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Eq + ?Sized,
    {
        self.current(thread_id).map(ThreadSessionSnapshot::value)
    }

    /// Returns retained snapshots in deterministic oldest-to-newest order.
    #[must_use]
    pub fn snapshots(&self) -> &[ThreadSessionSnapshot<K, V>] {
        &self.snapshots
    }

    /// Publishes `snapshot`, returning the same snapshot as the operation
    /// result.
    ///
    /// An existing snapshot with the same thread key is deleted and the new
    /// snapshot is appended. The returned value is the caller's published
    /// snapshot; the projection stores an equal clone so ownership remains
    /// explicit at both boundaries.
    #[must_use]
    pub fn publish(&mut self, snapshot: ThreadSessionSnapshot<K, V>) -> ThreadSessionSnapshot<K, V>
    where
        K: Clone,
        V: Clone,
    {
        if let Some(index) = self
            .snapshots
            .iter()
            .position(|current| current.thread_id() == snapshot.thread_id())
        {
            self.snapshots.remove(index);
        }

        self.snapshots.push(snapshot.clone());
        while self.snapshots.len() > MAX_RETAINED_SESSIONS {
            self.snapshots.remove(0);
        }

        snapshot
    }
}
