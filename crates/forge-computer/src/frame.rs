use base64::Engine as _;
use std::num::NonZeroU32;

const MAX_EDGE: f64 = 1568.0;
const MAX_PIXELS: f64 = 1_150_000.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    width: NonZeroU32,
    height: NonZeroU32,
    pixels: Box<[[u8; 3]]>,
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("a frame has no pixels at {width}x{height}")]
    Empty { width: u32, height: u32 },
    #[error("a {width}x{height} frame needs {expected} pixels, not {actual}")]
    Size {
        width: u32,
        height: u32,
        expected: u64,
        actual: usize,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("write the PNG header of a {width}x{height} frame")]
    Header {
        width: u32,
        height: u32,
        #[source]
        source: png::EncodingError,
    },
    #[error("write the PNG pixels of a {width}x{height} frame")]
    Pixels {
        width: u32,
        height: u32,
        #[source]
        source: png::EncodingError,
    },
}

impl Frame {
    pub fn new(width: u32, height: u32, pixels: Vec<[u8; 3]>) -> Result<Self, FrameError> {
        let (Some(nonzero_width), Some(nonzero_height)) =
            (NonZeroU32::new(width), NonZeroU32::new(height))
        else {
            return Err(FrameError::Empty { width, height });
        };
        let expected = u64::from(width) * u64::from(height);
        if u64::try_from(pixels.len()) != Ok(expected) {
            return Err(FrameError::Size {
                width,
                height,
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            width: nonzero_width,
            height: nonzero_height,
            pixels: pixels.into_boxed_slice(),
        })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width.get()
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height.get()
    }

    #[must_use]
    pub fn pixels(&self) -> &[[u8; 3]] {
        &self.pixels
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let at = u64::from(y) * u64::from(self.width()) + u64::from(x);
        let at = usize::try_from(at).expect("invariant: a frame's pixel index fits its storage");
        self.pixels[at]
    }

    #[must_use]
    pub fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> Option<Self> {
        let x = x.min(self.width());
        let y = y.min(self.height());
        let width = NonZeroU32::new(width.min(self.width() - x))?;
        let height = NonZeroU32::new(height.min(self.height() - y))?;
        let pixels = (y..y + height.get())
            .flat_map(|row| (x..x + width.get()).map(move |column| (column, row)))
            .map(|(column, row)| self.pixel(column, row))
            .collect();
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    #[must_use]
    pub fn shrink(&self, width: NonZeroU32, height: NonZeroU32) -> Self {
        let width = width.min(self.width);
        let height = height.min(self.height);
        if width == self.width && height == self.height {
            return self.clone();
        }
        let (sw, sh) = (u64::from(self.width()), u64::from(self.height()));
        let (dw, dh) = (u64::from(width.get()), u64::from(height.get()));
        let mut pixels = Vec::new();
        for oy in 0..dh {
            let y0 = oy * sh / dh;
            let y1 = ((oy + 1) * sh).div_ceil(dh).max(y0 + 1);
            for ox in 0..dw {
                let x0 = ox * sw / dw;
                let x1 = ((ox + 1) * sw).div_ceil(dw).max(x0 + 1);
                let mut sum = [0_u64; 3];
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let pixel = self.pixel(
                            u32::try_from(sx).expect("invariant: a source column fits u32"),
                            u32::try_from(sy).expect("invariant: a source row fits u32"),
                        );
                        for (total, channel) in sum.iter_mut().zip(pixel) {
                            *total += u64::from(channel);
                        }
                    }
                }
                let count = (y1 - y0) * (x1 - x0);
                pixels.push(sum.map(|channel| {
                    u8::try_from(channel / count).expect("invariant: an average channel fits u8")
                }));
            }
        }
        Self {
            width,
            height,
            pixels: pixels.into_boxed_slice(),
        }
    }

    pub fn png_base64(&self) -> Result<String, EncodeError> {
        let (width, height) = (self.width(), self.height());
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder
                .write_header()
                .map_err(|source| EncodeError::Header {
                    width,
                    height,
                    source,
                })?;
            writer
                .write_image_data(self.pixels.as_flattened())
                .map_err(|source| EncodeError::Pixels {
                    width,
                    height,
                    source,
                })?;
        }
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }
}

#[must_use]
pub fn scale_for(width: u32, height: u32) -> f64 {
    let (width, height) = (f64::from(width.max(1)), f64::from(height.max(1)));
    (MAX_EDGE / width.max(height))
        .min((MAX_PIXELS / (width * height)).sqrt())
        .min(1.0)
}

#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a scaled length is a rounded nonnegative pixel count; `as` saturates"
)]
pub fn scaled(length: u32, scale: f64) -> NonZeroU32 {
    NonZeroU32::new((f64::from(length) * scale).round() as u32).unwrap_or(NonZeroU32::MIN)
}
