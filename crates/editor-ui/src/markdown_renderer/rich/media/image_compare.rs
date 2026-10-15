//! `image-compare`: two images compared in one of four modes.
//!
//! - **Slider**: `after` fills the frame and `before` sits over it, clipped
//!   to a draggable divider, so the left shows `before` and the right
//!   `after`.
//! - **Side by side**: two frames next to each other.
//! - **Toggle**: one image at a time; clicking or Space flips it.
//! - **Blend**: `after` over `before` at an opacity set by dragging across.
//!
//! Both images share one frame: the wider of their pixel ratios, so the
//! narrower letterboxes and the two line up (else the hints, else 16:9),
//! capped in height. A side that cannot load replaces the frame with one
//! line naming it. Pressing in the frame focuses it; arrow keys then move
//! the divider or blend (Shift for finer steps) and double-clicking resets
//! it to the middle. The labels are glass pills that step aside while the
//! divider is dragged.

use crate::markdown_renderer::rich::budget;
use crate::theme::{ArtisanTheme, TextStep, TypographyTokens};
use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Div, ElementId, Entity, FocusHandle, IntoElement, KeyDownEvent,
    MouseButton, ParentElement as _, Pixels, RenderOnce, SharedString, Styled as _, Window, canvas,
    div, prelude::*, px, relative,
};
use serde::Deserialize;

use super::super::kit;
use super::super::{BlockContext, RichBlock};
use super::picture::{self, Loaded, SizeHint};
use super::source::MediaSource;
use crate::hover_card::{glass_handle, glass_pill};
use crate::segmented_control::{Segment, SegmentedControl};
use editor_assets::AssetId;

/// Diameter of the divider's handle.
const HANDLE: f32 = 32.0;

/// How far one arrow press moves the divider or blend; Shift for `FINE`.
const STEP: f32 = 0.05;
const FINE: f32 = 0.01;

/// How the two images are compared.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    #[default]
    Slider,
    SideBySide,
    Toggle,
    Blend,
}

impl Mode {
    const ALL: [Self; 4] = [Self::Slider, Self::SideBySide, Self::Toggle, Self::Blend];

    fn label(self) -> &'static str {
        match self {
            Self::Slider => "Slider",
            Self::SideBySide => "Side by side",
            Self::Toggle => "Toggle",
            Self::Blend => "Blend",
        }
    }
}

/// The `image-compare` spec.
#[derive(Debug, Deserialize)]
pub(super) struct ImageCompare {
    before: Side,
    after: Side,
    /// The mode the block opens in; the reader can switch.
    #[serde(default)]
    mode: Mode,
    #[serde(flatten)]
    hint: SizeHint,
}

#[derive(Clone, Debug, Deserialize)]
struct Side {
    src: MediaSource,
    #[serde(default)]
    label: Option<String>,
}

impl Side {
    fn label_or(&self, default: &str) -> SharedString {
        SharedString::from(
            self.label
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .unwrap_or(default)
                .to_owned(),
        )
    }
}

impl RichBlock for ImageCompare {
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.before.src.resident_bytes()
            + self.after.src.resident_bytes()
            + budget::optional(&self.before.label)
            + budget::optional(&self.after.label)
    }

    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        CompareView {
            id: cx.key("image-compare"),
            media: cx.media.cloned(),
            theme: *cx.theme,
            before: self.before.src.clone(),
            after: self.after.src.clone(),
            before_label: self.before.label_or("Before"),
            after_label: self.after.label_or("After"),
            mode: self.mode,
            hint: self.hint,
        }
        .into_any_element()
    }
}

#[derive(IntoElement)]
struct CompareView {
    media: Option<super::source::MediaFetcher>,
    id: SharedString,
    theme: ArtisanTheme,
    before: MediaSource,
    after: MediaSource,
    before_label: SharedString,
    after_label: SharedString,
    mode: Mode,
    hint: SizeHint,
}

struct CompareState {
    mode: Mode,
    /// The divider's position across the frame, or the blend's share of
    /// `after`, `0..=1`.
    split: f32,
    /// Toggle mode shows `after` when set.
    showing_after: bool,
    dragging: bool,
    focus: FocusHandle,
    /// The frame's bounds as last painted, to map the pointer to a split.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// What one frame shows, read from the state once per render.
#[derive(Clone, Copy)]
struct Shown {
    mode: Mode,
    split: f32,
    showing_after: bool,
    dragging: bool,
}

impl CompareState {
    fn new(mode: Mode, focus: FocusHandle) -> Self {
        Self {
            mode,
            split: 0.5,
            showing_after: false,
            dragging: false,
            focus,
            bounds: Rc::new(Cell::new(None)),
        }
    }

    /// Moves the divider under the pointer at window x `x`.
    fn follow(&mut self, x: Pixels) -> bool {
        let Some(bounds) = self.bounds.get() else {
            return false;
        };
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return false;
        }
        self.set_split(f32::from(x - bounds.origin.x) / width)
    }

    fn set_split(&mut self, split: f32) -> bool {
        let split = split.clamp(0.0, 1.0);
        let moved = (split - self.split).abs() > f32::EPSILON;
        self.split = split;
        moved
    }

    /// Applies a key press; whether anything changed.
    fn key(&mut self, event: &KeyDownEvent) -> bool {
        let step = if event.keystroke.modifiers.shift {
            FINE
        } else {
            STEP
        };
        match (self.mode, event.keystroke.key.as_str()) {
            (Mode::Toggle, "space" | "enter" | "left" | "right") => {
                self.showing_after = !self.showing_after;
                true
            }
            (Mode::Slider | Mode::Blend, "left") => self.set_split(self.split - step),
            (Mode::Slider | Mode::Blend, "right") => self.set_split(self.split + step),
            (Mode::Slider | Mode::Blend, "home") => self.set_split(0.0),
            (Mode::Slider | Mode::Blend, "end") => self.set_split(1.0),
            _ => false,
        }
    }
}

impl CompareView {
    fn part(&self, part: impl std::fmt::Display) -> ElementId {
        ElementId::Name(self.key(part))
    }

    /// The string form of [`Self::part`].
    fn key(&self, part: impl std::fmt::Display) -> SharedString {
        kit::part_key(&self.id, part)
    }
}

/// A small glass pill over the image.
fn pill(theme: &ArtisanTheme, text: impl Into<SharedString>) -> crate::hover_card::GlassCard {
    glass_pill(
        theme,
        div()
            .h_full()
            .px(px(9.0))
            .flex()
            .items_center()
            .text_size(TypographyTokens::size(TextStep::Xs))
            .line_height(px(16.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .whitespace_nowrap()
            .child(text.into()),
    )
}

/// A pill pinned to the top `left` or right corner of a frame.
fn corner(theme: &ArtisanTheme, text: SharedString, left: bool, hidden: bool) -> Div {
    let anchored = div().absolute().top(px(8.0));
    let anchored = if left {
        anchored.left(px(8.0))
    } else {
        anchored.right(px(8.0))
    };
    anchored
        .when(hidden, |pill| pill.opacity(0.0))
        .child(pill(theme, text))
}

/// The divider line and its glass handle at `split`.
fn divider(theme: &ArtisanTheme, split: f32) -> [Div; 2] {
    let line = theme.colors.background.with_alpha(0.9).to_paint();
    let ink = theme.colors.foreground.to_paint();
    [
        div()
            .absolute()
            .top_0()
            .h_full()
            .left(relative(split))
            .ml(px(-0.5))
            .w(px(1.0))
            .bg(line),
        div()
            .absolute()
            .top(relative(0.5))
            .left(relative(split))
            .mt(px(-HANDLE / 2.0))
            .ml(px(-HANDLE / 2.0))
            .child(glass_handle(
                theme,
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(kit::glyph(AssetId::TABLER_CHEVRON_LEFT, 12.0, ink))
                    .child(kit::glyph(AssetId::TABLER_CHEVRON_RIGHT, 12.0, ink)),
            )),
    ]
}

fn on_drag(
    state: &Entity<CompareState>,
) -> impl Fn(&gpui::MouseMoveEvent, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |event, _, cx| {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        state.update(cx, |state, cx| {
            if state.dragging && state.follow(event.position.x) {
                cx.notify();
            }
        });
    }
}

fn on_release(
    state: &Entity<CompareState>,
) -> impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |_, _, cx| {
        state.update(cx, |state, cx| {
            if std::mem::take(&mut state.dragging) {
                cx.notify();
            }
        });
    }
}

/// An image layer filling the frame.
fn layer(image: &Loaded, theme: &ArtisanTheme, src: &str) -> Div {
    div()
        .absolute()
        .size_full()
        .child(image.element(theme, src))
}

impl RenderOnce for CompareView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let mode = self.mode;
        let state = window.use_keyed_state(self.part("state"), cx, |_, cx| {
            CompareState::new(mode, cx.focus_handle())
        });
        let before = picture::load(
            window,
            cx,
            &self.key("before"),
            &self.before,
            self.media.as_ref(),
        );
        let after = picture::load(
            window,
            cx,
            &self.key("after"),
            &self.after,
            self.media.as_ref(),
        );
        let (aspect, max_width) =
            Loaded::frame(&[&before, &after], self.hint, picture::MAX_FRAME_HEIGHT);
        let mode = state.read(cx).mode;
        let view = match mode {
            Mode::SideBySide => self.side_by_side(&before, &after, aspect, max_width),
            // The cap sits on a wrapper: on the frame itself, the aspect
            // ratio would size its height from the uncapped width.
            Mode::Slider | Mode::Toggle | Mode::Blend => div()
                .w_full()
                .max_w(max_width)
                .child(self.single(&state, &before, &after, aspect, cx))
                .into_any_element(),
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.0))
            .child(view)
            .child(self.switcher(&state, mode))
            .into_any_element()
    }
}

impl CompareView {
    /// One frame showing the slider, toggle, or blend.
    fn single(
        &self,
        state: &Entity<CompareState>,
        before: &Loaded,
        after: &Loaded,
        aspect: f32,
        cx: &App,
    ) -> AnyElement {
        let theme = self.theme;
        let (mode, split, showing_after, dragging, focus, probe) = {
            let state = state.read(cx);
            (
                state.mode,
                state.split,
                state.showing_after,
                state.dragging,
                state.focus.clone(),
                Rc::clone(&state.bounds),
            )
        };

        let press = {
            let state = state.clone();
            let focus = focus.clone();
            move |event: &gpui::MouseDownEvent, window: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                window.focus(&focus, cx);
                state.update(cx, |state, cx| {
                    if state.mode == Mode::Toggle {
                        state.showing_after = !state.showing_after;
                    } else if event.click_count >= 2 {
                        state.set_split(0.5);
                    } else {
                        state.dragging = true;
                        state.follow(event.position.x);
                    }
                    cx.notify();
                });
            }
        };
        let keys = {
            let state = state.clone();
            move |event: &KeyDownEvent, _: &mut Window, cx: &mut App| {
                state.update(cx, |state, cx| {
                    if state.key(event) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                });
            }
        };

        let frame = div()
            .relative()
            .size_full()
            .id(self.part("frame"))
            .track_focus(&focus)
            .map(|frame| match mode {
                Mode::Toggle => frame.cursor_pointer(),
                _ => frame.cursor_col_resize(),
            })
            .on_mouse_down(MouseButton::Left, press)
            .on_mouse_move(on_drag(state))
            .on_mouse_up(MouseButton::Left, on_release(state))
            .on_mouse_up_out(MouseButton::Left, on_release(state))
            .on_key_down(keys)
            .child(
                canvas(
                    move |bounds, _, _| probe.set(Some(bounds)),
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            );

        let shown = Shown {
            mode,
            split,
            showing_after,
            dragging,
        };
        crate::media_frame::MediaFrame::new(theme, aspect, self.layers(frame, shown, before, after))
            .into_any_element()
    }

    /// The images, labels, and divider of a single frame in `mode`.
    fn layers(
        &self,
        frame: gpui::Stateful<Div>,
        shown: Shown,
        before: &Loaded,
        after: &Loaded,
    ) -> AnyElement {
        let theme = self.theme;
        let Shown {
            mode,
            split,
            showing_after,
            dragging,
        } = shown;
        match mode {
            Mode::Toggle => {
                let (image, src, text) = if showing_after {
                    (after, &self.after, &self.after_label)
                } else {
                    (before, &self.before, &self.before_label)
                };
                frame
                    .child(layer(image, &theme, src))
                    .child(corner(&theme, text.clone(), !showing_after, false))
                    .into_any_element()
            }
            Mode::Blend => frame
                .child(layer(before, &theme, &self.before))
                .child(layer(after, &theme, &self.after).opacity(split))
                .child(
                    div()
                        .absolute()
                        .bottom(px(8.0))
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(pill(
                            &theme,
                            format!(
                                "{} {} \u{00b7} {} {}",
                                self.before_label,
                                percent(1.0 - split),
                                self.after_label,
                                percent(split),
                            ),
                        )),
                )
                .into_any_element(),
            Mode::Slider | Mode::SideBySide => frame
                .child(layer(after, &theme, &self.after))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .h_full()
                        .w(relative(split))
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .h_full()
                                // The clip is `split` of the frame; this is the frame's width.
                                .w(relative(1.0 / split.max(0.001)))
                                .child(before.element(&theme, &self.before)),
                        ),
                )
                .child(corner(&theme, self.before_label.clone(), true, dragging))
                .child(corner(&theme, self.after_label.clone(), false, dragging))
                .children(divider(&theme, split))
                .into_any_element(),
        }
    }

    /// Two frames next to each other, each with its label.
    fn side_by_side(
        &self,
        before: &Loaded,
        after: &Loaded,
        aspect: f32,
        max_width: Pixels,
    ) -> AnyElement {
        let theme = self.theme;
        let half = |image: &Loaded, src: &str, label: &SharedString, left: bool| {
            div()
                .flex_1()
                .min_w_0()
                .child(crate::media_frame::MediaFrame::new(
                    theme,
                    aspect,
                    div()
                        .relative()
                        .size_full()
                        .child(layer(image, &theme, src))
                        .child(corner(&theme, label.clone(), left, false)),
                ))
        };
        div()
            .w_full()
            .max_w(max_width * 2.0 + px(8.0))
            .flex()
            .gap(px(8.0))
            .child(half(before, &self.before, &self.before_label, true))
            .child(half(after, &self.after, &self.after_label, true))
            .into_any_element()
    }

    /// The feature chooses the mode; shared controls own focus and selection.
    fn switcher(&self, state: &Entity<CompareState>, chosen: Mode) -> SegmentedControl<Mode> {
        let state = state.clone();
        SegmentedControl::new(
            self.part("modes"),
            self.theme,
            "Comparison mode",
            chosen,
            Segment::new(Mode::ALL[0], Mode::ALL[0].label()),
        )
        .options(
            Mode::ALL[1..]
                .iter()
                .map(|mode| Segment::new(*mode, mode.label())),
        )
        .on_change(move |mode, _, cx| {
            state.update(cx, |state, cx| {
                if state.mode != mode {
                    state.mode = mode;
                    state.dragging = false;
                    cx.notify();
                }
            });
        })
    }
}

/// `share` as a whole percentage.
fn percent(share: f32) -> String {
    format!("{:.0}%", share * 100.0)
}
