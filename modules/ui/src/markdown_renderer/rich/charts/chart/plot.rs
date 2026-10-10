//! A chart's geometry for one frame: marks, ticks, rules, and labels in plot
//! fractions, from the model and the current interaction state.

use gpui::{Hsla, SharedString};

use super::super::super::kit;
use super::super::frame::{self, EndLabel, PlotLabel, Tick};
use super::super::scale::{Domain, Ticks, f64_of, reading, ticks_within};
use super::super::scene::{Shape, hover_band, hrule, vrule};
use super::model::{Model, Track, XScale, y_ticks};
use super::{Axis, ChartSpec, Mark};
use crate::theme::ArtisanTheme;

/// The interaction state a frame draws from.
pub(super) struct Snapshot<'a> {
    pub(super) hidden: &'a [bool],
    pub(super) hover: Option<usize>,
    pub(super) zoom: Option<Domain>,
    pub(super) drag: Option<(f64, f64)>,
    /// Each threshold's current value.
    pub(super) thresholds: Vec<(SharedString, f64)>,
    /// The plot's width in pixels, as last painted.
    pub(super) width: f32,
}

impl Snapshot<'_> {
    pub(super) fn visible(&self, count: usize) -> Vec<bool> {
        (0..count)
            .map(|index| !self.hidden.get(index).copied().unwrap_or(false))
            .collect()
    }
}

/// Everything one frame of the plot draws.
pub(super) struct Geometry {
    pub(super) shapes: Vec<Shape>,
    pub(super) x_ticks: Vec<Tick>,
    pub(super) y_ticks: Vec<Tick>,
    pub(super) labels: Vec<PlotLabel>,
    /// Direct series labels for the right gutter.
    pub(super) end_labels: Vec<EndLabel>,
    /// The x range in view.
    pub(super) view: Domain,
}

/// The y axis as drawn: shares pin to 0..100 unless the spec pins it.
pub(super) fn effective_axis(spec: &ChartSpec) -> Axis {
    let mut axis = spec.y.clone();
    if spec.normalize {
        axis.min = axis.min.or(Some(0.0));
        axis.max = axis.max.or(Some(100.0));
        axis.unit = Some("%".to_owned());
    }
    axis
}

pub(super) fn layout(
    theme: &ArtisanTheme,
    spec: &ChartSpec,
    model: &Model,
    snapshot: &Snapshot<'_>,
) -> Geometry {
    let full = model.x_domain(spec.mark);
    let view = snapshot.zoom.unwrap_or(full);
    let visible = snapshot.visible(model.series.len());
    let tracks = model.tracks(&visible, spec.stacks(), spec.normalize);
    let axis = effective_axis(spec);
    let y = y_ticks(&tracks, view, spec.mark, spec.stacks(), &axis);
    let tick_unit = spec.normalize.then_some("%");
    let y_labels = frame::number_ticks(&y, tick_unit, true);
    let x_labels = x_ticks(model, view, snapshot.width);
    let mut shapes = frame::grid(theme, &y_labels, &[]);
    let mut labels = Vec::new();
    annotations(theme, spec, model, view, &y, &mut shapes, &mut labels);
    if let Some(hover) = snapshot.hover.and_then(|index| model.xs.get(index)) {
        if spec.mark == Mark::Bar {
            let half = model.band() / 2.0;
            shapes.push(hover_band(
                theme,
                view.fraction(hover - half),
                view.fraction(hover + half),
            ));
        } else {
            shapes.push(vrule(
                view.fraction(*hover),
                theme.colors.foreground.with_alpha(0.3).to_paint(),
                false,
            ));
        }
    }
    let marks = Marks {
        theme,
        model,
        view,
        y: y.domain,
        stacked: spec.stacks(),
    };
    match spec.mark {
        Mark::Line => marks.lines(&tracks, &mut shapes, false),
        Mark::Area => marks.lines(&tracks, &mut shapes, true),
        Mark::Bar => marks.bars(&tracks, &mut shapes),
        Mark::Scatter => marks.dots(&tracks, &mut shapes),
    }
    if let Some(hover) = snapshot.hover.and_then(|index| model.xs.get(index))
        && spec.mark != Mark::Bar
    {
        marks.hover_dots(&tracks, *hover, &mut shapes);
    }
    let warning = kit::Tone::Warning.color(theme).to_paint();
    for (label, value) in &snapshot.thresholds {
        let at = y.domain.fraction_down(*value);
        shapes.push(Shape::Line {
            points: vec![(0.0, at), (1.0, at)],
            width: 1.5,
            color: warning,
            dash: true,
        });
        labels.push(PlotLabel {
            x: 0.0,
            y: at,
            below: false,
            text: SharedString::from(format!("{label} {}", reading(*value, axis.unit.as_deref()))),
            color: warning,
        });
    }
    if let Some((from, to)) = snapshot.drag {
        shapes.push(Shape::Rect {
            x0: view.fraction(from),
            y0: 0.0,
            x1: view.fraction(to),
            y1: 1.0,
            color: theme.colors.foreground.with_alpha(0.08).to_paint(),
        });
    }
    let end_labels = end_labels(theme, spec, model, &tracks, view, &y, axis.unit.as_deref());
    Geometry {
        shapes,
        x_ticks: x_labels,
        y_ticks: y_labels,
        labels,
        end_labels,
        view,
    }
}

/// Most series labelled directly; more fall back to the legend alone.
const END_LABEL_SERIES_MAX: usize = 8;

/// Each visible line or area series named at its last value in view, so
/// the reader does not shuttle between the plot and the legend.
fn end_labels(
    theme: &ArtisanTheme,
    spec: &ChartSpec,
    model: &Model,
    tracks: &[Track],
    view: Domain,
    y: &Ticks,
    unit: Option<&str>,
) -> Vec<EndLabel> {
    if !matches!(spec.mark, Mark::Line | Mark::Area) {
        return Vec::new();
    }
    let shown = tracks.iter().filter(|track| !track.is_empty()).count();
    if shown == 0 || shown > END_LABEL_SERIES_MAX {
        return Vec::new();
    }
    let stacked = spec.stacks();
    let mut labels: Vec<EndLabel> = tracks
        .iter()
        .enumerate()
        .filter_map(|(index, track)| {
            let (_, (base, top)) = track
                .iter()
                .rev()
                .filter(|(x, _)| view.contains(*x))
                .find_map(|(x, extent)| extent.map(|extent| (*x, extent)))?;
            let (anchor, value) = if stacked {
                (f64::midpoint(base, top), top - base)
            } else {
                (top, top)
            };
            let name = &model.series.get(index)?.name;
            let value = if spec.normalize {
                format!("{}%", kit::format_number(value.round()))
            } else {
                reading(value, unit)
            };
            Some(EndLabel {
                y: y.domain.fraction_down(anchor).clamp(0.0, 1.0),
                text: SharedString::from(format!("{name} {value}")),
                color: kit::series_color(theme, index).to_paint(),
            })
        })
        .collect();
    frame::spread(&mut labels, 16.0 / frame::PLOT_HEIGHT);
    labels
}

/// X tick labels for the categories or numbers in `view`, thinned to fit.
fn x_ticks(model: &Model, view: Domain, width: f32) -> Vec<Tick> {
    match &model.x {
        XScale::Category(names) => {
            let inside: Vec<usize> = (0..names.len())
                .filter(|index| view.contains(f64_of(*index)))
                .collect();
            let budget = frame::label_budget(width, 76.0);
            frame::thinned(inside.len(), budget)
                .map(|at| {
                    let index = inside[at];
                    Tick {
                        at: view.fraction(f64_of(index)),
                        label: SharedString::from(names[index].clone()),
                    }
                })
                .collect()
        }
        XScale::Number => {
            let ticks = ticks_within(view, frame::label_budget(width, 96.0));
            ticks
                .values
                .iter()
                .map(|value| Tick {
                    at: view.fraction(*value),
                    label: SharedString::from(model.x_tick(*value)),
                })
                .collect()
        }
    }
}

fn annotations(
    theme: &ArtisanTheme,
    spec: &ChartSpec,
    model: &Model,
    view: Domain,
    y: &Ticks,
    shapes: &mut Vec<Shape>,
    labels: &mut Vec<PlotLabel>,
) {
    let color = theme.colors.muted_foreground.to_paint();
    let rule = theme.colors.muted_foreground.with_alpha(0.7).to_paint();
    // An event reads as a solid rule with its name in the foreground.
    let event_rule = theme.colors.foreground.with_alpha(0.55).to_paint();
    let event = theme.colors.foreground.to_paint();
    for annotation in &spec.annotations {
        let text = SharedString::from(annotation.label.trim().to_owned());
        let start = annotation.x.as_ref().and_then(|x| model.resolve_x(x));
        let end = annotation.x_end.as_ref().and_then(|x| model.resolve_x(x));
        let label = |x: f32, y: f32, below: bool| PlotLabel {
            x,
            y,
            below,
            text: text.clone(),
            color,
        };
        match (start, end, annotation.y) {
            (Some(start), Some(end), _) => {
                let (lo, hi) = (start.min(end), start.max(end));
                if hi < view.lo || lo > view.hi {
                    continue;
                }
                let (x0, x1) = (view.fraction(lo).max(0.0), view.fraction(hi).min(1.0));
                shapes.push(Shape::Rect {
                    x0,
                    y0: 0.0,
                    x1,
                    y1: 1.0,
                    color: theme.colors.foreground.with_alpha(0.06).to_paint(),
                });
                if !text.is_empty() {
                    labels.push(label(x0, 0.0, true));
                }
            }
            (Some(at), None, _) => {
                if !view.contains(at) {
                    continue;
                }
                let x = view.fraction(at);
                shapes.push(vrule(x, event_rule, false));
                if !text.is_empty() {
                    labels.push(PlotLabel {
                        color: event,
                        ..label(x, 0.0, true)
                    });
                }
            }
            (None, _, Some(value)) => {
                let at = y.domain.fraction_down(value);
                if !(0.0..=1.0).contains(&at) {
                    continue;
                }
                shapes.push(hrule(at, rule, true));
                if !text.is_empty() {
                    labels.push(label(1.0, at, false));
                }
            }
            (None, _, None) => {}
        }
    }
}

/// Draws series marks in plot fractions.
struct Marks<'a> {
    theme: &'a ArtisanTheme,
    model: &'a Model,
    view: Domain,
    y: Domain,
    stacked: bool,
}

impl Marks<'_> {
    fn color(&self, index: usize) -> Hsla {
        kit::series_color(self.theme, index).to_paint()
    }

    fn point(&self, x: f64, y: f64) -> (f32, f32) {
        (self.view.fraction(x), self.y.fraction_down(y))
    }

    /// Lines through each run of values; with `fill`, the area under them
    /// down to each point's base.
    fn lines(&self, tracks: &[Track], shapes: &mut Vec<Shape>, fill: bool) {
        for (index, track) in tracks.iter().enumerate() {
            let color = self.color(index);
            for run in runs(track) {
                let tops: Vec<(f32, f32)> = run
                    .iter()
                    .map(|(x, (_, top))| self.point(*x, *top))
                    .collect();
                if fill && run.len() > 1 {
                    let base = self.y.lo.max(0.0).min(self.y.hi);
                    let mut outline = tops.clone();
                    outline.extend(run.iter().rev().map(|(x, (bottom, _))| {
                        self.point(*x, if self.stacked { *bottom } else { base })
                    }));
                    shapes.push(Shape::Area {
                        points: outline,
                        color: Hsla {
                            alpha: if self.stacked { 0.55 } else { 0.22 },
                            ..color
                        },
                    });
                }
                if let [(x, y)] = tops.as_slice() {
                    shapes.push(Shape::Dot {
                        x: *x,
                        y: *y,
                        radius: 3.0,
                        color,
                    });
                } else {
                    shapes.push(Shape::Line {
                        points: tops,
                        width: 2.0,
                        color,
                        dash: false,
                    });
                }
            }
        }
    }

    fn bars(&self, tracks: &[Track], shapes: &mut Vec<Shape>) {
        let band = self.model.band();
        let shown: Vec<usize> = (0..tracks.len())
            .filter(|index| !tracks[*index].is_empty())
            .collect();
        let lanes = f64_of(shown.len().max(1));
        let floor = self.y.lo.max(0.0).min(self.y.hi);
        for (lane, index) in shown.iter().enumerate() {
            let color = self.color(*index);
            for (x, extent) in &tracks[*index] {
                let Some((base, top)) = extent else { continue };
                let (left, right) = if self.stacked {
                    (x - band * 0.36, x + band * 0.36)
                } else {
                    let group = band * 0.8;
                    let lane_width = group / lanes;
                    let left = x - group / 2.0 + f64_of(lane) * lane_width;
                    (left + lane_width * 0.06, left + lane_width * 0.94)
                };
                let base = if self.stacked { *base } else { floor };
                shapes.push(Shape::Rect {
                    x0: self.view.fraction(left),
                    y0: self.y.fraction_down(*top).clamp(0.0, 1.0),
                    x1: self.view.fraction(right),
                    y1: self.y.fraction_down(base).clamp(0.0, 1.0),
                    color,
                });
            }
        }
    }

    fn dots(&self, tracks: &[Track], shapes: &mut Vec<Shape>) {
        for (index, track) in tracks.iter().enumerate() {
            let color = self.color(index);
            for (x, extent) in track {
                if let Some((_, top)) = extent {
                    let (x, y) = self.point(*x, *top);
                    shapes.push(Shape::Dot {
                        x,
                        y,
                        radius: 3.5,
                        color,
                    });
                }
            }
        }
    }

    /// Emphasized dots where each series meets the hovered x.
    fn hover_dots(&self, tracks: &[Track], hover: f64, shapes: &mut Vec<Shape>) {
        for (index, track) in tracks.iter().enumerate() {
            let at = track
                .iter()
                .filter(|(x, _)| x.total_cmp(&hover).is_eq())
                .find_map(|(_, extent)| *extent);
            if let Some((_, top)) = at {
                let (x, y) = self.point(hover, top);
                shapes.push(Shape::Dot {
                    x,
                    y,
                    radius: 5.0,
                    color: self.theme.colors.background.to_paint(),
                });
                shapes.push(Shape::Dot {
                    x,
                    y,
                    radius: 3.5,
                    color: self.color(index),
                });
            }
        }
    }
}

/// The unbroken runs of a track: nulls split it.
pub(super) fn runs(track: &Track) -> Vec<Vec<(f64, (f64, f64))>> {
    let mut runs = Vec::new();
    let mut current = Vec::new();
    for (x, extent) in track {
        match extent {
            Some(extent) => current.push((*x, *extent)),
            None if !current.is_empty() => runs.push(std::mem::take(&mut current)),
            None => {}
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}
