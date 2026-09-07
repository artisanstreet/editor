//! Pure image intake policy used before an image is sent to an engine.
//!
//! The policy deliberately does not encode, decode, or inspect image bytes.
//! It only describes supported media types and computes a safe rescale target
//! for already-known dimensions.

/// The largest permitted image long edge in pixels.
pub const MAXIMUM_IMAGE_LONG_EDGE_PIXELS: f64 = 2576.0;

/// Media types accepted by the frontend image intake path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageMediaType {
    Gif,
    Jpeg,
    Png,
    Webp,
}

impl ImageMediaType {
    /// Returns the MIME type represented by this media type.
    #[must_use]
    pub const fn as_mime_type(self) -> &'static str {
        match self {
            Self::Gif => "image/gif",
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }
}

/// Formats ordered from worst to best compression, matching the source
/// policy's encoding ladder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageCompressionFormat {
    Png,
    Jpeg,
    Webp,
    Avif,
}

impl ImageCompressionFormat {
    /// Returns the MIME type represented by this compression format.
    #[must_use]
    pub const fn as_mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
        }
    }

    /// Converts a compression format to an intake media type when the format
    /// is one of the media types accepted by the frontend protocol.
    #[must_use]
    pub const fn as_image_media_type(self) -> Option<ImageMediaType> {
        match self {
            Self::Png => Some(ImageMediaType::Png),
            Self::Jpeg => Some(ImageMediaType::Jpeg),
            Self::Webp => Some(ImageMediaType::Webp),
            Self::Avif => None,
        }
    }
}

/// The image encoding ladder, ordered from worst to best compression.
pub const IMAGE_COMPRESSION_LADDER: [ImageCompressionFormat; 4] = [
    ImageCompressionFormat::Png,
    ImageCompressionFormat::Jpeg,
    ImageCompressionFormat::Webp,
    ImageCompressionFormat::Avif,
];

const CLAUDE_IMAGE_FORMATS: [ImageMediaType; 4] = [
    ImageMediaType::Gif,
    ImageMediaType::Jpeg,
    ImageMediaType::Png,
    ImageMediaType::Webp,
];

const CODEX_IMAGE_FORMATS: [ImageMediaType; 4] = [
    ImageMediaType::Gif,
    ImageMediaType::Jpeg,
    ImageMediaType::Png,
    ImageMediaType::Webp,
];

const UNIVERSAL_IMAGE_FORMATS: [ImageMediaType; 1] = [ImageMediaType::Png];

/// Chooses the best format in [`IMAGE_COMPRESSION_LADDER`] accepted by an
/// engine. Unknown or absent engines use PNG as the universal fallback.
#[must_use]
pub fn best_image_format(engine_id: Option<&str>) -> ImageMediaType {
    let accepted = match engine_id {
        Some("claude") => &CLAUDE_IMAGE_FORMATS[..],
        Some("codex") => &CODEX_IMAGE_FORMATS[..],
        _ => &UNIVERSAL_IMAGE_FORMATS[..],
    };

    for format in IMAGE_COMPRESSION_LADDER.iter().rev() {
        if let Some(media_type) = format.as_image_media_type()
            && accepted.contains(&media_type)
        {
            return media_type;
        }
    }

    ImageMediaType::Png
}

/// Width and height of an image in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageDimensions {
    pub height: f64,
    pub width: f64,
}

impl ImageDimensions {
    /// Returns whether both dimensions are finite and strictly positive.
    #[must_use]
    pub fn is_valid(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

/// Computes a rescale target using [`MAXIMUM_IMAGE_LONG_EDGE_PIXELS`].
///
/// Rust has no default function arguments, so callers that need a custom cap
/// should use [`image_rescale_target_with_long_edge`]. Invalid dimensions and
/// invalid caps return no target. Valid positive dimensions are scaled without
/// enlargement, and positive results use JavaScript-compatible rounding with a
/// one-pixel minimum.
#[must_use]
pub fn image_rescale_target(source: ImageDimensions) -> Option<ImageDimensions> {
    image_rescale_target_with_long_edge(source, MAXIMUM_IMAGE_LONG_EDGE_PIXELS)
}

/// Computes a rescale target using a caller-supplied long-edge cap.
#[must_use]
pub fn image_rescale_target_with_long_edge(
    source: ImageDimensions,
    long_edge: f64,
) -> Option<ImageDimensions> {
    if !source.is_valid() || !long_edge.is_finite() || long_edge <= 0.0 {
        return None;
    }

    let longest = source.width.max(source.height);
    if !longest.is_finite() || longest <= long_edge {
        return None;
    }

    let scale = long_edge / longest;
    Some(ImageDimensions {
        height: round_scaled_dimension(source.height * scale),
        width: round_scaled_dimension(source.width * scale),
    })
}

fn round_scaled_dimension(value: f64) -> f64 {
    value.round().max(1.0)
}
