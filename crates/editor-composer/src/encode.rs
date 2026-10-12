//! Every image Forge sees is WebP (`RADICAL_REFORM.md` §2.7): lossy quality 85
//! with sharp YUV, or lossless when that is smaller; the long edge fits the
//! largest engine cap; animated GIF becomes animated WebP. Runs off the UI
//! thread.

use anyhow::{Context as _, Result, anyhow, bail};
use image::{
    AnimationDecoder as _, DynamicImage, GenericImageView as _, ImageFormat, imageops::FilterType,
};

const QUALITY: f32 = 85.0;
/// Engines downscale past this anyway, so extra pixels are never seen.
const LONG_EDGE: u32 = 2576;

/// Converts JPEG, PNG, GIF, BMP or WebP bytes to the WebP Forge stores.
pub fn to_webp(bytes: &[u8]) -> Result<Vec<u8>> {
    let format = image::guess_format(bytes).context("recognise the image format")?;
    match format {
        ImageFormat::WebP => {
            let image =
                image::load_from_memory_with_format(bytes, format).context("decode the WebP")?;
            if fits(&image) {
                // Re-encoding only adds loss.
                return Ok(bytes.to_vec());
            }
            encode(&resize(image))
        }
        ImageFormat::Gif => {
            let frames = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
                .context("decode the GIF")?
                .into_frames()
                .collect_frames()
                .context("decode the GIF frames")?;
            if frames.len() > 1 {
                return encode_animation(frames);
            }
            let image =
                image::load_from_memory_with_format(bytes, format).context("decode the GIF")?;
            encode(&resize(image))
        }
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Bmp => {
            let image = image::load_from_memory_with_format(bytes, format)
                .with_context(|| format!("decode the {format:?}"))?;
            encode(&resize(image))
        }
        other => bail!("{other:?} images aren't supported; use PNG, JPEG, GIF, BMP or WebP"),
    }
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
            timestamp += i32::try_from(numerator / denominator.max(1))
                .unwrap_or(100)
                .max(20);
            let image = match resize(DynamicImage::ImageRgba8(frame.into_buffer())) {
                DynamicImage::ImageRgba8(buffer) => buffer,
                other => other.to_rgba8(),
            };
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
