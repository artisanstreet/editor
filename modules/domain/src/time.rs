//! Timestamps as signed Unix epoch milliseconds.
//!
//! The native application schema deliberately declares its millisecond
//! timestamps as *signed* Unix epoch milliseconds, so the domain mirrors that
//! contract exactly: the complete signed [`i64`] range is valid, negative
//! values are real instants before 1970 rather than clock errors, and no
//! cross-field ordering (such as creation preceding update) is enforced here.
//! Wall-clock acquisition stays outside the domain entirely.

/// One signed Unix epoch instant measured in milliseconds.
///
/// Constructed from and projected back to the raw wire value without any
/// range narrowing, keeping the domain-to-schema conversion total.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UnixMillis(i64);

impl UnixMillis {
    /// The largest representable instant.
    pub const MAX: Self = Self(i64::MAX);

    /// The smallest representable instant.
    pub const MIN: Self = Self(i64::MIN);

    /// The Unix epoch itself.
    pub const EPOCH: Self = Self(0);

    /// Creates a timestamp from raw signed Unix epoch milliseconds.
    ///
    /// # Policy
    ///
    /// Accepts the complete signed [`i64`] wire range. Negative values are
    /// allowed because the schema deliberately says signed epoch
    /// milliseconds; cross-field clock ordering is deliberately not enforced.
    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Returns the raw signed Unix epoch milliseconds.
    ///
    /// This is the total projection onto the wire schema's millisecond
    /// fields: every constructed value projects back unchanged.
    #[must_use]
    pub const fn as_millis(self) -> i64 {
        self.0
    }
}
