//! Every image Forge sees is WebP (`RADICAL_REFORM.md` §2.7): lossy quality 85
//! with sharp YUV, or lossless when that is smaller; the long edge fits the
//! largest engine cap; animated GIF becomes animated WebP. Runs off the UI
//! thread.

use anyhow::{Context as _, Result, anyhow, bail};
use image::{
    AnimationDecoder as _, DynamicImage, GenericImageView as _, ImageDecoder as _, ImageFormat,
    imageops::FilterType,
};

const QUALITY: f32 = 85.0;
/// Engines downscale past this anyway, so extra pixels are never seen.
const LONG_EDGE: u32 = 2576;
pub const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_PIXELS: u64 = 16_000_000;
const MAX_FRAMES: usize = 120;
const MAX_ANIMATION_BYTES: usize = 64 * 1024 * 1024;

/// Converts JPEG, PNG, GIF, BMP or WebP bytes to the WebP Forge stores.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "image::ImageFormat is non-exhaustive; every other format is refused"
)]
pub fn to_webp(bytes: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(
        bytes.len() <= MAX_INPUT_BYTES,
        "Images must be at most 32 MiB"
    );
    let format = image::guess_format(bytes).context("recognise the image format")?;
    match format {
        ImageFormat::Gif => {
            let mut decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
                .context("decode the GIF")?;
            check_dimensions(decoder.dimensions())?;
            decoder.set_limits(decode_limits())?;
            // Decode and resize each frame before reading the next. The encoder
            // borrows these resized buffers, so their aggregate stays bounded.
            let mut frames = Vec::new();
            let mut retained = 0usize;
            for frame in decoder.into_frames() {
                anyhow::ensure!(
                    frames.len() < MAX_FRAMES,
                    "Animations must have at most {MAX_FRAMES} frames"
                );
                let frame = frame.context("decode a GIF frame")?;
                let delay = frame.delay();
                let image = resize(DynamicImage::ImageRgba8(frame.into_buffer())).to_rgba8();
                retained = retained.saturating_add(image.len());
                anyhow::ensure!(
                    retained <= MAX_ANIMATION_BYTES,
                    "Animation pixels exceed 64 MiB"
                );
                frames.push(image::Frame::from_parts(image, 0, 0, delay));
            }
            if frames.len() == 1 {
                return encode(&DynamicImage::ImageRgba8(frames.remove(0).into_buffer()));
            }
            encode_animation(frames)
        }
        ImageFormat::WebP | ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Bmp => {
            let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
            reader.limits(decode_limits());
            let decoder = reader.into_decoder().context("read image dimensions")?;
            check_dimensions(decoder.dimensions())?;
            let image = DynamicImage::from_decoder(decoder).context("decode the image")?;
            if format == ImageFormat::WebP && fits(&image) {
                return Ok(bytes.to_vec());
            }
            encode(&resize(image))
        }
        other => bail!("{other:?} images aren't supported; use PNG, JPEG, GIF, BMP or WebP"),
    }
}

/// The long edge of the blurred stand-in shown while conversion runs.
const PLACEHOLDER_EDGE: u32 = 144;

/// A small BGRA still of the first frame, shown blurred until the WebP is
/// ready. Cheap next to the encode it stands in for.
pub fn placeholder(bytes: &[u8]) -> Option<std::sync::Arc<gpui::RenderImage>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(decode_limits());
    let decoder = reader.into_decoder().ok()?;
    check_dimensions(decoder.dimensions()).ok()?;
    let mut still = DynamicImage::from_decoder(decoder)
        .ok()?
        .thumbnail(PLACEHOLDER_EDGE, PLACEHOLDER_EDGE)
        .into_rgba8();
    for pixel in still.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some(std::sync::Arc::new(gpui::RenderImage::new(vec![
        image::Frame::new(still),
    ])))
}

fn decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(MAX_PIXELS * 4);
    limits
}

fn check_dimensions((width, height): (u32, u32)) -> Result<()> {
    anyhow::ensure!(
        width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "Images must have at most 16 million pixels"
    );
    Ok(())
}

fn fits(image: &DynamicImage) -> bool {
    image.width().max(image.height()) <= LONG_EDGE
}

fn resize(image: DynamicImage) -> DynamicImage {
    if fits(&image) {
        image
    } else {
        image.resize(LONG_EDGE, LONG_EDGE, FilterType::Lanczos3)
    }
}

fn config(lossless: bool) -> Result<webp::WebPConfig> {
    let mut config =
        webp::WebPConfig::new().map_err(|()| anyhow!("initialise the WebP encoder"))?;
    config.quality = QUALITY;
    config.use_sharp_yuv = 1;
    if lossless {
        config.lossless = 1;
        config.quality = 75.0;
    }
    Ok(config)
}

/// Lossy and lossless; whichever is smaller (flat UI shots and icons are
/// sometimes smaller lossless).
fn encode(image: &DynamicImage) -> Result<Vec<u8>> {
    let (width, height) = image.dimensions();
    let pixels;
    let encoder = if image.color().has_alpha() {
        pixels = image.to_rgba8().into_raw();
        webp::Encoder::from_rgba(&pixels, width, height)
    } else {
        pixels = image.to_rgb8().into_raw();
        webp::Encoder::from_rgb(&pixels, width, height)
    };
    let lossy = encoder
        .encode_advanced(&config(false)?)
        .map_err(|error| anyhow!("encode a {width}×{height} WebP: {error:?}"))?;
    let lossless = encoder.encode_advanced(&config(true)?).ok();
    Ok(match lossless {
        Some(lossless) if lossless.len() < lossy.len() => lossless.to_vec(),
        _ => lossy.to_vec(),
    })
}

fn encode_animation(frames: Vec<image::Frame>) -> Result<Vec<u8>> {
    let mut timestamp = 0_i32;
    let frames: Vec<(image::RgbaImage, i32)> = frames
        .into_iter()
        .map(|frame| {
            let at = timestamp;
            let (numerator, denominator) = frame.delay().numer_denom_ms();
            timestamp = timestamp.saturating_add(
                i32::try_from(numerator / denominator.max(1))
                    .unwrap_or(100)
                    .clamp(20, 60_000),
            );
            let image = frame.into_buffer();
            (image, at)
        })
        .collect();
    let (width, height) = frames
        .first()
        .map(|(image, _)| image.dimensions())
        .context("the GIF has no frames")?;
    let config = config(false)?;
    let mut encoder = webp::AnimEncoder::new(width, height, &config);
    encoder.set_loop_count(0);
    for (image, at) in &frames {
        encoder.add_frame(webp::AnimFrame::from_rgba(
            image.as_raw(),
            width,
            height,
            *at,
        ));
    }
    let animation = encoder
        .try_encode()
        .map_err(|error| anyhow!("encode an animated {width}×{height} WebP: {error:?}"))?;
    Ok(animation.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn intake_rejects_oversized_bytes_and_pixels_before_decode() {
        assert!(
            to_webp(&vec![0; MAX_INPUT_BYTES + 1])
                .unwrap_err()
                .to_string()
                .contains("32 MiB")
        );
        assert!(check_dimensions((100_000, 100_000)).is_err());
        assert!(check_dimensions((0, 10)).is_err());
        assert!(check_dimensions((4000, 4000)).is_ok());
    }
}
