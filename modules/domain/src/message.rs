//! Bounded multimodal message payloads shared by transport, storage, and
//! engine dispatch.
//!
//! Image bytes are owned values, never filesystem paths. The payload keeps
//! attachment order exactly as authored and permits absent or empty text when
//! at least one image is present, which makes image-only messages a first-class
//! command rather than a text placeholder.

use std::fmt;

use thiserror::Error;

use crate::bounds::{
    MESSAGE_BODY_MAX_BYTES, MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES,
    MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT, MESSAGE_IMAGE_ATTACHMENT_MIME_MAX_BYTES,
    MESSAGE_IMAGE_ATTACHMENT_NAME_MAX_BYTES, MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES,
};
use crate::identifiers::{MessageId, ThreadId};

/// Text authored alongside queued images.
///
/// Unlike [`crate::MessageBody`], this value may be empty or whitespace-only:
/// an image supplies the visible content for an image-only or image-led
/// submission. The original bytes are retained without trimming.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AuthoredText(String);

impl AuthoredText {
    /// Maximum UTF-8 byte length accepted for authored text.
    pub const MAX_BYTES: usize = MESSAGE_BODY_MAX_BYTES;

    /// Creates authored text after enforcing the shared body bound.
    pub fn parse(value: impl Into<String>) -> Result<Self, AuthoredTextError> {
        let value = value.into();
        if value.len() > Self::MAX_BYTES {
            return Err(AuthoredTextError::TooLong {
                length: value.len(),
                maximum: Self::MAX_BYTES,
            });
        }
        Ok(Self(value))
    }

    /// Creates an empty authored-text value.
    #[must_use]
    pub const fn empty() -> Self {
        Self(String::new())
    }

    /// Returns the authored text exactly as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the value contains no visible characters.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl fmt::Display for AuthoredText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Failure while validating optional authored message text.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AuthoredTextError {
    /// The text exceeded the shared UTF-8 byte ceiling.
    #[error("authored text is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// Offending UTF-8 byte length.
        length: usize,
        /// Documented byte ceiling.
        maximum: usize,
    },
}

/// MIME types accepted by both composer intake and native engine dispatch.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ImageMimeType {
    /// GIF image bytes.
    Gif,
    /// JPEG image bytes.
    Jpeg,
    /// PNG image bytes.
    Png,
    /// WebP image bytes.
    Webp,
}

impl ImageMimeType {
    /// Returns the stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gif => "image/gif",
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }

    /// Parses one of the four explicitly accepted image MIME types.
    pub fn parse(value: &str) -> Result<Self, ImageMimeTypeError> {
        if value.len() > MESSAGE_IMAGE_ATTACHMENT_MIME_MAX_BYTES {
            return Err(ImageMimeTypeError);
        }
        match value {
            "image/gif" => Ok(Self::Gif),
            "image/jpeg" => Ok(Self::Jpeg),
            "image/png" => Ok(Self::Png),
            "image/webp" => Ok(Self::Webp),
            _ => Err(ImageMimeTypeError),
        }
    }
}

impl fmt::Display for ImageMimeType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// An image MIME value was not in the accepted set.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("image MIME type is not supported")]
pub struct ImageMimeTypeError;

/// One ordered, owned image attachment.
///
/// `bytes` is the encoded image content. It is deliberately not represented
/// as a URI or path, so a client cannot ask the backend or engine to read an
/// arbitrary local file.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ImageAttachment {
    mime_type: ImageMimeType,
    bytes: Vec<u8>,
    name: String,
}

impl fmt::Debug for ImageAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageAttachment")
            .field("mime_type", &self.mime_type)
            .field("bytes_len", &self.bytes.len())
            .field("name", &self.name)
            .finish()
    }
}

impl ImageAttachment {
    /// Creates one validated image attachment from its wire MIME spelling,
    /// owned bytes, and display name.
    pub fn new(
        mime_type: impl AsRef<str>,
        bytes: Vec<u8>,
        name: impl Into<String>,
    ) -> Result<Self, ImageAttachmentError> {
        let mime_type = ImageMimeType::parse(mime_type.as_ref()).map_err(|_| {
            ImageAttachmentError::UnsupportedMimeType {
                mime_type: mime_type.as_ref().to_owned(),
            }
        })?;
        let bytes_len = bytes.len();
        if bytes.is_empty() {
            return Err(ImageAttachmentError::EmptyBytes);
        }
        if bytes_len > MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES {
            return Err(ImageAttachmentError::BytesTooLarge {
                length: bytes_len,
                maximum: MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES,
            });
        }

        let name = name.into();
        validate_attachment_name(&name)?;

        Ok(Self {
            mime_type,
            bytes,
            name,
        })
    }

    /// Returns the validated MIME type.
    #[must_use]
    pub const fn mime_type(&self) -> ImageMimeType {
        self.mime_type
    }

    /// Returns the validated MIME spelling.
    #[must_use]
    pub const fn mime_type_str(&self) -> &'static str {
        self.mime_type.as_str()
    }

    /// Returns the encoded image bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the encoded image byte length.
    #[must_use]
    pub const fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns the validated display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Renderer-visible metadata for one persisted image attachment.
///
/// This reference is intentionally byte-free. The owning message and thread
/// identities, ordered index, immutable metadata, and digest let an
/// authenticated bounded read retrieve exactly one attachment without
/// inflating every conversation snapshot or replay patch with image bytes.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ImageAttachmentRef {
    /// Message that owns the attachment.
    pub message_id: MessageId,
    /// Thread that owns the message.
    pub thread_id: ThreadId,
    /// Zero-based authored attachment position.
    pub index: u32,
    /// Accepted image MIME type.
    pub mime_type: ImageMimeType,
    /// Bounded filename-only display name.
    pub name: String,
    /// Exact encoded image byte length.
    pub size_bytes: u32,
    /// SHA-256 digest of the encoded image bytes.
    pub digest: [u8; 32],
}

impl ImageAttachmentRef {
    /// Builds one validated byte-free reference.
    pub fn new(
        message_id: MessageId,
        thread_id: ThreadId,
        index: u32,
        mime_type: impl AsRef<str>,
        name: impl Into<String>,
        size_bytes: u32,
        digest: [u8; 32],
    ) -> Result<Self, ImageAttachmentRefError> {
        let mime_type = ImageMimeType::parse(mime_type.as_ref())
            .map_err(|_| ImageAttachmentRefError::UnsupportedMimeType)?;
        if size_bytes == 0 || usize::try_from(size_bytes).unwrap_or(usize::MAX)
            > MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES
        {
            return Err(ImageAttachmentRefError::InvalidSize { size_bytes });
        }
        let name = name.into();
        validate_attachment_name(&name)
            .map_err(|_| ImageAttachmentRefError::InvalidName)?;
        Ok(Self {
            message_id,
            thread_id,
            index,
            mime_type,
            name,
            size_bytes,
            digest,
        })
    }

    /// Returns the stable MIME spelling.
    #[must_use]
    pub const fn mime_type_str(&self) -> &'static str {
        self.mime_type.as_str()
    }
}

/// Failure while validating a renderer-visible image reference.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ImageAttachmentRefError {
    /// MIME type was not in the accepted image set.
    #[error("image reference MIME type is not supported")]
    UnsupportedMimeType,
    /// Reference size was empty or exceeded the per-image bound.
    #[error("image reference size {size_bytes} is outside the accepted range")]
    InvalidSize {
        /// Supplied encoded size.
        size_bytes: u32,
    },
    /// Reference name was not a valid bounded filename.
    #[error("image reference name is invalid")]
    InvalidName,
}

/// Failure while validating one image attachment.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ImageAttachmentError {
    /// MIME type is outside the accepted image set.
    #[error("unsupported image MIME type `{mime_type}`")]
    UnsupportedMimeType {
        /// Rejected MIME spelling.
        mime_type: String,
    },
    /// Image content cannot be empty.
    #[error("image attachment bytes must not be empty")]
    EmptyBytes,
    /// One image exceeded its encoded-byte ceiling.
    #[error("image attachment is {length} bytes; the maximum is {maximum}")]
    BytesTooLarge {
        /// Offending byte length.
        length: usize,
        /// Documented per-image ceiling.
        maximum: usize,
    },
    /// Display names must stay short and cannot be filesystem paths.
    #[error("image attachment name is invalid")]
    InvalidName,
    /// Display name exceeded its UTF-8 byte ceiling.
    #[error("image attachment name is {length} UTF-8 bytes; the maximum is {maximum}")]
    NameTooLong {
        /// Offending UTF-8 byte length.
        length: usize,
        /// Documented name ceiling.
        maximum: usize,
    },
}

fn validate_attachment_name(name: &str) -> Result<(), ImageAttachmentError> {
    if name.len() > MESSAGE_IMAGE_ATTACHMENT_NAME_MAX_BYTES {
        return Err(ImageAttachmentError::NameTooLong {
            length: name.len(),
            maximum: MESSAGE_IMAGE_ATTACHMENT_NAME_MAX_BYTES,
        });
    }
    if name.trim().is_empty()
        || name == "."
        || name == ".."
        || name.chars().any(|character| {
            character.is_control() || matches!(character, '/' | '\\' | ':')
        })
    {
        return Err(ImageAttachmentError::InvalidName);
    }
    Ok(())
}

/// Ordered text and image content accepted by the general queue command.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QueueMessagePayload {
    text: Option<AuthoredText>,
    attachments: Vec<ImageAttachment>,
}

impl QueueMessagePayload {
    /// Validates a general message payload.
    ///
    /// At least one nonblank text value or one image is required. Empty text
    /// remains representable when images are present so the wire can preserve
    /// the difference between an absent text field and authored empty text.
    pub fn new(
        text: Option<AuthoredText>,
        attachments: Vec<ImageAttachment>,
    ) -> Result<Self, QueueMessagePayloadError> {
        if attachments.len() > MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT {
            return Err(QueueMessagePayloadError::TooManyAttachments {
                count: attachments.len(),
                maximum: MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT,
            });
        }

        let total_bytes = attachments
            .iter()
            .try_fold(0usize, |total, attachment| {
                total.checked_add(attachment.byte_len())
            })
            .ok_or(QueueMessagePayloadError::AttachmentsTooLarge {
                length: usize::MAX,
                maximum: MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES,
            })?;
        if total_bytes > MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES {
            return Err(QueueMessagePayloadError::AttachmentsTooLarge {
                length: total_bytes,
                maximum: MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES,
            });
        }
        if text.as_ref().is_none_or(AuthoredText::is_blank) && attachments.is_empty() {
            return Err(QueueMessagePayloadError::Empty);
        }

        Ok(Self { text, attachments })
    }

    /// Builds a text-only payload while retaining the general validation.
    pub fn text_only(text: impl Into<String>) -> Result<Self, QueueMessagePayloadError> {
        let text = AuthoredText::parse(text).map_err(QueueMessagePayloadError::Text)?;
        Self::new(Some(text), Vec::new())
    }

    /// Returns the optional authored text.
    #[must_use]
    pub fn text(&self) -> Option<&AuthoredText> {
        self.text.as_ref()
    }

    /// Returns images in the exact authored order.
    #[must_use]
    pub fn attachments(&self) -> &[ImageAttachment] {
        &self.attachments
    }

    /// Returns the aggregate encoded image byte length.
    #[must_use]
    pub fn total_attachment_bytes(&self) -> usize {
        self.attachments.iter().map(ImageAttachment::byte_len).sum()
    }

    /// Whether the payload contains no visible authored text.
    #[must_use]
    pub fn is_image_only(&self) -> bool {
        self.text.as_ref().is_none_or(AuthoredText::is_blank) && !self.attachments.is_empty()
    }
}

/// Failure while validating a general queued message payload.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum QueueMessagePayloadError {
    /// Authored text exceeded its UTF-8 byte ceiling.
    #[error("invalid authored text: {0}")]
    Text(#[source] AuthoredTextError),
    /// Too many images were supplied.
    #[error("message has {count} image attachments; the maximum is {maximum}")]
    TooManyAttachments {
        /// Offending image count.
        count: usize,
        /// Documented count ceiling.
        maximum: usize,
    },
    /// The aggregate encoded image bytes exceeded the payload ceiling.
    #[error("message image attachments total {length} bytes; the maximum is {maximum}")]
    AttachmentsTooLarge {
        /// Offending aggregate length.
        length: usize,
        /// Documented aggregate ceiling.
        maximum: usize,
    },
    /// A message must have visible text or at least one image.
    #[error("message must contain authored text or one image attachment")]
    Empty,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(byte: u8) -> ImageAttachment {
        ImageAttachment::new("image/png", vec![byte], "capture.png").expect("valid image")
    }

    #[test]
    fn image_only_payload_is_valid_and_ordered() {
        let payload = QueueMessagePayload::new(None, vec![image(1), image(2)]).expect("valid");
        assert!(payload.is_image_only());
        assert_eq!(payload.attachments()[0].bytes(), &[1]);
        assert_eq!(payload.attachments()[1].bytes(), &[2]);
    }

    #[test]
    fn present_empty_text_is_preserved_when_an_image_exists() {
        let payload = QueueMessagePayload::new(Some(AuthoredText::empty()), vec![image(1)])
            .expect("an image makes empty authored text valid");
        assert_eq!(payload.text().expect("text is present").as_str(), "");
        assert!(payload.is_image_only());
    }

    #[test]
    fn empty_payload_is_rejected() {
        assert_eq!(
            QueueMessagePayload::new(Some(AuthoredText::empty()), Vec::new()),
            Err(QueueMessagePayloadError::Empty)
        );
    }

    #[test]
    fn path_like_names_and_unknown_mime_are_rejected() {
        assert_eq!(
            ImageAttachment::new("image/tiff", vec![1], "capture.tiff").unwrap_err(),
            ImageAttachmentError::UnsupportedMimeType {
                mime_type: "image/tiff".to_owned()
            }
        );
        assert_eq!(
            ImageAttachment::new("image/png", vec![1], "..\\secret.png").unwrap_err(),
            ImageAttachmentError::InvalidName
        );
    }

    #[test]
    fn oversized_count_and_aggregate_are_rejected_at_the_domain_boundary() {
        assert!(matches!(
            ImageAttachment::new(
                "image/png",
                vec![0; MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES + 1],
                "too-large.png"
            ),
            Err(ImageAttachmentError::BytesTooLarge { .. })
        ));

        let too_many = vec![image(1); MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT + 1];
        assert!(matches!(
            QueueMessagePayload::new(None, too_many),
            Err(QueueMessagePayloadError::TooManyAttachments { .. })
        ));

        let four_megabytes = || {
            ImageAttachment::new(
                "image/png",
                vec![7; 4 * 1024 * 1024],
                "large.png",
            )
            .expect("each image remains under the per-image bound")
        };
        let aggregate = vec![four_megabytes(), four_megabytes(), four_megabytes(), image(8)];
        assert!(matches!(
            QueueMessagePayload::new(None, aggregate),
            Err(QueueMessagePayloadError::AttachmentsTooLarge { .. })
        ));
    }

    #[test]
    fn unicode_filename_and_byte_free_reference_are_preserved() {
        let name = "猫 🐈.png";
        let attachment = ImageAttachment::new("image/png", vec![1, 2, 3], name)
            .expect("unicode display names are valid filename-only names");
        assert_eq!(attachment.name(), name);

        let message_id = MessageId::parse("message-ref").expect("message id");
        let thread_id = ThreadId::parse("thread-ref").expect("thread id");
        let reference = ImageAttachmentRef::new(
            message_id.clone(),
            thread_id.clone(),
            2,
            attachment.mime_type_str(),
            attachment.name().to_owned(),
            attachment.byte_len() as u32,
            [9; 32],
        )
        .expect("reference metadata is valid");
        assert_eq!(reference.message_id, message_id);
        assert_eq!(reference.thread_id, thread_id);
        assert_eq!(reference.index, 2);
        assert_eq!(reference.name, name);
        assert_eq!(reference.size_bytes, 3);
        assert_eq!(reference.digest, [9; 32]);
    }
}
