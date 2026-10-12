//! `heatmap`: a grid of values colored on a sequential or diverging ramp,
//! with a legend ramp and a hover readout of the cell under the pointer.

use std::fmt::Write as _;

use gpui::{
    AnyElement, App, ElementId, IntoElement, MouseMoveEvent, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window, div, prelude::*, px,
};
use serde::Deserialize;

use super::super::{BlockContext, RichBlock, kit};
use super::frame::{self, Frame, ReadoutRow, Tick};
use super::scale::{Domain, exact, f32_of, f64_of, index_of, reading};
use super::scene::{Probe, Shape, fractions, plot_canvas, probe, probed_width};
use crate::theme::ArtisanTheme;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Scale {
    #[default]
    Sequential,
    Diverging,
}

/// The `heatmap` spec. Rows follow `y`; columns follow `x`.
#[derive(Clone, Debug, Deserialize)]
pub(super) struct Heatmap {
    #[serde(default)]
    x: Vec<String>,
    #[serde(default)]
    y: Vec<String>,
    values: Vec<Vec<Option<f64>>>,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    scale: Scale,
}

impl Heatmap {
    pub(super) fn rows(&self) -> usize {
        self.values.len().max(self.y.len())
    }

    pub(super) fn columns(&self) -> usize {
        self.values
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(self.x.len())
    }

    /// The finite value at `row`, `column`.
    pub(super) fn cell(&self, row: usize, column: usize) -> Option<f64> {
        self.values
            .get(row)
            .and_then(|cells| cells.get(column).copied().flatten())
            .filter(|value| value.is_finite())
    }

    fn label(labels: &[String], index: usize) -> String {
        labels
            .get(index)
            .cloned()
            .unwrap_or_else(|| (index + 1).to_string())
    }

    fn x_label(&self, column: usize) -> String {
        Self::label(&self.x, column)
    }

    fn y_label(&self, row: usize) -> String {
        Self::label(&self.y, row)
    }

    fn finite(&self) -> impl Iterator<Item = f64> + '_ {
        self.values
            .iter()
            .flatten()
            .filter_map(|value| *value)
            .filter(|value| value.is_finite())
    }

    /// The color position of `value`: 0..=1 sequential, -1..=1 diverging.
    pub(super) fn position(&self, value: f64) -> f32 {
        match self.scale {
            Scale::Sequential => {
                let domain = Domain::of(self.finite()).unwrap_or_else(|| Domain::new(0.0, 1.0));
                domain.fraction(value).clamp(0.0, 1.0)
            }
            Scale::Diverging => {
                let reach = self.finite().map(f64::abs).fold(0.0, f64::max);
                if reach > 0.0 {
                    f32_of(value / reach).clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            }
        }
    }

    fn color(&self, theme: &ArtisanTheme, value: f64) -> gpui::Hsla {
        let t = self.position(value);
        match self.scale {
            Scale::Sequential => kit::sequential_color(theme, t),
            Scale::Diverging => kit::diverging_color(theme, t),
        }
        .to_paint()
    }
}

impl RichBlock for Heatmap {
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        HeatmapView {
            id: cx.key("heatmap"),
            theme: *cx.theme,
            spec: self.clone(),
        }
        .into_any_element()
    }

    fn markdown(&self) -> String {
        let (rows, columns) = (self.rows(), self.columns());
        if rows == 0 || columns == 0 {
            return "_No data._\n".to_owned();
        }
        let mut header = vec![String::new()];
        header.extend((0..columns).map(|column| self.x_label(column)));
        let body: Vec<Vec<String>> = (0..rows)
            .map(|row| {
                let mut cells = vec![self.y_label(row)];
                cells.extend(
                    (0..columns)
                        .map(|column| self.cell(row, column).map(exact).unwrap_or_default()),
                );
                cells
            })
            .collect();
        let mut out = kit::markdown_table(&header, &body);
        if let Some(unit) = self
            .unit
            .as_deref()
            .map(str::trim)
            .filter(|unit| !unit.is_empty())
        {
            let _ = write!(out, "\n_Values in {unit}_\n");
        }
        out
    }
}

#[derive(IntoElement)]
struct HeatmapView {
    id: SharedString,
    theme: ArtisanTheme,
    spec: Heatmap,
}

struct HeatmapState {
    hover: Option<(usize, usize)>,
    probe: Probe,
}

impl RenderOnce for HeatmapView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let spec = &self.spec;
        let (rows, columns) = (spec.rows(), spec.columns());
        if rows == 0 || columns == 0 || spec.finite().next().is_none() {
            return frame::empty(&theme, "No data to map");
        }
        let state = kit::keyed(window, cx, ElementId::Name(self.id.clone()), || {
            HeatmapState {
                hover: None,
                probe: probe(),
            }
        });
        let (hover, plot_probe) = {
            let current = state.read(cx);
            (current.hover, current.probe.clone())
        };
        let (row_count, column_count) = (f64_of(rows), f64_of(columns));
        let shapes = self.shapes(hover);
        let center = |index: usize, count: f64| f32_of((f64_of(index) + 0.5) / count);
        let width = probed_width(&plot_probe, 480.0);
        let x_ticks = frame::thinned(columns, frame::label_budget(width, 64.0))
            .map(|column| Tick {
                at: center(column, column_count),
                label: spec.x_label(column).into(),
            })
            .collect();
        // Square cells: each row as tall as a column is wide, within reason.
        let row_height = (width / f32_of(column_count)).clamp(12.0, 36.0);
        let height = row_height * f32_of(row_count);
        let y_ticks = frame::thinned(rows, index_of(f64::from(height / 14.0)).max(1))
            .map(|row| Tick {
                at: center(row, row_count),
                label: spec.y_label(row).into(),
            })
            .collect();
        let readout = hover.map(|(row, column)| {
            let value = spec.cell(row, column).map_or_else(
                || "–".to_owned(),
                |value| reading(value, spec.unit.as_deref()),
            );
            frame::readout(
                &theme,
                center(column, column_count),
                Some(format!("{} · {}", spec.y_label(row), spec.x_label(column)).into()),
                vec![ReadoutRow::new(None, "Value", value)],
            )
        });
        let plot = self.plot(&state, shapes, readout, plot_probe);
        let longest = (0..rows)
            .map(|row| spec.y_label(row).chars().count())
            .max()
            .unwrap_or(0);
        let axis_width = (f32_of(f64_of(longest)) * 7.0 + 12.0).clamp(28.0, 96.0);
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                Frame {
                    end_labels: Vec::new(),
                    y_ticks,
                    x_ticks,
                    y_title: None,
                    x_title: None,
                    y_axis_width: axis_width,
                    height,
                }
                .wrap(&theme, plot),
            )
            .child(self.ramp(axis_width))
            .into_any_element()
    }
}

impl HeatmapView {
    /// The cells, and an outline around the hovered one.
    fn shapes(&self, hover: Option<(usize, usize)>) -> Vec<Shape> {
        let spec = &self.spec;
        let (rows, columns) = (spec.rows(), spec.columns());
        let (row_count, column_count) = (f64_of(rows), f64_of(columns));
        let fraction = |index: usize, count: f64| f32_of(f64_of(index) / count);
        let mut shapes = Vec::new();
        for row in 0..rows {
            for column in 0..columns {
                let Some(value) = spec.cell(row, column) else {
                    continue;
                };
                let (x0, x1) = (
                    fraction(column, column_count),
                    fraction(column + 1, column_count),
                );
                let (y0, y1) = (fraction(row, row_count), fraction(row + 1, row_count));
                // Cells are square, so equal fractions leave an even ~2 px gap.
                let (inset_x, inset_y) = ((x1 - x0) * 0.05, (y1 - y0) * 0.05);
                shapes.push(Shape::Rect {
                    x0: x0 + inset_x,
                    y0: y0 + inset_y,
                    x1: x1 - inset_x,
                    y1: y1 - inset_y,
                    color: spec.color(&self.theme, value),
                });
            }
        }
        if let Some((row, column)) = hover {
            let (x0, x1) = (
                fraction(column, column_count),
                fraction(column + 1, column_count),
            );
            let (y0, y1) = (fraction(row, row_count), fraction(row + 1, row_count));
            shapes.push(Shape::Line {
                points: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)],
                width: 2.0,
                color: self.theme.colors.foreground.to_paint(),
                dash: false,
            });
        }
        shapes
    }

    /// The grid surface: cells, readout, and hover tracking.
    fn plot(
        &self,
        state: &gpui::Entity<HeatmapState>,
        shapes: Vec<Shape>,
        readout: Option<gpui::Div>,
        plot_probe: Probe,
    ) -> AnyElement {
        let (rows, columns) = (self.spec.rows(), self.spec.columns());
        let (row_count, column_count) = (f64_of(rows), f64_of(columns));
        let moving = state.clone();
        let leaving = state.clone();
        let move_probe = plot_probe.clone();
        div()
            .id(ElementId::Name(format!("{}-grid", self.id).into()))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(plot_canvas(shapes, plot_probe))
            .children(readout)
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                let hover = fractions(&move_probe, event.position).and_then(|(fx, fy)| {
                    let inside = (0.0..1.0).contains(&fx) && (0.0..1.0).contains(&fy);
                    inside.then(|| {
                        let row = index_of(f64::from(fy) * row_count).min(rows - 1);
                        let column = index_of(f64::from(fx) * column_count).min(columns - 1);
                        (row, column)
                    })
                });
                moving.update(cx, |state, cx| {
                    if state.hover != hover {
                        state.hover = hover;
                        cx.notify();
                    }
                });
            })
            .on_hover(move |hovered, _, cx| {
                if !*hovered {
                    leaving.update(cx, |state, cx| {
                        if state.hover.take().is_some() {
                            cx.notify();
                        }
                    });
                }
            })
            .into_any_element()
    }

    /// The color ramp with its end values (and zero, when diverging).
    fn ramp(&self, indent: f32) -> gpui::Div {
        let theme = &self.theme;
        let spec = &self.spec;
        let unit = spec.unit.as_deref();
        let stop_count = 9;
        let (stops, ends): (Vec<gpui::Hsla>, [String; 2]) = match spec.scale {
            // A sequential ramp reads as amount, like a contribution calendar;
            // the exact values are on hover.
            Scale::Sequential => (
                (0..stop_count)
                    .map(|step| {
                        kit::sequential_color(theme, f32_of(f64_of(step) / f64_of(stop_count - 1)))
                            .to_paint()
                    })
                    .collect(),
                ["fewer".to_owned(), "more".to_owned()],
            ),
            Scale::Diverging => {
                let reach = spec.finite().map(f64::abs).fold(0.0, f64::max);
                (
                    (0..stop_count)
                        .map(|step| {
                            let t = f32_of(f64_of(step) / f64_of(stop_count - 1)) * 2.0 - 1.0;
                            kit::diverging_color(theme, t).to_paint()
                        })
                        .collect(),
                    [reading(-reach, unit), reading(reach, unit)],
                )
            }
        };
        let [low, high] = ends;
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .pl(px(indent))
            .child(kit::label(theme, low))
            .child(
                div().flex().gap(px(2.0)).children(
                    stops
                        .into_iter()
                        .map(|color| div().size(px(10.0)).rounded(px(2.0)).bg(color)),
                ),
            )
            .child(kit::label(theme, high))
    }
}
