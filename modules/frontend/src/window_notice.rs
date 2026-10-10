//! Window notices: glass cards naming what went wrong, stacked in the
//! top-right corner like the `sonner` toaster.
//!
//! The stack follows `svelte-sonner` 1.1.1 (`Toaster.svelte`/`Toast.svelte`):
//! each notice lives 4 s (`TOAST_LIFETIME`), paused while the pointer rests
//! on the stack; three show at once (`VISIBLE_TOASTS_AMOUNT`); collapsed, each
//! notice behind the front sits 14 px lower (`GAP`) and 5 % smaller, clipped
//! to the front's height; hovering expands the stack into a column with 14 px
//! gaps; a new notice enters from above and a dismissed one leaves upward,
//! all on 400 ms transitions, unmounting 200 ms after removal
//! (`TIME_BEFORE_UNMOUNT`). Cards are 356 px wide (`TOAST_WIDTH`).
//!
//! The card carries Varde's status glow (`card-status-glow` in
//! `modules/common/ui/src/global.css`): a wash of the status colour at 16 %
//! with an inset rim and a soft inner top glow, all faded by a top-to-bottom
//! mask (full, 31.25 % at 60 %, 18.75 % at the end), so the rim reads as an
//! upper half glow. GPUI has no mask, so the fade is rebuilt from layers: a
//! two-stop wash, the rim at its masked-down strength over the whole card,
//! and the full-strength rim clipped to the top 60 %.

#![forbid(unsafe_code)]

use artisan_assets::AssetId;
use artisan_ui::{
    asset_seam::asset_glyph,
    glass::{
        GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
        glass_highlight_layer, glass_material_layer,
    },
    gradient::VERTICAL_ANGLE_DEGREES,
    theme::{ArtisanTheme, Oklch, RadiusStep, RadiusTokens},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

use artisan_ui::motion::MotionCurve;
use gpui::prelude::{
    InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
};
use gpui::{
    AnyElement, App, BoxShadow, ClickEvent, Div, ElementTransform, IntoElement as _, Pixels,
    Stateful, Window, div, linear_color_stop, linear_gradient, point, px, relative,
};

/// `TOAST_LIFETIME`: how long a notice shows before it leaves on its own.
pub const NOTICE_LIFETIME: Duration = Duration::from_millis(4_000);
/// `VISIBLE_TOASTS_AMOUNT`: notices shown at once; older ones wait hidden.
const VISIBLE_NOTICES: usize = 3;
/// `GAP` between stacked notices, in pixels.
const NOTICE_GAP: f32 = 14.0;
/// `TOAST_WIDTH`, in pixels.
const NOTICE_WIDTH: f32 = 356.0;
/// The stack's transitions (`transform 400ms`, `opacity 400ms`).
const NOTICE_MOTION: Duration = Duration::from_millis(400);
/// `TIME_BEFORE_UNMOUNT`: a removed notice is dropped this long after it
/// starts leaving.
const NOTICE_UNMOUNT: Duration = Duration::from_millis(200);

/// Stable selector for the window notice card.
pub const WINDOW_NOTICE_SELECTOR: &str = "window-error";

/// How serious a window notice is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeSeverity {
    /// Something failed.
    Error,
    /// Something did not happen yet but may on a retry.
    Warning,
}

impl NoticeSeverity {
    /// The card's header.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Error => "An error occurred",
            Self::Warning => "Warning",
        }
    }

    fn color(self, theme: &ArtisanTheme) -> Oklch {
        match self {
            Self::Error => theme.colors.destructive,
            Self::Warning => theme.colors.banner_warning,
        }
    }
}

/// Renders one notice card; `on_close` runs when its close button is pressed.
#[must_use]
pub fn render_window_notice(
    severity: NoticeSeverity,
    message: String,
    theme: ArtisanTheme,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let radius = RadiusTokens::value(RadiusStep::Xl);
    let tone = severity.color(&theme);
    // Coloured elements colour their icons: the tone, muted to 80 %.
    let icon_color = tone.with_alpha(0.8).to_paint();
    div()
        .id(WINDOW_NOTICE_SELECTOR)
        .debug_selector(|| WINDOW_NOTICE_SELECTOR.to_owned())
        .relative()
        .w(px(NOTICE_WIDTH))
        .rounded(radius)
        .overflow_hidden()
        .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
        .bg(glass_foreground_base(&theme))
        .shadow(glass_card_shadows())
        .font_family(theme.typography.sans.family)
        .child(glass_material_layer(GlassStrength::Quiet, radius))
        .child(glass_highlight_layer(GlassStrength::Quiet, radius))
        .children(status_glow(tone, radius))
        .child(
            div()
                .relative()
                .flex()
                .items_start()
                .gap(px(12.0))
                .p(px(16.0))
                .pr(px(44.0))
                .child(
                    div()
                        .flex_none()
                        .pt(px(2.0))
                        .text_color(icon_color)
                        .child(asset_glyph(AssetId::TABLER_ALERT_TRIANGLE).size(px(16.0))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .line_height(px(20.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.colors.foreground.to_paint())
                                .child(severity.title()),
                        )
                        .child(
                            div()
                                .text_size(px(13.0))
                                .line_height(px(19.0))
                                .text_color(theme.colors.foreground.with_alpha(0.7).to_paint())
                                .child(message),
                        ),
                )
        )
        // The close button sits in the top-right corner.
        .child(
            div()
                .id("window-notice-close")
                .debug_selector(|| "window-notice-close".to_owned())
                .absolute()
                .top(px(10.0))
                .right(px(10.0))
                .size(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .cursor_pointer()
                .text_color(theme.colors.muted_foreground.to_paint())
                .hover(|style| style.bg(theme.colors.foreground.with_alpha(0.08).to_paint()))
                .role(gpui::Role::Button)
                .aria_label("Dismiss")
                .on_click(on_close)
                .child(asset_glyph(AssetId::TABLER_X).size(px(14.0))),
        )
}

/// The status glow layers, back to front.
fn status_glow(tone: Oklch, radius: Pixels) -> [Div; 3] {
    // Mask stops: full strength at the top, 31.25 % at 60 %, 18.75 % at the
    // bottom. The wash keeps two stops (GPUI's gradient limit), top to bottom.
    let wash = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(radius)
        .bg(linear_gradient(
            VERTICAL_ANGLE_DEGREES,
            linear_color_stop(tone.with_alpha(0.16).to_paint(), 0.0),
            linear_color_stop(tone.with_alpha(0.16 * 0.1875).to_paint(), 1.0),
        ));
    // The rim everywhere at the mask's lower strength...
    let faint_rim = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(radius)
        .shadow(rim_shadows(tone, 0.1875));
    // ...and at full strength over the top 60 %, where the mask is strongest.
    let upper_rim = div()
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .h(relative(0.6))
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h(relative(1.0 / 0.6))
                .rounded(radius)
                .shadow(rim_shadows(tone, 1.0)),
        );
    [wash, faint_rim, upper_rim]
}

/// Varde's three inset rim shadows at `strength` of their colour.
fn rim_shadows(tone: Oklch, strength: f32) -> Vec<BoxShadow> {
    let shadow = |alpha: f32, offset_y: f32, blur: f32, spread: f32| BoxShadow {
        color: tone.with_alpha(alpha * strength).to_paint(),
        offset: point(px(0.0), px(offset_y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: true,
    };
    vec![
        shadow(0.12, 0.5, 0.0, 0.0),
        shadow(0.18, 0.0, 0.0, 0.5),
        shadow(0.16, 6.0, 16.0, -12.0),
    ]
}

/// One value easing from where it was toward a target over
/// [`NOTICE_MOTION`]; retargeting starts from the value on screen.
#[derive(Clone, Copy, Debug)]
struct Tween {
    from: f32,
    to: f32,
    started: Instant,
}

impl Tween {
    fn at(value: f32, now: Instant) -> Self {
        Self {
            from: value,
            to: value,
            started: now,
        }
    }

    fn progress(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        (elapsed / NOTICE_MOTION.as_secs_f32()).clamp(0.0, 1.0)
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "the shared curve samples in f64; the paint value is f32"
    )]
    fn value(&self, now: Instant) -> f32 {
        let eased = MotionCurve::EaseOut.sample(f64::from(self.progress(now))) as f32;
        self.from + (self.to - self.from) * eased
    }

    fn moving(&self, now: Instant) -> bool {
        self.progress(now) < 1.0 && (self.to - self.from).abs() > f32::EPSILON
    }

    fn retarget(&mut self, to: f32, now: Instant) {
        if (to - self.to).abs() > 0.01 {
            *self = Self {
                from: self.value(now),
                to,
                started: now,
            };
        }
    }
}

/// One notice in the stack.
struct Notice {
    id: u64,
    severity: NoticeSeverity,
    message: String,
    /// Lifetime left; it only runs while the stack is not hovered.
    remaining: Duration,
    running_since: Option<Instant>,
    /// When it started leaving.
    removed: Option<Instant>,
    offset: Tween,
    scale: Tween,
    opacity: Tween,
}

/// The window's notice stack.
#[derive(Default)]
pub struct WindowNotices {
    notices: Vec<Notice>,
    next_id: u64,
    expanded: bool,
    /// Each notice's natural height, measured as it paints.
    heights: Rc<RefCell<HashMap<u64, Pixels>>>,
}

impl WindowNotices {
    /// Adds a notice at the front of the stack.
    pub fn push(&mut self, severity: NoticeSeverity, message: String, now: Instant) {
        let id = self.next_id;
        self.next_id += 1;
        self.notices.push(Notice {
            id,
            severity,
            message,
            remaining: NOTICE_LIFETIME,
            running_since: (!self.expanded).then_some(now),
            removed: None,
            // It enters from above and fades in.
            offset: Tween {
                from: -48.0,
                to: 0.0,
                started: now,
            },
            scale: Tween::at(1.0, now),
            opacity: Tween {
                from: 0.0,
                to: 1.0,
                started: now,
            },
        });
    }

    /// Whether no notice is showing or leaving.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.notices.is_empty()
    }

    /// The front notice's message, if one is showing.
    #[must_use]
    pub fn front_message(&self) -> Option<&str> {
        self.notices
            .iter()
            .rev()
            .find(|notice| notice.removed.is_none())
            .map(|notice| notice.message.as_str())
    }

    /// Starts one notice leaving.
    pub fn dismiss(&mut self, id: u64, now: Instant) {
        if let Some(notice) = self.notices.iter_mut().find(|notice| notice.id == id) {
            notice.removed.get_or_insert(now);
        }
    }

    /// Expands the stack and pauses every lifetime while it is hovered.
    pub fn set_hovered(&mut self, hovered: bool, now: Instant) {
        if hovered == self.expanded {
            return;
        }
        self.expanded = hovered;
        for notice in &mut self.notices {
            if hovered {
                if let Some(since) = notice.running_since.take() {
                    notice.remaining = notice
                        .remaining
                        .saturating_sub(now.saturating_duration_since(since));
                }
            } else if notice.removed.is_none() {
                notice.running_since = Some(now);
            }
        }
    }

    /// Starts expired notices leaving and drops those done leaving; returns
    /// how long until the next lifetime ends, if one is running.
    pub fn tick(&mut self, now: Instant) -> Option<Duration> {
        for notice in &mut self.notices {
            if notice.removed.is_none()
                && let Some(since) = notice.running_since
                && now.saturating_duration_since(since) >= notice.remaining
            {
                notice.removed = Some(now);
            }
        }
        let heights = Rc::clone(&self.heights);
        self.notices.retain(|notice| {
            let keep = notice
                .removed
                .is_none_or(|removed| now.saturating_duration_since(removed) < NOTICE_UNMOUNT);
            if !keep {
                heights.borrow_mut().remove(&notice.id);
            }
            keep
        });
        self.notices
            .iter()
            .filter(|notice| notice.removed.is_none())
            .filter_map(|notice| {
                notice.running_since.map(|since| {
                    notice
                        .remaining
                        .saturating_sub(now.saturating_duration_since(since))
                })
            })
            .min()
    }

    /// Lays the stack out for `now` and renders it; returns the element and
    /// whether anything is still moving.
    #[expect(
        clippy::cast_precision_loss,
        reason = "stack positions are bounded small counts"
    )]
    pub fn render(
        &mut self,
        theme: ArtisanTheme,
        now: Instant,
        on_close: impl Fn(u64) -> Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
        on_hover: impl Fn(&bool, &mut Window, &mut App) + 'static,
    ) -> Option<(AnyElement, bool)> {
        if self.notices.is_empty() {
            return None;
        }
        let heights = self.heights.borrow().clone();
        let height_of = |id: u64| heights.get(&id).map_or(72.0, |height| f32::from(*height));
        let front_height = self
            .notices
            .iter()
            .rev()
            .find(|notice| notice.removed.is_none())
            .map_or(72.0, |notice| height_of(notice.id));
        // Targets, front first.
        let mut index = 0usize;
        let mut column = 0.0f32;
        let expanded = self.expanded;
        for notice in self.notices.iter_mut().rev() {
            let height = height_of(notice.id);
            if notice.removed.is_some() {
                // It leaves upward and fades.
                notice.offset.retarget(notice.offset.to - height, now);
                notice.opacity.retarget(0.0, now);
                continue;
            }
            let shown = index < VISIBLE_NOTICES;
            let (offset, scale) = if expanded {
                (column + NOTICE_GAP * index as f32, 1.0)
            } else {
                (NOTICE_GAP * index as f32, 1.0 - 0.05 * index as f32)
            };
            notice.offset.retarget(offset, now);
            notice.scale.retarget(scale, now);
            notice.opacity.retarget(if shown { 1.0 } else { 0.0 }, now);
            column += height;
            index += 1;
        }
        let shown = index.min(VISIBLE_NOTICES);
        let stack_height = if expanded {
            column + NOTICE_GAP * shown.saturating_sub(1) as f32
        } else {
            front_height + NOTICE_GAP * shown.saturating_sub(1) as f32
        };
        let mut moving = false;
        let mut stack = div()
            .id("window-notice-stack")
            .relative()
            .w(px(NOTICE_WIDTH))
            .h(px(stack_height))
            .on_hover(on_hover);
        // Paint back to front: the oldest first, so the front is on top.
        for (position, notice) in self.notices.iter().enumerate() {
            moving |=
                notice.offset.moving(now) || notice.scale.moving(now) || notice.opacity.moving(now);
            let behind = notice.removed.is_none()
                && self.notices[position + 1..]
                    .iter()
                    .any(|later| later.removed.is_none());
            let card = render_window_notice(
                notice.severity,
                notice.message.clone(),
                theme,
                on_close(notice.id),
            );
            let measured = Rc::clone(&self.heights);
            let id = notice.id;
            let mut slot = div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .opacity(notice.opacity.value(now))
                .transform(
                    ElementTransform::scale(notice.scale.value(now))
                        .with_origin(point(0.5, 0.0))
                        .with_translate(point(px(0.0), px(notice.offset.value(now)))),
                )
                .child(
                    div()
                        .child(card)
                        .on_children_prepainted(move |children, _, _| {
                            if let Some(first) = children.first() {
                                measured.borrow_mut().insert(id, first.size.height);
                            }
                        }),
                );
            // Collapsed, a notice behind the front takes the front's height.
            if behind && !expanded {
                slot = slot
                    .h(px(front_height))
                    .overflow_hidden()
                    .rounded(RadiusTokens::value(RadiusStep::Xl));
            }
            stack = stack.child(slot);
        }
        Some((stack.into_any_element(), moving))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_leaves_after_its_lifetime_and_hover_pauses_it() {
        let start = Instant::now();
        let mut notices = WindowNotices::default();
        notices.push(NoticeSeverity::Error, "first".into(), start);
        assert_eq!(notices.tick(start), Some(NOTICE_LIFETIME));
        // Hovered for 10 s: the lifetime does not run.
        notices.set_hovered(true, start + Duration::from_secs(1));
        assert_eq!(notices.tick(start + Duration::from_secs(11)), None);
        notices.set_hovered(false, start + Duration::from_secs(11));
        assert_eq!(
            notices.tick(start + Duration::from_secs(11)),
            Some(Duration::from_secs(3))
        );
        // Lifetime ends, then it unmounts.
        let ended = start + Duration::from_secs(14);
        notices.tick(ended);
        assert!(notices.front_message().is_none());
        assert!(!notices.is_empty(), "it is still leaving");
        notices.tick(ended + NOTICE_UNMOUNT);
        assert!(notices.is_empty());
    }

    #[test]
    fn the_newest_notice_is_at_the_front() {
        let now = Instant::now();
        let mut notices = WindowNotices::default();
        notices.push(NoticeSeverity::Error, "older".into(), now);
        notices.push(NoticeSeverity::Warning, "newer".into(), now);
        assert_eq!(notices.front_message(), Some("newer"));
        let newer = notices.notices[1].id;
        notices.dismiss(newer, now);
        assert_eq!(notices.front_message(), Some("older"));
    }
}
