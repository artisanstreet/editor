use aqua_media::decode::StillFormat;
use image::{
    AnimationDecoder as _, DynamicImage, GenericImageView as _, ImageDecoder as _, ImageFormat,
    imageops::FilterType,
};

const QUALITY: f32 = 85.0;
const LONG_EDGE: u32 = 2576;
pub(crate) const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_PIXELS: u64 = 16_000_000;
const MAX_FRAMES: usize = 120;
const MAX_ANIMATION_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("Images must be at most 32 MiB")]
    TooLarge { bytes: usize },
    #[error(
        "This image format isn't supported; use PNG, JPEG, WebP, GIF, BMP, TIFF, ICO, AVIF or HEIC"
    )]
    Unsupported,
    #[error("Images must have at most 16 million pixels")]
    TooManyPixels { width: u32, height: u32 },
    #[error("Animations must have at most {MAX_FRAMES} frames")]
    TooManyFrames,
    #[error("Animation pixels exceed 64 MiB")]
    AnimationTooLarge,
    #[error("the GIF has no frames")]
    NoFrames,
    #[error("decode the GIF")]
    Gif(#[source] image::ImageError),
    #[error("limit the GIF decoder")]
    Limits(#[source] image::ImageError),
    #[error("decode a GIF frame")]
    Frame(#[source] image::ImageError),
    #[error("read image dimensions")]
    Dimensions(#[source] image::ImageError),
    #[error("decode the image")]
    Decode(#[source] image::ImageError),
    #[error("decode the image")]
    Media(#[source] aqua_media::decode::MediaError),
    #[error("decode the image: {width}×{height} pixels do not fill an image")]
    Pixels { width: u32, height: u32 },
    #[error("initialise the WebP encoder")]
    Encoder,
    #[error("encode a {width}×{height} WebP: {failure:?}")]
    Still {
        width: u32,
        height: u32,
        failure: webp::WebPEncodingError,
    },
    #[error("encode an animated {width}×{height} WebP: {failure:?}")]
    Animation {
        width: u32,
        height: u32,
        failure: webp::AnimEncodeError,
    },
}

pub fn to_webp(bytes: &[u8]) -> Result<Vec<u8>, EncodeError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(EncodeError::TooLarge { bytes: bytes.len() });
    }
    let format = aqua_media::decode::still::sniff(bytes).ok_or(EncodeError::Unsupported)?;
    match (format, container(format)) {
        (StillFormat::Gif, _) => {
            let mut decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
                .map_err(EncodeError::Gif)?;
            check_dimensions(decoder.dimensions())?;
            decoder
                .set_limits(decode_limits())
                .map_err(EncodeError::Limits)?;
            let mut frames = Vec::new();
            let mut retained = 0usize;
            for frame in decoder.into_frames() {
                if frames.len() >= MAX_FRAMES {
                    return Err(EncodeError::TooManyFrames);
                }
                let frame = frame.map_err(EncodeError::Frame)?;
                let delay = frame.delay();
                let image = resize(DynamicImage::ImageRgba8(frame.into_buffer())).to_rgba8();
                retained = retained.saturating_add(image.len());
                if retained > MAX_ANIMATION_BYTES {
                    return Err(EncodeError::AnimationTooLarge);
                }
                frames.push(image::Frame::from_parts(image, 0, 0, delay));
            }
            if frames.len() == 1 {
                return encode(&DynamicImage::ImageRgba8(frames.remove(0).into_buffer()));
            }
            encode_animation(frames)
        }
        (_, Some(container)) => {
            let mut reader =
                image::ImageReader::with_format(std::io::Cursor::new(bytes), container);
            reader.limits(decode_limits());
            let mut decoder = reader.into_decoder().map_err(EncodeError::Dimensions)?;
            check_dimensions(decoder.dimensions())?;
            let orientation = decoder
                .orientation()
                .unwrap_or(image::metadata::Orientation::NoTransforms);
            let mut image = DynamicImage::from_decoder(decoder).map_err(EncodeError::Decode)?;
            image.apply_orientation(orientation);
            if format == StillFormat::WebP && fits(&image) {
                return Ok(bytes.to_vec());
            }
            encode(&resize(without_opaque_alpha(image)))
        }
        (_, None) => encode(&resize(transcoded(bytes, LONG_EDGE)?)),
    }
}

fn container(format: StillFormat) -> Option<ImageFormat> {
    match format {
        StillFormat::WebP => Some(ImageFormat::WebP),
        StillFormat::Png => Some(ImageFormat::Png),
        StillFormat::Jpeg => Some(ImageFormat::Jpeg),
        StillFormat::Bmp => Some(ImageFormat::Bmp),
        StillFormat::Tiff => Some(ImageFormat::Tiff),
        StillFormat::Ico => Some(ImageFormat::Ico),
        StillFormat::Gif | StillFormat::Avif | StillFormat::Heic => None,
    }
}

fn transcoded(bytes: &[u8], long_edge: u32) -> Result<DynamicImage, EncodeError> {
    check_dimensions(aqua_media::decode::still::dimensions(bytes).map_err(EncodeError::Media)?)?;
    #[expect(
        clippy::cast_precision_loss,
        reason = "edges are far below f32's exact integer range"
    )]
    let detail = aqua_media::decode::Detail::covering(long_edge as f32);
    let still = aqua_media::decode::decode_still(bytes, detail, MAX_PIXELS * 4)
        .map_err(EncodeError::Media)?;
    let (width, height) = (still.image.width(), still.image.height());
    image::RgbaImage::from_raw(width, height, still.image.pixels().to_vec())
        .map(|image| without_opaque_alpha(DynamicImage::ImageRgba8(image)))
        .ok_or(EncodeError::Pixels { width, height })
}

fn without_opaque_alpha(image: DynamicImage) -> DynamicImage {
    let opaque = image
        .as_rgba8()
        .is_some_and(|rgba| rgba.pixels().all(|pixel| pixel.0[3] == u8::MAX));
    if opaque {
        DynamicImage::ImageRgb8(image.into_rgb8())
    } else {
        image
    }
}

pub fn placeholder(bytes: &[u8]) -> Result<image::RgbaImage, EncodeError> {
    let format = aqua_media::decode::still::sniff(bytes).ok_or(EncodeError::Unsupported)?;
    if matches!(format, StillFormat::Avif | StillFormat::Heic) {
        return Ok(transcoded(bytes, 256)?.thumbnail(144, 144).into_rgba8());
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| EncodeError::Decode(image::ImageError::IoError(error)))?;
    reader.limits(decode_limits());
    let decoder = reader.into_decoder().map_err(EncodeError::Decode)?;
    check_dimensions(decoder.dimensions())?;
    Ok(DynamicImage::from_decoder(decoder)
        .map_err(EncodeError::Decode)?
        .thumbnail(144, 144)
        .into_rgba8())
}

fn decode_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(MAX_PIXELS * 4);
    limits
}

fn check_dimensions((width, height): (u32, u32)) -> Result<(), EncodeError> {
    if width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_PIXELS {
        Ok(())
    } else {
        Err(EncodeError::TooManyPixels { width, height })
    }
}

fn fits(image: &DynamicImage) -> bool {
    image.width().max(image.height()) <= LONG_EDGE
}

fn resize(image: DynamicImage) -> DynamicImage {
    if fits(&image) {
        image
    } else {
        image.resize(LONG_EDGE, LONG_EDGE, FilterType::Triangle)
    }
}

fn lossy() -> Result<webp::WebPConfig, EncodeError> {
    let mut config = webp::WebPConfig::new().map_err(|()| EncodeError::Encoder)?;
    config.quality = QUALITY;
    config.method = 2;
    Ok(config)
}

fn lossless() -> Result<webp::WebPConfig, EncodeError> {
    let mut config = webp::WebPConfig::new().map_err(|()| EncodeError::Encoder)?;
    config.lossless = 1;
    config.quality = 0.0;
    config.method = 0;
    Ok(config)
}

fn encode(image: &DynamicImage) -> Result<Vec<u8>, EncodeError> {
    let (width, height) = image.dimensions();
    let pixels;
    let encoder = if image.color().has_alpha() {
        pixels = image.to_rgba8().into_raw();
        webp::Encoder::from_rgba(&pixels, width, height)
    } else {
        pixels = image.to_rgb8().into_raw();
        webp::Encoder::from_rgb(&pixels, width, height)
    };
    let (lossy, lossless) = (lossy()?, lossless()?);
    let (lossy, lossless) = std::thread::scope(|scope| {
        let lossless = scope.spawn(|| encoder.encode_advanced(&lossless).map(|webp| webp.to_vec()));
        let lossy = encoder.encode_advanced(&lossy).map(|webp| webp.to_vec());
        (
            lossy,
            lossless
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
        )
    });
    let lossy = lossy.map_err(|failure| EncodeError::Still {
        width,
        height,
        failure,
    })?;
    Ok(match lossless {
        Ok(lossless) if lossless.len() < lossy.len() => lossless,
        Ok(_) => lossy,
        Err(failure) => {
            tracing::debug!(
                width,
                height,
                ?failure,
                "lossless WebP failed; keeping the lossy encoding"
            );
            lossy
        }
    })
}

fn encode_animation(frames: Vec<image::Frame>) -> Result<Vec<u8>, EncodeError> {
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
        .ok_or(EncodeError::NoFrames)?;
    let config = lossy()?;
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
        .map_err(|failure| EncodeError::Animation {
            width,
            height,
            failure,
        })?;
    Ok(animation.to_vec())
}

#[cfg(test)]
#[path = "../../tests/composer/encode.rs"]
mod tests;
