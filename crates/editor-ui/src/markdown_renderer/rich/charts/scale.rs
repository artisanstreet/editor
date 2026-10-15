//! Domains, nice ticks, and the narrow numeric casts every chart shares.
//!
//! Data stays `f64`; plot positions are `f32` fractions of the plot, 0 at
//! the left or top edge and 1 at the right or bottom edge.

use super::super::kit;

/// A plot fraction from a data fraction.
#[expect(clippy::cast_possible_truncation, reason = "plot fractions fit an f32")]
pub(super) fn f32_of(value: f64) -> f32 {
    value as f32
}

/// A count as a float.
#[expect(clippy::cast_precision_loss, reason = "counts stay far below 2^52")]
pub(super) fn f64_of(count: usize) -> f64 {
    count as f64
}

/// The floor of `value` as an index; 0 for negative or non-finite values.
pub(super) fn index_of(value: f64) -> usize {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "checked finite and positive; the cast saturates"
    )]
    let index = value.floor() as usize;
    index
}

/// A number written exactly, for Markdown: `3`, `0.1`, `-2.5`.
pub(super) fn exact(value: f64) -> String {
    if value.is_finite() {
        format!("{value}")
    } else {
        String::new()
    }
}

/// A number for reading, with its unit.
pub(super) fn reading(value: f64, unit: Option<&str>) -> String {
    kit::with_unit(&kit::format_number(value), unit)
}

/// A share in 0..=1 as a percentage.
pub(super) fn percent(share: f64) -> String {
    if !share.is_finite() {
        return "–".to_owned();
    }
    let percent = share * 100.0;
    if percent > 0.0 && percent < 1.0 {
        format!("{percent:.1}%")
    } else {
        format!("{percent:.0}%")
    }
}

/// A closed interval of data values; never zero-width and always finite.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Domain {
    pub(super) lo: f64,
    pub(super) hi: f64,
}

impl Domain {
    /// The interval between `a` and `b`, widened when they meet.
    pub(super) fn new(a: f64, b: f64) -> Self {
        let (a, b) = match (a.is_finite(), b.is_finite()) {
            (true, true) => (a, b),
            (true, false) => (a, a),
            (false, true) => (b, b),
            (false, false) => (0.0, 1.0),
        };
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let scale = lo.abs().max(hi.abs()).max(1.0);
        if hi - lo > scale * 1e-12 {
            return Self { lo, hi };
        }
        let pad = if lo.abs() > 0.0 { lo.abs() * 0.1 } else { 1.0 };
        Self {
            lo: lo - pad,
            hi: hi + pad,
        }
    }

    /// The extent of the finite `values`; `None` when there are none.
    pub(super) fn of(values: impl IntoIterator<Item = f64>) -> Option<Self> {
        let mut extent: Option<(f64, f64)> = None;
        for value in values.into_iter().filter(|value| value.is_finite()) {
            extent = Some(match extent {
                Some((lo, hi)) => (lo.min(value), hi.max(value)),
                None => (value, value),
            });
        }
        extent.map(|(lo, hi)| Self::new(lo, hi))
    }

    /// The domain stretched to include `value`.
    pub(super) fn including(self, value: f64) -> Self {
        if value.is_finite() {
            Self::new(self.lo.min(value), self.hi.max(value))
        } else {
            self
        }
    }

    pub(super) fn span(self) -> f64 {
        self.hi - self.lo
    }

    pub(super) fn contains(self, value: f64) -> bool {
        (self.lo..=self.hi).contains(&value)
    }

    /// Where `value` falls, 0 at `lo` and 1 at `hi`; unclamped.
    pub(super) fn fraction(self, value: f64) -> f32 {
        f32_of((value - self.lo) / self.span())
    }

    /// Where `value` falls measured from the top: 0 at `hi`, 1 at `lo`.
    pub(super) fn fraction_down(self, value: f64) -> f32 {
        1.0 - self.fraction(value)
    }

    /// The value at fraction `t`.
    pub(super) fn value_at(self, t: f32) -> f64 {
        self.lo + f64::from(t) * self.span()
    }
}

/// A round step near `raw`: 1, 2, or 5 times a power of ten.
pub(super) fn nice_step(raw: f64) -> f64 {
    if !raw.is_finite() || raw <= 0.0 {
        return 1.0;
    }
    let base = 10f64.powf(raw.log10().floor());
    let fraction = raw / base;
    let nice = if fraction <= 1.0 {
        1.0
    } else if fraction <= 2.0 {
        2.0
    } else if fraction <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * base
}

/// Tick values over a domain, and the domain they cover.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Ticks {
    pub(super) domain: Domain,
    pub(super) values: Vec<f64>,
}

/// About `count` round ticks over `domain`. Unpinned ends extend outward to
/// the next round value; pinned ends stay where they are.
pub(super) fn nice_ticks(domain: Domain, count: usize, pin_lo: bool, pin_hi: bool) -> Ticks {
    let step = nice_step(domain.span() / f64_of(count.max(1)));
    let lo = if pin_lo {
        domain.lo
    } else {
        (domain.lo / step).floor() * step
    };
    let hi = if pin_hi {
        domain.hi
    } else {
        (domain.hi / step).ceil() * step
    };
    let domain = Domain::new(lo, hi);
    let first = (domain.lo / step).ceil();
    let mut values = Vec::new();
    for k in 0..=200u32 {
        let value = (first + f64::from(k)) * step;
        if value > domain.hi + step * 1e-9 {
            break;
        }
        // Snap float noise such as 0.30000000000000004 and -0.
        let value = if value.abs() < step * 1e-9 {
            0.0
        } else {
            (value / step).round() * step
        };
        values.push(value);
    }
    Ticks { domain, values }
}

/// Ticks inside `domain` without moving its ends: zoomed or pinned axes.
pub(super) fn ticks_within(domain: Domain, count: usize) -> Ticks {
    nice_ticks(domain, count, true, true)
}

/// Linear-interpolated quantile `q` (0..=1) of already sorted values.
pub(super) fn quantile(sorted: &[f64], q: f64) -> Option<f64> {
    let last = sorted.len().checked_sub(1)?;
    let position = q.clamp(0.0, 1.0) * f64_of(last);
    let below = index_of(position).min(last);
    let above = (below + 1).min(last);
    let t = position - f64_of(below);
    Some(sorted[below] * (1.0 - t) + sorted[above] * t)
}

/// The finite values, sorted.
pub(super) fn sorted_finite(values: impl IntoIterator<Item = f64>) -> Vec<f64> {
    let mut sorted: Vec<f64> = values
        .into_iter()
        .filter(|value| value.is_finite())
        .collect();
    sorted.sort_by(f64::total_cmp);
    sorted
}
