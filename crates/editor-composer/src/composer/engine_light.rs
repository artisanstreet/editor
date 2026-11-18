use aqua::images::{ImageAssets, ImageHandle};
use aqua_ui::{
    element::{Element, Media},
    style::{Length, Style},
};
use artisan_api::EngineId;
use editor_ui::chrome::glide::{CubicBezier, Glide, Transition};
use editor_ui::theme::{ArtisanTheme, ThemeMode};
use std::time::{Duration, Instant};

const LIGHT_WIDTH: f32 = 32.0;
const LIGHT_HEIGHT: f32 = 24.0;
const LIGHT_LIFT: f32 = -2.0;

const TRAVEL: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: CubicBezier::new(0.22, 1.0, 0.36, 1.0),
};

#[derive(Debug)]
struct Lit {
    engine: EngineId,
    glide: Glide,
}

#[derive(Debug, Default)]
pub struct EngineLight {
    lit: Option<Lit>,
}

impl EngineLight {
    pub fn measure(
        &mut self,
        engine: EngineId,
        left: f32,
        width: f32,
        now: Instant,
        reduced_motion: bool,
    ) -> bool {
        let target = light_left(left, width);
        let glide = match self.lit.take() {
            Some(lit) if lit.engine == engine => {
                let unchanged = lit.glide.target().to_bits() == target.to_bits();
                let glide = match unchanged {
                    true => lit.glide,
                    false => Glide::Resting(target),
                };
                self.lit = Some(Lit { engine, glide });
                return !unchanged;
            }
            Some(lit) => lit.glide.retarget(target, now, TRAVEL, reduced_motion),
            None => Glide::Resting(target),
        };
        self.lit = Some(Lit { engine, glide });
        true
    }

    pub fn animating(&self) -> bool {
        self.lit.as_ref().is_some_and(|lit| lit.glide.running())
    }

    pub fn frame(&mut self, now: Instant) -> Option<(f32, bool)> {
        let lit = self.lit.as_mut()?;
        let left = lit.glide.value(now, TRAVEL);
        lit.glide = lit.glide.settle(now);
        Some((left, lit.glide.running()))
    }
}

fn light_left(left: f32, width: f32) -> f32 {
    left + ((width - LIGHT_WIDTH) / 2.0).max(0.0)
}

#[derive(Default)]
pub struct EngineLightImage {
    cached: Option<(ThemeMode, ImageHandle)>,
}
impl EngineLightImage {
    pub(crate) fn element(
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
            .width(LIGHT_WIDTH)
            .height(LIGHT_HEIGHT)
            .style(
                Style::new()
                    .absolute_at(Some(left), Some(LIGHT_LIFT), None, None)
                    .width(Length::Px(LIGHT_WIDTH))
                    .height(Length::Px(LIGHT_HEIGHT)),
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

#[cfg(test)]
#[path = "../../tests/composer/engine_light.rs"]
mod tests;
