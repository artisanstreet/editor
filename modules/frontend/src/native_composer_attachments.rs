//! Native image intake and custody for the GPUI composer.
//!
//! File reads, decoding, resizing, encoding, and image-policy inspection all
//! happen in the background executor. The composer receives only bounded
//! presentation data and never retains a source path.

#![forbid(unsafe_code)]

use std::{
    fmt,
    fs::File,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use gpui::{ClipboardEntry, ClipboardItem, Image, ImageFormat, RenderImage, SvgRenderer};
use image::{
    DynamicImage, ImageDecoder, ImageFormat as EncodedImageFormat, ImageReader, Limits,
    imageops::FilterType,
};
use sha2::{Digest, Sha256};

use crate::{
    composer_draft_session_policy::ComposerImageAttachment,
    image_policy::{ImageDimensions, ImageMediaType, best_image_format, image_rescale_target},
};

/// The maximum number of images accepted by one composer draft.
pub(super) const MAXIMUM_ATTACHMENT_COUNT: usize = 10;
/// The maximum number of encoded bytes retained for one image.
pub(super) const MAXIMUM_ATTACHMENT_BYTES: usize = 5 * 1024 * 1024;
/// The maximum encoded-byte total retained by one composer draft.
pub(super) const MAXIMUM_ATTACHMENT_TOTAL_BYTES: usize = 12 * 1024 * 1024;
/// The maximum source bytes read for one intake operation.
///
/// Electron's five-megabyte limit applies to the encoded attachment sent to
/// the engine. A separate bounded source ceiling permits a compressible image
/// to be resized or re-encoded without ever allowing an unbounded file read.
pub(super) const MAXIMUM_RAW_ATTACHMENT_BYTES: usize = 32 * 1024 * 1024;
/// The maximum source bytes admitted from one clipboard batch. Files are read
/// one at a time, so their per-file source ceiling is the relevant bound.
pub(super) const MAXIMUM_RAW_ATTACHMENT_TOTAL_BYTES: usize = 32 * 1024 * 1024;
/// The maximum decoded RGBA pixel count allowed before a preview is retained.
pub(super) const MAXIMUM_DECODED_IMAGE_PIXELS: usize = 16 * 1024 * 1024;
const MAXIMUM_BASE64_INPUT_BYTES: usize = ((MAXIMUM_RAW_ATTACHMENT_BYTES + 2) / 3) * 4 + 4;

pub(super) const ATTACHMENT_COUNT_LIMIT_MESSAGE: &str = "Attach up to 10 images at a time.";
pub(super) const ATTACHMENT_TOTAL_LIMIT_MESSAGE: &str =
    "Attached images together cannot exceed 12 MiB.";
pub(super) const ATTACHMENT_UNSUPPORTED_FORMAT_MESSAGE: &str =
    "That file is not a JPEG, PNG, WebP, or GIF image.";
pub(super) const ATTACHMENT_TOO_LARGE_MESSAGE: &str = "That image exceeds the 5 MiB limit.";
const ATTACHMENT_SOURCE_TOO_LARGE_MESSAGE: &str =
    "That source image is too large to process safely.";

/// The exact typed image upload seam needed by the native transport follow-up.
///
/// The legacy first-message command accepted only `MessageBody`, so intake
/// itself never sends this value. The typed composer path maps the same bytes
/// into domain image parts. Its vector order is the authored attachment
/// order; no text marker is needed for an image-only message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeComposerAttachmentPayload {
    /// Request-local identity referenced by a future image content part.
    pub client_token: String,
    /// Protocol media type, one of the four accepted image types.
    pub media_type: String,
    /// Output file name matching `media_type` when native encoding changed it.
    pub name: String,
    /// Exact encoded bytes to pass to the future typed transport command.
    pub bytes: Vec<u8>,
    /// Position in the ordered attachment vector.
    pub position: usize,
}

/// A generation-fenced, ordered snapshot for submission or retry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeComposerAttachmentSnapshot {
    /// Draft scope generation at which the snapshot was taken.
    pub draft_generation: u64,
    /// Ready image payloads in attachment order.
    pub attachments: Vec<NativeComposerAttachmentPayload>,
}

/// Why the future typed upload seam cannot produce an image payload yet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AttachmentPayloadError {
    /// At least one live attachment is still being read or decoded.
    NotReady { attachment_id: String },
}

impl fmt::Display for AttachmentPayloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotReady { .. } => formatter.write_str("an image is still being prepared"),
        }
    }
}

impl std::error::Error for AttachmentPayloadError {}

/// The image bytes and metadata received from a clipboard image entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClipboardImageCandidate {
    pub(super) name: String,
    pub(super) format: ImageFormat,
    pub(super) bytes: Vec<u8>,
}

/// Clipboard content classified without turning a path into message text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ClipboardInput {
    /// One or more image entries, preferred over text when both are present.
    Images(Vec<ClipboardImageCandidate>),
    /// External paths supplied by the platform clipboard.
    Files(Vec<PathBuf>),
    /// Ordinary text to insert into the editor.
    Text(String),
    /// No supported clipboard content.
    Empty,
}

/// Converts clipboard entries into attachment work or editor text.
///
/// GPUI's convenience [`ClipboardItem::text`] method falls back to joining
/// external paths. That fallback is intentionally not used here: a dropped or
/// copied path must be read as an image candidate, never serialized into a
/// user message.
pub(super) fn classify_clipboard(item: ClipboardItem) -> ClipboardInput {
    let mut images = Vec::new();
    let mut files = Vec::new();
    let mut text = String::new();

    for entry in item.into_entries() {
        match entry {
            ClipboardEntry::Image(image) => images.push(ClipboardImageCandidate {
                name: format!("Pasted image.{}", image.format.extension()),
                format: image.format,
                bytes: image.bytes,
            }),
            ClipboardEntry::ExternalPaths(paths) => {
                files.extend(paths.paths().iter().cloned());
            }
            ClipboardEntry::String(value) => text.push_str(value.text()),
        }
    }

    if !images.is_empty() {
        ClipboardInput::Images(images)
    } else if !files.is_empty() {
        ClipboardInput::Files(files)
    } else if !text.is_empty() {
        ClipboardInput::Text(text)
    } else {
        ClipboardInput::Empty
    }
}

/// A bounded failure from image intake or restoration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum AttachmentPreparationError {
    /// The source or encoded output exceeded a byte budget.
    TooLarge { size: usize, maximum: usize },
    /// The platform-provided format or file signature is outside the native
    /// composer contract.
    UnsupportedFormat,
    /// A filesystem entry could not be read as a regular file.
    NotAFile,
    /// A bounded filesystem read failed without retaining the source path.
    ReadFailed,
    /// The bytes did not decode as the declared image format.
    InvalidImage,
    /// The decoded preview would exceed the decompression ceiling.
    DecodedImageTooLarge { width: u32, height: u32 },
    /// Restored base64 content was malformed.
    InvalidBase64,
    /// Restored content did not match a source digest when an exact raw-byte
    /// comparison was possible.
    DigestMismatch,
}

impl fmt::Display for AttachmentPreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { maximum, .. } if *maximum == MAXIMUM_ATTACHMENT_TOTAL_BYTES => {
                formatter.write_str(ATTACHMENT_TOTAL_LIMIT_MESSAGE)
            }
            Self::TooLarge { maximum, .. }
                if *maximum == MAXIMUM_RAW_ATTACHMENT_BYTES
                    || *maximum == MAXIMUM_RAW_ATTACHMENT_TOTAL_BYTES =>
            {
                formatter.write_str(ATTACHMENT_SOURCE_TOO_LARGE_MESSAGE)
            }
            Self::TooLarge { .. } => formatter.write_str(ATTACHMENT_TOO_LARGE_MESSAGE),
            Self::UnsupportedFormat => formatter.write_str(ATTACHMENT_UNSUPPORTED_FORMAT_MESSAGE),
            Self::NotAFile | Self::ReadFailed => {
                formatter.write_str("That file could not be read as an image.")
            }
            Self::InvalidImage => formatter.write_str("That image could not be decoded."),
            Self::DecodedImageTooLarge { .. } => {
                formatter.write_str("That image is too large to preview safely.")
            }
            Self::InvalidBase64 | Self::DigestMismatch => {
                formatter.write_str("That saved image could not be restored safely.")
            }
        }
    }
}

impl std::error::Error for AttachmentPreparationError {}

/// The result of one source read/decode, keyed so the UI can fence it.
#[derive(Debug)]
pub(super) struct AttachmentPreparationOutcome {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) result: Result<PreparedComposerAttachment, AttachmentPreparationError>,
}

/// A decoded, policy-processed image ready for GPUI presentation.
#[derive(Clone, Debug)]
pub(super) struct PreparedComposerAttachment {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) format: ImageFormat,
    pub(super) mime_type: String,
    pub(super) bytes: Arc<Vec<u8>>,
    /// Draft-safe base64 prepared with the encoded payload off the UI thread.
    pub(super) content_base64: String,
    pub(super) thumbnail: Arc<RenderImage>,
    pub(super) dimensions: ImageDimensions,
    pub(super) recommended_media_type: ImageMediaType,
    pub(super) rescale_target: Option<ImageDimensions>,
    pub(super) source_digest: String,
    pub(super) encoded_digest: String,
    pub(super) source_size_bytes: usize,
    pub(super) size_bytes: usize,
}

/// One live composer attachment, including a pending slot while background
/// work is in progress.
#[derive(Clone, Debug)]
pub(super) struct ComposerAttachment {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) format: Option<ImageFormat>,
    pub(super) mime_type: String,
    pub(super) bytes: Option<Arc<Vec<u8>>>,
    /// Draft-safe base64 for the encoded payload. Pending slots keep this empty.
    pub(super) content_base64: String,
    pub(super) thumbnail: Option<Arc<RenderImage>>,
    pub(super) dimensions: Option<ImageDimensions>,
    pub(super) recommended_media_type: Option<ImageMediaType>,
    pub(super) rescale_target: Option<ImageDimensions>,
    pub(super) source_digest: String,
    pub(super) encoded_digest: String,
    pub(super) source_size_bytes: usize,
    pub(super) size_bytes: usize,
}

impl ComposerAttachment {
    pub(super) fn pending(
        id: impl Into<String>,
        name: impl Into<String>,
        format: Option<ImageFormat>,
        mime_type: impl Into<String>,
        size_bytes: usize,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            format,
            mime_type: mime_type.into(),
            bytes: None,
            content_base64: String::new(),
            thumbnail: None,
            dimensions: None,
            recommended_media_type: None,
            rescale_target: None,
            source_digest: String::new(),
            encoded_digest: String::new(),
            source_size_bytes: size_bytes,
            size_bytes,
        }
    }

    pub(super) fn from_prepared(prepared: PreparedComposerAttachment) -> Self {
        Self {
            id: prepared.id,
            name: prepared.name,
            format: Some(prepared.format),
            mime_type: prepared.mime_type,
            bytes: Some(prepared.bytes),
            content_base64: prepared.content_base64,
            thumbnail: Some(prepared.thumbnail),
            dimensions: Some(prepared.dimensions),
            recommended_media_type: Some(prepared.recommended_media_type),
            rescale_target: prepared.rescale_target,
            source_digest: prepared.source_digest,
            encoded_digest: prepared.encoded_digest,
            source_size_bytes: prepared.source_size_bytes,
            size_bytes: prepared.size_bytes,
        }
    }

    pub(super) fn is_ready(&self) -> bool {
        self.bytes.is_some() && self.thumbnail.is_some()
    }

    pub(super) fn source_bytes_len(&self) -> usize {
        self.bytes
            .as_ref()
            .map_or(self.source_size_bytes, |bytes| bytes.len())
    }

    pub(super) fn payload(
        &self,
        position: usize,
    ) -> Result<NativeComposerAttachmentPayload, AttachmentPayloadError> {
        let bytes = self
            .bytes
            .as_ref()
            .ok_or_else(|| AttachmentPayloadError::NotReady {
                attachment_id: self.id.clone(),
            })?;
        Ok(NativeComposerAttachmentPayload {
            client_token: self.id.clone(),
            media_type: self.mime_type.clone(),
            name: self.name.clone(),
            bytes: bytes.as_ref().clone(),
            position,
        })
    }

    pub(super) fn draft_value(&self) -> ComposerImageAttachment {
        ComposerImageAttachment {
            content_base64: self.content_base64.clone(),
            id: self.id.clone(),
            mime_type: self.mime_type.clone(),
            name: self.name.clone(),
            preview_url: format!("native://composer/attachment/{}", self.id),
            ready: self.is_ready(),
            size_bytes: self.size_bytes,
            source_digest: self.source_digest.clone(),
            source_size_bytes: self.source_size_bytes,
        }
    }
}

/// Input needed to revive one stored attachment without interpreting its
/// preview reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RestoredAttachmentInput {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) mime_type: String,
    pub(super) content_base64: String,
    pub(super) size_bytes: usize,
    pub(super) source_digest: String,
    pub(super) source_size_bytes: usize,
}

/// One already-validated encoded image being recalled from a queued message.
///
/// Unlike draft-session restoration this input already owns the exact bytes
/// accepted by the queue. The preparation worker only decodes them to build a
/// bounded thumbnail; it never resizes or re-encodes the message payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RecalledAttachmentInput {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) mime_type: String,
    pub(super) bytes: Vec<u8>,
}

/// Prepares a batch of clipboard images sequentially on a background worker.
pub(super) fn prepare_clipboard_batch(
    items: Vec<(String, ClipboardImageCandidate)>,
    engine_id: Option<String>,
) -> Vec<AttachmentPreparationOutcome> {
    let mut input_total: usize = 0;
    let mut output_total = 0;
    items
        .into_iter()
        .map(|(id, candidate)| {
            let name = candidate.name.clone();
            let input_size = candidate.bytes.len();
            let result =
                if input_total.saturating_add(input_size) > MAXIMUM_RAW_ATTACHMENT_TOTAL_BYTES {
                    Err(AttachmentPreparationError::TooLarge {
                        size: input_total.saturating_add(input_size),
                        maximum: MAXIMUM_RAW_ATTACHMENT_TOTAL_BYTES,
                    })
                } else {
                    input_total = input_total.saturating_add(input_size);
                    limit_batch_output(
                        prepare_image_bytes(
                            id.clone(),
                            candidate.name,
                            candidate.format,
                            candidate.bytes,
                            engine_id.as_deref(),
                        ),
                        &mut output_total,
                    )
                };
            AttachmentPreparationOutcome { id, name, result }
        })
        .collect()
}

/// Reads and prepares dropped files sequentially on a background worker.
pub(super) fn prepare_file_batch(
    items: Vec<(String, PathBuf)>,
    engine_id: Option<String>,
) -> Vec<AttachmentPreparationOutcome> {
    let mut output_total = 0;
    items
        .into_iter()
        .map(|(id, path)| {
            let name = display_file_name(&path);
            let result = limit_batch_output(
                read_and_prepare_file(id.clone(), path, engine_id.as_deref()),
                &mut output_total,
            );
            AttachmentPreparationOutcome { id, name, result }
        })
        .collect()
}

/// Revives ready values retained by the draft policy.
pub(super) fn prepare_restored_batch(
    items: Vec<RestoredAttachmentInput>,
    engine_id: Option<String>,
) -> Vec<AttachmentPreparationOutcome> {
    let mut output_total = 0;
    items
        .into_iter()
        .map(|item| {
            let id = item.id.clone();
            let name = item.name.clone();
            let result = limit_batch_output(
                decode_and_prepare_restored(item, engine_id.as_deref()),
                &mut output_total,
            );
            AttachmentPreparationOutcome { id, name, result }
        })
        .collect()
}

/// Prepares recalled queue bytes on a background worker while preserving the
/// exact encoded payload accepted by Forge.
pub(super) fn prepare_recalled_batch(
    items: Vec<RecalledAttachmentInput>,
) -> Vec<AttachmentPreparationOutcome> {
    let mut output_total = 0;
    items
        .into_iter()
        .map(|item| {
            let RecalledAttachmentInput {
                id,
                name,
                mime_type,
                bytes,
            } = item;
            let outcome_id = id.clone();
            let outcome_name = name.clone();
            let result = ImageFormat::from_mime_type(&mime_type).map_or_else(
                || Err(AttachmentPreparationError::UnsupportedFormat),
                |format| prepare_preserved_encoded(id, name, format, bytes, String::new(), 0),
            );
            let result = limit_batch_output(result, &mut output_total);
            AttachmentPreparationOutcome {
                id: outcome_id,
                name: outcome_name,
                result,
            }
        })
        .collect()
}

fn limit_batch_output(
    result: Result<PreparedComposerAttachment, AttachmentPreparationError>,
    output_total: &mut usize,
) -> Result<PreparedComposerAttachment, AttachmentPreparationError> {
    let prepared = result?;
    let total = output_total.saturating_add(prepared.size_bytes);
    if total > MAXIMUM_ATTACHMENT_TOTAL_BYTES {
        return Err(AttachmentPreparationError::TooLarge {
            size: total,
            maximum: MAXIMUM_ATTACHMENT_TOTAL_BYTES,
        });
    }
    *output_total = total;
    Ok(prepared)
}

/// Reads, validates, decodes, resizes, encodes, and policy-checks one image.
pub(super) fn prepare_image_bytes(
    id: String,
    name: String,
    format: ImageFormat,
    bytes: Vec<u8>,
    engine_id: Option<&str>,
) -> Result<PreparedComposerAttachment, AttachmentPreparationError> {
    validate_format(format)?;
    validate_raw_size(bytes.len())?;

    let source_digest = sha256_hex(&bytes);
    let (decoded, source_dimensions) = decode_bounded(&bytes, format)?;
    let source_dimensions = ImageDimensions {
        width: f64::from(source_dimensions.0),
        height: f64::from(source_dimensions.1),
    };

    // Electron intentionally keeps GIF bytes/animation untouched. The native
    // preview uses its first safely-decoded frame, while the future payload
    // receives the original animated bytes.
    let is_gif = format == ImageFormat::Gif;
    let rescale_target = (!is_gif)
        .then(|| image_rescale_target(source_dimensions))
        .flatten();
    let display_image = if let Some(target) = rescale_target {
        decoded.resize_exact(
            target.width as u32,
            target.height as u32,
            FilterType::Lanczos3,
        )
    } else {
        decoded
    };

    let recommended_media_type = if is_gif {
        ImageMediaType::Gif
    } else {
        best_image_format(engine_id)
    };
    let recommended_format = encoded_format_for_media_type(recommended_media_type);
    let (output_bytes, output_format, output_name) = if is_gif {
        (bytes.clone(), format, name.clone())
    } else {
        let encoded = encode_image(&display_image, recommended_format);
        let use_encoded = rescale_target.is_some()
            || encoded
                .as_ref()
                .is_ok_and(|encoded| encoded.len() < bytes.len());
        if !use_encoded {
            (bytes.clone(), format, name.clone())
        } else {
            let encoded = encoded.map_err(|_| AttachmentPreparationError::InvalidImage)?;
            validate_encoded_size(encoded.len())?;
            let encoded_format = to_gpui_format(recommended_format)
                .ok_or(AttachmentPreparationError::UnsupportedFormat)?;
            (
                encoded,
                encoded_format,
                encoded_name(&name, recommended_format),
            )
        }
    };
    validate_encoded_size(output_bytes.len())?;

    let output_dimensions = ImageDimensions {
        width: f64::from(display_image.width()),
        height: f64::from(display_image.height()),
    };
    let thumbnail_bytes = encode_image(&display_image.thumbnail(256, 256), EncodedImageFormat::Png)
        .map_err(|_| AttachmentPreparationError::InvalidImage)?;
    let thumbnail = render_preview(ImageFormat::Png, thumbnail_bytes)?;

    Ok(PreparedComposerAttachment {
        id,
        name: output_name,
        format: output_format,
        mime_type: output_format.mime_type().to_owned(),
        bytes: Arc::new(output_bytes.clone()),
        content_base64: BASE64.encode(&output_bytes),
        thumbnail,
        dimensions: output_dimensions,
        recommended_media_type,
        rescale_target,
        source_digest,
        encoded_digest: sha256_hex(&output_bytes),
        source_size_bytes: bytes.len(),
        size_bytes: output_bytes.len(),
    })
}

fn read_and_prepare_file(
    id: String,
    path: PathBuf,
    engine_id: Option<&str>,
) -> Result<PreparedComposerAttachment, AttachmentPreparationError> {
    let name = display_file_name(&path);
    let bytes = read_bounded_file(&path)?;
    let format = detect_image_format(&bytes)?;
    prepare_image_bytes(id, name, format, bytes, engine_id)
}

fn decode_and_prepare_restored(
    item: RestoredAttachmentInput,
    _engine_id: Option<&str>,
) -> Result<PreparedComposerAttachment, AttachmentPreparationError> {
    let format = ImageFormat::from_mime_type(&item.mime_type)
        .ok_or(AttachmentPreparationError::UnsupportedFormat)?;
    if item.size_bytes > MAXIMUM_ATTACHMENT_BYTES {
        return Err(AttachmentPreparationError::TooLarge {
            size: item.size_bytes,
            maximum: MAXIMUM_ATTACHMENT_BYTES,
        });
    }
    if item.source_size_bytes > MAXIMUM_RAW_ATTACHMENT_BYTES {
        return Err(AttachmentPreparationError::TooLarge {
            size: item.source_size_bytes,
            maximum: MAXIMUM_RAW_ATTACHMENT_BYTES,
        });
    }
    if item.content_base64.len() > MAXIMUM_BASE64_INPUT_BYTES {
        return Err(AttachmentPreparationError::TooLarge {
            size: item.content_base64.len(),
            maximum: MAXIMUM_BASE64_INPUT_BYTES,
        });
    }
    let bytes = BASE64
        .decode(item.content_base64)
        .map_err(|_| AttachmentPreparationError::InvalidBase64)?;
    validate_raw_size(bytes.len())?;
    if item.size_bytes != 0 && item.size_bytes != bytes.len() {
        return Err(AttachmentPreparationError::DigestMismatch);
    }

    if item.source_size_bytes != 0
        && item.source_size_bytes == bytes.len()
        && !item.source_digest.is_empty()
        && item.source_digest != sha256_hex(&bytes)
    {
        return Err(AttachmentPreparationError::DigestMismatch);
    }

    let source_digest = item.source_digest;
    let source_size_bytes = item.source_size_bytes;
    prepare_preserved_encoded(
        item.id,
        item.name,
        format,
        bytes,
        source_digest,
        source_size_bytes,
    )
}

/// Decodes an encoded image only for metadata and preview generation.
///
/// The byte vector is the already accepted queue/draft representation. It is
/// retained byte-for-byte, so a recall or restart cannot silently introduce a
/// second resize or a lossy quality change.
fn prepare_preserved_encoded(
    id: String,
    name: String,
    format: ImageFormat,
    bytes: Vec<u8>,
    source_digest: String,
    source_size_bytes: usize,
) -> Result<PreparedComposerAttachment, AttachmentPreparationError> {
    let recommended_media_type = match format {
        ImageFormat::Gif => ImageMediaType::Gif,
        ImageFormat::Jpeg => ImageMediaType::Jpeg,
        ImageFormat::Png => ImageMediaType::Png,
        ImageFormat::Webp => ImageMediaType::Webp,
        ImageFormat::Svg
        | ImageFormat::Bmp
        | ImageFormat::Tiff
        | ImageFormat::Ico
        | ImageFormat::Pnm => return Err(AttachmentPreparationError::UnsupportedFormat),
    };
    let encoded_size = bytes.len();
    validate_encoded_size(encoded_size)?;
    let bytes = Arc::new(bytes);
    let encoded_digest = sha256_hex(bytes.as_ref());
    let (decoded, source_dimensions) = decode_bounded(bytes.as_ref(), format)?;
    let thumbnail_bytes = encode_image(&decoded.thumbnail(256, 256), EncodedImageFormat::Png)
        .map_err(|_| AttachmentPreparationError::InvalidImage)?;
    let thumbnail = render_preview(ImageFormat::Png, thumbnail_bytes)?;
    let source_digest = if source_digest.is_empty() {
        encoded_digest.clone()
    } else {
        source_digest
    };
    let source_size_bytes = if source_size_bytes == 0 {
        encoded_size
    } else {
        source_size_bytes
    };

    Ok(PreparedComposerAttachment {
        id,
        name,
        format,
        mime_type: format.mime_type().to_owned(),
        content_base64: BASE64.encode(bytes.as_ref()),
        bytes,
        thumbnail,
        dimensions: ImageDimensions {
            width: f64::from(source_dimensions.0),
            height: f64::from(source_dimensions.1),
        },
        recommended_media_type,
        rescale_target: None,
        source_digest,
        encoded_digest,
        source_size_bytes,
        size_bytes: encoded_size,
    })
}

fn validate_format(format: ImageFormat) -> Result<(), AttachmentPreparationError> {
    match format {
        ImageFormat::Gif | ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::Webp => Ok(()),
        ImageFormat::Svg
        | ImageFormat::Bmp
        | ImageFormat::Tiff
        | ImageFormat::Ico
        | ImageFormat::Pnm => Err(AttachmentPreparationError::UnsupportedFormat),
    }
}

fn validate_raw_size(size: usize) -> Result<(), AttachmentPreparationError> {
    if size > MAXIMUM_RAW_ATTACHMENT_BYTES {
        Err(AttachmentPreparationError::TooLarge {
            size,
            maximum: MAXIMUM_RAW_ATTACHMENT_BYTES,
        })
    } else {
        Ok(())
    }
}

fn validate_encoded_size(size: usize) -> Result<(), AttachmentPreparationError> {
    if size > MAXIMUM_ATTACHMENT_BYTES {
        Err(AttachmentPreparationError::TooLarge {
            size,
            maximum: MAXIMUM_ATTACHMENT_BYTES,
        })
    } else {
        Ok(())
    }
}

fn read_bounded_file(path: &Path) -> Result<Vec<u8>, AttachmentPreparationError> {
    let metadata = std::fs::metadata(path).map_err(|_| AttachmentPreparationError::ReadFailed)?;
    if !metadata.is_file() {
        return Err(AttachmentPreparationError::NotAFile);
    }
    if metadata.len() > MAXIMUM_RAW_ATTACHMENT_BYTES as u64 {
        return Err(AttachmentPreparationError::TooLarge {
            size: usize::try_from(metadata.len()).unwrap_or(MAXIMUM_RAW_ATTACHMENT_BYTES + 1),
            maximum: MAXIMUM_RAW_ATTACHMENT_BYTES,
        });
    }

    let mut file = File::open(path).map_err(|_| AttachmentPreparationError::ReadFailed)?;
    let mut bytes =
        Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(MAXIMUM_RAW_ATTACHMENT_BYTES));
    file.by_ref()
        .take((MAXIMUM_RAW_ATTACHMENT_BYTES as u64) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AttachmentPreparationError::ReadFailed)?;
    validate_raw_size(bytes.len())?;
    Ok(bytes)
}

pub(super) fn display_file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Pasted image".to_owned())
}

fn detect_image_format(bytes: &[u8]) -> Result<ImageFormat, AttachmentPreparationError> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| AttachmentPreparationError::UnsupportedFormat)?;
    let format = reader
        .format()
        .ok_or(AttachmentPreparationError::UnsupportedFormat)?;
    to_gpui_format(format).ok_or(AttachmentPreparationError::UnsupportedFormat)
}

fn decode_bounded(
    bytes: &[u8],
    format: ImageFormat,
) -> Result<(DynamicImage, (u32, u32)), AttachmentPreparationError> {
    let mut reader = ImageReader::new(Cursor::new(bytes));
    reader.set_format(to_encoded_format(format));
    reader.limits(decoding_limits());
    let decoder = reader
        .into_decoder()
        .map_err(|_| AttachmentPreparationError::InvalidImage)?;
    let dimensions = decoder.dimensions();
    if !decoded_dimensions_are_bounded(dimensions.0, dimensions.1) {
        return Err(AttachmentPreparationError::DecodedImageTooLarge {
            width: dimensions.0,
            height: dimensions.1,
        });
    }
    let decoded = DynamicImage::from_decoder(decoder)
        .map_err(|_| AttachmentPreparationError::InvalidImage)?;
    Ok((decoded, dimensions))
}

fn decoding_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAXIMUM_DECODED_IMAGE_PIXELS as u32);
    limits.max_image_height = Some(MAXIMUM_DECODED_IMAGE_PIXELS as u32);
    limits.max_alloc = Some(
        (MAXIMUM_DECODED_IMAGE_PIXELS as u64)
            .saturating_mul(4)
            .saturating_add(16 * 1024 * 1024),
    );
    limits
}

fn decoded_dimensions_are_bounded(width: u32, height: u32) -> bool {
    width != 0
        && height != 0
        && usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .is_some_and(|pixels| pixels <= MAXIMUM_DECODED_IMAGE_PIXELS)
}

fn encode_image(image: &DynamicImage, format: EncodedImageFormat) -> image::ImageResult<Vec<u8>> {
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, format)?;
    Ok(output.into_inner())
}

fn render_preview(
    format: ImageFormat,
    bytes: Vec<u8>,
) -> Result<Arc<RenderImage>, AttachmentPreparationError> {
    Image::from_bytes(format, bytes)
        .to_image_data(SvgRenderer::new(Arc::new(())))
        .map_err(|_| AttachmentPreparationError::InvalidImage)
}

/// Decodes exactly one selected payload with the same limits as intake, then
/// gives GPUI a bounded PNG generated from that decoded image. The full
/// RenderImage exists only while the viewer is open; attachments retain only
/// encoded bytes and their small tray thumbnail.
pub(super) fn render_full_preview(
    format: ImageFormat,
    bytes: &[u8],
) -> Result<Arc<RenderImage>, AttachmentPreparationError> {
    let (decoded, _) = decode_bounded(bytes, format)?;
    let preview_bytes = encode_image(&decoded, EncodedImageFormat::Png)
        .map_err(|_| AttachmentPreparationError::InvalidImage)?;
    render_preview(ImageFormat::Png, preview_bytes)
}

fn to_encoded_format(format: ImageFormat) -> EncodedImageFormat {
    match format {
        ImageFormat::Gif => EncodedImageFormat::Gif,
        ImageFormat::Jpeg => EncodedImageFormat::Jpeg,
        ImageFormat::Png => EncodedImageFormat::Png,
        ImageFormat::Webp => EncodedImageFormat::WebP,
        ImageFormat::Svg
        | ImageFormat::Bmp
        | ImageFormat::Tiff
        | ImageFormat::Ico
        | ImageFormat::Pnm => EncodedImageFormat::Png,
    }
}

fn encoded_format_for_media_type(media_type: ImageMediaType) -> EncodedImageFormat {
    match media_type {
        ImageMediaType::Gif => EncodedImageFormat::Gif,
        ImageMediaType::Jpeg => EncodedImageFormat::Jpeg,
        ImageMediaType::Png => EncodedImageFormat::Png,
        ImageMediaType::Webp => EncodedImageFormat::WebP,
    }
}

fn to_gpui_format(format: EncodedImageFormat) -> Option<ImageFormat> {
    match format {
        EncodedImageFormat::Gif => Some(ImageFormat::Gif),
        EncodedImageFormat::Jpeg => Some(ImageFormat::Jpeg),
        EncodedImageFormat::Png => Some(ImageFormat::Png),
        EncodedImageFormat::WebP => Some(ImageFormat::Webp),
        _ => None,
    }
}

fn encoded_name(name: &str, format: EncodedImageFormat) -> String {
    let stem = Path::new(name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "Pasted image".to_owned());
    let extension = match format {
        EncodedImageFormat::Gif => "gif",
        EncodedImageFormat::Jpeg => "jpg",
        EncodedImageFormat::Png => "png",
        EncodedImageFormat::WebP => "webp",
        _ => "png",
    };
    format!("{stem}.{extension}")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use fmt::Write as _;
        write!(output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    // A valid 1x1 RGBA PNG. Keeping the fixture inline makes the focused
    // tests independent of the filesystem and network.
    const ONE_BY_ONE_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x72, 0x9c, 0x52, 0x67, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn clipboard_image_entries_are_preferred_over_text() {
        let item = ClipboardItem {
            entries: vec![
                ClipboardEntry::Image(Image::from_bytes(ImageFormat::Png, ONE_BY_ONE_PNG.to_vec())),
                ClipboardEntry::String(gpui::ClipboardString::new("ignored".into())),
            ],
        };
        assert!(matches!(
            classify_clipboard(item),
            ClipboardInput::Images(images) if images.len() == 1
        ));
    }

    #[test]
    fn clipboard_paths_are_classified_as_files_not_text() {
        let path = PathBuf::from("/private/source.png");
        let paths = gpui::ExternalPaths([path.clone()].into_iter().collect());
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::ExternalPaths(paths)],
        };
        assert_eq!(classify_clipboard(item), ClipboardInput::Files(vec![path]));
    }

    #[test]
    fn intake_resizes_encodes_and_retains_source_digest() {
        let prepared = prepare_image_bytes(
            "attachment:0".into(),
            "pixel.png".into(),
            ImageFormat::Png,
            ONE_BY_ONE_PNG.to_vec(),
            Some("claude"),
        )
        .expect("fixture decodes");
        assert_eq!(prepared.dimensions.width, 1.0);
        assert_eq!(prepared.dimensions.height, 1.0);
        assert_eq!(prepared.recommended_media_type, ImageMediaType::Webp);
        assert!(prepared.rescale_target.is_none());
        assert_eq!(prepared.source_size_bytes, ONE_BY_ONE_PNG.len());
        assert_eq!(prepared.size_bytes, prepared.bytes.len());
        assert_eq!(prepared.source_digest.len(), 64);
        assert_eq!(prepared.encoded_digest.len(), 64);
        assert_eq!(
            prepared.content_base64,
            BASE64.encode(prepared.bytes.as_ref())
        );
        assert_eq!(prepared.source_digest, sha256_hex(ONE_BY_ONE_PNG));
        assert_eq!(prepared.encoded_digest, sha256_hex(prepared.bytes.as_ref()));
        assert!(prepared.size_bytes <= MAXIMUM_ATTACHMENT_BYTES);
    }

    #[test]
    fn recalled_preparation_preserves_exact_encoded_bytes() {
        let outcomes = prepare_recalled_batch(vec![RecalledAttachmentInput {
            id: "attachment:recall".into(),
            name: "recalled.png".into(),
            mime_type: "image/png".into(),
            bytes: ONE_BY_ONE_PNG.to_vec(),
        }]);
        let prepared = outcomes
            .into_iter()
            .next()
            .expect("one recall outcome")
            .result
            .expect("PNG fixture decodes");

        assert_eq!(prepared.bytes.as_ref(), ONE_BY_ONE_PNG);
        assert_eq!(prepared.content_base64, BASE64.encode(ONE_BY_ONE_PNG));
        assert_eq!(prepared.encoded_digest, sha256_hex(ONE_BY_ONE_PNG));
        assert_eq!(prepared.source_digest, sha256_hex(ONE_BY_ONE_PNG));
        assert!(prepared.rescale_target.is_none());
    }

    #[test]
    fn resize_uses_the_policy_long_edge_without_enlarging_or_distorting() {
        let source = DynamicImage::new_rgba8(2577, 3);
        let bytes = encode_image(&source, EncodedImageFormat::Png).expect("PNG source");
        let prepared = prepare_image_bytes(
            "attachment:0".into(),
            "wide.png".into(),
            ImageFormat::Png,
            bytes,
            None,
        )
        .expect("wide fixture decodes");

        assert_eq!(
            prepared.rescale_target,
            Some(ImageDimensions {
                width: 2576.0,
                height: 3.0,
            })
        );
        assert_eq!(prepared.dimensions.width, 2576.0);
        assert_eq!(prepared.dimensions.height, 3.0);
        assert_eq!(prepared.format, ImageFormat::Png);
        assert_ne!(prepared.source_digest, prepared.encoded_digest);
    }

    #[test]
    fn intake_rejects_unsupported_format_before_decoding() {
        let result = prepare_image_bytes(
            "attachment:0".into(),
            "vector.svg".into(),
            ImageFormat::Svg,
            b"<svg/>".to_vec(),
            None,
        );
        assert!(matches!(
            result,
            Err(AttachmentPreparationError::UnsupportedFormat)
        ));
    }

    #[test]
    fn encoded_output_keeps_the_five_mib_limit_separate_from_raw_intake() {
        let result = validate_encoded_size(MAXIMUM_ATTACHMENT_BYTES + 1);
        assert!(matches!(
            result,
            Err(AttachmentPreparationError::TooLarge {
                size,
                maximum: MAXIMUM_ATTACHMENT_BYTES,
            }) if size == MAXIMUM_ATTACHMENT_BYTES + 1
        ));
        assert!(validate_raw_size(MAXIMUM_ATTACHMENT_BYTES + 1).is_ok());
        assert!(matches!(
            validate_raw_size(MAXIMUM_RAW_ATTACHMENT_BYTES + 1),
            Err(AttachmentPreparationError::TooLarge {
                size,
                maximum: MAXIMUM_RAW_ATTACHMENT_BYTES,
            }) if size == MAXIMUM_RAW_ATTACHMENT_BYTES + 1
        ));
    }

    #[test]
    fn base64_round_trip_uses_the_shared_engine_and_is_bounded() {
        for bytes in [b"".as_slice(), b"f", b"fo", b"foo", b"native image"] {
            let encoded = BASE64.encode(bytes);
            assert_eq!(BASE64.decode(encoded).expect("valid base64"), bytes);
        }
        assert_eq!(BASE64.encode(b"foobar"), "Zm9vYmFy");
        assert!(BASE64.decode("not-base64!").is_err());
    }

    #[test]
    fn sha256_uses_the_shared_digest_implementation() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn file_format_detection_uses_image_reader_guessing() {
        assert_eq!(
            detect_image_format(ONE_BY_ONE_PNG).expect("PNG format"),
            ImageFormat::Png
        );
        assert_eq!(
            detect_image_format(b"not an image"),
            Err(AttachmentPreparationError::UnsupportedFormat)
        );
    }

    #[test]
    fn decoding_limits_reject_large_dimensions_before_raster_allocation() {
        let mut png = ONE_BY_ONE_PNG.to_vec();
        // Valid header and CRC, but a raster larger than the pixel budget.
        png[16..20].copy_from_slice(&4097_u32.to_be_bytes());
        png[20..24].copy_from_slice(&4096_u32.to_be_bytes());
        png[29..33].copy_from_slice(&0x1d614f29_u32.to_be_bytes());
        let result = prepare_image_bytes(
            "attachment:0".into(),
            "wide.png".into(),
            ImageFormat::Png,
            png,
            None,
        );
        assert!(matches!(
            result,
            Err(AttachmentPreparationError::DecodedImageTooLarge { .. })
        ));
    }
}
