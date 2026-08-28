//! Typed, side-effect-free object-URL lifecycle values.
//!
//! This is the native boundary for `lib/browser/object-url.ts`. It preserves
//! the renderer's create and release calls as values and a capability seam;
//! it does not create browser resources or make a claim about native GPUI
//! image loading. The eventual host adapter owns those effects.

#![allow(clippy::module_name_repetitions)]

/// The exact bytes and media-type text for one object-URL create operation.
///
/// The fields are intentionally unvalidated and text-backed. An empty
/// payload, an empty media type, and arbitrary byte or text values remain
/// representable because this boundary only records the caller's request.
#[must_use = "an object-URL create intent should be handled by a capability"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectUrlCreateIntent {
    /// Byte payload forwarded to the create adapter without interpretation.
    pub bytes: Vec<u8>,
    /// Media-type text forwarded to the create adapter without validation.
    pub media_type: String,
}

impl ObjectUrlCreateIntent {
    /// Creates an intent while preserving every supplied byte and character.
    ///
    /// The payload is copied into the owned intent. No Blob, URL, MIME
    /// parser, normalization, deduplication, or other host operation occurs.
    #[must_use = "an object-URL create intent should be handled by a capability"]
    pub fn new(bytes: impl AsRef<[u8]>, media_type: impl Into<String>) -> Self {
        Self {
            bytes: bytes.as_ref().to_vec(),
            media_type: media_type.into(),
        }
    }
}

/// Opaque source returned by a successful create operation.
///
/// The source is only carried between the create result and a later release
/// intent. This type does not parse, normalize, compare, deduplicate, or
/// release the source, and dropping it has no lifecycle meaning.
#[must_use = "an object-URL source must be retained or explicitly released by its caller"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ObjectUrlSource(String);

impl ObjectUrlSource {
    /// Wraps adapter-provided source text without inspecting or changing it.
    #[must_use = "an object-URL source must be retained or explicitly released by its caller"]
    pub fn new(source: impl Into<String>) -> Self {
        Self(source.into())
    }

    /// Borrows the exact source text for an adapter or deterministic test.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ObjectUrlSource {
    fn from(source: String) -> Self {
        Self::new(source)
    }
}

impl From<&str> for ObjectUrlSource {
    fn from(source: &str) -> Self {
        Self::new(source)
    }
}

/// The exact opaque source for one object-URL release operation.
///
/// No ownership ledger is attached to this value. Whether or when a caller
/// releases a source remains the caller's responsibility.
#[must_use = "an object-URL release intent should be handled by a capability"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ObjectUrlReleaseIntent {
    /// Opaque source forwarded to the release adapter without modification.
    pub source: ObjectUrlSource,
}

impl ObjectUrlReleaseIntent {
    /// Creates a release intent for the caller-selected source.
    #[must_use = "an object-URL release intent should be handled by a capability"]
    pub fn new(source: impl Into<ObjectUrlSource>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

/// A create or release failure that preserves the adapter's typed cause.
///
/// The boundary does not stringify, classify, replace, or discard the cause.
/// The adapter chooses its own error type, including a deterministic test
/// error or a future host-specific error.
#[must_use = "an object-URL failure should be handled or returned"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ObjectUrlFailure<Cause> {
    /// The exact typed failure supplied by the capability adapter.
    pub cause: Cause,
}

impl<Cause> ObjectUrlFailure<Cause> {
    /// Wraps an adapter cause without erasing its type or value.
    #[must_use = "an object-URL failure should be handled or returned"]
    pub const fn new(cause: Cause) -> Self {
        Self { cause }
    }

    /// Borrows the exact adapter cause.
    #[must_use]
    pub const fn cause(&self) -> &Cause {
        &self.cause
    }

    /// Returns the exact adapter cause.
    #[must_use]
    pub fn into_cause(self) -> Cause {
        self.cause
    }
}

/// The two renderer-resource operations available at this boundary.
///
/// An implementation is the only place where a host effect may eventually be
/// connected. This trait itself performs no operation; it receives the exact
/// typed intent and returns either the typed success or the adapter cause in
/// the shared [`ObjectUrlFailure`] boundary.
pub trait ObjectUrlCapability {
    /// Adapter-specific failure retained by [`ObjectUrlFailure`].
    type Error;

    /// Attempts to create one source for the supplied bytes and media text.
    ///
    /// # Errors
    ///
    /// Returns the adapter's exact typed failure inside [`ObjectUrlFailure`].
    #[must_use = "object-URL create results must be handled"]
    fn create(
        &self,
        intent: &ObjectUrlCreateIntent,
    ) -> Result<ObjectUrlSource, ObjectUrlFailure<Self::Error>>;

    /// Attempts to release the exact source in the supplied intent.
    ///
    /// # Errors
    ///
    /// Returns the adapter's exact typed failure inside [`ObjectUrlFailure`].
    #[must_use = "object-URL release results must be handled"]
    fn release(&self, intent: &ObjectUrlReleaseIntent)
    -> Result<(), ObjectUrlFailure<Self::Error>>;
}
