//! Shared glass frames and hover treatments. Placement belongs to the caller.

use gpui::prelude::FluentBuilder as _;
use std::ops::Range;

use gpui::{
    Anchor, AnyElement, App, Bounds, BoxShadow, Context, Div, FontWeight, HighlightStyle, Hsla,
    InteractiveElement as _, IntoElement, ParentElement, Pixels, RenderOnce, SharedString, Size,
    Stateful, Styled, StyledText, Task, Window, anchored, deferred, div, point, px,
};
use std::time::Duration;

use crate::glass::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use crate::gradient::{hover_fill_stops, vertical_gradient};
use crate::theme::{
    ArtisanTheme, Oklch, Opacity, RadiusStep, RadiusTokens, TextStep, TypographyTokens,
};

/// Line height of hover card prose.
pub const HOVER_CARD_LINE_HEIGHT_PX: f32 = 19.0;

/// The hairline inset ring that edges the hover fill.
#[must_use]
pub(crate) fn hover_highlight_shadow(theme: &ArtisanTheme) -> Vec<BoxShadow> {
    hover_ring(HoverColors::new(theme, HoverTone::Neutral).ring)
}

fn hover_ring(color: Oklch) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: color.to_paint(),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(0.5),
        inset: true,
    }]
}

/// The two looks of the pointer's hover fill.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HoverFill {
    /// The gradient alone: menu items, icon buttons, triggers.
    Plain,
    /// The gradient edged by a hairline inset ring: list rows, segments and
    /// the sliding pill.
    Edged,
}

/// The hue of the hover fill under a row.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HoverTone {
    /// The text colour.
    #[default]
    Neutral,
    /// Varde's muted red glass tint: rows that archive, delete or stop.
    Destructive,
}

/// The hover fill's colours in one tone: the gradient's stops and the
/// edging ring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoverColors {
    top: Oklch,
    bottom: Oklch,
    ring: Oklch,
}

impl HoverColors {
    #[must_use]
    pub const fn new(theme: &ArtisanTheme, tone: HoverTone) -> Self {
        match tone {
            HoverTone::Neutral => {
                let ink = theme.colors.text;
                let (top, bottom) = hover_fill_stops(theme, ink);
                Self {
                    top,
                    bottom,
                    ring: ink.at(Opacity::HoverRing),
                }
            }
            HoverTone::Destructive => {
                // Match Varde's glass-destructive material: the actual red
                // at 28% / 14%, rather than red at the text's lightness.
                let ink = theme.colors.destructive_hover;
                let strong = ink.at(Opacity::DestructiveHoverStrong);
                let soft = ink.at(Opacity::DestructiveHoverSoft);
                let (top, bottom) = if theme.colors.mode.is_dark() {
                    (strong, soft)
                } else {
                    (soft, strong)
                };
                Self {
                    top,
                    bottom,
                    ring: strong,
                }
            }
        }
    }

    /// Part-way to `to` while the pill flies between tones.
    #[must_use]
    pub fn animate_to(self, to: Self, progress: f32) -> Self {
        Self {
            top: self.top.animate_to(to.top, progress),
            bottom: self.bottom.animate_to(to.bottom, progress),
            ring: self.ring.animate_to(to.ring, progress),
        }
    }

    /// Paints `element` with these colours.
    #[must_use]
    pub fn paint<E: Styled>(self, element: E, fill: HoverFill) -> E {
        let element = element.bg(vertical_gradient(self.top, self.bottom));
        match fill {
            HoverFill::Plain => element,
            HoverFill::Edged => element.shadow(hover_ring(self.ring)),
        }
    }
}

/// The pointer's hover fill.
///
/// The one entry point for it: inside a
/// `.hover(…)` refinement, or on the element the pointer is over now. Never
/// for selected or keyboard-highlighted state; that is [`highlight_fill`].
#[must_use]
pub fn hover_fill<E: Styled>(element: E, theme: &ArtisanTheme, fill: HoverFill) -> E {
    hover_fill_toned(element, theme, fill, HoverTone::Neutral)
}

/// [`hover_fill`] in `tone`.
#[must_use]
pub fn hover_fill_toned<E: Styled>(
    element: E,
    theme: &ArtisanTheme,
    fill: HoverFill,
    tone: HoverTone,
) -> E {
    HoverColors::new(theme, tone).paint(element, fill)
}

/// The keyboard's highlighted row (a select option, a completion): the
/// accent well and its foreground.
#[must_use]
pub fn highlight_fill<E: Styled>(element: E, theme: &ArtisanTheme) -> E {
    element
        .bg(theme.colors.selected.to_paint())
        .text_color(theme.colors.text.to_paint())
}

/// A tooltip shows once the pointer rests this long.
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(500);
/// The room between a tooltip and the element it belongs to.
const TOOLTIP_GAP_PX: f32 = 8.0;

/// A tooltip keyed by the element it belongs to.
///
/// Mounted hidden, shown once the pointer has rested for [`TOOLTIP_DELAY`].
/// Its owner measures the
/// pending element with a [`crate::measure::Measured`] probe and places it
/// with [`place_tooltip`].
pub struct DelayedTooltip<T> {
    pending: Option<(String, T)>,
    visible: bool,
    generation: u64,
    task: Option<Task<()>>,
}

impl<T> Default for DelayedTooltip<T> {
    fn default() -> Self {
        Self {
            pending: None,
            visible: false,
            generation: 0,
            task: None,
        }
    }
}

impl<T: 'static> DelayedTooltip<T> {
    /// Mounts `content` for `key` and shows it after the delay. `slot`
    /// finds this tooltip in its owner when the delay ends.
    pub fn begin<V: 'static>(
        &mut self,
        key: impl Into<String>,
        content: T,
        cx: &mut Context<V>,
        slot: fn(&mut V) -> &mut Self,
    ) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.pending = Some((key.into(), content));
        self.visible = false;
        self.task = Some(cx.spawn(async move |owner, cx| {
            cx.background_executor().timer(TOOLTIP_DELAY).await;
            // A released view has nothing left to update.
            owner
                .update(cx, |owner, cx| {
                    let tooltip = slot(owner);
                    if tooltip.generation == generation && tooltip.pending.is_some() {
                        tooltip.visible = true;
                        cx.notify();
                    }
                })
                .ok();
        }));
    }

    /// Drops the tooltip, shown or waiting.
    pub fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        self.visible = false;
        self.task = None;
    }

    /// Drops the tooltip if it belongs to `key`.
    pub fn clear_for(&mut self, key: &str) {
        if self.pending() == Some(key) {
            self.clear();
        }
    }

    /// The key of the element the tooltip belongs to, shown or waiting.
    #[must_use]
    pub fn pending(&self) -> Option<&str> {
        self.pending.as_ref().map(|(key, _)| key.as_str())
    }

    /// The content once the delay has passed.
    #[must_use]
    pub fn shown(&self) -> Option<&T> {
        self.pending
            .as_ref()
            .filter(|_| self.visible)
            .map(|(_, content)| content)
    }
}

/// Where a tooltip sits against its element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TooltipPlacement {
    /// Beside the element, on whichever side has room, at most `max_width`.
    Beside { max_width: f32 },
    /// Centred above the element, at most `max_width`.
    Above { max_width: f32 },
}

/// Places a tooltip against `anchor` (the element's measured bounds).
///
/// `surface` builds the tooltip for the width it may take; `fixed` asks it
/// to take exactly that width (beside) rather than at most (above).
pub fn place_tooltip(
    anchor: Bounds<Pixels>,
    viewport: Size<Pixels>,
    placement: TooltipPlacement,
    surface: impl FnOnce(Pixels, bool) -> AnyElement,
) -> AnyElement {
    let viewport_width = f32::from(viewport.width).max(1.0);
    let (anchor_corner, position, offset, width, fixed) = match placement {
        TooltipPlacement::Above { max_width } => (
            Anchor::BottomCenter,
            point(anchor.center().x, anchor.top()),
            point(px(0.0), px(-TOOLTIP_GAP_PX)),
            max_width.min(viewport_width),
            false,
        ),
        TooltipPlacement::Beside { max_width } => {
            let right_space = viewport_width - f32::from(anchor.right()) - TOOLTIP_GAP_PX;
            let left_space = f32::from(anchor.left()) - TOOLTIP_GAP_PX;
            let right = right_space >= max_width || right_space >= left_space;
            let available = if right { right_space } else { left_space };
            let width = max_width.min(available.max(1.0));
            if right {
                (
                    Anchor::LeftCenter,
                    point(anchor.right(), anchor.center().y),
                    point(px(TOOLTIP_GAP_PX), px(0.0)),
                    width,
                    true,
                )
            } else {
                (
                    Anchor::RightCenter,
                    point(anchor.left(), anchor.center().y),
                    point(px(-TOOLTIP_GAP_PX), px(0.0)),
                    width,
                    true,
                )
            }
        }
    };
    let mut placed = anchored()
        .anchor(anchor_corner)
        .position(position)
        .offset(offset);
    if !fixed {
        placed = placed.snap_to_window();
    }
    deferred(placed.child(surface(px(width), fixed)))
        .with_priority(3)
        .into_any_element()
}

/// Where an element last painted, for a tooltip laid against it. Record it
/// with [`TooltipAnchor::probe`] as a child of the element.
#[derive(Clone, Default)]
pub struct TooltipAnchor(std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>);

impl TooltipAnchor {
    /// A child that fills the element and records its bounds each frame.
    #[must_use]
    pub fn probe(&self) -> impl IntoElement {
        let cell = std::rc::Rc::clone(&self.0);
        gpui::canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, (), _, _| {})
            .absolute()
            .size_full()
    }

    /// A native tooltip (GPUI owns its delay and dismissal) laid against
    /// the element instead of the pointer: beside it, centred, on the side
    /// with room.
    pub fn tooltip(
        &self,
        content: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
        cx: &mut App,
    ) -> gpui::AnyView {
        let anchor = std::rc::Rc::clone(&self.0);
        tooltip(
            move |window, cx| {
                let content = content(window, cx);
                let Some(bounds) = anchor.get() else {
                    return content;
                };
                let right = f32::from(window.viewport_size().width) - f32::from(bounds.right())
                    >= f32::from(bounds.left());
                let (corner, position, offset) = if right {
                    (
                        Anchor::LeftCenter,
                        point(bounds.right(), bounds.center().y),
                        point(px(TOOLTIP_GAP_PX), px(0.0)),
                    )
                } else {
                    (
                        Anchor::RightCenter,
                        point(bounds.left(), bounds.center().y),
                        point(px(-TOOLTIP_GAP_PX), px(0.0)),
                    )
                };
                // GPUI roots the tooltip at the pointer; the card itself
                // sits against the element.
                div()
                    .child(
                        anchored()
                            .anchor(corner)
                            .position(position)
                            .offset(offset)
                            .child(content),
                    )
                    .into_any_element()
            },
            cx,
        )
    }
}

/// The glass tooltip frame: strong glass, muted 12 px text, padded; `fixed`
/// takes exactly `width`, otherwise at most.
#[must_use]
pub fn glass_tooltip(
    id: impl Into<SharedString>,
    theme: &ArtisanTheme,
    width: Pixels,
    fixed: bool,
) -> Stateful<Div> {
    let radius = RadiusTokens::value(RadiusStep::X2l);
    crate::popover::glass_frame(div().id(id.into()), theme, GlassStrength::Strong, radius)
        .when(fixed, |surface| surface.w(width))
        .max_w(width)
        .px(px(12.0))
        .py(px(8.0))
        .text_size(TypographyTokens::size(TextStep::Xs))
        .line_height(px(16.0))
        .whitespace_normal()
        .text_color(theme.colors.text_muted.to_paint())
}

enum GlassShape {
    Card,
    Navigator,
    Pill,
    Handle,
}

/// A glass frame with component-owned material and corner geometry.
#[derive(IntoElement)]
pub struct GlassCard {
    theme: ArtisanTheme,
    content: AnyElement,
    shape: GlassShape,
}
impl RenderOnce for GlassCard {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (radius, fill) = match self.shape {
            GlassShape::Card => (RadiusTokens::value(RadiusStep::X2l), false),
            GlassShape::Navigator => (RadiusTokens::value(RadiusStep::Xl), false),
            // Half the 22 px height: a fully round end.
            GlassShape::Pill => (px(11.0), true),
            GlassShape::Handle => (RadiusTokens::value(RadiusStep::XlMid), true),
        };
        let strength = if matches!(self.shape, GlassShape::Card | GlassShape::Navigator) {
            GlassStrength::Strong
        } else {
            GlassStrength::Quiet
        };
        let frame = glass_surface(&self.theme, radius, self.content, fill, strength);
        match self.shape {
            GlassShape::Card | GlassShape::Navigator => frame,
            GlassShape::Pill => frame.h(px(22.0)),
            GlassShape::Handle => frame.size(px(32.0)),
        }
    }
}

/// A floating glass card. Place and size it with an outer layout element.
#[must_use]
pub fn glass_hover_card(theme: &ArtisanTheme, content: impl IntoElement) -> GlassCard {
    GlassCard {
        theme: *theme,
        content: content.into_any_element(),
        shape: GlassShape::Card,
    }
}

/// The transcript navigator's original 14px strong-glass face.
pub fn glass_navigator_card(theme: &ArtisanTheme, content: impl IntoElement) -> GlassCard {
    GlassCard {
        theme: *theme,
        content: content.into_any_element(),
        shape: GlassShape::Navigator,
    }
}

/// A compact label over media, with one height and radius in every state.
#[must_use]
pub fn glass_pill(theme: &ArtisanTheme, content: impl IntoElement) -> GlassCard {
    GlassCard {
        theme: *theme,
        content: content.into_any_element(),
        shape: GlassShape::Pill,
    }
}

/// A circular 32 px handle over media.
#[must_use]
pub fn glass_handle(theme: &ArtisanTheme, content: impl IntoElement) -> GlassCard {
    GlassCard {
        theme: *theme,
        content: content.into_any_element(),
        shape: GlassShape::Handle,
    }
}

fn glass_surface(
    theme: &ArtisanTheme,
    radius: Pixels,
    content: impl IntoElement,
    fill: bool,
    strength: GlassStrength,
) -> Div {
    div()
        .relative()
        .rounded(radius)
        .backdrop_blur(glass_blur_radius(strength))
        .bg(glass_foreground_base(theme))
        .border_1()
        .border_color(theme.colors.text.at(Opacity::GlassBorder).to_paint())
        .shadow(glass_card_shadows(theme, crate::theme::Elevation::Overlay))
        .font_family(theme.typography.sans.family)
        .child(glass_material_layer(theme, strength, radius))
        .child(glass_highlight_layer(theme, strength, radius))
        .child(
            div()
                .relative()
                .when(fill, gpui::Styled::size_full)
                .text_color(if fill {
                    theme.colors.text.to_paint()
                } else {
                    theme.colors.text_muted.to_paint()
                })
                .child(content),
        )
}

/// The card title: semibold, control-sized, inheriting the card text colour.
#[must_use]
pub fn hover_card_title(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(theme.typography.control_text)
        .line_height(px(18.0))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// Muted prose with the `facts` byte ranges lifted to the foreground.
#[must_use]
pub fn hover_card_text(
    theme: &ArtisanTheme,
    text: impl Into<SharedString>,
    facts: &[Range<usize>],
) -> Div {
    let text = text.into();
    let foreground = HighlightStyle {
        color: Some(theme.colors.text.to_paint()),
        ..HighlightStyle::default()
    };
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = facts
        .iter()
        .filter(|range| range.start < range.end && range.end <= text.len())
        .filter(|range| text.is_char_boundary(range.start) && text.is_char_boundary(range.end))
        .cloned()
        .map(|range| (range, foreground))
        .collect();
    highlights.sort_by_key(|(range, _)| range.start);
    highlights.dedup_by(|later, earlier| later.0.start < earlier.0.end);
    div()
        .text_size(theme.typography.label_text)
        .line_height(px(HOVER_CARD_LINE_HEIGHT_PX))
        .text_color(theme.colors.text_muted.to_paint())
        .child(StyledText::new(text).with_highlights(highlights))
}

/// One fact row: an optional colour key, a muted label, and the value in
/// the foreground.
#[must_use]
pub fn hover_card_row(
    theme: &ArtisanTheme,
    key: Option<Hsla>,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .whitespace_nowrap()
        .text_size(theme.typography.label_text)
        .line_height(px(HOVER_CARD_LINE_HEIGHT_PX))
        .children(key.map(|color| {
            div()
                .flex_shrink_0()
                .size(px(8.0))
                .rounded(RadiusTokens::value(RadiusStep::Xs))
                .bg(color)
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme.colors.text_muted.to_paint())
                .child(label.into()),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_color(theme.colors.text.to_paint())
                .child(value.into()),
        )
}

/// A lazily-created native tooltip view. GPUI owns delay, placement and dismissal.
pub fn tooltip(
    content: impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyElement + 'static,
    cx: &mut gpui::App,
) -> gpui::AnyView {
    use gpui::AppContext as _;
    cx.new(|_| Tooltip {
        content: Box::new(content),
    })
    .into()
}
type TooltipContent = Box<dyn Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyElement>;
struct Tooltip {
    content: TooltipContent,
}
impl gpui::Render for Tooltip {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        (self.content)(window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext};

    struct GlassGallery;
    impl Render for GlassGallery {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let theme = ArtisanTheme::resolved();
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .child(
                    div()
                        .debug_selector(|| "glass-handle-test".to_owned())
                        .child(glass_handle(&theme, div().size_full())),
                )
                .child(
                    div()
                        .w(px(80.0))
                        .debug_selector(|| "glass-pill-test".to_owned())
                        .child(glass_pill(&theme, div().size_full().child("Before"))),
                )
        }
    }

    #[gpui::test]
    fn media_glass_recipes_own_the_complete_outer_geometry(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| GlassGallery);
        let handle = cx.debug_bounds("glass-handle-test").expect("handle");
        let pill = cx.debug_bounds("glass-pill-test").expect("pill");
        assert_eq!(handle.size, gpui::size(px(32.0), px(32.0)));
        assert_eq!(pill.size, gpui::size(px(80.0), px(22.0)));
    }
}
