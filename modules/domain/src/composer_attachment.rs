//! Uploading picked images to the Forge's composer attachment store and
//! reading them back.
//!
//! A picked image can be larger than one transport frame (up to
//! [`COMPOSER_ATTACHMENT_MAX_BYTES`]), so it crosses the wire in chunks of at
//! most [`COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES`]: an upload names the digest
//! of the whole image and each chunk's offset, and the Forge stores the
//! image once every byte arrived and the digest matches. A read names a
//! window of the stored bytes.

use std::fmt;

use crate::bounds::{COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES, COMPOSER_ATTACHMENT_MAX_BYTES};
use crate::composer_draft::{
    ComposerAttachmentDigest, ComposerAttachmentRef, ComposerDraftError, ComposerImage,
};
use crate::identifiers::RequestId;
use crate::message::{ImageMimeType, validate_attachment_name};

/// One chunk of a picked image on its way to the attachment store.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ComposerAttachmentChunk {
    digest: ComposerAttachmentDigest,
    mime_type: ImageMimeType,
    name: String,
    total_bytes: u32,
    offset: u32,
    bytes: Vec<u8>,
}

impl fmt::Debug for ComposerAttachmentChunk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComposerAttachmentChunk")
            .field("digest", &self.digest)
            .field("mime_type", &self.mime_type)
            .field("total_bytes", &self.total_bytes)
            .field("offset", &self.offset)
            .field("bytes_len", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

impl ComposerAttachmentChunk {
    /// Validates one chunk of a whole image of `total_bytes` whose SHA-256
    /// is `digest`.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerDraftError::InvalidAttachment`] for an image over
    /// [`COMPOSER_ATTACHMENT_MAX_BYTES`], an empty chunk or one over
    /// [`COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES`], a chunk past the image's end,
    /// or an invalid display name.
    pub fn new(
        digest: ComposerAttachmentDigest,
        mime_type: ImageMimeType,
        name: impl Into<String>,
        total_bytes: u32,
        offset: u32,
        bytes: Vec<u8>,
    ) -> Result<Self, ComposerDraftError> {
        let name = name.into();
        let within_image = u64::from(offset)
            .checked_add(bytes.len() as u64)
            .is_some_and(|end| end <= u64::from(total_bytes));
        if usize::try_from(total_bytes).map_or(true, |total| total > COMPOSER_ATTACHMENT_MAX_BYTES)
            || bytes.is_empty()
            || bytes.len() > COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES
            || !within_image
            || validate_attachment_name(&name).is_err()
        {
            return Err(ComposerDraftError::InvalidAttachment);
        }
        Ok(Self {
            digest,
            mime_type,
            name,
            total_bytes,
            offset,
            bytes,
        })
    }

    /// Splits a picked image into its chunks, in order.
    #[must_use]
    pub fn split(image: &ComposerImage, digest: ComposerAttachmentDigest) -> Vec<Self> {
        let total_bytes = u32::try_from(image.byte_len()).unwrap_or(u32::MAX);
        image
            .bytes()
            .chunks(COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES)
            .scan(0_u32, |offset, bytes| {
                let chunk = Self {
                    digest,
                    mime_type: image.mime_type(),
                    name: image.name().to_owned(),
                    total_bytes,
                    offset: *offset,
                    bytes: bytes.to_vec(),
                };
                *offset = offset.saturating_add(u32::try_from(bytes.len()).unwrap_or(u32::MAX));
                Some(chunk)
            })
            .collect()
    }

    /// SHA-256 of the whole image.
    #[must_use]
    pub const fn digest(&self) -> &ComposerAttachmentDigest {
        &self.digest
    }

    /// The image's MIME type.
    #[must_use]
    pub const fn mime_type(&self) -> ImageMimeType {
        self.mime_type
    }

    /// The image's authored display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Byte length of the whole image.
    #[must_use]
    pub const fn total_bytes(&self) -> u32 {
        self.total_bytes
    }

    /// Where this chunk starts in the whole image.
    #[must_use]
    pub const fn offset(&self) -> u32 {
        self.offset
    }

    /// This chunk's bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// What one upload request carries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposerUpload {
    /// A whole image that fits one frame.
    Image(ComposerImage),
    /// One chunk of an image of any size up to the store's bound.
    Chunk(ComposerAttachmentChunk),
}

/// Stores one image (or one chunk of it) in the Forge composer attachment
/// store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadComposerAttachment {
    /// Client request identity.
    pub request_id: RequestId,
    /// The image or chunk.
    pub upload: ComposerUpload,
}

/// Acknowledgement of one upload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerAttachmentUploaded {
    /// Client request identity echoed by the enclosing response.
    pub request_id: RequestId,
    /// Reference naming the (stored or, while chunks are missing, still
    /// arriving) bytes under the uploaded display name.
    pub reference: ComposerAttachmentRef,
    /// Bytes of a chunked upload still missing; zero once stored.
    pub pending_bytes: u32,
}

/// Reads a window of the bytes of one stored attachment.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReadComposerAttachment {
    /// Store key.
    pub digest: ComposerAttachmentDigest,
    /// First byte of the window.
    pub offset: u32,
    /// Largest window to answer; zero answers every byte from `offset`.
    pub max_bytes: u32,
}

impl ReadComposerAttachment {
    /// Reads every byte in one answer (an image that fits one frame).
    #[must_use]
    pub const fn whole(digest: ComposerAttachmentDigest) -> Self {
        Self {
            digest,
            offset: 0,
            max_bytes: 0,
        }
    }
}

/// A window of the bytes of one stored attachment.
#[derive(Clone, Eq, PartialEq)]
pub struct ComposerAttachmentResult {
    /// Store key.
    pub digest: ComposerAttachmentDigest,
    /// Stored MIME type.
    pub mime_type: ImageMimeType,
    /// Encoded image bytes of the window.
    pub bytes: Vec<u8>,
    /// Byte length of the whole image.
    pub total_bytes: u32,
    /// Where the window starts.
    pub offset: u32,
}

impl ComposerAttachmentResult {
    /// A window holding the whole stored image.
    #[must_use]
    pub fn whole(
        digest: ComposerAttachmentDigest,
        mime_type: ImageMimeType,
        bytes: Vec<u8>,
    ) -> Self {
        let total_bytes = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        Self {
            digest,
            mime_type,
            bytes,
            total_bytes,
            offset: 0,
        }
    }
}

impl fmt::Debug for ComposerAttachmentResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComposerAttachmentResult")
            .field("digest", &self.digest)
            .field("mime_type", &self.mime_type)
            .field("bytes_len", &self.bytes.len())
            .field("total_bytes", &self.total_bytes)
            .field("offset", &self.offset)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picked_image_splits_into_bounded_ordered_chunks() {
        let bytes = vec![7_u8; COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES * 2 + 3];
        let image = ComposerImage::new("image/png", bytes, "large.png").expect("image");
        let digest = ComposerAttachmentDigest::new([1; 32]);
        let chunks = ComposerAttachmentChunk::split(&image, digest);
        assert_eq!(chunks.len(), 3);
        let offsets: Vec<_> = chunks.iter().map(ComposerAttachmentChunk::offset).collect();
        let chunk = u32::try_from(COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES).expect("chunk");
        assert_eq!(offsets, [0, chunk, chunk * 2]);
        assert_eq!(chunks[2].bytes().len(), 3);
        let total = chunk * 2 + 3;
        assert!(chunks.iter().all(|piece| piece.total_bytes() == total));
    }

    #[test]
    fn chunks_stay_inside_their_image_and_the_store_bound() {
        let digest = ComposerAttachmentDigest::new([2; 32]);
        let chunk = |total, offset, len| {
            ComposerAttachmentChunk::new(
                digest,
                ImageMimeType::Png,
                "a.png",
                total,
                offset,
                vec![0; len],
            )
        };
        assert!(chunk(10, 0, 10).is_ok());
        assert!(chunk(10, 5, 6).is_err(), "past the end");
        assert!(chunk(10, 0, 0).is_err(), "empty");
        let too_large = u32::try_from(COMPOSER_ATTACHMENT_MAX_BYTES + 1).expect("bound");
        assert!(chunk(too_large, 0, 1).is_err());
        assert!(chunk(too_large - 1, 0, COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES + 1).is_err());
    }
}
