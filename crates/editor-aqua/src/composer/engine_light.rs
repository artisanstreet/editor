use crate::ui::theme::{ArtisanTheme, ThemeMode};
use aqua::images::{ImageAssets, ImageHandle};
use aqua_ui::{
    element::{Element, Media},
    style::{Length, Style},
};
use artisan_api::EngineId;
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct EngineLight {
    lit: Option<EngineId>,
    left: f32,
    width: f32,
    flight: Option<(f32, f32, Instant)>,
}

impl EngineLight {
    pub fn measure(&mut self, engine: EngineId, left: f32, width: f32, now: Instant) -> bool {
        let moved = self.lit != Some(engine);
        if !moved && self.left == left && self.width == width {
            return false;
        }
        self.flight = if moved && self.lit.is_some() {
            Some((
                light_left(self.left, self.width),
                light_left(left, width),
                now,
            ))
        } else {
            None
        };
        self.lit = Some(engine);
        self.left = left;
        self.width = width;
        true
    }
    pub fn animating(&self) -> bool {
        self.flight.is_some()
    }

    pub fn frame(&mut self, now: Instant) -> Option<(f32, bool)> {
        self.lit?;
        let target = light_left(self.left, self.width);
        let Some((from, to, started)) = self.flight else {
            return Some((target, false));
        };
        let progress = (now.saturating_duration_since(started).as_secs_f32()
            / Duration::from_millis(250).as_secs_f32())
        .clamp(0.0, 1.0);
        if progress >= 1.0 {
            self.flight = None;
            return Some((target, false));
        }
        let eased = crate::ui::motion::smooth_out(progress);
        Some(((to - from).mul_add(eased, from), true))
    }
}

fn light_left(left: f32, width: f32) -> f32 {
    left + ((width - 32.0) / 2.0).max(0.0)
}

#[derive(Default)]
pub struct EngineLightImage {
    cached: Option<(ThemeMode, ImageHandle)>,
}
impl EngineLightImage {
    pub fn element(
        &mut self,
        theme: &ArtisanTheme,
        assets: &ImageAssets,
        left: f32,
    ) -> Result<Element, aqua::images::ImageAssetError> {
        if self
            .cached
            .as_ref()
            .is_none_or(|(mode, _)| *mode != theme.colors.mode)
        {
            let image = bake(theme);
            self.cached = Some((theme.colors.mode, assets.insert(image)?));
        }
        let image = self
            .cached
            .as_ref()
            .expect("light image just loaded")
            .1
            .clone();
        Ok(Media::new(image)
            .width(32.0)
            .height(24.0)
            .style(
                Style::new()
                    .absolute_at(Some(left), Some(-2.0), None, None)
                    .width(Length::Px(32.0))
                    .height(Length::Px(24.0)),
            )
            .build())
    }
}

fn bake(theme: &ArtisanTheme) -> aqua::images::DecodedImage {
    let color = theme.colors.text.to_srgb();
    let (red, green, blue) = (byte(color.r), byte(color.g), byte(color.b));
    let padding = 24;
    let gradient = image::GrayImage::from_fn(128 + 2 * padding, 96 + 2 * padding, |x, y| {
        let inside = x >= padding && x < padding + 128 && y >= padding && y < padding + 96;
        let alpha = if inside {
            interpolate(
                (y - padding) as f32 / 96.0,
                &[(0.0, 0.32), (0.26, 0.10), (0.52, 0.02), (0.74, 0.0)],
            )
        } else {
            0.0
        };
        image::Luma([byte(alpha)])
    });
    let blurred = image::imageops::blur(&gradient, 8.0);
    let buffer = image::ImageBuffer::from_fn(128, 96, |x, y| {
        let nx = (x as f32 + 0.5) / 128.0;
        let ny = (y as f32 + 0.5) / 96.0;
        let distance = ((nx - 0.5) / 0.48).hypot((ny - 0.35) / 0.70);
        let mask = interpolate(
            distance,
            &[(0.0, 1.0), (0.42, 0.5), (0.68, 0.1), (0.88, 0.0)],
        );
        let alpha = f32::from(blurred.get_pixel(x + padding, y + padding)[0]) / 255.0;
        image::Rgba([red, green, blue, byte(alpha * mask)])
    });
    aqua::images::DecodedImage::from_rgba(128, 96, buffer.into_raw())
        .expect("engine light dimensions are valid")
}

fn interpolate(value: f32, stops: &[(f32, f32)]) -> f32 {
    let Some(&(position, alpha)) = stops.first() else {
        return 0.0;
    };
    if value <= position {
        return alpha;
    }
    for pair in stops.windows(2) {
        let [(start, alpha), (end, next)] = pair else {
            continue;
        };
        if value <= *end {
            let span = *end - *start;
            if span <= f32::EPSILON {
                return *next;
            }
            return alpha + (next - alpha) * ((value - start) / span);
        }
    }
    stops.last().map_or(0.0, |(_, alpha)| *alpha)
}
fn byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
