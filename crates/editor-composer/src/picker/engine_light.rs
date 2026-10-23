//! The light under the active engine tab.

use super::*;

const ENGINE_LIGHT_WIDTH_PX: f32 = 32.0;
const ENGINE_LIGHT_HEIGHT_PX: f32 = 24.0;
const ENGINE_LIGHT_RASTER_SCALE: u32 = 4;
const ENGINE_LIGHT_RASTER_WIDTH: u32 = 128;
const ENGINE_LIGHT_RASTER_HEIGHT: u32 = 96;
/// The light's slide between tabs.
const ENGINE_LIGHT_FLIGHT: Duration = Duration::from_millis(250);

/// The engine light's slide from one tab to another.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LightFlight {
    from_left: f32,
    to_left: f32,
    generation: u64,
}

/// The light under the active engine tab.
#[derive(Debug, Default)]
pub(super) struct EngineLight {
    pub(super) lit: Option<EngineId>,
    left: f32,
    width: f32,
    flight: Option<LightFlight>,
    generation: u64,
}

impl EngineLight {
    /// Records the active tab's place in its strip: a new tab slides the
    /// light from the old one; a resize moves it at once.
    #[expect(
        clippy::float_cmp,
        reason = "the light re-measures only when the measured place changes exactly"
    )]
    pub(super) fn measure(&mut self, engine: EngineId, left: f32, width: f32) -> bool {
        let moved = self.lit != Some(engine);
        if !moved && self.left == left && self.width == width {
            return false;
        }
        self.flight = if moved && self.lit.is_some() {
            self.generation = self.generation.wrapping_add(1);
            Some(LightFlight {
                from_left: light_left(self.left, self.width),
                to_left: light_left(left, width),
                generation: self.generation,
            })
        } else {
            None
        };
        self.lit = Some(engine);
        self.left = left;
        self.width = width;
        true
    }
}

/// The light's left edge, centred under a tab.
fn light_left(tab_left: f32, tab_width: f32) -> f32 {
    tab_left + ((tab_width - ENGINE_LIGHT_WIDTH_PX) / 2.0).max(0.0)
}

/// The light under the active engine tab (`render_engine_light`): a soft
/// glow baked into a cached raster, sliding from the last tab to this one.
pub(super) fn engine_light(theme: &ArtisanTheme, light: &EngineLight) -> AnyElement {
    let target = light_left(light.left, light.width);
    let image = img(ImageSource::Render(engine_light_image(theme)))
        .absolute()
        .top(px(-2.0))
        .left(px(target))
        .w(px(ENGINE_LIGHT_WIDTH_PX))
        .h(px(ENGINE_LIGHT_HEIGHT_PX));
    let Some(flight) = light.flight else {
        return image.into_any_element();
    };
    let LightFlight {
        from_left,
        to_left,
        generation,
    } = flight;
    image
        .left(px(from_left))
        .with_animation(
            ElementId::Name(format!("composer-engine-light-{generation}").into()),
            Animation::new(ENGINE_LIGHT_FLIGHT).with_easing(smooth_out),
            move |image, progress| {
                image.left(px((to_left - from_left).mul_add(progress, from_left)))
            },
        )
        .into_any_element()
}

thread_local! {
    /// The baked light, per theme mode: its colour is the mode's text.
    static ENGINE_LIGHT_IMAGE: RefCell<Option<(ThemeMode, Arc<RenderImage>)>> = const { RefCell::new(None) };
}

fn engine_light_image(theme: &ArtisanTheme) -> Arc<RenderImage> {
    ENGINE_LIGHT_IMAGE.with(|image| {
        let mut image = image.borrow_mut();
        let mode = theme.colors.mode;
        if let Some((baked, image)) = image.as_ref()
            && *baked == mode
        {
            return std::sync::Arc::clone(image);
        }
        let baked = Arc::new(RenderImage::new(vec![engine_light_frame(theme)]));
        *image = Some((mode, std::sync::Arc::clone(&baked)));
        baked
    })
}

/// The light's raster: the text colour under a vertical alpha
/// profile, blurred 2 px, then masked by a soft ellipse.
#[expect(
    clippy::cast_precision_loss,
    reason = "raster coordinates are a few hundred pixels, exact in f32"
)]
fn engine_light_frame(theme: &ArtisanTheme) -> image::Frame {
    let foreground = theme.colors.text.to_srgb();
    let red = color_channel_to_byte(foreground.r);
    let green = color_channel_to_byte(foreground.g);
    let blue = color_channel_to_byte(foreground.b);
    // The blur runs before the mask: bake it into the cached alpha, with
    // transparent padding for the filter's edge samples.
    let padding = 6 * ENGINE_LIGHT_RASTER_SCALE;
    let gradient = image::GrayImage::from_fn(
        ENGINE_LIGHT_RASTER_WIDTH + 2 * padding,
        ENGINE_LIGHT_RASTER_HEIGHT + 2 * padding,
        |x, y| {
            let inside = x >= padding
                && x < padding + ENGINE_LIGHT_RASTER_WIDTH
                && y >= padding
                && y < padding + ENGINE_LIGHT_RASTER_HEIGHT;
            let alpha = if inside {
                interpolate_profile(
                    (y - padding) as f32 / ENGINE_LIGHT_RASTER_HEIGHT as f32,
                    &[(0.0, 0.32), (0.26, 0.10), (0.52, 0.02), (0.74, 0.0)],
                )
            } else {
                0.0
            };
            image::Luma([color_channel_to_byte(alpha)])
        },
    );
    let blurred = image::imageops::blur(&gradient, 2.0 * ENGINE_LIGHT_RASTER_SCALE as f32);
    let buffer = image::ImageBuffer::from_fn(
        ENGINE_LIGHT_RASTER_WIDTH,
        ENGINE_LIGHT_RASTER_HEIGHT,
        |x, y| {
            let nx = (x as f32 + 0.5) / ENGINE_LIGHT_RASTER_WIDTH as f32;
            let ny = (y as f32 + 0.5) / ENGINE_LIGHT_RASTER_HEIGHT as f32;
            let distance = ((nx - 0.5) / 0.48).hypot((ny - 0.35) / 0.70);
            let mask = interpolate_profile(
                distance,
                &[(0.0, 1.0), (0.42, 0.5), (0.68, 0.1), (0.88, 0.0)],
            );
            let alpha = f32::from(blurred.get_pixel(x + padding, y + padding)[0]) / 255.0;
            // RenderImage takes BGRA.
            image::Rgba([blue, green, red, color_channel_to_byte(alpha * mask)])
        },
    );
    image::Frame::new(buffer)
}

fn interpolate_profile(value: f32, stops: &[(f32, f32)]) -> f32 {
    let Some(&(first_position, first_alpha)) = stops.first() else {
        return 0.0;
    };
    if value <= first_position {
        return first_alpha;
    }
    for pair in stops.windows(2) {
        let [(start_position, start_alpha), (end_position, end_alpha)] = pair else {
            continue;
        };
        if value <= *end_position {
            let span = *end_position - *start_position;
            if span <= f32::EPSILON {
                return *end_alpha;
            }
            let progress = (value - *start_position) / span;
            return start_alpha + ((*end_alpha - *start_alpha) * progress);
        }
    }
    stops.last().map_or(0.0, |(_, alpha)| *alpha)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255 before the cast"
)]
fn color_channel_to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
