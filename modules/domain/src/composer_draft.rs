//! Forge-owned composer drafts and their stored image attachments.
//!
//! A draft belongs to one composer scope: an existing thread, or the
//! new-task composer of an attached project before its first message creates
//! a thread. The Forge owns its revision: every save applies (the last save
//! to arrive wins) and the Forge assigns the next revision, which the
//! acknowledgement reports.
//!
//! Attachment bytes live once in a content-addressed Forge store keyed by the
//! SHA-256 digest of the encoded image. Drafts, and messages sent from them,
//! name stored attachments by [`ComposerAttachmentRef`] instead of carrying
//! bytes. The store holds each image exactly as the user picked it
//! ([`ComposerImage`], up to [`COMPOSER_ATTACHMENT_MAX_BYTES`]); the Forge
//! applies its engine's image policy when a draft is sent.

use std::fmt;

use thiserror::Error;

use crate::bounds::{
    COMPOSER_ATTACHMENT_MAX_BYTES, COMPOSER_ATTACHMENTS_MAX_TOTAL_BYTES,
    MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES, MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT,
    MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES,
};
use crate::commands::SteerTarget;
use crate::identifiers::{ProjectId, RequestId, ThreadId};
use crate::message::{AuthoredText, ImageMimeType, validate_attachment_name};
use crate::time::UnixMillis;

/// The composer a draft belongs to.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ComposerDraftScope {
    /// The composer of one existing thread.
    Thread(ThreadId),
    /// The new-task composer of one attached project.
    Project(ProjectId),
}

impl ComposerDraftScope {
    /// Stable storage spelling of the scope kind.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Thread(_) => "thread",
            Self::Project(_) => "project",
        }
    }

    /// The scoped identity.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Thread(thread) => thread.as_str(),
            Self::Project(project) => project.as_str(),
        }
    }
}

/// Forge-assigned, per-scope monotonic draft revision.
///
/// Zero means "no draft stored"; the first save stores one. Values are
/// bounded to SQLite's signed integer range.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ComposerDraftRevision(u64);

impl ComposerDraftRevision {
    const MAX_VALUE: u64 = i64::MAX.cast_unsigned();

    /// Creates a revision from its unsigned representation.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerDraftError::RevisionOutOfRange`] past SQLite's
    /// signed integer range.
    pub const fn new(value: u64) -> Result<Self, ComposerDraftError> {
        if value > Self::MAX_VALUE {
            return Err(ComposerDraftError::RevisionOutOfRange { value });
        }
        Ok(Self(value))
    }

    /// The unsigned revision.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The signed representation stored by SQLite.
    #[must_use]
    pub const fn as_i64(self) -> i64 {
        self.0.cast_signed()
    }

    /// The next revision, or `None` at the last representable one.
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        if self.0 >= Self::MAX_VALUE {
            None
        } else {
            Some(Self(self.0 + 1))
        }
    }
}

impl fmt::Display for ComposerDraftRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// SHA-256 digest of one stored attachment's encoded bytes; the store key.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ComposerAttachmentDigest([u8; 32]);

impl ComposerAttachmentDigest {
    /// Wraps a digest computed over the encoded image bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parses a digest from a wire or storage slice.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerDraftError::InvalidDigest`] unless the slice is
    /// exactly 32 bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, ComposerDraftError> {
        <[u8; 32]>::try_from(bytes)
            .map(Self)
            .map_err(|_| ComposerDraftError::InvalidDigest {
                length: bytes.len(),
            })
    }

    /// The raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ComposerAttachmentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ComposerAttachmentDigest({self})")
    }
}

impl fmt::Display for ComposerAttachmentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
            .iter()
            .try_for_each(|byte| write!(formatter, "{byte:02x}"))
    }
}

/// Byte-free reference to one stored composer attachment, as authored.
///
/// The digest names the stored bytes; the MIME type and size repeat the
/// stored metadata so the Forge can verify the reference, and the name is the
/// authored display name of this use of the image.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ComposerAttachmentRef {
    digest: ComposerAttachmentDigest,
    mime_type: ImageMimeType,
    name: String,
    size_bytes: u32,
}

impl ComposerAttachmentRef {
    /// Builds one validated reference.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerDraftError::InvalidAttachment`] for an empty image,
    /// one over [`COMPOSER_ATTACHMENT_MAX_BYTES`], or an invalid display name.
    pub fn new(
        digest: ComposerAttachmentDigest,
        mime_type: ImageMimeType,
        name: impl Into<String>,
        size_bytes: u32,
    ) -> Result<Self, ComposerDraftError> {
        let name = name.into();
        let size_fits = usize::try_from(size_bytes)
            .is_ok_and(|size| size > 0 && size <= COMPOSER_ATTACHMENT_MAX_BYTES);
        if !size_fits || validate_attachment_name(&name).is_err() {
            return Err(ComposerDraftError::InvalidAttachment);
        }
        Ok(Self {
            digest,
            mime_type,
            name,
            size_bytes,
        })
    }

    /// Store key of the referenced bytes.
    #[must_use]
    pub const fn digest(&self) -> &ComposerAttachmentDigest {
        &self.digest
    }

    /// Accepted image MIME type.
    #[must_use]
    pub const fn mime_type(&self) -> ImageMimeType {
        self.mime_type
    }

    /// Authored display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Exact encoded byte length.
    #[must_use]
    pub const fn size_bytes(&self) -> u32 {
        self.size_bytes
    }
}

/// Checks an ordered reference list against a per-image and total bound:
/// the composer store's for drafts, the message's for a message sent by
/// reference.
fn validate_references(
    attachments: &[ComposerAttachmentRef],
    image_max: usize,
    total_max: usize,
) -> Result<(), ComposerDraftError> {
    if attachments.len() > MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT {
        return Err(ComposerDraftError::TooManyAttachments {
            count: attachments.len(),
        });
    }
    if attachments.iter().any(|attachment| {
        usize::try_from(attachment.size_bytes).map_or(true, |size| size > image_max)
    }) {
        return Err(ComposerDraftError::InvalidAttachment);
    }
    let total = attachments
        .iter()
        .map(|attachment| u64::from(attachment.size_bytes))
        .sum::<u64>();
    if usize::try_from(total).map_or(true, |total| total > total_max) {
        return Err(ComposerDraftError::AttachmentsTooLarge { total });
    }
    Ok(())
}

/// Checks a draft's references against the composer store bounds.
fn validate_draft_references(
    attachments: &[ComposerAttachmentRef],
) -> Result<(), ComposerDraftError> {
    validate_references(
        attachments,
        COMPOSER_ATTACHMENT_MAX_BYTES,
        COMPOSER_ATTACHMENTS_MAX_TOTAL_BYTES,
    )
}

/// One stored composer draft.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerDraft {
    revision: ComposerDraftRevision,
    text: AuthoredText,
    attachments: Vec<ComposerAttachmentRef>,
    updated_at: UnixMillis,
}

impl ComposerDraft {
    /// Builds one bounded draft.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerDraftError`] when the attachment list violates the
    /// message image bounds.
    pub fn new(
        revision: ComposerDraftRevision,
        text: AuthoredText,
        attachments: Vec<ComposerAttachmentRef>,
        updated_at: UnixMillis,
    ) -> Result<Self, ComposerDraftError> {
        validate_draft_references(&attachments)?;
        Ok(Self {
            revision,
            text,
            attachments,
            updated_at,
        })
    }

    /// Stored revision.
    #[must_use]
    pub const fn revision(&self) -> ComposerDraftRevision {
        self.revision
    }

    /// Authored text exactly as typed.
    #[must_use]
    pub const fn text(&self) -> &AuthoredText {
        &self.text
    }

    /// Stored attachments in authored order.
    #[must_use]
    pub fn attachments(&self) -> &[ComposerAttachmentRef] {
        &self.attachments
    }

    /// Forge instant of the last applied save.
    #[must_use]
    pub const fn updated_at(&self) -> UnixMillis {
        self.updated_at
    }

    /// Whether the draft holds neither text nor attachments.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.as_str().is_empty() && self.attachments.is_empty()
    }
}

/// Replaces one scope's draft; the Forge assigns the next revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveComposerDraft {
    request_id: RequestId,
    scope: ComposerDraftScope,
    text: AuthoredText,
    attachments: Vec<ComposerAttachmentRef>,
}

impl SaveComposerDraft {
    /// Builds one bounded save.
    ///
    /// # Errors
    ///
    /// Returns a bound failure for the attachment list.
    pub fn new(
        request_id: RequestId,
        scope: ComposerDraftScope,
        text: AuthoredText,
        attachments: Vec<ComposerAttachmentRef>,
    ) -> Result<Self, ComposerDraftError> {
        validate_draft_references(&attachments)?;
        Ok(Self {
            request_id,
            scope,
            text,
            attachments,
        })
    }

    /// Client request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Draft scope.
    #[must_use]
    pub const fn scope(&self) -> &ComposerDraftScope {
        &self.scope
    }

    /// Proposed text.
    #[must_use]
    pub const fn text(&self) -> &AuthoredText {
        &self.text
    }

    /// Proposed attachment references in authored order.
    #[must_use]
    pub fn attachments(&self) -> &[ComposerAttachmentRef] {
        &self.attachments
    }
}

/// Acknowledgement of one draft save.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerDraftSaved {
    /// Client request identity echoed by the enclosing response.
    pub request_id: RequestId,
    /// Draft scope.
    pub scope: ComposerDraftScope,
    /// Revision the Forge assigned to this save.
    pub revision: ComposerDraftRevision,
}

/// Reads one scope's stored draft.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReadComposerDraft {
    /// Draft scope.
    pub scope: ComposerDraftScope,
}

/// Result of one draft read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerDraftResult {
    /// Draft scope.
    pub scope: ComposerDraftScope,
    /// Stored draft, or `None` when the scope never saved one.
    pub draft: Option<ComposerDraft>,
}

/// One image exactly as the user picked it, with its display name.
///
/// Unlike a message image it is not yet fitted to any engine: it may be
/// larger than a message allows (up to [`COMPOSER_ATTACHMENT_MAX_BYTES`]),
/// and the Forge rescales and re-encodes it for the thread's engine when the
/// draft is sent.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ComposerImage {
    mime_type: ImageMimeType,
    bytes: Vec<u8>,
    name: String,
}

impl fmt::Debug for ComposerImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComposerImage")
            .field("mime_type", &self.mime_type)
            .field("bytes_len", &self.bytes.len())
            .field("name", &self.name)
            .finish()
    }
}

impl ComposerImage {
    /// Validates one picked image.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerDraftError::InvalidAttachment`] for an unsupported
    /// MIME spelling, empty bytes, bytes over
    /// [`COMPOSER_ATTACHMENT_MAX_BYTES`], or an invalid display name.
    pub fn new(
        mime_type: impl AsRef<str>,
        bytes: Vec<u8>,
        name: impl Into<String>,
    ) -> Result<Self, ComposerDraftError> {
        let mime_type = ImageMimeType::parse(mime_type.as_ref())
            .map_err(|_| ComposerDraftError::InvalidAttachment)?;
        let name = name.into();
        if bytes.is_empty()
            || bytes.len() > COMPOSER_ATTACHMENT_MAX_BYTES
            || validate_attachment_name(&name).is_err()
        {
            return Err(ComposerDraftError::InvalidAttachment);
        }
        Ok(Self {
            mime_type,
            bytes,
            name,
        })
    }

    /// Validated MIME type.
    #[must_use]
    pub const fn mime_type(&self) -> ImageMimeType {
        self.mime_type
    }

    /// Validated MIME spelling.
    #[must_use]
    pub const fn mime_type_str(&self) -> &'static str {
        self.mime_type.as_str()
    }

    /// Encoded bytes as picked.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Encoded byte length.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    /// Display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Stores one image in the Forge composer attachment store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadComposerAttachment {
    /// Client request identity.
    pub request_id: RequestId,
    /// The image as picked, with its authored display name.
    pub image: ComposerImage,
}

/// Acknowledgement of one stored attachment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerAttachmentUploaded {
    /// Client request identity echoed by the enclosing response.
    pub request_id: RequestId,
    /// Reference naming the stored bytes under the uploaded display name.
    pub reference: ComposerAttachmentRef,
}

/// Reads the bytes of one stored attachment.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReadComposerAttachment {
    /// Store key.
    pub digest: ComposerAttachmentDigest,
}

/// Bytes of one stored attachment.
#[derive(Clone, Eq, PartialEq)]
pub struct ComposerAttachmentResult {
    /// Store key.
    pub digest: ComposerAttachmentDigest,
    /// Stored MIME type.
    pub mime_type: ImageMimeType,
    /// Encoded image bytes.
    pub bytes: Vec<u8>,
}

impl fmt::Debug for ComposerAttachmentResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComposerAttachmentResult")
            .field("digest", &self.digest)
            .field("mime_type", &self.mime_type)
            .field("bytes_len", &self.bytes.len())
            .finish()
    }
}

/// Queues one message whose images are stored composer attachments.
///
/// The Forge resolves every reference to its stored bytes and then admits
/// the resulting message exactly like [`crate::QueueMessage`], so replay,
/// idempotency, and dispatch are unchanged; only the bytes stay off the wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueStoredMessage {
    request_id: RequestId,
    thread_id: ThreadId,
    text: Option<AuthoredText>,
    attachments: Vec<ComposerAttachmentRef>,
    steer_target: Option<SteerTarget>,
}

impl QueueStoredMessage {
    /// Builds one reference-carrying message.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerDraftError::NoStoredAttachments`] without an image
    /// (text-only messages use [`crate::QueueMessage`]) or a bound failure.
    pub fn new(
        request_id: RequestId,
        thread_id: ThreadId,
        text: Option<AuthoredText>,
        attachments: Vec<ComposerAttachmentRef>,
        steer_target: Option<SteerTarget>,
    ) -> Result<Self, ComposerDraftError> {
        if attachments.is_empty() {
            return Err(ComposerDraftError::NoStoredAttachments);
        }
        validate_references(
            &attachments,
            MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES,
            MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES,
        )?;
        Ok(Self {
            request_id,
            thread_id,
            text,
            attachments,
            steer_target,
        })
    }

    /// Client request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Target thread.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Optional authored text.
    #[must_use]
    pub const fn text(&self) -> Option<&AuthoredText> {
        self.text.as_ref()
    }

    /// Stored images in authored order.
    #[must_use]
    pub fn attachments(&self) -> &[ComposerAttachmentRef] {
        &self.attachments
    }

    /// Observed live run to steer into, if named.
    #[must_use]
    pub const fn steer_target(&self) -> Option<&SteerTarget> {
        self.steer_target.as_ref()
    }
}

/// Validation failure for a composer draft value.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ComposerDraftError {
    /// The revision exceeded SQLite's signed integer range.
    #[error("composer draft revision {value} is out of range")]
    RevisionOutOfRange {
        /// Rejected value.
        value: u64,
    },
    /// A digest was not exactly 32 bytes.
    #[error("composer attachment digest is {length} bytes; expected 32")]
    InvalidDigest {
        /// Supplied length.
        length: usize,
    },
    /// An attachment reference had an invalid size or name.
    #[error("composer attachment reference is invalid")]
    InvalidAttachment,
    /// Too many attachments.
    #[error(
        "composer draft has {count} attachments; the maximum is {MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT}"
    )]
    TooManyAttachments {
        /// Supplied count.
        count: usize,
    },
    /// The aggregate attachment size exceeded the message bound.
    #[error(
        "composer draft attachments total {total} bytes; the maximum is {MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES}"
    )]
    AttachmentsTooLarge {
        /// Supplied total.
        total: u64,
    },
    /// A stored-attachment message named no attachment.
    #[error("a stored-attachment message must name at least one attachment")]
    NoStoredAttachments,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(byte: u8, size_bytes: u32) -> ComposerAttachmentRef {
        ComposerAttachmentRef::new(
            ComposerAttachmentDigest::new([byte; 32]),
            ImageMimeType::Png,
            "capture.png",
            size_bytes,
        )
        .expect("valid reference")
    }

    #[test]
    fn revisions_stay_within_sqlite_range() {
        assert!(ComposerDraftRevision::new(u64::MAX).is_err());
        let last = ComposerDraftRevision::new(i64::MAX.cast_unsigned()).unwrap();
        assert_eq!(last.next(), None);
        assert_eq!(
            ComposerDraftRevision::new(4)
                .unwrap()
                .next()
                .map(ComposerDraftRevision::get),
            Some(5)
        );
    }

    #[test]
    fn references_keep_the_message_image_bounds() {
        assert_eq!(
            ComposerAttachmentRef::new(
                ComposerAttachmentDigest::new([1; 32]),
                ImageMimeType::Png,
                "../escape.png",
                3,
            ),
            Err(ComposerDraftError::InvalidAttachment)
        );
        assert!(ComposerAttachmentDigest::from_slice(&[0; 31]).is_err());
        let too_many = vec![reference(1, 1); MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT + 1];
        assert!(matches!(
            ComposerDraft::new(
                ComposerDraftRevision::new(1).unwrap(),
                AuthoredText::empty(),
                too_many,
                UnixMillis::from_millis(0),
            ),
            Err(ComposerDraftError::TooManyAttachments { .. })
        ));
        let large = u32::try_from(MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES).unwrap();
        assert!(matches!(
            QueueStoredMessage::new(
                RequestId::parse("request-b").unwrap(),
                ThreadId::parse("thread-b").unwrap(),
                None,
                vec![
                    reference(1, large),
                    reference(2, large),
                    reference(3, large)
                ],
                None,
            ),
            Err(ComposerDraftError::AttachmentsTooLarge { .. })
        ));
        assert_eq!(
            format!("{}", ComposerAttachmentDigest::new([0xab; 32])).len(),
            64
        );
    }
}
