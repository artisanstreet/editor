//! Fits a draft's picked images to the engine a message is sent to.
//!
//! The Editor uploads each image exactly as the user picked it; when a draft
//! is sent the Forge applies the engine's image policy (formerly the
//! Editor's intake): every image is decoded within a pixel budget, rescaled
//! so its long edge fits [`MAXIMUM_IMAGE_LONG_EDGE_PIXELS`], and re-encoded
//! in the engine's best accepted format when it was rescaled or the encoding
//! is smaller. GIF bytes (and their animation) pass through untouched. The
//! fitted images must fit the message bounds; anything else is refused with
//! a reason the Editor shows as it is.

#![forbid(unsafe_code)]

use std::io::Cursor;
use std::path::Path;

use artisan_domain::{
    ComposerAttachmentResult, ImageAttachment, ImageMimeType, MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES,
    MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES,
};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, imageops::FilterType};

use crate::image_policy::{
    ImageDimensions, ImageMediaType, MAXIMUM_IMAGE_LONG_EDGE_PIXELS, best_image_format,
    image_rescale_target,
};

/// The decoded-pixel budget one image may use while it is fitted.
const MAXIMUM_DECODED_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;

const TOO_LARGE: &str = "That image exceeds the 5 MiB limit.";
const TOTAL_TOO_LARGE: &str = "Attached images together cannot exceed 12 MiB.";
const UNDECODABLE: &str = "That image could not be decoded.";
const TOO_MANY_PIXELS: &str = "That image is too large to process safely.";

/// Fits every picked image of a draft, in order, for `engine_id`.
///
/// # Errors
///
/// Returns the presentation-ready reason the first image, or the images
/// together, cannot be sent to the engine.
pub(crate) fn fit_draft_images(
    engine_id: &str,
    picked: &[(ComposerAttachmentResult, String)],
) -> Result<Vec<ImageAttachment>, String> {
    let mut fitted = Vec::with_capacity(picked.len());
    let mut total = 0_usize;
    for (image, name) in picked {
        let image =
            fit_image(engine_id, image, name).map_err(|reason| format!("{name}: {reason}"))?;
        total = total.saturating_add(image.byte_len());
        if total > MESSAGE_IMAGE_ATTACHMENTS_MAX_TOTAL_BYTES {
            return Err(TOTAL_TOO_LARGE.to_owned());
        }
        fitted.push(image);
    }
    Ok(fitted)
}

/// Fits one picked image for `engine_id`.
fn fit_image(
    engine_id: &str,
    picked: &ComposerAttachmentResult,
    name: &str,
) -> Result<ImageAttachment, &'static str> {
    let bytes = picked.bytes.as_slice();
    let (bytes, mime_type, name) = if picked.mime_type == ImageMimeType::Gif {
        (bytes.to_vec(), picked.mime_type, name.to_owned())
    } else {
        let decoded = decode_bounded(bytes, picked.mime_type)?;
        let source = ImageDimensions {
            width: f64::from(decoded.width()),
            height: f64::from(decoded.height()),
        };
        let target = image_rescale_target(source);
        let fitted = match target {
            Some(target) => decoded.resize_exact(
                dimension(target.width),
                dimension(target.height),
                FilterType::Lanczos3,
            ),
            None => decoded,
        };
        let media_type = best_image_format(Some(engine_id));
        let encoded = encode(&fitted, media_type);
        let use_encoded = target.is_some()
            || encoded
                .as_ref()
                .is_ok_and(|encoded| encoded.len() < bytes.len());
        if use_encoded {
            let encoded = encoded.map_err(|()| UNDECODABLE)?;
            (encoded, mime_of(media_type), renamed(name, media_type))
        } else {
            (bytes.to_vec(), picked.mime_type, name.to_owned())
        }
    };
    if bytes.len() > MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES {
        return Err(TOO_LARGE);
    }
    ImageAttachment::new(mime_type.as_str(), bytes, name).map_err(|_| UNDECODABLE)
}

fn decode_bounded(bytes: &[u8], mime_type: ImageMimeType) -> Result<DynamicImage, &'static str> {
    let mut reader = ImageReader::new(Cursor::new(bytes));
    reader.set_format(match mime_type {
        ImageMimeType::Gif => ImageFormat::Gif,
        ImageMimeType::Jpeg => ImageFormat::Jpeg,
        ImageMimeType::Png => ImageFormat::Png,
        ImageMimeType::Webp => ImageFormat::WebP,
    });
    let mut limits = Limits::default();
    limits.max_alloc = Some(
        MAXIMUM_DECODED_IMAGE_PIXELS
            .saturating_mul(4)
            .saturating_add(16 * 1024 * 1024),
    );
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(|_| UNDECODABLE)?;
    let (width, height) = decoder.dimensions();
    if width == 0
        || height == 0
        || u64::from(width) * u64::from(height) > MAXIMUM_DECODED_IMAGE_PIXELS
    {
        return Err(TOO_MANY_PIXELS);
    }
    DynamicImage::from_decoder(decoder).map_err(|_| UNDECODABLE)
}

fn encode(image: &DynamicImage, media_type: ImageMediaType) -> Result<Vec<u8>, ()> {
    let format = match media_type {
        ImageMediaType::Gif => ImageFormat::Gif,
        ImageMediaType::Jpeg => ImageFormat::Jpeg,
        ImageMediaType::Png => ImageFormat::Png,
        ImageMediaType::Webp => ImageFormat::WebP,
    };
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, format).map_err(|_| ())?;
    Ok(output.into_inner())
}

fn mime_of(media_type: ImageMediaType) -> ImageMimeType {
    ImageMimeType::parse(media_type.as_mime_type())
        .expect("every media type the policy encodes is an accepted image MIME type")
}

/// The display name with the extension of its new encoding.
fn renamed(name: &str, media_type: ImageMediaType) -> String {
    let stem = Path::new(name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "Pasted image".to_owned());
    let extension = match media_type {
        ImageMediaType::Gif => "gif",
        ImageMediaType::Jpeg => "jpg",
        ImageMediaType::Webp => "webp",
        ImageMediaType::Png => "png",
    };
    format!("{stem}.{extension}")
}

/// One whole-pixel dimension of a rescale target, which is finite, positive,
/// and no larger than the long-edge cap.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rescale targets are finite, positive, whole pixels no larger than the long-edge cap"
)]
fn dimension(value: f64) -> u32 {
    value.clamp(1.0, MAXIMUM_IMAGE_LONG_EDGE_PIXELS) as u32
}

#[cfg(test)]
#[path = "attachment_policy/tests.rs"]
mod tests;
