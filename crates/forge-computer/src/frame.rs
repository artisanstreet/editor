//! Screen images: what a capture returns, the size agents see it at, and
//! the PNG they get.

use anyhow::{Context, Result};
use base64::Engine as _;

/// The longest edge and the most pixels an image keeps before models
/// shrink it themselves; past these, the agent's coordinates would no
/// longer match the image it saw.
const MAX_EDGE: f64 = 1568.0;
const MAX_PIXELS: f64 = 1_150_000.0;

/// An RGB image, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Three bytes per pixel.
    pub rgb: Vec<u8>,
}

impl Frame {
    /// The part of the frame at (`x`, `y`), `width` by `height`, clamped
    /// to the frame.
    #[must_use]
    pub fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> Frame {
        let x = x.min(self.width);
        let y = y.min(self.height);
        let width = width.min(self.width - x);
        let height = height.min(self.height - y);
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for row in y..y + height {
            let start = ((row * self.width + x) * 3) as usize;
            rgb.extend_from_slice(&self.rgb[start..start + (width * 3) as usize]);
        }
        Frame { width, height, rgb }
    }

    /// The frame at `width` by `height`, each pixel the average of the
    /// pixels it covers. Only shrinks.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // Image allocation already bounds these indexes to usize.
    pub fn shrink(&self, width: u32, height: u32) -> Frame {
        let width = width.min(self.width);
        let height = height.min(self.height);
        if width >= self.width && height >= self.height {
            return self.clone();
        }
        let (sw, sh) = (u64::from(self.width), u64::from(self.height));
        let (dw, dh) = (u64::from(width.max(1)), u64::from(height.max(1)));
        let mut rgb = Vec::with_capacity((dw * dh * 3) as usize);
        for oy in 0..dh {
            let y0 = oy * sh / dh;
            let y1 = ((oy + 1) * sh).div_ceil(dh).max(y0 + 1);
            for ox in 0..dw {
                let x0 = ox * sw / dw;
                let x1 = ((ox + 1) * sw).div_ceil(dw).max(x0 + 1);
                let mut sum = [0_u64; 3];
                for sy in y0..y1 {
                    let row = (sy * sw) as usize;
                    for sx in x0..x1 {
                        let at = (row + sx as usize) * 3;
                        sum[0] += u64::from(self.rgb[at]);
                        sum[1] += u64::from(self.rgb[at + 1]);
                        sum[2] += u64::from(self.rgb[at + 2]);
                    }
                }
                let count = (y1 - y0) * (x1 - x0);
                #[allow(clippy::cast_possible_truncation)]
                rgb.extend(sum.map(|channel| (channel / count) as u8));
            }
        }
        Frame {
            width: u32::try_from(dw).unwrap_or(u32::MAX),
            height: u32::try_from(dh).unwrap_or(u32::MAX),
            rgb,
        }
    }

    /// The frame as a base64 PNG.
    pub fn png_base64(&self) -> Result<String> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, self.width, self.height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().context("write the PNG header")?;
            writer
                .write_image_data(&self.rgb)
                .context("write the PNG pixels")?;
        }
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }
}

/// How much a `width` by `height` image shrinks to stay within what
/// models take as is: 1.0 when it already fits.
#[must_use]
pub fn scale_for(width: u32, height: u32) -> f64 {
    let (width, height) = (f64::from(width.max(1)), f64::from(height.max(1)));
    (MAX_EDGE / width.max(height))
        .min((MAX_PIXELS / (width * height)).sqrt())
        .min(1.0)
}

/// `length` pixels at `scale`, at least one.
#[must_use]
pub fn scaled(length: u32, scale: f64) -> u32 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let scaled = (f64::from(length) * scale).round() as u32;
    scaled.max(1)
}
