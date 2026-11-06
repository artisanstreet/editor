use super::model::{Model, Track, XScale, y_ticks};
use super::{Axis, ChartStyle, Mark};
use crate::ui::rich::charts::scale::{Domain, Ticks, f64_of, reading, ticks_within};
use crate::ui::rich::charts::scene::Shape;
use crate::ui::theme::{ArtisanTheme, Oklch, Opacity};
use aqua_core::{GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, GpuiHsla};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub(crate) struct Tick {
    pub at: f32,
    pub label: String,
}
#[derive(Clone, Debug)]
pub(crate) struct EndLabel {
    pub y: f32,
    pub text: String,
    pub color: GpuiHsla,
}
#[derive(Clone, Debug)]
pub(crate) struct PlotLabel {
    pub x: f32,
    pub y: f32,
    pub below: bool,
    pub text: String,
    pub color: GpuiHsla,
}
fn series_color(theme: &ArtisanTheme, index: usize) -> Oklch {
    theme.colors.palette[index % 8]
}
fn hrule(y: f32, color: GpuiHsla, dash: bool) -> Shape {
    Shape::Line {
        points: vec![(0.0, y), (1.0, y)],
        width: 1.0,
        color,
        dash,
    }
}
fn vrule(x: f32, color: GpuiHsla, dash: bool) -> Shape {
    Shape::Line {
        points: vec![(x, 0.0), (x, 1.0)],
        width: 1.0,
        color,
        dash,
    }
}
fn hover_band(theme: &ArtisanTheme, x0: f32, x1: f32) -> Shape {
    let (top, bottom) = crate::ui::hover::hover_fill_stops(theme, theme.colors.text);
    Shape::HoverBand {
        x0,
        x1,
        fill: GpuiBackground::LinearGradient {
            angle: 180.0,
            color_space: GpuiGradientColorSpace::Oklab,
            stops: [
                GpuiGradientStop {
                    percentage: 0.0,
                    color: top.to_reference_paint(),
                },
                GpuiGradientStop {
                    percentage: 1.0,
                    color: bottom.to_reference_paint(),
                },
            ],
        },
        ring: theme
            .colors
            .text
            .at(Opacity::HoverRing)
            .to_reference_paint(),
    }
}
fn number_ticks(ticks: &Ticks, unit: Option<&str>, down: bool) -> Vec<Tick> {
    ticks
        .values
        .iter()
        .map(|value| Tick {
            at: if down {
                ticks.domain.fraction_down(*value)
            } else {
                ticks.domain.fraction(*value)
            },
            label: reading(*value, unit),
        })
        .collect()
}
fn grid(theme: &ArtisanTheme, y: &[Tick], x: &[Tick]) -> Vec<Shape> {
    let color = theme.colors.text.at(Opacity::Rule).to_reference_paint();
    y.iter()
        .map(|tick| hrule(tick.at, color, false))
        .chain(x.iter().map(|tick| vrule(tick.at, color, false)))
        .collect()
}
fn label_budget(width: f32, spacing: f32) -> usize {
    crate::ui::rich::charts::scale::index_of(f64::from(
        (width / spacing.max(1.0)).floor().clamp(2.0, 24.0),
    ))
}
fn thinned(count: usize, budget: usize) -> impl Iterator<Item = usize> {
    (0..count).step_by(count.div_ceil(budget.max(1)).max(1))
}
fn spread(labels: &mut [EndLabel], gap: f32) {
    labels.sort_by(|a, b| a.y.total_cmp(&b.y));
    for i in 1..labels.len() {
        labels[i].y = labels[i].y.max(labels[i - 1].y + gap);
    }
    if let Some(last) = labels.last() {
        let overflow = last.y - 1.0;
        if overflow > 0.0 {
            for label in labels.iter_mut() {
                label.y -= overflow;
            }
        }
    }
    for i in (0..labels.len().saturating_sub(1)).rev() {
        labels[i].y = labels[i].y.min(labels[i + 1].y - gap);
    }
}

pub(super) struct Snapshot<'a> {
    pub(super) hidden: &'a [bool],
    pub(super) hover: Option<usize>,
    pub(super) zoom: Option<Domain>,
    pub(super) drag: Option<(f64, f64)>,
    pub(super) thresholds: Vec<(String, f64)>,
    pub(super) width: f32,
}

impl Snapshot<'_> {
    pub(super) fn visible(&self, count: usize) -> Vec<bool> {
        (0..count)
            .map(|index| !self.hidden.get(index).copied().unwrap_or(false))
            .collect()
    }
}

pub(super) struct Geometry {
    pub(super) shapes: Rc<[Shape]>,
    tracks: Vec<Track>,
    y: Domain,
    pub(super) x_ticks: Vec<Tick>,
    pub(super) y_ticks: Vec<Tick>,
    pub(super) labels: Vec<PlotLabel>,
    pub(super) end_labels: Vec<EndLabel>,
    pub(super) view: Domain,
}

pub(super) fn effective_axis(spec: &ChartStyle) -> Axis {
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
    spec: &ChartStyle,
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
    let y_labels = number_ticks(&y, tick_unit, true);
    let x_labels = x_ticks(model, view, snapshot.width);
    let mut shapes = grid(theme, &y_labels, &[]);
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
                theme
                    .colors
                    .text
                    .at(Opacity::ChartRule)
                    .to_reference_paint(),
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
    let warning = theme.colors.banner_warning.text.to_reference_paint();
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
            text: String::from(format!("{label} {}", reading(*value, axis.unit.as_deref()))),
            color: warning,
        });
    }
    if let Some((from, to)) = snapshot.drag {
        shapes.push(Shape::Rect {
            x0: view.fraction(from),
            y0: 0.0,
            x1: view.fraction(to),
            y1: 1.0,
            color: theme
                .colors
                .text
                .at(Opacity::ChartSelection)
                .to_reference_paint(),
        });
    }
    let end_labels = end_labels(theme, spec, model, &tracks, view, &y, axis.unit.as_deref());
    Geometry {
        shapes: shapes.into(),
        tracks,
        y: y.domain,
        x_ticks: x_labels,
        y_ticks: y_labels,
        labels,
        end_labels,
        view,
    }
}

pub(super) fn interaction(
    theme: &ArtisanTheme,
    spec: &ChartStyle,
    model: &Model,
    geometry: &Geometry,
    snapshot: &Snapshot<'_>,
) -> Vec<Shape> {
    let mut shapes = Vec::new();
    if let Some(x) = snapshot.hover.and_then(|index| model.xs.get(index)) {
        if spec.mark == Mark::Bar {
            let half = model.band() / 2.0;
            shapes.push(hover_band(
                theme,
                geometry.view.fraction(x - half),
                geometry.view.fraction(x + half),
            ));
        } else {
            shapes.push(vrule(
                geometry.view.fraction(*x),
                theme
                    .colors
                    .text
                    .at(Opacity::ChartRule)
                    .to_reference_paint(),
                false,
            ));
            Marks {
                theme,
                model,
                view: geometry.view,
                y: geometry.y,
                stacked: spec.stacks(),
            }
            .hover_dots(&geometry.tracks, *x, &mut shapes);
        }
    }
    if let Some((from, to)) = snapshot.drag {
        shapes.push(Shape::Rect {
            x0: geometry.view.fraction(from),
            y0: 0.0,
            x1: geometry.view.fraction(to),
            y1: 1.0,
            color: theme
                .colors
                .text
                .at(Opacity::ChartSelection)
                .to_reference_paint(),
        });
    }
    shapes
}

const END_LABEL_SERIES_MAX: usize = 8;

fn end_labels(
    theme: &ArtisanTheme,
    spec: &ChartStyle,
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
                format!("{}%", crate::ui::rich::kit::format_number(value.round()))
            } else {
                reading(value, unit)
            };
            Some(EndLabel {
                y: y.domain.fraction_down(anchor).clamp(0.0, 1.0),
                text: String::from(format!("{name} {value}")),
                color: series_color(theme, index).to_reference_paint(),
            })
        })
        .collect();
    spread(&mut labels, 16.0 / 240.0);
    labels
}

fn x_ticks(model: &Model, view: Domain, width: f32) -> Vec<Tick> {
    match &model.x {
        XScale::Category(names) => {
            let inside: Vec<usize> = (0..names.len())
                .filter(|index| view.contains(f64_of(*index)))
                .collect();
            let budget = label_budget(width, 76.0);
            thinned(inside.len(), budget)
                .map(|at| {
                    let index = inside[at];
                    Tick {
                        at: view.fraction(f64_of(index)),
                        label: String::from(names[index].clone()),
                    }
                })
                .collect()
        }
        XScale::Number => {
            let ticks = ticks_within(view, label_budget(width, 96.0));
            ticks
                .values
                .iter()
                .map(|value| Tick {
                    at: view.fraction(*value),
                    label: String::from(model.x_tick(*value)),
                })
                .collect()
        }
    }
}

fn annotations(
    theme: &ArtisanTheme,
    spec: &ChartStyle,
    model: &Model,
    view: Domain,
    y: &Ticks,
    shapes: &mut Vec<Shape>,
    labels: &mut Vec<PlotLabel>,
) {
    let color = theme.colors.text_muted.to_reference_paint();
    let rule = theme
        .colors
        .text_muted
        .at(Opacity::AnnotationRule)
        .to_reference_paint();
    let event_rule = theme
        .colors
        .text
        .at(Opacity::EventRule)
        .to_reference_paint();
    let event = theme.colors.text.to_reference_paint();
    for annotation in &spec.annotations {
        let text = String::from(annotation.label.trim().to_owned());
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
                    color: theme
                        .colors
                        .text
                        .at(Opacity::AnnotationBand)
                        .to_reference_paint(),
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

struct Marks<'a> {
    theme: &'a ArtisanTheme,
    model: &'a Model,
    view: Domain,
    y: Domain,
    stacked: bool,
}

impl Marks<'_> {
    fn color(&self, index: usize) -> GpuiHsla {
        series_color(self.theme, index).to_reference_paint()
    }

    fn series_fill(&self, index: usize) -> GpuiHsla {
        let purpose = if self.stacked {
            Opacity::AreaStacked
        } else {
            Opacity::Area
        };
        series_color(self.theme, index)
            .at(purpose)
            .to_reference_paint()
    }

    fn point(&self, x: f64, y: f64) -> (f32, f32) {
        (self.view.fraction(x), self.y.fraction_down(y))
    }

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
                        color: self.series_fill(index),
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
                    let left = f64_of(lane).mul_add(lane_width, x - group / 2.0);
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
                    color: self.theme.colors.background.to_reference_paint(),
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
