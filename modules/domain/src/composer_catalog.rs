//! Domain commands and queries for the native composer catalog projection.
//!
//! Catalog discovery is a read against an already owned engine profile; it
//! does not start a run or create a provider session. Favorite mutations carry
//! the catalog revision observed by the caller so the backend can reject a
//! stale selection before changing the durable preference.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::ModelFavoriteId;
use crate::identifiers::{EngineProfileId, RequestId, ThreadId};

/// Maximum UTF-8 byte length of a runtime catalog revision.
pub const CATALOG_REVISION_MAX_BYTES: usize = 4_096;

/// A non-empty, bounded revision token emitted by canonical catalog data.
///
/// The token is opaque to the domain. It is deliberately stricter than an
/// arbitrary display string so it can safely cross the protocol boundary and
/// participate in stale-catalog checks without carrying control characters.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CatalogRevision(String);

impl CatalogRevision {
    /// Validates and owns one catalog revision token.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogRevisionError`] for an empty value, a control
    /// character, or a value larger than the UTF-8 byte bound.
    pub fn parse(value: impl Into<String>) -> Result<Self, CatalogRevisionError> {
        let value = value.into();
        if value.is_empty() {
            return Err(CatalogRevisionError::Empty);
        }
        if let Some(character) = value.chars().find(|character| character.is_control()) {
            return Err(CatalogRevisionError::ControlCharacter { character });
        }
        let length = value.len();
        if length > CATALOG_REVISION_MAX_BYTES {
            return Err(CatalogRevisionError::TooLong {
                length,
                maximum: CATALOG_REVISION_MAX_BYTES,
            });
        }
        Ok(Self(value))
    }

    /// Returns the validated opaque revision token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CatalogRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for CatalogRevision {
    type Err = CatalogRevisionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Validation failure for a catalog revision token.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CatalogRevisionError {
    /// The revision must identify a canonical catalog snapshot.
    #[error("catalog revision must not be empty")]
    Empty,
    /// Control characters cannot cross the domain/protocol boundary.
    #[error("catalog revision must not contain control characters; found {character:?}")]
    ControlCharacter {
        /// The offending Unicode scalar value.
        character: char,
    },
    /// The revision exceeded its UTF-8 byte ceiling.
    #[error("catalog revision is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// The rejected byte length.
        length: usize,
        /// The maximum accepted byte length.
        maximum: usize,
    },
}

/// Reads the runtime catalog for one thread/profile ownership scope.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReadComposerCatalog {
    /// Thread requesting the catalog projection.
    pub thread_id: ThreadId,
    /// Native engine profile whose certified session owns the catalog.
    pub profile_id: EngineProfileId,
}

impl ReadComposerCatalog {
    /// Creates a catalog read scoped to one thread and engine profile.
    #[must_use]
    pub const fn new(thread_id: ThreadId, profile_id: EngineProfileId) -> Self {
        Self {
            thread_id,
            profile_id,
        }
    }

    /// Returns the owning thread identity.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the queried engine profile identity.
    #[must_use]
    pub const fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }
}

/// Reads the durable model-favorites projection.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ReadModelFavorites;

/// Changes one durable model favorite after checking a caller's catalog
/// revision.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SetModelFavorite {
    /// Stable client request identity for idempotent mutation handling.
    pub request_id: RequestId,
    /// Thread that observed and is submitting against the catalog.
    pub thread_id: ThreadId,
    /// Native profile that supplied the catalog revision.
    pub profile_id: EngineProfileId,
    /// Canonical catalog revision observed by the caller.
    pub catalog_revision: CatalogRevision,
    /// Stable catalog model identity to add or remove.
    pub model_id: ModelFavoriteId,
    /// Desired favorite state.
    pub favorite: bool,
}

impl SetModelFavorite {
    /// Creates one revision-checked favorite mutation.
    #[must_use]
    pub const fn new(
        request_id: RequestId,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        catalog_revision: CatalogRevision,
        model_id: ModelFavoriteId,
        favorite: bool,
    ) -> Self {
        Self {
            request_id,
            thread_id,
            profile_id,
            catalog_revision,
            model_id,
            favorite,
        }
    }

    /// Returns the stable mutation request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Returns the owning thread identity.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the catalog-owning engine profile.
    #[must_use]
    pub const fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }

    /// Returns the revision used for stale-catalog admission.
    #[must_use]
    pub const fn catalog_revision(&self) -> &CatalogRevision {
        &self.catalog_revision
    }

    /// Returns the stable model identity being changed.
    #[must_use]
    pub const fn model_id(&self) -> &ModelFavoriteId {
        &self.model_id
    }

    /// Returns the requested favorite state.
    #[must_use]
    pub const fn favorite(&self) -> bool {
        self.favorite
    }
}
