//! Catalog, engine-settings, model-favorite, and rich-link wire values.

#![forbid(unsafe_code)]
#![allow(
    clippy::module_name_repetitions,
    reason = "catalog wire values retain their catalog context at protocol boundaries"
)]

use std::fmt;

use artisan_catalog::wire::{NativeModelCatalogWireError, decode_catalog};
use artisan_domain::{
    EngineConfigRevision, EngineProfileId, EngineRunConfig, ModelFavoriteId,
    ModelFavoritesRevision, ModelFavoritesSnapshot as DomainModelFavoritesSnapshot,
    ModelFavoritesSnapshotError, ReceiptDisposition, RequestId, ThreadId,
};
use thiserror::Error;

use super::{
    CATALOG_SNAPSHOT_MAX_BYTES, ProtocolValueError, RICH_LINK_PAGE_NAME_MAX_BYTES,
    RICH_LINK_URL_MAX_BYTES,
};
/// Exact, bounded bytes of a shared native model catalog snapshot.
///
/// The bytes remain private so every instance has passed the shared catalog
/// decoder before it crosses the protocol boundary. Debug output intentionally
/// reports only its size and never formats the catalog payload.
#[derive(Clone, Eq, PartialEq)]
pub struct CatalogSnapshotWire(Vec<u8>);

impl CatalogSnapshotWire {
    /// Validates the byte bound and shared catalog encoding before retaining it.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogSnapshotWireError::TooLarge`] before parsing an
    /// oversized payload, or [`CatalogSnapshotWireError::InvalidCatalog`] when
    /// the shared catalog wire decoder rejects it.
    pub fn new(bytes: Vec<u8>) -> Result<Self, CatalogSnapshotWireError> {
        let length = bytes.len();
        if length > CATALOG_SNAPSHOT_MAX_BYTES {
            return Err(CatalogSnapshotWireError::TooLarge {
                length,
                maximum: CATALOG_SNAPSHOT_MAX_BYTES,
            });
        }
        decode_catalog(&bytes)
            .map_err(|source| CatalogSnapshotWireError::InvalidCatalog { source })?;
        Ok(Self(bytes))
    }

    /// Borrows the exact accepted shared-catalog bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Decodes the snapshot through the shared catalog wire API.
    ///
    /// # Errors
    ///
    /// Returns [`NativeModelCatalogWireError`] when the stored bytes are no
    /// longer accepted by the shared catalog decoder. The error preserves the
    /// shared decoder's typed failure without exposing raw payload data.
    pub fn decoded(
        &self,
    ) -> Result<artisan_catalog::NativeModelCatalog, NativeModelCatalogWireError> {
        decode_catalog(&self.0)
    }
}

impl fmt::Debug for CatalogSnapshotWire {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CatalogSnapshotWire")
            .field("bytes_len", &self.0.len())
            .finish()
    }
}

/// Failure while validating a shared catalog snapshot at the protocol edge.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CatalogSnapshotWireError {
    /// The bytes exceeded the protocol payload ceiling.
    #[error("catalog snapshot is {length} bytes; the maximum is {maximum}")]
    TooLarge {
        /// Rejected byte length.
        length: usize,
        /// Maximum accepted byte length.
        maximum: usize,
    },
    /// The shared catalog wire decoder rejected the payload.
    #[error("catalog snapshot failed shared validation: {source}")]
    InvalidCatalog {
        /// Shared typed decoder failure.
        #[source]
        source: NativeModelCatalogWireError,
    },
}

/// Complete runtime catalog response correlated to one thread/profile scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerCatalogResult {
    /// Thread that requested the projection.
    pub thread_id: ThreadId,
    /// Native engine profile that owns the projection.
    pub profile_id: EngineProfileId,
    /// Exact shared catalog bytes.
    pub snapshot: CatalogSnapshotWire,
}

impl ComposerCatalogResult {
    /// Creates a catalog result after checking its embedded runtime scope.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::InvalidCatalogSnapshot`] when the snapshot
    /// fails shared decoding, or [`ProtocolValueError::CatalogScopeMissing`]
    /// and [`ProtocolValueError::CatalogScopeMismatch`] when the decoded
    /// catalog does not carry the result's own profile scope.
    pub fn new(
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        snapshot: CatalogSnapshotWire,
    ) -> Result<Self, ProtocolValueError> {
        let result = Self {
            thread_id,
            profile_id,
            snapshot,
        };
        result.validate_scope()?;
        Ok(result)
    }

    /// Validates that the decoded catalog belongs to the response profile.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::InvalidCatalogSnapshot`] when the snapshot
    /// fails shared decoding, or [`ProtocolValueError::CatalogScopeMissing`]
    /// and [`ProtocolValueError::CatalogScopeMismatch`] when the decoded
    /// catalog does not carry this result's own profile scope.
    pub fn validate_scope(&self) -> Result<(), ProtocolValueError> {
        let catalog = self
            .snapshot
            .decoded()
            .map_err(|_| ProtocolValueError::InvalidCatalogSnapshot)?;
        let scope = catalog
            .scope
            .as_ref()
            .ok_or(ProtocolValueError::CatalogScopeMissing)?;
        if scope.profile_id != self.profile_id.as_str() {
            return Err(ProtocolValueError::CatalogScopeMismatch);
        }
        Ok(())
    }
}

/// One bounded rich-link metadata read for an absolute HTTP(S) URL.
///
/// The URL is untrusted assistant-authored text: this value re-applies the
/// shared absolute-HTTP(S) policy at the protocol edge, and Forge re-applies
/// its own outbound policy again before any fetch.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ResolveRichLinkRequest {
    url: String,
}

impl ResolveRichLinkRequest {
    /// Validates one absolute HTTP(S) URL.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::RichLink`] when the URL is empty,
    /// exceeds [`RICH_LINK_URL_MAX_BYTES`], is not an absolute `http(s)`
    /// URL, or contains whitespace or control characters. The error carries
    /// no URL payload.
    pub fn new(url: impl Into<String>) -> Result<Self, ProtocolValueError> {
        let url = url.into();
        validate_rich_link_url(&url)?;
        Ok(Self { url })
    }

    /// Returns the validated absolute URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// Resolved rich-link page metadata for one requested URL.
///
/// `page_name` is the backend's resolved display title and is never empty.
/// `cache_expires_at_ms` is the backend cache entry's absolute Unix epoch
/// millisecond expiry so clients bound their retained titles by the same
/// freshness decision Forge used; the URL is echoed for exact correlation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RichLinkPageMetadata {
    /// Optional favicon bytes, bounded to 64 KiB.
    pub favicon: Vec<u8>,
    /// Exact URL the caller requested, echoed for correlation.
    pub requested_url: String,
    /// Resolved display title.
    pub page_name: String,
    /// Backend cache expiry as Unix epoch milliseconds.
    pub cache_expires_at_ms: i64,
}

impl RichLinkPageMetadata {
    /// Adds a bounded optional favicon.
    ///
    /// # Errors
    /// Returns an error when the image exceeds 64 KiB.
    pub fn with_favicon(mut self, favicon: Vec<u8>) -> Result<Self, ProtocolValueError> {
        self.favicon = favicon;
        self.validate()?;
        Ok(self)
    }

    /// Creates response metadata after checking both bounded text fields.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::RichLink`] when the requested URL fails
    /// the shared absolute-HTTP(S) policy or the page name is empty or
    /// exceeds [`RICH_LINK_PAGE_NAME_MAX_BYTES`]. The error carries no
    /// payload.
    pub fn new(
        requested_url: impl Into<String>,
        page_name: impl Into<String>,
        cache_expires_at_ms: i64,
    ) -> Result<Self, ProtocolValueError> {
        let metadata = Self {
            favicon: Vec::new(),
            requested_url: requested_url.into(),
            page_name: page_name.into(),
            cache_expires_at_ms,
        };
        metadata.validate()?;
        Ok(metadata)
    }

    /// Validates the bounded URL and page-name fields.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolValueError::RichLink`] when the requested URL fails
    /// the shared absolute-HTTP(S) policy or the page name is empty or
    /// exceeds [`RICH_LINK_PAGE_NAME_MAX_BYTES`]. The error carries no
    /// payload.
    pub fn validate(&self) -> Result<(), ProtocolValueError> {
        validate_rich_link_url(&self.requested_url)?;
        if self.favicon.len() > 65_536 {
            return Err(ProtocolValueError::RichLink {
                reason: "favicon exceeds its byte bound",
            });
        }
        if self.page_name.trim().is_empty() || self.page_name.len() > RICH_LINK_PAGE_NAME_MAX_BYTES
        {
            return Err(ProtocolValueError::RichLink {
                reason: "page name is empty or exceeds its byte bound",
            });
        }
        Ok(())
    }
}

fn validate_rich_link_url(url: &str) -> Result<(), ProtocolValueError> {
    if url.is_empty() || url.len() > RICH_LINK_URL_MAX_BYTES {
        return Err(ProtocolValueError::RichLink {
            reason: "url is empty or exceeds its byte bound",
        });
    }
    if url
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(ProtocolValueError::RichLink {
            reason: "url contains whitespace or control characters",
        });
    }
    let scheme_length = "https://".len();
    let scheme_ok = url
        .get(..scheme_length)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        || url
            .get(.."http://".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"));
    if !scheme_ok {
        return Err(ProtocolValueError::RichLink {
            reason: "url must be absolute HTTP(S)",
        });
    }
    Ok(())
}

/// Ordered, domain-validated durable model favorites projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelFavoritesSnapshot {
    /// Monotonic durable favorites revision.
    pub revision: ModelFavoritesRevision,
    /// Exact persisted favorite order.
    pub model_ids: Vec<ModelFavoriteId>,
}

impl ModelFavoritesSnapshot {
    /// Creates a protocol snapshot using the existing domain validation.
    ///
    /// # Errors
    ///
    /// Returns [`ModelFavoritesSnapshotError`] when the domain snapshot rejects
    /// the model count, a repeated model id, or the encoded byte ceiling.
    pub fn new(
        revision: ModelFavoritesRevision,
        model_ids: Vec<ModelFavoriteId>,
    ) -> Result<Self, ModelFavoritesSnapshotError> {
        DomainModelFavoritesSnapshot::new(revision, model_ids.clone())?;
        Ok(Self {
            revision,
            model_ids,
        })
    }

    /// Copies an already domain-validated snapshot into protocol ownership.
    #[must_use]
    pub fn from_domain(value: &DomainModelFavoritesSnapshot) -> Self {
        Self {
            revision: value.revision(),
            model_ids: value.model_ids().to_vec(),
        }
    }

    /// Converts this owned protocol value back through domain validation.
    ///
    /// # Errors
    ///
    /// Returns [`ModelFavoritesSnapshotError`] when the domain snapshot rejects
    /// the model count, a repeated model id, or the encoded byte ceiling.
    pub fn into_domain(self) -> Result<DomainModelFavoritesSnapshot, ModelFavoritesSnapshotError> {
        DomainModelFavoritesSnapshot::new(self.revision, self.model_ids)
    }
}

/// Correlated receipt for one favorite mutation and its complete post-state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetModelFavoriteReceipt {
    /// Stable client mutation identity echoed by the enclosing response.
    pub request_id: RequestId,
    /// Stable model identity changed by the mutation.
    pub model_id: ModelFavoriteId,
    /// Requested resulting favorite state.
    pub favorite: bool,
    /// Newly accepted or exact duplicate replay.
    pub disposition: ReceiptDisposition,
    /// Complete durable state after the mutation.
    pub snapshot: ModelFavoritesSnapshot,
}

/// Authoritative persisted thread engine settings read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThreadEngineSettingsResult {
    /// No engine configuration has been stored for this thread.
    Unconfigured { thread_id: ThreadId },
    /// Complete persisted configuration with its one-based revision.
    Configured {
        thread_id: ThreadId,
        revision: EngineConfigRevision,
        config: Box<EngineRunConfig>,
    },
}

impl ThreadEngineSettingsResult {
    /// Returns the thread owning these settings.
    #[must_use]
    pub fn thread_id(&self) -> &ThreadId {
        match self {
            Self::Unconfigured { thread_id } | Self::Configured { thread_id, .. } => thread_id,
        }
    }

    /// Returns the stored revision when configured.
    #[must_use]
    pub fn revision(&self) -> Option<EngineConfigRevision> {
        match self {
            Self::Unconfigured { .. } => None,
            Self::Configured { revision, .. } => Some(*revision),
        }
    }

    /// Returns the stored configuration when configured.
    #[must_use]
    pub fn config(&self) -> Option<&EngineRunConfig> {
        match self {
            Self::Unconfigured { .. } => None,
            Self::Configured { config, .. } => Some(config),
        }
    }
}

/// Registered engine profiles catalogue with absence distinction.
///
/// `RegistryMissing` means no registry file exists; `RegistryPresent` means
/// the registry file exists and contains exactly the ordered profile ids
/// supplied by the authority, which may be empty and contains no home, path,
/// or executable details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegisteredEngineProfilesResult {
    /// The profile registry does not exist.
    RegistryMissing,
    /// The registry exists and contains the exact ordered profile ids.
    RegistryPresent { profile_ids: Vec<EngineProfileId> },
}

impl RegisteredEngineProfilesResult {
    /// Returns whether the registry is missing.
    #[must_use]
    pub const fn is_missing(&self) -> bool {
        matches!(self, Self::RegistryMissing)
    }

    /// Returns the present ordered profile ids, if present.
    #[must_use]
    pub fn profile_ids(&self) -> Option<&[EngineProfileId]> {
        match self {
            Self::RegistryMissing => None,
            Self::RegistryPresent { profile_ids } => Some(profile_ids),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ResolveRichLinkRequest, RichLinkPageMetadata};
    use crate::types::{RICH_LINK_PAGE_NAME_MAX_BYTES, RICH_LINK_URL_MAX_BYTES};

    #[test]
    fn rich_link_request_accepts_only_bounded_absolute_http_urls() {
        let request = ResolveRichLinkRequest::new("https://example.com/docs?q=1#frag")
            .expect("absolute https URL is valid");
        assert_eq!(request.url(), "https://example.com/docs?q=1#frag");
        assert!(ResolveRichLinkRequest::new("http://example.com").is_ok());

        assert!(ResolveRichLinkRequest::new("").is_err());
        assert!(ResolveRichLinkRequest::new("example.com").is_err());
        assert!(ResolveRichLinkRequest::new("file:///etc/passwd").is_err());
        assert!(ResolveRichLinkRequest::new("mailto:user@example.com").is_err());
        assert!(ResolveRichLinkRequest::new("//example.com/path").is_err());
        assert!(ResolveRichLinkRequest::new("https://example.com/a b").is_err());
        assert!(
            ResolveRichLinkRequest::new(format!(
                "https://example.com/{}",
                "a".repeat(RICH_LINK_URL_MAX_BYTES)
            ))
            .is_err()
        );
    }

    #[test]
    fn rich_link_metadata_requires_url_and_nonempty_bounded_page_name() {
        let metadata =
            RichLinkPageMetadata::new("https://example.com/a", "Example", 1_700_000_000_000)
                .expect("valid metadata");
        assert_eq!(metadata.page_name, "Example");
        metadata.validate().expect("validated metadata stays valid");

        assert!(RichLinkPageMetadata::new("https://example.com/a", "", 0).is_err());
        assert!(RichLinkPageMetadata::new("https://example.com/a", "  ", 0).is_err());
        assert!(RichLinkPageMetadata::new("not-a-url", "Example", 0).is_err());
        assert!(
            RichLinkPageMetadata::new(
                "https://example.com/a",
                "t".repeat(RICH_LINK_PAGE_NAME_MAX_BYTES + 1),
                0,
            )
            .is_err()
        );
    }
}
