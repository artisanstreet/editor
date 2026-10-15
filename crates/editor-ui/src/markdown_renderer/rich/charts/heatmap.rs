//! `heatmap`: a grid of values colored on a sequential or diverging ramp,
//! with a legend ramp and a hover readout of the cell under the pointer.

use crate::markdown_renderer::rich::budget;
use std::sync::Arc;

use gpui::{
    AnyElement, App, ElementId, IntoElement, MouseMoveEvent, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window, div, prelude::*, px,
};
use serde::Deserialize;

use super::super::{BlockContext, RichBlock, kit};
use super::frame::{self, ReadoutRow, Tick};
use super::scale::{Domain, f32_of, f64_of, index_of, reading};
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
#[derive(Debug, Deserialize)]
struct HeatmapSpec {
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

#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "HeatmapSpec")]
pub(super) struct Heatmap(Arc<HeatmapData>);
#[derive(Debug, PartialEq)]
pub(super) struct HeatmapData {
    x: Vec<String>,
    y: Vec<String>,
    values: Vec<Vec<Option<f64>>>,
    unit: Option<String>,
    scale: Scale,
    domain: Domain,
    reach: f64,
    rows: usize,
    columns: usize,
    has_data: bool,
}
impl std::ops::Deref for Heatmap {
    type Target = HeatmapData;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl TryFrom<HeatmapSpec> for Heatmap {
    type Error = String;
    fn try_from(spec: HeatmapSpec) -> Result<Self, String> {
        let finite = || {
            spec.values
                .iter()
                .flatten()
                .filter_map(|value| *value)
                .filter(|value| value.is_finite())
        };
        let rows = spec.values.len().max(spec.y.len());
        let columns = spec
            .values
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(spec.x.len());
        let has_data = finite().next().is_some();
        let domain = Domain::of(finite()).unwrap_or_else(|| Domain::new(0.0, 1.0));
        let reach = finite().map(f64::abs).fold(0.0, f64::max);
        if !domain.lo.is_finite() || !domain.hi.is_finite() || !domain.span().is_finite() {
            return Err("heatmap range exceeds the finite preview domain".to_owned());
        }
        Ok(Self(Arc::new(HeatmapData {
            x: spec.x,
            y: spec.y,
            values: spec.values,
            unit: spec.unit,
            scale: spec.scale,
            domain,
            reach,
            rows,
            columns,
            has_data,
        })))
    }
}

impl Heatmap {
    pub(super) fn rows(&self) -> usize {
        self.0.rows
    }
    pub(super) fn columns(&self) -> usize {
        self.0.columns
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

    /// The color position of `value`: 0..=1 sequential, -1..=1 diverging.
    pub(super) fn position(&self, value: f64) -> f32 {
        match self.scale {
            Scale::Sequential => self.domain.fraction(value).clamp(0.0, 1.0),
            Scale::Diverging => {
                let reach = self.reach;
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
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<HeatmapData>()
            + budget::strings(&self.x)
            + budget::strings(&self.y)
            + budget::vector(&self.values)
            + self.values.iter().map(budget::vector).sum::<usize>()
            + budget::optional(&self.unit)
    }

    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        HeatmapView {
            id: cx.key("heatmap"),
            theme: *cx.theme,
            spec: self.clone(),
        }
        .into_any_element()
    }
}

#[derive(IntoElement)]
struct HeatmapView {
    id: SharedString,
    theme: ArtisanTheme,
    spec: Heatmap,
}

struct HeatmapState {
    model: Arc<HeatmapData>,
    hover: Option<(usize, usize)>,
    probe: Probe,
    rows: gpui::UniformListScrollHandle,
}

impl RenderOnce for HeatmapView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let spec = &self.spec;
        let (rows, columns) = (spec.rows(), spec.columns());
        if rows == 0 || columns == 0 || !spec.has_data {
            return frame::empty(&theme, "No data to map");
        }
        let state = kit::keyed(window, cx, ElementId::Name(self.id.clone()), || {
            let list = gpui::UniformListScrollHandle::new();
            HeatmapState {
                model: spec.0.clone(),
                hover: None,
                probe: probe(),
                rows: list,
            }
        });
        state.update(cx, |state, _| {
            if !Arc::ptr_eq(&state.model, &spec.0) {
                let changed = *state.model != *spec.0;
                state.model = spec.0.clone();
                if changed {
                    state.hover = None;
                    state.rows = gpui::UniformListScrollHandle::new();
                }
            }
        });
        let (hover, plot_probe) = {
            let current = state.read(cx);
            (current.hover, current.probe.clone())
        };
        let column_count = f64_of(columns);
        let center = |index: usize, count: f64| f32_of((f64_of(index) + 0.5) / count);
        let width = probed_width(&plot_probe, 480.0);
        let x_ticks = frame::thinned(columns, frame::label_budget(width, 64.0))
            .map(|column| Tick {
                at: center(column, column_count),
                label: spec.x_label(column).into(),
            })
            .collect();
        let mut selection_label = None;
        let readout = hover.map(|(row, column)| {
            let value = spec.cell(row, column).map_or_else(
                || "–".to_owned(),
                |value| reading(value, spec.unit.as_deref()),
            );
            selection_label = Some(format!(
                "{} · {}: {}",
                spec.y_label(row),
                spec.x_label(column),
                value
            ));
            frame::readout(
                &theme,
                center(column, column_count),
                Some(format!("{} · {}", spec.y_label(row), spec.x_label(column)).into()),
                vec![ReadoutRow::new(None, "Value", value)],
            )
        });
        let list_state = state.read(cx).rows.clone();
        let row_model = spec.clone();
        let row_state = state.clone();
        let row_id = self.id.clone();
        let plot = gpui::uniform_list(kit::part(&self.id, "rows"), rows, move |range, _, _| {
            range
                .map(|row| {
                    let row_probe = probe();
                    let moving_probe = row_probe.clone();
                    let moving = row_state.clone();
                    let id = row_id.clone();
                    div()
                        .id(kit::part(&row_id, format_args!("row-{row}")))
                        .debug_selector(move || format!("{id}-row-{row}"))
                        .w_full()
                        .h(theme.density.control_sm)
                        .flex()
                        .child(
                            div()
                                .w(px(96.0))
                                .min_w_0()
                                .pr(px(8.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(theme.typography.control_text)
                                .text_color(theme.colors.muted_foreground.to_paint())
                                .child(row_model.y_label(row)),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .child(plot_canvas(
                                    row_shapes(&row_model, &theme, row, hover),
                                    row_probe,
                                ))
                                .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                                    let selected = fractions(&moving_probe, event.position)
                                        .and_then(|(x, y)| {
                                            ((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y))
                                                .then(|| {
                                                    (
                                                        row,
                                                        index_of(f64::from(x) * column_count)
                                                            .min(columns - 1),
                                                    )
                                                })
                                        });
                                    moving.update(cx, |state, cx| {
                                        if state.hover != selected {
                                            state.hover = selected;
                                            cx.notify();
                                        }
                                    });
                                }),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&list_state)
        .w_full()
        .h(theme.density.control_sm * rows.min(10) as f32);
        let plot = div()
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .child(plot)
            .child(
                div()
                    .pl(px(96.0))
                    .child(frame::x_tick_strip(&theme, x_ticks)),
            )
            .children(readout)
            .child(
                div()
                    .absolute()
                    .left(px(96.0))
                    .right_0()
                    .top_0()
                    .h(px(1.0))
                    .child(plot_canvas(Vec::new(), plot_probe)),
            );
        let keyboard = state.clone();
        let mut navigation = crate::chart_navigation::ChartNavigation::new(
            ElementId::Name(format!("{}-navigation", self.id).into()), theme,
            "Heatmap. Arrows explore cells; Home and End select endpoints; Escape clears the reading.",
            0..rows.saturating_mul(columns), plot,
            move |command, _, cx| keyboard.update(cx, |state, cx| {
                use crate::chart_navigation::ChartCommand;
                state.hover = match command {
                    ChartCommand::Select(index) | ChartCommand::Activate(index) => {
                        list_state.scroll_to_item(index / columns, gpui::ScrollStrategy::Nearest);
                        Some((index / columns, index % columns))
                    },
                    ChartCommand::Parent | ChartCommand::Reset => None,
                };
                cx.notify();
            }),
        ).selected(hover.map(|(row, column)| row * columns + column)).content_height();
        if let Some(label) = selection_label {
            navigation = navigation.selection_label(label);
        }
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(navigation)
            .child(self.ramp(96.0))
            .into_any_element()
    }
}

impl HeatmapView {
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
                let reach = spec.reach;
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

fn row_shapes(
    spec: &Heatmap,
    theme: &ArtisanTheme,
    row: usize,
    hover: Option<(usize, usize)>,
) -> Vec<Shape> {
    let columns = spec.columns() as f64;
    let mut shapes = Vec::new();
    if let Some(cells) = spec.values.get(row) {
        for column in 0..cells.len() {
            let Some(value) = spec.cell(row, column) else {
                continue;
            };
            let (x0, x1) = (
                f32_of(column as f64 / columns),
                f32_of((column + 1) as f64 / columns),
            );
            let inset = (x1 - x0) * 0.05;
            shapes.push(Shape::Rect {
                x0: x0 + inset,
                x1: x1 - inset,
                y0: 0.05,
                y1: 0.95,
                color: spec.color(theme, value),
            });
        }
    }
    if let Some((_, column)) = hover.filter(|(selected, _)| *selected == row) {
        let (x0, x1) = (
            f32_of(column as f64 / columns),
            f32_of((column + 1) as f64 / columns),
        );
        shapes.push(Shape::Line {
            points: vec![(x0, 0.0), (x1, 0.0), (x1, 1.0), (x0, 1.0), (x0, 0.0)],
            width: 2.0,
            color: theme.colors.foreground.to_paint(),
            dash: false,
        });
    }
    shapes
}

#[cfg(test)]
mod native_window_tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext};
    struct Fixture {
        spec: Heatmap,
        focus: gpui::FocusHandle,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(640.0))
                .track_focus(&self.focus)
                .tab_stop(false)
                .on_action(crate::focus_navigation::focus_next)
                .on_action(crate::focus_navigation::focus_previous)
                .child(HeatmapView {
                    id: "native-heatmap".into(),
                    theme: ArtisanTheme::default(),
                    spec: self.spec.clone(),
                })
        }
    }
    #[gpui::test]
    fn offscreen_heatmap_rows_are_not_constructed_and_end_reveals_last(cx: &mut TestAppContext) {
        cx.update(crate::focus_navigation::bind_actions);
        let (view, cx) = cx.add_window_view(|window, cx| Fixture {
            focus: crate::focus_navigation::fixture_root_focus(window, cx),
            spec: Heatmap::try_from(HeatmapSpec {
                x: vec!["first".into(), "last".into()],
                y: Vec::new(),
                values: (0..1000)
                    .map(|row| vec![Some(row as f64), Some(1.0)])
                    .collect(),
                unit: None,
                scale: Scale::Sequential,
            })
            .unwrap(),
        });
        assert_eq!(
            cx.debug_bounds("native-heatmap-row-0").unwrap().size.height,
            ArtisanTheme::default().density.control_sm
        );
        assert!(cx.debug_bounds("native-heatmap-row-50").is_none());
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        cx.simulate_keystrokes("end");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            cx.debug_bounds("native-heatmap-row-999")
                .unwrap()
                .size
                .height,
            ArtisanTheme::default().density.control_sm
        );
        // Parsing an unchanged source can produce a new Arc. It must preserve
        // the selected endpoint and the native list's viewport.
        cx.update(|_, cx| {
            view.update(cx, |fixture, cx| {
                let data = &fixture.spec;
                fixture.spec = Heatmap::try_from(HeatmapSpec {
                    x: data.x.clone(),
                    y: data.y.clone(),
                    values: data.values.clone(),
                    unit: data.unit.clone(),
                    scale: data.scale,
                })
                .unwrap();
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            cx.debug_bounds("native-heatmap-row-999")
                .unwrap()
                .size
                .height,
            ArtisanTheme::default().density.control_sm
        );
        cx.simulate_keystrokes("home");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            cx.debug_bounds("native-heatmap-row-0").unwrap().size.height,
            ArtisanTheme::default().density.control_sm
        );
        assert!(cx.debug_bounds("native-heatmap-row-999").is_none());
    }
    #[test]
    fn a_visible_row_only_builds_its_own_populated_cells() {
        let map = Heatmap::try_from(HeatmapSpec {
            x: Vec::new(),
            y: Vec::new(),
            values: vec![vec![Some(1.0), None], vec![Some(2.0), Some(3.0)]],
            unit: None,
            scale: Scale::Sequential,
        })
        .unwrap();
        assert_eq!(row_shapes(&map, &ArtisanTheme::default(), 0, None).len(), 1);
        assert_eq!(row_shapes(&map, &ArtisanTheme::default(), 1, None).len(), 2);
    }
}
