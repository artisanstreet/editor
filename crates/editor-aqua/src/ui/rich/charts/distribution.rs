use super::chart::series_name;
use super::scale::{Domain, f64_of, index_of, nice_step, quantile, sorted_finite};
use serde::Deserialize;

mod view;
#[cfg(feature = "test-support")]
pub use view::DistributionFixture;
pub(crate) use view::{DistributionState, PlotFrame, ReadoutRow, Services, Tick, view};

const MAX_AUTO_BINS: usize = 50;
const MAX_BINS: usize = 200;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode {
    #[default]
    Histogram,
    Box,
    Cdf,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SeriesSpec {
    #[serde(default)]
    name: String,
    #[serde(default)]
    values: Vec<Option<f64>>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct DistributionSpec {
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Sample {
    pub(crate) name: String,
    pub(crate) sorted: Vec<f64>,
    pub(crate) summary: Option<Summary>,
    pub(crate) counts: Vec<usize>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct Distribution {
    pub(crate) mode: Mode,
    pub(crate) unit: Option<String>,
    pub(crate) threshold: Option<f64>,
    pub(crate) samples: Vec<Sample>,
    pub(crate) bins: Bins,
    domain: Domain,
    slider: (Domain, f64, f64),
}

pub(crate) fn prepare(value: &serde_json::Value) -> Result<Distribution, String> {
    let spec = DistributionSpec::deserialize(value).map_err(|error| error.to_string())?;
    let data = Distribution::from_spec(spec);
    if !data.domain.lo.is_finite() || !data.domain.hi.is_finite() || !data.domain.span().is_finite()
    {
        return Err("distribution range exceeds the finite preview domain".to_owned());
    }
    Ok(data)
}

impl Distribution {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::{retained_optional, retained_vec};
        std::mem::size_of::<Self>()
            + retained_optional(self.unit.as_ref())
            + retained_vec(&self.samples)
            + self
                .samples
                .iter()
                .map(|sample| {
                    sample.name.capacity()
                        + retained_vec(&sample.sorted)
                        + retained_vec(&sample.counts)
                        + sample
                            .summary
                            .as_ref()
                            .map_or(0, |summary| retained_vec(&summary.outliers))
                })
                .sum::<usize>()
    }
    pub(crate) fn from_spec(spec: DistributionSpec) -> Self {
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

    pub(crate) fn is_empty(&self) -> bool {
        self.samples.iter().all(|sample| sample.sorted.is_empty())
    }

    pub(crate) const fn domain(&self) -> Domain {
        self.domain
    }

    pub(crate) const fn slider(&self) -> (Domain, f64, f64) {
        self.slider
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bins {
    pub(crate) domain: Domain,
    pub(crate) count: usize,
}

impl Bins {
    pub(crate) fn new(pooled: &[f64], requested: Option<usize>) -> Self {
        let domain = Domain::of(pooled.iter().copied()).unwrap_or_else(|| Domain::new(0.0, 1.0));
        let count = requested.map_or_else(
            || auto_bins(pooled, domain).clamp(1, MAX_AUTO_BINS),
            |count| count.clamp(1, MAX_BINS),
        );
        Self { domain, count }
    }

    pub(crate) fn width(self) -> f64 {
        self.domain.span() / f64_of(self.count)
    }

    pub(crate) fn index(self, value: f64) -> usize {
        index_of((value - self.domain.lo) / self.width()).min(self.count - 1)
    }

    pub(crate) fn edges(self, index: usize) -> (f64, f64) {
        let lo = f64_of(index).mul_add(self.width(), self.domain.lo);
        (lo, lo + self.width())
    }

    pub(crate) fn counts(self, sorted: &[f64]) -> Vec<usize> {
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Summary {
    pub(crate) count: usize,
    pub(crate) min: f64,
    pub(crate) q1: f64,
    pub(crate) median: f64,
    pub(crate) q3: f64,
    pub(crate) max: f64,
    pub(crate) mean: f64,
    pub(crate) whiskers: (f64, f64),
    pub(crate) outliers: Vec<f64>,
}

pub(crate) fn summary(sorted: &[f64]) -> Option<Summary> {
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

pub(crate) fn share_at_or_below(sorted: &[f64], threshold: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    f64_of(sorted.partition_point(|value| *value <= threshold)) / f64_of(sorted.len())
}
