const THRESHOLDS: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

use image::{GrayImage, Luma, RgbaImage};

#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    image: &'a RgbaImage,
}

impl<'a> Source<'a> {
    #[must_use]
    pub const fn new(image: &'a RgbaImage) -> Self {
        Self { image }
    }

    fn width(self) -> u32 {
        self.image.width()
    }

    fn height(self) -> u32 {
        self.image.height()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub pixel_size: f32,
    pub contrast: f32,
    pub brightness: f32,
    pub threshold_bias: f32,
    pub max_resolution: f32,
    pub fade_right: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paint {
    pub rgb: [u8; 3],
    pub opacity: f32,
    pub fade_in_left: f32,
}

const LIT: Luma<u8> = Luma([u8::MAX]);
const DARK: Luma<u8> = Luma([0]);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mask {
    image: GrayImage,
}

impl Mask {
    #[must_use]
    pub fn from_fn(width: u32, height: u32, mut lit: impl FnMut(u32, u32) -> bool) -> Self {
        Self {
            image: GrayImage::from_fn(width, height, |x, y| if lit(x, y) { LIT } else { DARK }),
        }
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        self.image.width()
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.image.height()
    }

    #[must_use]
    pub fn lit(&self, x: u32, y: u32) -> bool {
        self.image.get_pixel(x, y) == &LIT
    }
}

#[must_use]
fn grid(box_width: f32, box_height: f32, params: Params) -> (u32, u32) {
    let resolution = params.max_resolution.max(64.0);
    let sample = params.pixel_size.max(0.5);
    let sampled = (
        (box_width / sample).max(1.0),
        (box_height / sample).max(1.0),
    );
    let reduction = 1.0_f32
        .min(resolution / sampled.0)
        .min(resolution * 1.5 / sampled.1);
    (
        crate::measure::saturate((sampled.0 * reduction).round().max(1.0)),
        crate::measure::saturate((sampled.1 * reduction).round().max(1.0)),
    )
}

#[must_use]
pub fn dither(source: Source<'_>, box_width: f32, box_height: f32, params: Params) -> Mask {
    let (width, height) = grid(box_width, box_height, params);
    let scale = (crate::measure::exact(width) / crate::measure::exact(source.width()))
        .min(crate::measure::exact(height) / crate::measure::exact(source.height()));
    let drawn = (
        crate::measure::exact(source.width()) * scale,
        crate::measure::exact(source.height()) * scale,
    );
    let top = crate::measure::exact(height) - drawn.1;
    let fade_right = params.fade_right.clamp(0.0, 1.0);
    Mask::from_fn(width, height, |x, y| {
        let Some([r, g, b, a]) = area(
            source,
            crate::measure::exact(x) / scale,
            (crate::measure::exact(y) - top) / scale,
            1.0 / scale,
        ) else {
            return false;
        };
        let luminance = b.mul_add(0.0722, g.mul_add(0.7152, r * 0.2126)) / 255.0;
        let threshold = (f32::from(THRESHOLDS[((y % 4) * 4 + x % 4) as usize]) + 0.5) / 16.0;
        let fade = if fade_right > 0.0 {
            1.0 - ((crate::measure::exact(x) / crate::measure::exact(width) - (1.0 - fade_right))
                / fade_right)
                .max(0.0)
        } else {
            1.0
        };
        let adjusted = (luminance * params.brightness - 0.5)
            .mul_add(params.contrast, 0.5)
            .clamp(0.0, 1.0)
            * fade;
        adjusted > threshold + params.threshold_bias && a > 0.0
    })
}

#[must_use]
pub fn paint(mask: &Mask, paint: Paint) -> RgbaImage {
    let width = crate::measure::exact(mask.width());
    RgbaImage::from_fn(mask.width(), mask.height(), |x, y| {
        let reveal = if paint.fade_in_left > 0.0 {
            ((crate::measure::exact(x) + 0.5) / width / paint.fade_in_left).min(1.0)
        } else {
            1.0
        };
        let alpha = if mask.lit(x, y) {
            crate::measure::channel((255.0 * paint.opacity * reveal).round())
        } else {
            0
        };
        image::Rgba([paint.rgb[0], paint.rgb[1], paint.rgb[2], alpha])
    })
}

fn area(source: Source<'_>, left: f32, top: f32, span: f32) -> Option<[f32; 4]> {
    let right = left + span;
    let bottom = top + span;
    if right <= 0.0
        || bottom <= 0.0
        || left >= crate::measure::exact(source.width())
        || top >= crate::measure::exact(source.height())
    {
        return None;
    }
    let columns = crate::measure::saturate(left.max(0.0).floor())
        ..crate::measure::saturate(right.min(crate::measure::exact(source.width())).ceil());
    let rows = crate::measure::saturate(top.max(0.0).floor())
        ..crate::measure::saturate(bottom.min(crate::measure::exact(source.height())).ceil());
    let mut total = [0.0_f32; 4];
    let mut weight = 0.0_f32;
    for y in rows {
        let cover_y = (bottom.min(crate::measure::exact(y) + 1.0)
            - top.max(crate::measure::exact(y)))
        .max(0.0);
        for x in columns.clone() {
            let cover = cover_y
                * (right.min(crate::measure::exact(x) + 1.0) - left.max(crate::measure::exact(x)))
                    .max(0.0);
            let pixel = source.image.get_pixel(x, y);
            for (sum, channel) in total.iter_mut().zip(pixel.0) {
                *sum = f32::from(channel).mul_add(cover, *sum);
            }
            weight += cover;
        }
    }
    (weight > 0.0).then(|| total.map(|sum| sum / weight))
}
