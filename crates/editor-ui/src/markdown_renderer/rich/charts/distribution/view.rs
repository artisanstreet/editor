//! The distribution widget: one plot per mode, the hover readout, and the
//! threshold slider.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, ElementId, Entity, Hsla, IntoElement, MouseMoveEvent, ParentElement as _,
    RenderOnce, SharedString, Styled as _, Window, div, prelude::*, px,
};

use super::super::super::kit;
use super::super::frame::{self, Frame, LegendEntry, PlotLabel, ReadoutRow, Tick};
use super::super::scale::{
    Domain, f32_of, f64_of, index_of, nice_ticks, percent, reading, ticks_within,
};
use super::super::scene::{
    Probe, Shape, fractions, hover_band, plot_canvas, probe, probed_width, vrule,
};
use super::super::slider::{self, SliderState};
use super::{Distribution, Mode, share_at_or_below, summary};
use crate::theme::ArtisanTheme;

#[derive(IntoElement)]
pub(super) struct DistributionView {
    pub(super) id: SharedString,
    pub(super) theme: ArtisanTheme,
    pub(super) data: Rc<Distribution>,
}

struct DistributionState {
    /// The pointer over the plot, in plot fractions.
    hover: Option<(f32, f32)>,
    probe: Probe,
}

/// One frame of the plot.
struct Layout {
    shapes: Vec<Shape>,
    x_ticks: Vec<Tick>,
    y_ticks: Vec<Tick>,
    y_axis_width: f32,
    height: f32,
    readout: Option<Div>,
    labels: Vec<PlotLabel>,
}

impl RenderOnce for DistributionView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        if self.data.is_empty() {
            return frame::empty(&theme, "No values to plot");
        }
        let state = kit::keyed(window, cx, ElementId::Name(self.id.clone()), || {
            DistributionState {
                hover: None,
                probe: probe(),
            }
        });
        let with_slider = self.data.mode != Mode::Box;
        let slider_state = with_slider.then(|| {
            let (range, step, start) = self.data.slider();
            slider::keyed_slider(window, cx, format!("{}-threshold", self.id).into(), || {
                SliderState::new(range.lo, range.hi, Some(step), start)
            })
        });
        let (hover, plot_probe) = {
            let current = state.read(cx);
            (current.hover, current.probe.clone())
        };
        let threshold = slider_state.as_ref().map(|slider| slider.read(cx).value);
        let width = probed_width(&plot_probe, 480.0);
        let mut layout = match self.data.mode {
            Mode::Histogram => self.histogram(hover, width),
            Mode::Cdf => self.cdf(hover, width),
            Mode::Box => self.boxes(hover, width),
        };
        if let Some(threshold) = threshold {
            self.threshold_rule(threshold, &mut layout);
        }
        let plot = self.plot(&mut layout, &state, plot_probe);
        let framed = Frame {
            end_labels: Vec::new(),
            y_ticks: layout.y_ticks,
            x_ticks: layout.x_ticks,
            y_title: (self.data.mode == Mode::Histogram).then(|| "Count".into()),
            x_title: self.data.unit.clone().map(SharedString::from),
            y_axis_width: layout.y_axis_width,
            height: layout.height,
        }
        .wrap(&theme, plot);
        let legend = (self.data.samples.len() > 1 && self.data.mode != Mode::Box).then(|| {
            let entries = self
                .data
                .samples
                .iter()
                .enumerate()
                .map(|(index, sample)| LegendEntry {
                    name: sample.name.clone().into(),
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
                slider::slider(
                    &theme,
                    format!("{}-slider", self.id).into(),
                    "Threshold".into(),
                    self.shares(threshold).into(),
                    entity,
                    cx,
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
        slider::follow_drag(root, &sliders).into_any_element()
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
                let share = percent(share_at_or_below(&sample.sorted, threshold));
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
        let counts: Vec<Vec<usize>> = data
            .samples
            .iter()
            .map(|sample| bins.counts(&sample.sorted))
            .collect();
        let tallest = counts.iter().flatten().copied().max().unwrap_or(0).max(1);
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
        let readout = hovered.map(|bin| {
            let (lo, hi) = bins.edges(bin);
            let rows = data
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
            let count = f64_of(sample.sorted.len().max(1));
            let mut points = vec![(0.0, 1.0)];
            for (rank, value) in sample.sorted.iter().enumerate() {
                let at = x.fraction(*value);
                points.push((at, y.fraction_down(f64_of(rank) / count)));
                points.push((at, y.fraction_down(f64_of(rank + 1) / count)));
            }
            points.push((1.0, y.fraction_down(1.0)));
            shapes.push(Shape::Line {
                points,
                width: 2.0,
                color: self.color(index),
                dash: false,
            });
        }
        let readout = hovered.map(|at| {
            let rows = data
                .samples
                .iter()
                .enumerate()
                .map(|(index, sample)| {
                    let share = percent(share_at_or_below(&sample.sorted, at));
                    ReadoutRow::new(
                        Some(self.color(index)),
                        sample.name.clone(),
                        format!("{share} at or below"),
                    )
                })
                .collect();
            let heading = reading(at, data.unit.as_deref());
            frame::readout(&self.theme, x.fraction(at), Some(heading.into()), rows)
        });
        Layout {
            shapes,
            x_ticks: value_ticks(x, width),
            y_ticks,
            y_axis_width: frame::NUMBER_AXIS_WIDTH,
            height: frame::PLOT_HEIGHT,
            readout,
            labels: Vec::new(),
        }
    }

    fn boxes(&self, hover: Option<(f32, f32)>, width: f32) -> Layout {
        let data = &self.data;
        let ticks = nice_ticks(
            data.domain(),
            frame::label_budget(width, 96.0),
            false,
            false,
        );
        let x = ticks.domain;
        let rows = data.samples.len().max(1);
        let lane = 1.0 / f32_of(f64_of(rows));
        let center = |row: usize| (f32_of(f64_of(row)) + 0.5) * lane;
        let x_ticks = frame::number_ticks(&ticks, None, false);
        let mut shapes = frame::grid(&self.theme, &[], &x_ticks);
        let hovered = hover
            .filter(|(_, fy)| (0.0..1.0).contains(fy))
            .map(|(_, fy)| index_of(f64::from(fy / lane)).min(rows - 1));
        if let Some(row) = hovered {
            shapes.push(Shape::Rect {
                x0: 0.0,
                y0: f32_of(f64_of(row)) * lane,
                x1: 1.0,
                y1: f32_of(f64_of(row + 1)) * lane,
                color: self.theme.colors.foreground.with_alpha(0.04).to_paint(),
            });
        }
        for (row, sample) in data.samples.iter().enumerate() {
            let Some(summary) = summary(&sample.sorted) else {
                continue;
            };
            box_shapes(&summary, x, center(row), lane, self.color(row), &mut shapes);
        }
        let readout = hovered.and_then(|row| {
            let sample = data.samples.get(row)?;
            let summary = summary(&sample.sorted)?;
            let unit = data.unit.as_deref();
            let rows = vec![
                ReadoutRow::new(None, "n", summary.count.to_string()),
                ReadoutRow::new(None, "min", reading(summary.min, unit)),
                ReadoutRow::new(None, "q1", reading(summary.q1, unit)),
                ReadoutRow::new(None, "median", reading(summary.median, unit)),
                ReadoutRow::new(None, "q3", reading(summary.q3, unit)),
                ReadoutRow::new(None, "max", reading(summary.max, unit)),
                ReadoutRow::new(None, "mean", reading(summary.mean, unit)),
                ReadoutRow::new(None, "outliers", summary.outliers.len().to_string()),
            ];
            Some(frame::readout(
                &self.theme,
                x.fraction(summary.median),
                Some(sample.name.clone().into()),
                rows,
            ))
        });
        let y_ticks = data
            .samples
            .iter()
            .enumerate()
            .map(|(row, sample)| Tick {
                at: center(row),
                label: sample.name.clone().into(),
            })
            .collect();
        Layout {
            shapes,
            x_ticks,
            y_ticks,
            y_axis_width: 96.0,
            height: (f32_of(f64_of(rows)) * 48.0).clamp(240.0, 360.0),
            readout,
            labels: Vec::new(),
        }
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
            .id(ElementId::Name(format!("{}-plot", self.id).into()))
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
    for outlier in &summary.outliers {
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
