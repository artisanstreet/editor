//! `distribution`: the shape of values as a histogram, box plot, or CDF,
//! several series overlaid, with a threshold slider in histogram and CDF
//! modes reporting the share at or below it.

use crate::markdown_renderer::rich::budget;
use std::sync::Arc;

use gpui::{AnyElement, IntoElement as _};
use serde::Deserialize;
use serde_json::Value;

use super::super::{BlockContext, RichBlock, spec};
use super::chart::series_name;
use super::scale::{Domain, f64_of, index_of, nice_step, quantile, sorted_finite};

mod view;

/// Bins a histogram never exceeds unless the spec asks.
const MAX_AUTO_BINS: usize = 50;

/// Bins a spec may ask for.
const MAX_BINS: usize = 200;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Mode {
    #[default]
    Histogram,
    Box,
    Cdf,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct SeriesSpec {
    #[serde(default)]
    name: String,
    #[serde(default)]
    values: Vec<Option<f64>>,
}

/// The `distribution` spec.
#[derive(Clone, Debug, Deserialize)]
pub(super) struct DistributionSpec {
    #[serde(default)]
    mode: Mode,
    series: Vec<SeriesSpec>,
    #[serde(default)]
    bins: Option<usize>,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    threshold: Option<f64>,
}

/// One series' finite values, sorted.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Sample {
    pub(super) name: String,
    pub(super) sorted: Vec<f64>,
    pub(super) summary: Option<Summary>,
    pub(super) counts: Vec<usize>,
}

/// A parsed distribution, its samples sorted once.
#[derive(Debug, PartialEq)]
pub(super) struct Distribution {
    pub(super) mode: Mode,
    pub(super) unit: Option<String>,
    pub(super) threshold: Option<f64>,
    pub(super) samples: Vec<Sample>,
    pub(super) bins: Bins,
    domain: Domain,
    slider: (Domain, f64, f64),
}

pub(super) fn parse(value: &Value) -> Result<Arc<dyn RichBlock>, String> {
    let spec = spec::<DistributionSpec>(value)?;
    let data = Distribution::from_spec(spec);
    if !data.domain.lo.is_finite() || !data.domain.hi.is_finite() || !data.domain.span().is_finite()
    {
        return Err("distribution range exceeds the finite preview domain".to_owned());
    }
    Ok(Arc::new(Block(Arc::new(data))))
}

impl Distribution {
    pub(super) fn from_spec(spec: DistributionSpec) -> Self {
        let mut samples: Vec<Sample> = spec
            .series
            .into_iter()
            .enumerate()
            .map(|(index, series)| Sample {
                name: series_name(&series.name, index).to_string(),
                sorted: sorted_finite(series.values.into_iter().flatten()),
                summary: None,
                counts: Vec::new(),
            })
            .collect();
        let pooled = sorted_finite(
            samples
                .iter()
                .flat_map(|sample| sample.sorted.iter().copied()),
        );
        let threshold = spec.threshold.filter(|value| value.is_finite());
        let mut bins = Bins::new(&pooled, spec.bins);
        let start = threshold
            .or_else(|| quantile(&pooled, 0.5))
            .unwrap_or(bins.domain.lo);
        let domain = bins.domain.including(start);
        bins.domain = domain;
        let slider = (domain, nice_step(domain.span() / 100.0), start);
        for sample in &mut samples {
            sample.summary = summary(&sample.sorted);
            sample.counts = bins.counts(&sample.sorted);
        }
        Self {
            mode: spec.mode,
            unit: spec.unit.filter(|unit| !unit.trim().is_empty()),
            threshold,
            samples,
            bins,
            domain,
            slider,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.samples.iter().all(|sample| sample.sorted.is_empty())
    }

    /// The extent of every value.
    pub(super) fn domain(&self) -> Domain {
        self.domain
    }

    /// The threshold slider's range, step, and starting value.
    pub(super) fn slider(&self) -> (Domain, f64, f64) {
        self.slider
    }
}

/// Equal-width bins over the pooled values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Bins {
    pub(super) domain: Domain,
    pub(super) count: usize,
}

impl Bins {
    /// `requested` bins, or Freedman–Diaconis bins (the square-root rule
    /// when the spread is zero), capped.
    pub(super) fn new(pooled: &[f64], requested: Option<usize>) -> Self {
        let domain = Domain::of(pooled.iter().copied()).unwrap_or_else(|| Domain::new(0.0, 1.0));
        let count = match requested {
            Some(count) => count.clamp(1, MAX_BINS),
            None => auto_bins(pooled, domain).clamp(1, MAX_AUTO_BINS),
        };
        Self { domain, count }
    }

    pub(super) fn width(self) -> f64 {
        self.domain.span() / f64_of(self.count)
    }

    /// The bin holding `value`; the top edge belongs to the last bin.
    pub(super) fn index(self, value: f64) -> usize {
        index_of((value - self.domain.lo) / self.width()).min(self.count - 1)
    }

    /// Bin `index`'s edges.
    pub(super) fn edges(self, index: usize) -> (f64, f64) {
        let lo = self.domain.lo + f64_of(index) * self.width();
        (lo, lo + self.width())
    }

    /// How many of `sorted` fall in each bin.
    pub(super) fn counts(self, sorted: &[f64]) -> Vec<usize> {
        let mut counts = vec![0; self.count];
        for value in sorted {
            counts[self.index(*value)] += 1;
        }
        counts
    }
}

fn auto_bins(pooled: &[f64], domain: Domain) -> usize {
    let n = pooled.len();
    if n < 2 {
        return 1;
    }
    let sqrt_rule = index_of(f64_of(n).sqrt().ceil());
    let (Some(q1), Some(q3)) = (quantile(pooled, 0.25), quantile(pooled, 0.75)) else {
        return sqrt_rule;
    };
    let width = 2.0 * (q3 - q1) / f64_of(n).cbrt();
    if width > 0.0 && width.is_finite() {
        index_of((domain.span() / width).ceil()).max(1)
    } else {
        sqrt_rule
    }
}

/// The five-number summary with Tukey whiskers.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Summary {
    pub(super) count: usize,
    pub(super) min: f64,
    pub(super) q1: f64,
    pub(super) median: f64,
    pub(super) q3: f64,
    pub(super) max: f64,
    pub(super) mean: f64,
    /// The furthest values within 1.5 IQR of the box.
    pub(super) whiskers: (f64, f64),
    pub(super) outliers: Vec<f64>,
}

pub(super) fn summary(sorted: &[f64]) -> Option<Summary> {
    let (min, max) = (*sorted.first()?, *sorted.last()?);
    let q1 = quantile(sorted, 0.25)?;
    let median = quantile(sorted, 0.5)?;
    let q3 = quantile(sorted, 0.75)?;
    let reach = 1.5 * (q3 - q1);
    let (fence_lo, fence_hi) = (q1 - reach, q3 + reach);
    let inside = || {
        sorted
            .iter()
            .copied()
            .filter(|value| (fence_lo..=fence_hi).contains(value))
    };
    let whiskers = (
        inside().next().unwrap_or(q1),
        inside().next_back().unwrap_or(q3),
    );
    let outliers = sorted
        .iter()
        .copied()
        .filter(|value| !(fence_lo..=fence_hi).contains(value))
        .collect();
    Some(Summary {
        count: sorted.len(),
        min,
        q1,
        median,
        q3,
        max,
        mean: {
            let scale = min.abs().max(max.abs());
            if scale == 0.0 {
                0.0
            } else {
                sorted.iter().map(|value| value / scale).sum::<f64>() / f64_of(sorted.len()) * scale
            }
        },
        whiskers,
        outliers,
    })
}

/// The share of `sorted` at or below `threshold`.
pub(super) fn share_at_or_below(sorted: &[f64], threshold: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    f64_of(sorted.partition_point(|value| *value <= threshold)) / f64_of(sorted.len())
}

/// The block: a shared distribution, so frames don't copy the samples.
#[derive(Debug)]
pub(super) struct Block(pub(super) Arc<Distribution>);

impl RichBlock for Block {
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<Distribution>()
            + budget::optional(&self.0.unit)
            + budget::vector(&self.0.samples)
            + self
                .0
                .samples
                .iter()
                .map(|sample| {
                    sample.name.capacity()
                        + budget::vector(&sample.sorted)
                        + budget::vector(&sample.counts)
                        + sample
                            .summary
                            .as_ref()
                            .map_or(0, |summary| budget::vector(&summary.outliers))
                })
                .sum::<usize>()
    }

    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        view::DistributionView {
            id: cx.key("distribution"),
            theme: *cx.theme,
            data: Arc::clone(&self.0),
        }
        .into_any_element()
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn threshold_and_summary_remain_finite_at_numeric_boundaries() {
        let data = Distribution::from_spec(
            serde_json::from_value(
                serde_json::json!({"series":[{"values":[1,2]}],"threshold":100}),
            )
            .unwrap(),
        );
        assert!(data.domain().contains(100.0));
        assert_eq!(data.slider().0, data.domain());
        assert!(summary(&[1e308, 1e308]).unwrap().mean.is_finite());
    }
}
