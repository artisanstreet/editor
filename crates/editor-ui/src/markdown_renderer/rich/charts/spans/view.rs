//! Drawing a span [`Layout`]: rows of bars over a time axis, row names on
//! the left, markers as labelled rules, and a glass readout on hover.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Entity, FontWeight, IntoElement, ParentElement as _, RenderOnce, SharedString,
    Styled as _, Window, div, prelude::*, px, relative,
};

use super::super::super::kit;
use super::super::frame::{self, Frame, PlotLabel, ReadoutRow, Tick};
use super::super::scale::{Domain, f32_of, f64_of, nice_ticks};
use super::super::scene::{Probe, Shape, hrule, plot_canvas, probe, probed_width, vrule};
use super::{Layout, Span};
use crate::hover_card::hover_highlight_shadow;
use crate::theme::ArtisanTheme;

/// Pitch of one row and the height of a bar in it.
const ROW_PITCH: f32 = 24.0;
const BAR_HEIGHT: f32 = 18.0;

/// Room above the first row for marker labels.
const MARKER_ROOM: f32 = 20.0;

/// Width of the row names.
const NAME_WIDTH: f32 = 132.0;

/// A bar narrower than this hides its text.
const MIN_TEXT_WIDTH: f32 = 44.0;

/// Plot width assumed before the first paint.
const FALLBACK_WIDTH: f32 = 560.0;

#[derive(IntoElement)]
pub(super) struct SpansView {
    pub(super) id: SharedString,
    pub(super) theme: ArtisanTheme,
    pub(super) layout: Rc<Layout>,
}

/// Where things land in the plot.
#[derive(Clone, Copy)]
struct Placement {
    domain: Domain,
    width: f32,
    /// Room above the first row for marker labels.
    marker_room: f32,
    height: f32,
}

impl Placement {
    /// The top of `row` as a fraction of the plot height.
    fn row_at(self, row: f32) -> f32 {
        (self.marker_room + row * ROW_PITCH) / self.height
    }
}

impl SpansView {
    /// The grid, lane dividers and marker rules, with the markers' labels.
    fn guides(&self, place: Placement, x_ticks: &[Tick]) -> (Vec<Shape>, Vec<PlotLabel>) {
        let theme = &self.theme;
        let mut shapes = frame::grid(theme, &[], x_ticks);
        for lane in self.layout.lanes.iter().skip(1) {
            shapes.push(hrule(
                place.row_at(f32_of(f64_of(lane.first_row))),
                kit::hairline(theme),
                false,
            ));
        }
        let mut labels = Vec::new();
        for marker in &self.layout.markers {
            let x = place.domain.fraction(marker.at);
            shapes.push(vrule(
                x,
                theme.colors.foreground.with_alpha(0.55).to_paint(),
                true,
            ));
            if !marker.label.is_empty() {
                labels.push(PlotLabel {
                    x,
                    y: 0.0,
                    below: true,
                    text: marker.label.clone(),
                    color: theme.colors.foreground.to_paint(),
                });
            }
        }
        (shapes, labels)
    }
}

struct SpansState {
    hover: Option<usize>,
    probe: Probe,
}

impl RenderOnce for SpansView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let layout = &self.layout;
        if layout.spans.is_empty() {
            return frame::empty(&theme, "No spans");
        }
        let state = kit::keyed(window, cx, kit::part(&self.id, "state"), || SpansState {
            hover: None,
            probe: probe(),
        });
        let (hover, plot_probe) = {
            let current = state.read(cx);
            (current.hover, current.probe.clone())
        };
        let width = probed_width(&plot_probe, FALLBACK_WIDTH);
        let ticks = nice_ticks(
            layout.domain,
            frame::label_budget(width, 96.0),
            false,
            false,
        );
        let domain = ticks.domain;
        let x_ticks: Vec<Tick> = ticks
            .values
            .iter()
            .map(|value| Tick {
                at: domain.fraction(*value),
                label: SharedString::from(layout.time(*value)),
            })
            .collect();
        let marker_room = if layout.markers.is_empty() {
            0.0
        } else {
            MARKER_ROOM
        };
        let rows_height = f32_of(f64_of(layout.rows.max(1))) * ROW_PITCH;
        let height = rows_height + marker_room;
        let place = Placement {
            domain,
            width,
            marker_room,
            height,
        };
        let y_ticks = self.row_names(place);

        let (shapes, labels) = self.guides(place, &x_ticks);

        let bars = layout
            .spans
            .iter()
            .enumerate()
            .map(|(index, span)| self.bar(&state, index, span, hover == Some(index), place));
        let readout = hover.and_then(|index| layout.spans.get(index)).map(|span| {
            let x = f32::midpoint(domain.fraction(span.start), domain.fraction(span.end));
            let high = place.row_at(f32_of(f64_of(span.row)) + 0.5) < 0.5;
            self.readout(span, x, high)
        });

        let plot = div()
            .id(kit::part(&self.id, "plot"))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(plot_canvas(shapes, plot_probe))
            .children(bars)
            .children(
                labels
                    .into_iter()
                    .map(|label| frame::plot_label(&theme, label)),
            )
            .children(readout);
        Frame {
            end_labels: Vec::new(),
            y_ticks,
            x_ticks,
            y_title: None,
            x_title: None,
            y_axis_width: NAME_WIDTH,
            height,
        }
        .wrap(&theme, plot)
        .into_any_element()
    }
}

impl SpansView {
    /// Lane names at the middle of their rows, else every span's label.
    fn row_names(&self, place: Placement) -> Vec<Tick> {
        let layout = &self.layout;
        if layout.lanes.is_empty() {
            return layout
                .spans
                .iter()
                .map(|span| Tick {
                    at: place.row_at(f32_of(f64_of(span.row)) + 0.5),
                    label: span.label.clone(),
                })
                .collect();
        }
        layout
            .lanes
            .iter()
            .map(|lane| Tick {
                at: place.row_at(f32_of(f64_of(lane.first_row)) + 0.5),
                label: lane.name.clone(),
            })
            .collect()
    }

    fn bar(
        &self,
        state: &Entity<SpansState>,
        index: usize,
        span: &Span,
        hovered: bool,
        place: Placement,
    ) -> AnyElement {
        let theme = self.theme;
        let layout = &self.layout;
        let x0 = place.domain.fraction(span.start);
        let x1 = place.domain.fraction(span.end);
        let wide = (x1 - x0) * place.width >= MIN_TEXT_WIDTH;
        // Laned rows name the lane, so bars carry their own label; a
        // waterfall names each row by its span, so bars carry the length.
        let text = if layout.lanes.is_empty() {
            SharedString::from(layout.duration(span))
        } else {
            span.label.clone()
        };
        let color = kit::series_color(&theme, span.color);
        let hovering = state.clone();
        div()
            .id(kit::part(&self.id, format_args!("span-{index}")))
            .absolute()
            .left(relative(x0))
            .w(relative(x1 - x0))
            .min_w(px(2.0))
            .top(relative(place.row_at(f32_of(f64_of(span.row)))))
            .mt(px((ROW_PITCH - BAR_HEIGHT) / 2.0))
            .h(px(BAR_HEIGHT))
            .flex()
            .items_center()
            .px(px(6.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .rounded(px(4.0))
            .bg(color
                .with_alpha(if hovered { 0.75 } else { 0.55 })
                .to_paint())
            .when(hovered, |bar| bar.shadow(hover_highlight_shadow(&theme)))
            .text_size(px(11.0))
            .line_height(px(14.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.colors.foreground.to_paint())
            .when(wide, |bar| bar.child(text))
            .on_hover(move |entered, _, cx| {
                hovering.update(cx, |state, cx| {
                    let next = if *entered {
                        Some(index)
                    } else if state.hover == Some(index) {
                        None
                    } else {
                        return;
                    };
                    if state.hover != next {
                        state.hover = next;
                        cx.notify();
                    }
                });
            })
            .into_any_element()
    }

    fn readout(&self, span: &Span, x: f32, high: bool) -> gpui::Div {
        let layout = &self.layout;
        let theme = &self.theme;
        let mut rows = vec![
            ReadoutRow::new(
                Some(kit::series_color(theme, span.color).to_paint()),
                "Length",
                layout.duration(span),
            ),
            ReadoutRow::new(None, "Start", layout.time(span.start)),
            ReadoutRow::new(None, "End", layout.time(span.end)),
        ];
        if let Some(lane) = span.lane.and_then(|lane| layout.lanes.get(lane)) {
            rows.push(ReadoutRow::new(None, "Lane", lane.name.clone()));
        }
        if let Some(group) = &span.group {
            rows.push(ReadoutRow::new(None, "Group", group.clone()));
        }
        let heading = (!span.label.is_empty()).then(|| span.label.clone());
        if high {
            frame::readout_low(theme, x, heading, rows)
        } else {
            frame::readout(theme, x, heading, rows)
        }
    }
}
