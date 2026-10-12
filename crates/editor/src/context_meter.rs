//! The titlebar's context-window ring and the card it opens on hover.
//! Ported from the previous Editor's `native_context_usage.rs`: the ring is
//! drawn as a track and an arc on one circle, blue while the window is
//! roomy, then warning and danger as it nears compaction.

use editor_ui::hover_card::{glass_hover_card, hover_card_text, hover_card_title};
use editor_ui::progress::{ProgressFraction, ProgressStyle, progress};
use editor_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use gpui::prelude::{InteractiveElement as _, ParentElement as _, Styled as _};
use gpui::{Bounds, Div, Hsla, Path, PathBuilder, Pixels, Stateful, canvas, div, point, px};

pub const CONTEXT_METER_SELECTOR: &str = "artisan-native-context-meter";
const RING_EDGE_PX: f32 = 16.0;
const RING_STROKE_PX: f32 = 2.5;
const RING_RADIUS_PX: f32 = 5.75;
const RING_START_DEGREES: f32 = -90.0;
const FULL_RING_EPSILON_DEGREES: f32 = 0.01;
/// Where the window stops being unremarkable.
const WARN_FROM: f64 = 50.0;
/// Where the ramp reddens.
const DANGER_FROM: f64 = 80.0;

/// One run's context reading.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextReading {
    pub model_name: String,
    pub context_tokens: u64,
    pub window_tokens: u64,
}

impl ContextReading {
    /// `None` without a window to measure against.
    #[must_use]
    pub fn new(model_name: String, context_tokens: u64, window_tokens: u64) -> Option<Self> {
        (window_tokens > 0).then_some(Self {
            model_name,
            context_tokens,
            window_tokens,
        })
    }

    /// How full the window is, `0..=100`.
    #[must_use]
    #[expect(clippy::cast_precision_loss, reason = "token counts far below 2^53")]
    pub fn percent(&self) -> f64 {
        (self.context_tokens as f64 / self.window_tokens as f64 * 100.0).clamp(0.0, 100.0)
    }

    fn color(&self, theme: &ArtisanTheme) -> Hsla {
        let percent = self.percent();
        if percent > DANGER_FROM {
            theme.colors.banner_error.to_paint()
        } else if percent > WARN_FROM {
            theme.colors.banner_warning.to_paint()
        } else {
            theme.colors.banner_info.to_paint()
        }
    }

    fn percent_sentence(&self) -> String {
        format!(
            "The context window for {} is {}% full.",
            self.model_name,
            self.percent().round()
        )
    }

    fn capacity_sentence(&self) -> String {
        format!(
            "{} has a context window of {} tokens.",
            self.model_name,
            compact_tokens(self.window_tokens)
        )
    }
}

/// `950`, `200K`, `1M`, as `Intl` compact notation writes them.
#[must_use]
pub fn compact_tokens(count: u64) -> String {
    #[expect(clippy::cast_precision_loss, reason = "display rounding")]
    let value = count as f64;
    let (scaled, suffix) = if value >= 999_500.0 {
        (value / 1e6, "M")
    } else if value >= 999.5 {
        (value / 1e3, "K")
    } else {
        (value, "")
    };
    let text = if scaled < 10.0 && !suffix.is_empty() {
        format!("{scaled:.1}")
    } else {
        format!("{scaled:.0}")
    };
    let text = text.strip_suffix(".0").unwrap_or(&text).to_owned();
    format!("{text}{suffix}")
}

/// The ring gauge, the hover trigger of [`context_card`].
#[must_use]
pub fn context_ring(reading: &ContextReading, theme: &ArtisanTheme) -> Stateful<Div> {
    let ring_color = reading.color(theme);
    let track_color = theme.colors.foreground.with_alpha(0.16).to_paint();
    #[expect(clippy::cast_possible_truncation, reason = "a clamped 0..=1 share")]
    let share = (reading.percent() / 100.0) as f32;
    // Track and arc share one centre, radius, and stroke, so the arc lies
    // exactly on a true circle.
    let arc = canvas(
        move |bounds, _, _| (ring_path(bounds, 1.0), ring_path(bounds, share)),
        move |_, (track, arc): (Option<Path<Pixels>>, Option<Path<Pixels>>), window, _| {
            if let Some(track) = track {
                window.paint_path(track, track_color);
            }
            if let Some(arc) = arc {
                window.paint_path(arc, ring_color);
            }
        },
    )
    .absolute()
    .inset_0();
    div()
        .id(CONTEXT_METER_SELECTOR)
        .debug_selector(|| CONTEXT_METER_SELECTOR.to_owned())
        .size(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(RadiusTokens::value(RadiusStep::Sm))
        .child(
            div()
                .relative()
                .size(px(RING_EDGE_PX))
                .flex()
                .items_center()
                .justify_center()
                .child(arc),
        )
}

/// The card shown while the ring is hovered: the model, how full its window
/// is and how large, then a progress bar.
#[must_use]
pub fn context_card(reading: &ContextReading, theme: &ArtisanTheme) -> Div {
    let model = reading.model_name.as_str();
    let sentence = |text: String, size: Option<String>| {
        let model_range = text
            .find(model)
            .filter(|_| !model.is_empty())
            .map(|start| start..start + model.len());
        let percent_range = percent_token(&text);
        let after_model = model_range.as_ref().map_or(0, |range| range.end);
        let size_range = size.filter(|size| !size.is_empty()).and_then(|size| {
            text[after_model..]
                .find(&size)
                .map(|start| after_model + start..after_model + start + size.len())
        });
        let facts: Vec<_> = model_range
            .into_iter()
            .chain(percent_range)
            .chain(size_range)
            .collect();
        hover_card_text(theme, text, &facts)
    };
    #[expect(clippy::cast_possible_truncation, reason = "a clamped 0..=1 share")]
    let fill = ProgressFraction::new((reading.percent() / 100.0) as f32);
    let content = div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .p(px(16.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(hover_card_title(theme, "Context Window"))
                .child(sentence(reading.percent_sentence(), None))
                .child(sentence(
                    reading.capacity_sentence(),
                    Some(compact_tokens(reading.window_tokens)),
                )),
        )
        .child(progress(ProgressStyle::resolve(*theme), fill));
    glass_hover_card(theme, content)
        .debug_selector(|| format!("{CONTEXT_METER_SELECTOR}-card"))
        .w(px(280.0))
}

/// The byte range of the first `N%` token in `text`, if any.
fn percent_token(text: &str) -> Option<std::ops::Range<usize>> {
    let end = text.find('%')? + 1;
    let start = text[..end - 1]
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map_or(0, |index| index + 1);
    (start < end - 1).then_some(start..end)
}

fn ring_path(bounds: Bounds<Pixels>, share: f32) -> Option<Path<Pixels>> {
    if share <= 0.0 {
        return None;
    }
    let center = point(
        bounds.origin.x + bounds.size.width / 2.0,
        bounds.origin.y + bounds.size.height / 2.0,
    );
    let end = RING_START_DEGREES + (share * 360.0).min(360.0 - FULL_RING_EPSILON_DEGREES);
    let mut builder = PathBuilder::stroke(px(RING_STROKE_PX));
    builder.move_to(on_circle(center, RING_START_DEGREES));
    builder.arc_to(
        point(px(RING_RADIUS_PX), px(RING_RADIUS_PX)),
        px(0.0),
        share >= 0.5,
        true,
        on_circle(center, end),
    );
    builder.build().ok()
}

fn on_circle(center: gpui::Point<Pixels>, degrees: f32) -> gpui::Point<Pixels> {
    let radians = degrees.to_radians();
    point(
        center.x + px(RING_RADIUS_PX * radians.cos()),
        center.y + px(RING_RADIUS_PX * radians.sin()),
    )
}

#[cfg(test)]
mod tests {
    use super::{ContextReading, compact_tokens};

    #[test]
    fn the_card_names_the_model_how_full_and_how_large() {
        let reading = ContextReading::new("Opus 5.5".into(), 90_000, 200_000).unwrap();
        assert_eq!(
            reading.percent_sentence(),
            "The context window for Opus 5.5 is 45% full."
        );
        assert_eq!(
            reading.capacity_sentence(),
            "Opus 5.5 has a context window of 200K tokens."
        );
        assert_eq!(compact_tokens(1_000_000), "1M");
        assert_eq!(compact_tokens(400_000), "400K");
        assert!(ContextReading::new("x".into(), 1, 0).is_none());
    }
}
