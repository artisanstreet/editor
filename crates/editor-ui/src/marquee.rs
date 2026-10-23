//! One line of text that scrolls when it doesn't fit, like a ticker: it
//! rests at its start, then moves left at a reading pace and repeats
//! seamlessly, two copies a gap apart. Text that fits stands still.
//!
//! The line is shaped once at paint; every later frame repaints it at the
//! time's offset (`Window::paint_animated`), so its view never renders or
//! lays out again while it scrolls. Under reduced motion it stands at its
//! start, cut at the edge.

use gpui::{
    App, Bounds, ContentMask, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, Pixels, SharedString, Style, TextAlign, Window, point, px, relative,
};
use std::time::{Duration, Instant};

/// How fast the text moves, in pixels a second: about three words.
const SPEED: f32 = 36.0;
/// How long the start shows before the text moves.
const REST: Duration = Duration::from_millis(1600);
/// Between the end of the text and its next copy.
const GAP: f32 = 48.0;

/// A ticker line in the inherited text style.
pub struct Marquee {
    id: ElementId,
    text: SharedString,
    color: Option<Hsla>,
}

/// A line of `text` that scrolls when it is wider than its space. `id`
/// keeps its place across renders; new text starts again from the rest.
#[must_use]
pub fn marquee(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Marquee {
    Marquee {
        id: id.into(),
        text: text.into(),
        color: None,
    }
}

/// When the shown text started, kept across renders.
struct Clock {
    text: SharedString,
    started: Instant,
}

impl Marquee {
    /// The text's colour, instead of the inherited one.
    #[must_use]
    pub const fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl IntoElement for Marquee {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Marquee {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let line_height = window.text_style().line_height_in_pixels(window.rem_size());
        let mut style = Style {
            // Takes the row's remaining width; never pushes its siblings out.
            flex_grow: 1.0,
            flex_shrink: 1.0,
            ..Style::default()
        };
        style.min_size.width = px(0.).into();
        style.size.width = relative(1.).into();
        style.size.height = line_height.into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let style = window.text_style();
        let mut run = style.to_run(self.text.len());
        if let Some(color) = self.color {
            run.color = color;
        }
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = style.line_height_in_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(self.text.clone(), font_size, &[run], None);
        let width = f32::from(line.width);
        let fits = width <= f32::from(bounds.size.width);
        let mask = ContentMask { bounds };
        if fits || cx.reduce_motion() {
            window.with_content_mask(Some(mask), |window| {
                // A failed paint leaves this element blank for one frame.
                line.paint(
                    bounds.origin,
                    line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            });
            return;
        }
        let clock = window.use_keyed_state(self.id.clone(), cx, |_, _| Clock {
            text: self.text.clone(),
            started: Instant::now(),
        });
        let started = clock.update(cx, |clock, _| {
            if clock.text != self.text {
                clock.text = self.text.clone();
                clock.started = Instant::now();
            }
            clock.started
        });
        let cycle = width + GAP;
        window.paint_animated(
            move |window, cx| {
                let moving = started.elapsed().saturating_sub(REST).as_secs_f32();
                let offset = (moving * SPEED).rem_euclid(cycle);
                window.with_content_mask(Some(mask), |window| {
                    for copy in [0.0, cycle] {
                        let x = bounds.origin.x + px(copy - offset);
                        // A failed paint leaves this element blank for one frame.
                        line.paint(
                            point(x, bounds.origin.y),
                            line_height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        )
                        .ok();
                    }
                });
                !cx.reduce_motion()
            },
            cx,
        );
    }
}
