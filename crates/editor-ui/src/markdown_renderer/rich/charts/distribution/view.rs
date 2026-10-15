//! The distribution widget: one plot per mode, the hover readout, and the
//! threshold slider.

use std::sync::Arc;

use gpui::{
    AnyElement, App, Div, ElementId, Entity, Hsla, IntoElement, MouseMoveEvent, ParentElement as _,
    RenderOnce, SharedString, Styled as _, Window, div, prelude::*, px,
};

use super::super::super::kit;
use super::super::frame::{self, Frame, LegendEntry, PlotLabel, ReadoutRow, Tick};
use super::super::scale::{Domain, f64_of, index_of, nice_ticks, percent, reading, ticks_within};
use super::super::scene::{
    Probe, Shape, fractions, hover_band, plot_canvas, probe, probed_width, vrule,
};
use super::{Distribution, Mode, share_at_or_below};
use crate::slider::{Slider, SliderRange, SliderState};
use crate::theme::ArtisanTheme;

#[derive(IntoElement)]
pub(super) struct DistributionView {
    pub(super) id: SharedString,
    pub(super) theme: ArtisanTheme,
    pub(super) data: Arc<Distribution>,
}

struct DistributionState {
    model: Arc<Distribution>,
    /// The pointer over the plot, in plot fractions.
    hover: Option<(f32, f32)>,
    probe: Probe,
    boxes: gpui::UniformListScrollHandle,
}

/// One frame of the plot.
struct Layout {
    shapes: Vec<Shape>,
    x_ticks: Vec<Tick>,
    y_ticks: Vec<Tick>,
    y_axis_width: f32,
    height: f32,
    readout: Option<Div>,
    selection_label: Option<SharedString>,
    labels: Vec<PlotLabel>,
}

impl RenderOnce for DistributionView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        if self.data.is_empty() {
            return frame::empty(&theme, "No values to plot");
        }
        let state = kit::keyed(window, cx, ElementId::Name(self.id.clone()), || {
            let boxes = gpui::UniformListScrollHandle::new();
            DistributionState {
                model: self.data.clone(),
                hover: None,
                probe: probe(),
                boxes,
            }
        });
        let replaced = state.update(cx, |state, _| {
            if Arc::ptr_eq(&state.model, &self.data) {
                false
            } else {
                let changed = *state.model != *self.data;
                state.model = self.data.clone();
                if changed {
                    state.hover = None;
                    state.boxes = gpui::UniformListScrollHandle::new();
                }
                changed
            }
        });
        let with_slider = self.data.mode != Mode::Box;
        let slider_state = with_slider.then(|| {
            let (range, step, start) = self.data.slider();
            SliderState::keyed(
                window,
                cx,
                format!("{}-threshold", self.id),
                SliderRange::new(range.lo, range.hi, Some(step)),
                start,
            )
        });
        if replaced {
            if let Some(slider) = &slider_state {
                let (range, step, start) = self.data.slider();
                slider.update(cx, |state, _| {
                    state.reset(SliderRange::new(range.lo, range.hi, Some(step)), start)
                });
            }
        }
        let (hover, plot_probe) = {
            let current = state.read(cx);
            (current.hover, current.probe.clone())
        };
        let threshold = slider_state.as_ref().map(|slider| slider.read(cx).value());
        let width = probed_width(&plot_probe, 480.0);
        let mut layout = match self.data.mode {
            Mode::Histogram => self.histogram(hover, width),
            Mode::Cdf => self.cdf(hover, width),
            Mode::Box => return self.box_window(state, hover, plot_probe, cx),
        };
        if let Some(threshold) = threshold {
            self.threshold_rule(threshold, &mut layout);
        }
        let plot = self.plot(&mut layout, &state, plot_probe);
        let mode = self.data.mode;
        let count = match mode {
            Mode::Histogram => self.data.bins.count,
            Mode::Cdf => 101,
            Mode::Box => self.data.samples.len(),
        };
        let selected = hover.map(|(x, y)| match mode {
            Mode::Box => ((y * count as f32) as usize).min(count.saturating_sub(1)),
            Mode::Histogram => ((x * count as f32) as usize).min(count.saturating_sub(1)),
            Mode::Cdf => (x * 100.0).round() as usize,
        });
        let keyboard = state.clone();
        let mut navigation = crate::chart_navigation::ChartNavigation::new(
            ElementId::Name(format!("{}-navigation", self.id).into()),
            theme,
            "Distribution chart. Arrows explore readings; Escape clears the reading.",
            0..count,
            plot,
            move |command, _, cx| {
                keyboard.update(cx, |state, cx| {
                    use crate::chart_navigation::ChartCommand;
                    state.hover = match command {
                        ChartCommand::Select(index) | ChartCommand::Activate(index) => {
                            Some(match mode {
                                Mode::Box => (0.5, (index as f32 + 0.5) / count as f32),
                                Mode::Histogram => ((index as f32 + 0.5) / count as f32, 0.5),
                                Mode::Cdf => (index as f32 / 100.0, 0.5),
                            })
                        }
                        ChartCommand::Parent | ChartCommand::Reset => None,
                    };
                    cx.notify();
                })
            },
        )
        .selected(selected);
        if let Some(label) = layout.selection_label.clone() {
            navigation = navigation.selection_label(label);
        }
        let framed = Frame {
            end_labels: Vec::new(),
            y_ticks: layout.y_ticks,
            x_ticks: layout.x_ticks,
            y_title: (self.data.mode == Mode::Histogram).then(|| "Count".into()),
            x_title: self.data.unit.clone().map(SharedString::from),
            y_axis_width: layout.y_axis_width,
            height: layout.height,
        }
        .wrap(&theme, navigation);
        let legend = (self.data.samples.len() > 1 && self.data.mode != Mode::Box).then(|| {
            let entries = self
                .data
                .samples
                .iter()
                .enumerate()
                .map(|(index, sample)| LegendEntry {
                    name: if sample.sorted.is_empty() {
                        format!("{} (no data)", sample.name).into()
                    } else {
                        sample.name.clone().into()
                    },
                    color: kit::series_color(&theme, index).to_paint(),
                    hidden: false,
                })
                .collect();
            frame::legend(&theme, &self.id, entries, None)
        });
        let slider_row = slider_state
            .as_ref()
            .zip(threshold)
            .map(|(entity, threshold)| {
                Slider::new(
                    format!("{}-slider", self.id),
                    theme,
                    "Threshold",
                    self.shares(threshold),
                    entity.clone(),
                )
            });
        let root = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .children(legend)
            .child(framed)
            .children(slider_row);
        let sliders: Vec<Entity<SliderState>> = slider_state.into_iter().collect();
        Slider::follow_drag(root, &sliders).into_any_element()
    }
}

impl DistributionView {
    fn color(&self, index: usize) -> Hsla {
        kit::series_color(&self.theme, index).to_paint()
    }

    /// The share of each series at or below `threshold`.
    fn shares(&self, threshold: f64) -> String {
        let unit = self.data.unit.as_deref();
        let shares: Vec<String> = self
            .data
            .samples
            .iter()
            .map(|sample| {
                let share = if sample.sorted.is_empty() {
                    "no data".to_owned()
                } else {
                    percent(share_at_or_below(&sample.sorted, threshold))
                };
                if self.data.samples.len() > 1 {
                    format!("{} {share}", sample.name)
                } else {
                    share
                }
            })
            .collect();
        format!("≤ {} · {}", reading(threshold, unit), shares.join(" · "))
    }

    fn histogram(&self, hover: Option<(f32, f32)>, width: f32) -> Layout {
        let data = &self.data;
        let bins = data.bins;
        let counts: Vec<&[usize]> = data
            .samples
            .iter()
            .map(|sample| sample.counts.as_slice())
            .collect();
        let tallest = counts
            .iter()
            .flat_map(|counts| counts.iter())
            .copied()
            .max()
            .unwrap_or(0)
            .max(1);
        let y = nice_ticks(Domain::new(0.0, f64_of(tallest)), 4, true, false);
        let x = bins.domain;
        let y_ticks = frame::number_ticks(&y, None, true);
        let mut shapes = frame::grid(&self.theme, &y_ticks, &[]);
        let hovered = hover
            .filter(|(fx, _)| (0.0..=1.0).contains(fx))
            .map(|(fx, _)| bins.index(x.value_at(fx)));
        if let Some(bin) = hovered {
            let (lo, hi) = bins.edges(bin);
            shapes.push(hover_band(&self.theme, x.fraction(lo), x.fraction(hi)));
        }
        let alpha = if counts.len() > 1 { 0.5 } else { 0.85 };
        for (index, counts) in counts.iter().enumerate() {
            let color = Hsla {
                alpha,
                ..self.color(index)
            };
            for (bin, count) in counts.iter().enumerate().filter(|(_, count)| **count > 0) {
                let (lo, hi) = bins.edges(bin);
                let gap = bins.width() * 0.04;
                shapes.push(Shape::Rect {
                    x0: x.fraction(lo + gap),
                    y0: y.domain.fraction_down(f64_of(*count)),
                    x1: x.fraction(hi - gap),
                    y1: 1.0,
                    color,
                });
            }
        }
        let mut selection_label = None;
        let readout = hovered.map(|bin| {
            let (lo, hi) = bins.edges(bin);
            let rows: Vec<ReadoutRow> = data
                .samples
                .iter()
                .enumerate()
                .map(|(index, sample)| {
                    let count = counts[index][bin];
                    let share = percent(f64_of(count) / f64_of(sample.sorted.len().max(1)));
                    ReadoutRow::new(
                        Some(self.color(index)),
                        sample.name.clone(),
                        format!("{count} · {share}"),
                    )
                })
                .collect();
            let unit = data.unit.as_deref();
            let heading = format!("{} – {}", reading(lo, unit), reading(hi, unit));
            selection_label = Some(frame::reading_label(Some(&heading), &rows));
            frame::readout(
                &self.theme,
                x.fraction(f64::midpoint(lo, hi)),
                Some(heading.into()),
                rows,
            )
        });
        Layout {
            shapes,
            x_ticks: value_ticks(x, width),
            y_ticks,
            y_axis_width: frame::NUMBER_AXIS_WIDTH,
            height: frame::PLOT_HEIGHT,
            readout,
            selection_label,
            labels: Vec::new(),
        }
    }

    fn cdf(&self, hover: Option<(f32, f32)>, width: f32) -> Layout {
        let data = &self.data;
        let x = data.domain();
        let y = Domain::new(0.0, 1.0);
        let y_ticks: Vec<Tick> = [0.0, 0.25, 0.5, 0.75, 1.0]
            .into_iter()
            .map(|share| Tick {
                at: y.fraction_down(share),
                label: percent(share).into(),
            })
            .collect();
        let mut shapes = frame::grid(&self.theme, &y_ticks, &[]);
        let hovered = hover
            .filter(|(fx, _)| (0.0..=1.0).contains(fx))
            .map(|(fx, _)| x.value_at(fx));
        if let Some(at) = hovered {
            shapes.push(vrule(
                x.fraction(at),
                self.theme.colors.foreground.with_alpha(0.3).to_paint(),
                false,
            ));
        }
        for (index, sample) in data.samples.iter().enumerate() {
            if sample.sorted.is_empty() {
                continue;
            }
            let count = f64_of(sample.sorted.len());
            let mut points = vec![(0.0, 1.0)];
            let mut rank = 0;
            while rank < sample.sorted.len() {
                let value = sample.sorted[rank];
                let end = sample.sorted.partition_point(|sample| *sample <= value);
                let at = x.fraction(value);
                points.push((at, y.fraction_down(f64_of(rank) / count)));
                points.push((at, y.fraction_down(f64_of(end) / count)));
                rank = end;
            }
            points.push((1.0, y.fraction_down(1.0)));
            shapes.push(Shape::Line {
                points,
                width: 2.0,
                color: self.color(index),
                dash: false,
            });
        }
        let mut selection_label = None;
        let readout = hovered.map(|at| {
            let rows: Vec<ReadoutRow> = data
                .samples
                .iter()
                .enumerate()
                .map(|(index, sample)| {
                    let value = if sample.sorted.is_empty() {
                        "No data".to_owned()
                    } else {
                        format!(
                            "{} at or below",
                            percent(share_at_or_below(&sample.sorted, at))
                        )
                    };
                    ReadoutRow::new(Some(self.color(index)), sample.name.clone(), value)
                })
                .collect();
            let heading = reading(at, data.unit.as_deref());
            selection_label = Some(frame::reading_label(Some(&heading), &rows));
            frame::readout(&self.theme, x.fraction(at), Some(heading.into()), rows)
        });
        Layout {
            shapes,
            x_ticks: value_ticks(x, width),
            y_ticks,
            y_axis_width: frame::NUMBER_AXIS_WIDTH,
            height: frame::PLOT_HEIGHT,
            readout,
            selection_label,
            labels: Vec::new(),
        }
    }

    fn box_window(
        &self,
        state: Entity<DistributionState>,
        hover: Option<(f32, f32)>,
        plot_probe: Probe,
        cx: &mut App,
    ) -> AnyElement {
        let theme = self.theme;
        let count = self.data.samples.len();
        let selected = hover.map(|(_, y)| index_of(f64::from(y) * f64_of(count)).min(count - 1));
        let ticks = nice_ticks(
            self.data.domain(),
            frame::label_budget(probed_width(&plot_probe, 480.0) * 0.75, 96.0),
            false,
            false,
        );
        let x = ticks.domain;
        let rows = state.read(cx).boxes.clone();
        let row_pitch = theme.density.control_sm;
        let data = self.data.clone();
        let row_state = state.clone();
        let row_id = self.id.clone();
        let grid = frame::grid(&theme, &[], &frame::number_ticks(&ticks, None, false));
        let body = gpui::uniform_list(kit::part(&self.id, "rows"), count, move |range, _, _| {
            range
                .map(|row| {
                    let Some(sample) = data.samples.get(row) else {
                        return div().into_any_element();
                    };
                    let mut shapes = grid.clone();
                    if let Some(summary) = &sample.summary {
                        box_shapes(
                            summary,
                            x,
                            0.5,
                            1.0,
                            kit::series_color(&theme, row).to_paint(),
                            &mut shapes,
                        );
                    }
                    let entering = row_state.clone();
                    let selector = row_id.clone();
                    div()
                        .id(kit::part(&row_id, format_args!("box-row-{row}")))
                        .debug_selector(move || format!("{selector}-box-row-{row}"))
                        .w_full()
                        .h(row_pitch)
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .w(gpui::relative(0.25))
                                .min_w_0()
                                .pr(px(8.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(theme.typography.control_text)
                                .text_color(theme.colors.muted_foreground.to_paint())
                                .child(sample.name.clone()),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .child(plot_canvas(shapes, probe())),
                        )
                        .on_hover(move |over, _, cx| {
                            entering.update(cx, |state, cx| {
                                let index = state.hover.map(|(_, y)| {
                                    index_of(f64::from(y) * f64_of(count)).min(count - 1)
                                });
                                if *over {
                                    state.hover = Some((0.5, (row as f32 + 0.5) / count as f32));
                                } else if index == Some(row) {
                                    state.hover = None;
                                } else {
                                    return;
                                }
                                cx.notify();
                            })
                        })
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&rows)
        .w_full()
        .h(row_pitch * count.min(10) as f32);
        let reading = selected
            .and_then(|row| self.data.samples.get(row))
            .and_then(|sample| {
                let summary = sample.summary.as_ref()?;
                let rows = box_reading(summary, self.data.unit.as_deref());
                let label = frame::reading_label(Some(&sample.name), &rows);
                Some((
                    frame::readout(&theme, 0.5, Some(sample.name.clone().into()), rows),
                    label,
                ))
            });
        let selection_label = reading.as_ref().map(|(_, label)| label.clone());
        let content = div()
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .child(body)
            .child(
                div()
                    .w_full()
                    .flex()
                    .child(div().w(gpui::relative(0.25)))
                    .child(div().flex_1().min_w_0().child(frame::x_tick_strip(
                        &theme,
                        frame::number_ticks(&ticks, None, false),
                    ))),
            )
            .child(
                div()
                    .absolute()
                    .size_full()
                    .child(super::super::scene::plot_canvas(Vec::new(), plot_probe)),
            )
            .children(reading.map(|(card, _)| card));
        let keyboard = state.clone();
        let mut navigation = crate::chart_navigation::ChartNavigation::new(
            kit::part(&self.id, "navigation"),
            theme,
            "Box distributions. Arrows select rows; Escape clears the reading.",
            0..count,
            content,
            move |command, _, cx| {
                keyboard.update(cx, |state, cx| {
                    use crate::chart_navigation::ChartCommand;
                    state.hover = match command {
                        ChartCommand::Select(row) | ChartCommand::Activate(row) => {
                            rows.scroll_to_item(row, gpui::ScrollStrategy::Nearest);
                            Some((0.5, (row as f32 + 0.5) / count as f32))
                        }
                        ChartCommand::Parent | ChartCommand::Reset => None,
                    };
                    cx.notify();
                })
            },
        )
        .content_height()
        .selected(selected);
        if let Some(label) = selection_label {
            navigation = navigation.selection_label(label);
        }
        navigation.into_any_element()
    }

    fn threshold_rule(&self, threshold: f64, layout: &mut Layout) {
        let x = match self.data.mode {
            Mode::Histogram => self.data.bins.domain,
            _ => self.data.domain(),
        };
        let at = x.fraction(threshold);
        let warning = kit::Tone::Warning.color(&self.theme).to_paint();
        layout.shapes.push(Shape::Line {
            points: vec![(at, 0.0), (at, 1.0)],
            width: 1.5,
            color: warning,
            dash: true,
        });
        layout.labels.push(PlotLabel {
            x: at,
            y: 0.0,
            below: true,
            text: format!("≤ {}", reading(threshold, self.data.unit.as_deref())).into(),
            color: warning,
        });
    }

    fn plot(
        &self,
        layout: &mut Layout,
        state: &Entity<DistributionState>,
        probe: Probe,
    ) -> AnyElement {
        let moving = state.clone();
        let leaving = state.clone();
        let move_probe = probe.clone();
        let labels = std::mem::take(&mut layout.labels)
            .into_iter()
            .map(|label| frame::plot_label(&self.theme, label));
        div()
            .id(kit::part(&self.id, "plot"))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(plot_canvas(std::mem::take(&mut layout.shapes), probe))
            .children(labels)
            .children(layout.readout.take())
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                let hover = fractions(&move_probe, event.position);
                moving.update(cx, |state, cx| {
                    state.hover = hover;
                    cx.notify();
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
}

/// Numeric tick labels along a value axis that stays where it is.
fn value_ticks(domain: Domain, width: f32) -> Vec<Tick> {
    frame::number_ticks(
        &ticks_within(domain, frame::label_budget(width, 96.0)),
        None,
        false,
    )
}

/// A horizontal box with Tukey whiskers and outliers, centered on `cy`.
fn box_shapes(
    summary: &super::Summary,
    x: Domain,
    cy: f32,
    lane: f32,
    color: Hsla,
    shapes: &mut Vec<Shape>,
) {
    let half = lane * 0.26;
    let cap = lane * 0.14;
    let (lo, hi) = (
        x.fraction(summary.whiskers.0),
        x.fraction(summary.whiskers.1),
    );
    let (q1, median, q3) = (
        x.fraction(summary.q1),
        x.fraction(summary.median),
        x.fraction(summary.q3),
    );
    let stroke = |points: Vec<(f32, f32)>, width: f32| Shape::Line {
        points,
        width,
        color,
        dash: false,
    };
    shapes.push(stroke(vec![(lo, cy), (q1, cy)], 1.5));
    shapes.push(stroke(vec![(q3, cy), (hi, cy)], 1.5));
    shapes.push(stroke(vec![(lo, cy - cap), (lo, cy + cap)], 1.5));
    shapes.push(stroke(vec![(hi, cy - cap), (hi, cy + cap)], 1.5));
    shapes.push(Shape::Rect {
        x0: q1,
        y0: cy - half,
        x1: q3,
        y1: cy + half,
        color: Hsla {
            alpha: 0.25,
            ..color
        },
    });
    shapes.push(stroke(
        vec![
            (q1, cy - half),
            (q3, cy - half),
            (q3, cy + half),
            (q1, cy + half),
            (q1, cy - half),
        ],
        1.5,
    ));
    shapes.push(stroke(vec![(median, cy - half), (median, cy + half)], 2.5));
    let mut at = 0;
    while let Some(outlier) = summary.outliers.get(at) {
        // Equal samples have the same mark. Counts remain in the summary,
        // while repeated dots add no positional information.
        at += summary.outliers[at..].partition_point(|value| value == outlier);
        shapes.push(Shape::Dot {
            x: x.fraction(*outlier),
            y: cy,
            radius: 3.0,
            color: Hsla {
                alpha: 0.8,
                ..color
            },
        });
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn an_empty_series_has_no_cdf_curve() {
        let data = Arc::new(Distribution::from_spec(serde_json::from_value(serde_json::json!({"mode":"cdf","series":[{"name":"empty","values":[]},{"name":"present","values":[1,2]}]})).unwrap()));
        let view = DistributionView {
            id: "fixture".into(),
            theme: ArtisanTheme::for_mode(crate::theme::ThemeMode::Dark),
            data,
        };
        let layout = view.cdf(None, 480.0);
        let curves = layout
            .shapes
            .iter()
            .filter(|shape| matches!(shape, Shape::Line { width, .. } if *width == 2.0))
            .count();
        assert_eq!(curves, 1);
    }
}

fn box_reading(summary: &super::Summary, unit: Option<&str>) -> Vec<ReadoutRow> {
    vec![
        ReadoutRow::new(None, "n", summary.count.to_string()),
        ReadoutRow::new(None, "min", reading(summary.min, unit)),
        ReadoutRow::new(None, "q1", reading(summary.q1, unit)),
        ReadoutRow::new(None, "median", reading(summary.median, unit)),
        ReadoutRow::new(None, "q3", reading(summary.q3, unit)),
        ReadoutRow::new(None, "max", reading(summary.max, unit)),
        ReadoutRow::new(None, "mean", reading(summary.mean, unit)),
        ReadoutRow::new(None, "outliers", summary.outliers.len().to_string()),
    ]
}

#[cfg(test)]
mod native_box_window_tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext};
    struct Fixture {
        data: Arc<Distribution>,
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
                .child(DistributionView {
                    id: "native-box".into(),
                    theme: ArtisanTheme::default(),
                    data: self.data.clone(),
                })
        }
    }
    #[gpui::test]
    fn large_box_dataset_keeps_native_row_pitch_and_reveals_keyboard_selection(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::focus_navigation::bind_actions);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let series: Vec<_> = (0..1000).map(|index| serde_json::json!({"name":format!("Long labelled series {index}"),"values":[1,2,3]})).collect();
            let spec = serde_json::from_value(serde_json::json!({"mode":"box","series":series})).unwrap();
            Fixture { data: Arc::new(Distribution::from_spec(spec)), focus: crate::focus_navigation::fixture_root_focus(window,cx) }
        });
        assert_eq!(
            cx.debug_bounds("native-box-box-row-0").unwrap().size.height,
            ArtisanTheme::default().density.control_sm
        );
        assert!(
            cx.debug_bounds("native-box-box-row-50").is_none(),
            "offscreen rows must not be instantiated"
        );
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
            cx.debug_bounds("native-box-box-row-999")
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
            cx.debug_bounds("native-box-box-row-0").unwrap().size.height,
            ArtisanTheme::default().density.control_sm
        );
        assert!(cx.debug_bounds("native-box-box-row-999").is_none());
    }
}

#[cfg(test)]
mod outlier_mark_tests {
    use super::*;
    #[test]
    fn repeated_outliers_have_one_mark_per_value_without_losing_count() {
        let values: Vec<_> = std::iter::repeat_n(0.0, 9000)
            .chain(std::iter::repeat_n(10.0, 1000))
            .collect();
        let spec = serde_json::from_value(
            serde_json::json!({"mode":"box","series":[{"name":"many","values":values}]}),
        )
        .unwrap();
        let data = Distribution::from_spec(spec);
        let summary = data.samples[0].summary.as_ref().unwrap();
        assert_eq!(summary.outliers.len(), 1000);
        let mut shapes = Vec::new();
        box_shapes(
            summary,
            data.domain(),
            0.5,
            1.0,
            ArtisanTheme::default().colors.foreground.to_paint(),
            &mut shapes,
        );
        assert_eq!(
            shapes
                .iter()
                .filter(|shape| matches!(shape, Shape::Dot { .. }))
                .count(),
            1
        );
    }
}
