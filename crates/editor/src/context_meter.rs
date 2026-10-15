//! The titlebar context ring changes tone as usage approaches compaction.
//! Its tooltip shows the measured token count and model capacity.

use editor_ui::hover_card::{glass_hover_card, hover_card_text, hover_card_title};
use editor_ui::motion::MotionDuration;
use editor_ui::picker_motion::{MenuEntrance, MenuMotion, animate_menu};
use editor_ui::progress::Progress;
use editor_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use gpui::prelude::{InteractiveElement as _, ParentElement as _, Styled as _};
use gpui::{
    App, Bounds, Div, Hsla, IntoElement, Path, PathBuilder, Pixels, RenderOnce, Stateful,
    StatefulInteractiveElement as _, Window, canvas, div, point, px,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

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

/// The titlebar owns placement; this control owns geometry, semantics and tooltip.
#[derive(IntoElement)]
pub struct ContextMeter {
    reading: ContextReading,
    theme: ArtisanTheme,
}
impl ContextMeter {
    pub fn new(reading: ContextReading, theme: ArtisanTheme) -> Self {
        Self { reading, theme }
    }
}
struct MeterHover {
    open: bool,
    motion: Rc<RefCell<MenuMotion>>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    settle: Option<gpui::Task<()>>,
}
impl MeterHover {
    fn set_open(&mut self, open: bool, cx: &mut gpui::Context<Self>) {
        if self.open == open {
            return;
        }
        self.open = open;
        let generation = if open {
            self.motion.borrow_mut().begin_open()
        } else {
            self.motion.borrow_mut().begin_close()
        };
        let settle = move |this: &mut Self| {
            if open {
                this.motion.borrow_mut().finish_open(generation)
            } else {
                this.motion.borrow_mut().finish_close(generation)
            }
        };
        if cx.reduce_motion() {
            settle(self);
            self.settle = None;
        } else {
            let duration = if open {
                MotionDuration::Fast
            } else {
                MotionDuration::Quick
            }
            .as_duration();
            self.settle = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(duration).await;
                let _ = this.update(cx, |this, cx| {
                    if settle(this) {
                        this.settle = None;
                        cx.notify();
                    }
                });
            }));
        }
        cx.notify();
    }
}
impl RenderOnce for ContextMeter {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self { reading, theme } = self;
        let hover = window.use_keyed_state("context-meter-hover", cx, |_, _| MeterHover {
            open: false,
            motion: Default::default(),
            bounds: Default::default(),
            settle: None,
        });
        let state = hover.read(cx);
        let bounds = state.bounds.clone();
        let motion = state.motion.clone();
        let painted = motion.borrow().painted();
        div()
            .child(context_ring(&reading, &theme).on_hover(move |over, _, cx| {
                hover.update(cx, |state, cx| state.set_open(*over, cx));
            }))
            .on_children_prepainted({
                let bounds = bounds.clone();
                move |children, _, _| {
                    if let Some(first) = children.first() {
                        bounds.set(*first);
                    }
                }
            })
            .children(painted.then(|| {
                let ring = bounds.get();
                gpui::deferred(
                    gpui::anchored()
                        .anchor(gpui::Anchor::TopRight)
                        .position(ring.bottom_right() + point(px(0.0), px(8.0)))
                        .child(animate_menu(
                            div()
                                .id("artisan-context-card")
                                .child(context_card(&reading, &theme)),
                            &motion,
                            "context",
                            MenuEntrance::Scale {
                                origin: point(1.0, 0.0),
                            },
                            MotionDuration::Fast.as_duration(),
                            MotionDuration::Quick.as_duration(),
                        )),
                )
                .with_priority(1)
            }))
    }
}

/// The ring gauge, the hover trigger of [`context_card`].
#[must_use]
fn context_ring(reading: &ContextReading, theme: &ArtisanTheme) -> Stateful<Div> {
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
        .role(gpui::Role::ProgressIndicator)
        .aria_label("Context window usage")
        .aria_value(format!(
            "{:.0}%: {} of {} tokens for {}",
            reading.percent(),
            reading.context_tokens,
            reading.window_tokens,
            reading.model_name
        ))
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
fn context_card(reading: &ContextReading, theme: &ArtisanTheme) -> Div {
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
    let fill = (reading.percent() / 100.0) as f32;
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
        .child(Progress::new("context-progress", *theme, fill));
    div()
        .debug_selector(|| format!("{CONTEXT_METER_SELECTOR}-card"))
        .w(px(280.0))
        .child(glass_hover_card(theme, content))
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

#[cfg(all(test, feature = "capture"))]
mod native_tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext};
    struct ReadingView {
        tokens: u64,
    }
    impl Render for ReadingView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().p(px(40.0)).child(ContextMeter::new(
                ContextReading::new("Model".into(), self.tokens, 200_000).unwrap(),
                ArtisanTheme::default(),
            ))
        }
    }
    #[gpui::test]
    fn usage_updates_do_not_change_titlebar_geometry(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| ReadingView { tokens: 0 });
        let before = cx.debug_bounds(CONTEXT_METER_SELECTOR).unwrap();
        assert_eq!(before.size, gpui::size(px(24.0), px(24.0)));
        view.update(cx, |view, cx| {
            view.tokens = u64::MAX;
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds(CONTEXT_METER_SELECTOR).unwrap(), before);
    }

    #[gpui::test]
    fn context_card_opens_on_hover_without_a_tooltip_delay(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| ReadingView { tokens: 90_000 });
        let ring = cx.debug_bounds(CONTEXT_METER_SELECTOR).unwrap();
        assert!(
            cx.debug_bounds("artisan-native-context-meter-card")
                .is_none()
        );
        cx.simulate_event(gpui::MouseMoveEvent {
            position: ring.center(),
            ..Default::default()
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let card = cx
            .debug_bounds("artisan-native-context-meter-card")
            .expect("the original card opens immediately on hover");
        assert_eq!(card.size.width, px(280.0));
        assert!(card.top() >= ring.bottom(), "card={card:?}, ring={ring:?}");
        assert_eq!(cx.debug_bounds(CONTEXT_METER_SELECTOR).unwrap(), ring);
    }
}
