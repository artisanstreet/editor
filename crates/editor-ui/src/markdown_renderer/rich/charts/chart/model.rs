//! A chart's data resolved onto one x scale: sorted series, the union of
//! their x positions, stacking, and the lookups hover and zoom need.

use std::collections::HashMap;

use super::super::super::kit;
use super::super::scale::{Domain, Ticks, exact, f64_of, index_of, nice_ticks};
use super::{Axis, ChartSpec, Mark, XValue, series_name};

/// How x positions read: category indices, or numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XScale {
    /// Position `i` is category `i`.
    Category(Vec<String>),
    Number,
}

/// One series as `(x, y)` points sorted by x; a `None` y breaks the line.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub(crate) name: String,
    pub(crate) points: Vec<(f64, Option<f64>)>,
}

/// Per x of a series: where its mark spans, base to top, or a gap.
pub type Track = Vec<(f64, Option<(f64, f64)>)>;

/// The resolved chart data.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub(crate) x: XScale,
    /// Every x any series has, sorted and distinct; every category index
    /// on a category scale.
    pub(crate) xs: Vec<f64>,
    pub(crate) series: Vec<Series>,
}

impl Model {
    pub(crate) fn from_spec(spec: &ChartSpec) -> Self {
        let text_points = spec
            .series
            .iter()
            .filter_map(|series| series.points.as_ref())
            .flatten()
            .any(|(x, _)| matches!(x, XValue::Text(_)));
        let values_only = spec
            .series
            .iter()
            .any(|series| series.values.is_some() && series.points.is_none());
        let categorical = spec.categories.is_some() || text_points || values_only;
        let mut model = if categorical {
            Self::categorical(spec)
        } else {
            Self::numeric(spec)
        };
        for series in &mut model.series {
            series.points.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        model.xs = match &model.x {
            XScale::Category(names) => (0..names.len()).map(f64_of).collect(),
            XScale::Number => {
                let mut xs: Vec<f64> = model
                    .series
                    .iter()
                    .flat_map(|series| series.points.iter().map(|(x, _)| *x))
                    .collect();
                xs.sort_by(f64::total_cmp);
                xs.dedup_by(|a, b| a.total_cmp(b).is_eq());
                xs
            }
        };
        model
    }

    fn categorical(spec: &ChartSpec) -> Self {
        let mut names = spec.categories.clone().unwrap_or_default();
        let longest = spec
            .series
            .iter()
            .filter(|series| series.points.is_none())
            .filter_map(|series| series.values.as_ref().map(Vec::len))
            .max()
            .unwrap_or(0);
        while names.len() < longest {
            names.push((names.len() + 1).to_string());
        }
        let mut index: HashMap<String, usize> = HashMap::new();
        for (position, name) in names.iter().enumerate() {
            index.entry(name.clone()).or_insert(position);
        }
        let mut series = Vec::with_capacity(spec.series.len());
        for (position, spec) in spec.series.iter().enumerate() {
            let points = match (&spec.values, &spec.points) {
                (Some(values), None) => values
                    .iter()
                    .enumerate()
                    .map(|(at, value)| (f64_of(at), finite(*value)))
                    .collect(),
                (_, Some(points)) => points
                    .iter()
                    .map(|(x, y)| {
                        let name = match x {
                            XValue::Text(text) => text.clone(),
                            XValue::Number(number) => exact(*number),
                        };
                        let at = *index.entry(name.clone()).or_insert_with(|| {
                            names.push(name);
                            names.len() - 1
                        });
                        (f64_of(at), finite(*y))
                    })
                    .collect(),
                (None, None) => Vec::new(),
            };
            series.push(Series {
                name: series_name(&spec.name, position).to_string(),
                points,
            });
        }
        Self {
            x: XScale::Category(names),
            xs: Vec::new(),
            series,
        }
    }

    fn numeric(spec: &ChartSpec) -> Self {
        let series = spec
            .series
            .iter()
            .enumerate()
            .map(|(position, spec)| Series {
                name: series_name(&spec.name, position).to_string(),
                points: spec
                    .points
                    .iter()
                    .flatten()
                    .filter_map(|(x, y)| match x {
                        XValue::Number(x) if x.is_finite() => Some((*x, finite(*y))),
                        XValue::Number(_) | XValue::Text(_) => None,
                    })
                    .collect(),
            })
            .collect();
        Self {
            x: XScale::Number,
            xs: Vec::new(),
            series,
        }
    }

    pub(crate) const fn is_categorical(&self) -> bool {
        matches!(self.x, XScale::Category(_))
    }

    /// Whether any series has a value to draw.
    pub(crate) fn has_values(&self) -> bool {
        self.series
            .iter()
            .any(|series| series.points.iter().any(|(_, y)| y.is_some()))
    }

    /// The x written exactly: its category, or the number.
    pub(crate) fn x_label(&self, x: f64) -> String {
        match &self.x {
            XScale::Category(names) => names.get(index_of(x.round())).cloned().unwrap_or_default(),
            XScale::Number => exact(x),
        }
    }

    /// The x as a tick reads: its category, or the number compacted.
    pub(crate) fn x_tick(&self, x: f64) -> String {
        match &self.x {
            XScale::Category(_) => self.x_label(x),
            XScale::Number => kit::format_number(x),
        }
    }

    /// The position of an annotation's x on this scale.
    pub(crate) fn resolve_x(&self, value: &XValue) -> Option<f64> {
        match (&self.x, value) {
            (XScale::Number, XValue::Number(number)) => Some(*number),
            (XScale::Number, XValue::Text(text)) => text.trim().parse().ok(),
            (XScale::Category(names), XValue::Text(text)) => {
                names.iter().position(|name| name == text).map(f64_of)
            }
            (XScale::Category(names), XValue::Number(number)) => {
                let label = exact(*number);
                names
                    .iter()
                    .position(|name| *name == label)
                    .map(f64_of)
                    .or_else(|| {
                        (number.is_finite() && *number >= 0.0 && *number <= f64_of(names.len()))
                            .then_some(*number)
                    })
            }
        }
    }

    /// The x a bar spans around its position.
    pub(crate) fn band(&self) -> f64 {
        if self.is_categorical() {
            return 1.0;
        }
        let gap = self
            .xs
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .filter(|gap| *gap > 0.0)
            .fold(f64::INFINITY, f64::min);
        if gap.is_finite() {
            gap
        } else {
            Domain::of(self.xs.iter().copied()).map_or(1.0, |domain| domain.span() * 0.1)
        }
    }

    /// The whole x extent for `mark`; bars and lone points get half a band
    /// of room on each side.
    pub(crate) fn x_domain(&self, mark: Mark) -> Domain {
        let extent = Domain::of(self.xs.iter().copied());
        let roomy = mark == Mark::Bar || self.xs.len() < 2;
        match extent {
            Some(extent) if !roomy => Domain::new(
                self.xs.first().copied().unwrap_or(extent.lo),
                self.xs.last().copied().unwrap_or(extent.hi),
            ),
            Some(_) => {
                let lo = self.xs.first().copied().unwrap_or(0.0);
                let hi = self.xs.last().copied().unwrap_or(lo);
                let half = self.band() / 2.0;
                Domain::new(lo - half, hi + half)
            }
            None => Domain::new(0.0, 1.0),
        }
    }

    /// Series `index`'s value at each of `xs`; `None` where absent or null.
    pub(crate) fn aligned(&self, index: usize) -> Vec<Option<f64>> {
        let Some(series) = self.series.get(index) else {
            return vec![None; self.xs.len()];
        };
        self.xs.iter().map(|x| lookup(&series.points, *x)).collect()
    }

    /// Series `index`'s value at exactly `x`.
    pub(crate) fn value_at(&self, index: usize, x: f64) -> Option<f64> {
        self.series
            .get(index)
            .and_then(|series| lookup(&series.points, x))
    }

    /// The index into `xs` nearest `x` among those inside `view`.
    pub(crate) fn nearest(&self, x: f64, view: Domain) -> Option<usize> {
        let lo = self.xs.partition_point(|value| *value < view.lo);
        let hi = self.xs.partition_point(|value| *value <= view.hi);
        if lo >= hi {
            return None;
        }
        let after = lo + self.xs[lo..hi].partition_point(|value| *value < x);
        match (
            after.checked_sub(1).filter(|index| *index >= lo),
            (after < hi).then_some(after),
        ) {
            (Some(before), Some(after)) => Some(
                if (self.xs[before] - x).abs() <= (self.xs[after] - x).abs() {
                    before
                } else {
                    after
                },
            ),
            (before, after) => before.or(after),
        }
    }

    /// Each series' extents. Hidden series get empty tracks. Unstacked
    /// series rise from zero along their own points; stacked series pile
    /// on the visible series before them at every shared x, positives up
    /// and negatives down, rescaled to percent shares when `normalize`.
    pub(crate) fn tracks(&self, visible: &[bool], stacked: bool, normalize: bool) -> Vec<Track> {
        let shown = |index: usize| visible.get(index).copied().unwrap_or(true);
        if !(stacked || normalize) {
            return self
                .series
                .iter()
                .enumerate()
                .map(|(index, series)| {
                    if !shown(index) {
                        return Vec::new();
                    }
                    series
                        .points
                        .iter()
                        .map(|(x, y)| (*x, y.map(|y| (0.0, y))))
                        .collect()
                })
                .collect();
        }
        let aligned: Vec<Vec<Option<f64>>> = (0..self.series.len())
            .map(|index| self.aligned(index))
            .collect();
        let mut tracks: Vec<Track> = (0..self.series.len())
            .map(|index| {
                if shown(index) {
                    Vec::with_capacity(self.xs.len())
                } else {
                    Vec::new()
                }
            })
            .collect();
        for (column, x) in self.xs.iter().enumerate() {
            let scale = (0..self.series.len())
                .filter(|index| shown(*index))
                .filter_map(|index| aligned[index][column])
                .map(f64::abs)
                .fold(0.0, f64::max);
            let total: f64 = if normalize && scale > 0.0 {
                (0..self.series.len())
                    .filter(|index| shown(*index))
                    .filter_map(|index| aligned[index][column])
                    .map(|value| value.abs() / scale)
                    .sum()
            } else {
                0.0
            };
            let (mut up, mut down) = (0.0, 0.0);
            for (index, track) in tracks.iter_mut().enumerate() {
                if !shown(index) {
                    continue;
                }
                let extent = aligned[index][column].map(|value| {
                    let value = if normalize && total > 0.0 {
                        (value / scale) / total * 100.0
                    } else {
                        value
                    };
                    let base: &mut f64 = if value >= 0.0 { &mut up } else { &mut down };
                    let start = *base;
                    *base += value;
                    (start, *base)
                });
                track.push((*x, extent));
            }
        }
        tracks
    }

    /// The share of visible values at or above `threshold`: (at or above, all).
    pub(crate) fn at_or_above(&self, visible: &[bool], threshold: f64) -> (usize, usize) {
        let values = self
            .series
            .iter()
            .enumerate()
            .filter(|(index, _)| visible.get(*index).copied().unwrap_or(true))
            .flat_map(|(_, series)| series.points.iter().filter_map(|(_, y)| *y));
        values.fold((0, 0), |(above, all), value| {
            (above + usize::from(value >= threshold), all + 1)
        })
    }
}

/// The y ticks for `tracks` seen through x `view`. Bars and areas, and
/// stacks, keep their bases in view; `axis` pins either end.
pub fn y_ticks(tracks: &[Track], view: Domain, mark: Mark, stacked: bool, axis: &Axis) -> Ticks {
    let with_base = stacked || matches!(mark, Mark::Bar | Mark::Area);
    let values = tracks
        .iter()
        .flatten()
        .filter(|(x, _)| view.contains(*x))
        .filter_map(|(_, extent)| *extent)
        .flat_map(|(base, top)| [Some(top), with_base.then_some(base)])
        .flatten()
        .chain(with_base.then_some(0.0));
    let data = Domain::of(values).unwrap_or_else(|| Domain::new(0.0, 1.0));
    let lo = axis.min.filter(|min| min.is_finite());
    let hi = axis.max.filter(|max| max.is_finite());
    let domain = Domain::new(lo.unwrap_or(data.lo), hi.unwrap_or(data.hi));
    nice_ticks(domain, 5, lo.is_some(), hi.is_some())
}

fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite())
}

fn lookup(points: &[(f64, Option<f64>)], x: f64) -> Option<f64> {
    points
        .binary_search_by(|(at, _)| at.total_cmp(&x))
        .ok()
        .and_then(|index| points[index].1)
}
