//! Typed access to first-party, vendored visual assets.

/// Stable identifier for an embedded asset.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AssetId(&'static str);

impl AssetId {
    /// Creates an asset identifier for generated catalog code.
    #[must_use]
    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    /// Returns the stable catalog key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}
