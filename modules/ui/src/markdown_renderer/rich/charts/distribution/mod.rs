//! `distribution`: the shape of values as a histogram, box plot, or CDF,
//! several series overlaid, with a threshold slider in histogram and CDF
//! modes reporting the share at or below it.

use std::fmt::Write as _;
use std::rc::Rc;

use gpui::{AnyElement, IntoElement as _};
use serde::Deserialize;
use serde_json::Value;

use super::super::{BlockContext, RichBlock, kit, spec};
use super::chart::series_name;
use super::scale::{Domain, exact, f64_of, index_of, nice_step, percent, quantile, sorted_finite};

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
#[derive(Clone, Debug)]
pub(super) struct Sample {
    pub(super) name: String,
    pub(super) sorted: Vec<f64>,
}

/// A parsed distribution, its samples sorted once.
#[derive(Debug)]
pub(super) struct Distribution {
    pub(super) mode: Mode,
    pub(super) unit: Option<String>,
    pub(super) threshold: Option<f64>,
    pub(super) samples: Vec<Sample>,
    pub(super) bins: Bins,
}

pub(super) fn parse(value: &Value) -> Result<Rc<dyn RichBlock>, String> {
    let spec = spec::<DistributionSpec>(value)?;
    Ok(Rc::new(Block(Rc::new(Distribution::from_spec(spec)))))
}

impl Distribution {
    pub(super) fn from_spec(spec: DistributionSpec) -> Self {
        let samples: Vec<Sample> = spec
            .series
            .into_iter()
            .enumerate()
            .map(|(index, series)| Sample {
                name: series_name(&series.name, index).to_string(),
                sorted: sorted_finite(series.values.into_iter().flatten()),
            })
            .collect();
        let pooled = sorted_finite(
            samples
                .iter()
                .flat_map(|sample| sample.sorted.iter().copied()),
        );
        let bins = Bins::new(&pooled, spec.bins);
        Self {
            mode: spec.mode,
            unit: spec.unit.filter(|unit| !unit.trim().is_empty()),
            threshold: spec.threshold.filter(|threshold| threshold.is_finite()),
            samples,
            bins,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.samples.iter().all(|sample| sample.sorted.is_empty())
    }

    /// The extent of every value.
    pub(super) fn domain(&self) -> Domain {
        Domain::of(
            self.samples
                .iter()
                .flat_map(|sample| sample.sorted.iter().copied()),
        )
        .unwrap_or_else(|| Domain::new(0.0, 1.0))
    }

    /// The threshold slider's range, step, and starting value.
    pub(super) fn slider(&self) -> (Domain, f64, f64) {
        let domain = self.domain();
        let step = nice_step(domain.span() / 100.0);
        let pooled = sorted_finite(
            self.samples
                .iter()
                .flat_map(|sample| sample.sorted.iter().copied()),
        );
        let start = self
            .threshold
            .or_else(|| quantile(&pooled, 0.5))
            .unwrap_or(domain.lo);
        (domain.including(start), step, start)
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
        mean: sorted.iter().sum::<f64>() / f64_of(sorted.len()),
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
pub(super) struct Block(pub(super) Rc<Distribution>);

impl RichBlock for Block {
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        view::DistributionView {
            id: cx.key("distribution"),
            theme: *cx.theme,
            data: Rc::clone(&self.0),
        }
        .into_any_element()
    }

    fn markdown(&self) -> String {
        self.0.markdown()
    }
}

impl Distribution {
    /// The summary table, the bins in histogram mode, and the threshold.
    pub(super) fn markdown(&self) -> String {
        if self.is_empty() {
            return "_No data._\n".to_owned();
        }
        let header: Vec<String> = ["Series", "n", "min", "q1", "median", "q3", "max", "mean"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let rows: Vec<Vec<String>> = self
            .samples
            .iter()
            .filter_map(|sample| {
                let summary = summary(&sample.sorted)?;
                Some(vec![
                    sample.name.clone(),
                    summary.count.to_string(),
                    kit::format_number(summary.min),
                    kit::format_number(summary.q1),
                    kit::format_number(summary.median),
                    kit::format_number(summary.q3),
                    kit::format_number(summary.max),
                    kit::format_number(summary.mean),
                ])
            })
            .collect();
        let mut out = kit::markdown_table(&header, &rows);
        if self.mode == Mode::Histogram {
            out.push('\n');
            out.push_str(&self.bins_table());
        }
        if let Some(unit) = &self.unit {
            let _ = write!(out, "\n_Values in {}_\n", unit.trim());
        }
        if let Some(threshold) = self.threshold {
            let _ = write!(out, "\n- Threshold: {}", exact(threshold));
            for sample in &self.samples {
                let _ = write!(
                    out,
                    "\n  - {}: {} at or below",
                    sample.name,
                    percent(share_at_or_below(&sample.sorted, threshold))
                );
            }
            out.push('\n');
        }
        out
    }
}

impl Distribution {
    fn bins_table(&self) -> String {
        let mut header = vec!["Bin".to_owned()];
        header.extend(self.samples.iter().map(|sample| sample.name.clone()));
        let counts: Vec<Vec<usize>> = self
            .samples
            .iter()
            .map(|sample| self.bins.counts(&sample.sorted))
            .collect();
        let rows: Vec<Vec<String>> = (0..self.bins.count)
            .map(|bin| {
                let (lo, hi) = self.bins.edges(bin);
                let mut row = vec![format!(
                    "{} – {}",
                    kit::format_number(lo),
                    kit::format_number(hi)
                )];
                row.extend(counts.iter().map(|counts| counts[bin].to_string()));
                row
            })
            .collect();
        kit::markdown_table(&header, &rows)
    }
}
