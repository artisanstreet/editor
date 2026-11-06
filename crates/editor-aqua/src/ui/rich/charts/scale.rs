use super::super::kit;

#[expect(clippy::cast_possible_truncation, reason = "plot fractions fit an f32")]
pub(crate) const fn f32_of(value: f64) -> f32 {
    value as f32
}

#[expect(clippy::cast_precision_loss, reason = "counts stay far below 2^52")]
pub(crate) const fn f64_of(count: usize) -> f64 {
    count as f64
}

#[expect(
    clippy::cast_precision_loss,
    reason = "counts a view lays out stay far below 2^24"
)]
pub(crate) const fn f32_of_count(count: usize) -> f32 {
    count as f32
}

pub(crate) fn index_of(value: f64) -> usize {
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

pub(crate) fn exact(value: f64) -> String {
    if value.is_finite() {
        format!("{value}")
    } else {
        String::new()
    }
}

pub(crate) fn reading(value: f64, unit: Option<&str>) -> String {
    kit::with_unit(&kit::format_number(value), unit)
}

pub(crate) fn percent(share: f64) -> String {
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Domain {
    pub(crate) lo: f64,
    pub(crate) hi: f64,
}

impl Domain {
    pub(crate) fn new(a: f64, b: f64) -> Self {
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

    pub(crate) fn of(values: impl IntoIterator<Item = f64>) -> Option<Self> {
        let mut extent: Option<(f64, f64)> = None;
        for value in values.into_iter().filter(|value| value.is_finite()) {
            extent = Some(match extent {
                Some((lo, hi)) => (lo.min(value), hi.max(value)),
                None => (value, value),
            });
        }
        extent.map(|(lo, hi)| Self::new(lo, hi))
    }

    pub(crate) fn including(self, value: f64) -> Self {
        if value.is_finite() {
            Self::new(self.lo.min(value), self.hi.max(value))
        } else {
            self
        }
    }

    pub(crate) fn span(self) -> f64 {
        self.hi - self.lo
    }

    pub(crate) fn contains(self, value: f64) -> bool {
        (self.lo..=self.hi).contains(&value)
    }

    pub(crate) fn fraction(self, value: f64) -> f32 {
        f32_of((value - self.lo) / self.span())
    }

    pub(crate) fn fraction_down(self, value: f64) -> f32 {
        1.0 - self.fraction(value)
    }

    pub(crate) fn value_at(self, t: f32) -> f64 {
        f64::from(t).mul_add(self.span(), self.lo)
    }
}

pub(crate) fn nice_step(raw: f64) -> f64 {
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Ticks {
    pub(crate) domain: Domain,
    pub(crate) values: Vec<f64>,
}

pub(crate) fn nice_ticks(domain: Domain, count: usize, pin_lo: bool, pin_hi: bool) -> Ticks {
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
        let value = if value.abs() < step * 1e-9 {
            0.0
        } else {
            (value / step).round() * step
        };
        values.push(value);
    }
    Ticks { domain, values }
}

pub(crate) fn ticks_within(domain: Domain, count: usize) -> Ticks {
    nice_ticks(domain, count, true, true)
}

pub(crate) fn quantile(sorted: &[f64], q: f64) -> Option<f64> {
    let last = sorted.len().checked_sub(1)?;
    let position = q.clamp(0.0, 1.0) * f64_of(last);
    let below = index_of(position).min(last);
    let above = (below + 1).min(last);
    let t = position - f64_of(below);
    Some(sorted[above].mul_add(t, sorted[below] * (1.0 - t)))
}

pub(crate) fn sorted_finite(values: impl IntoIterator<Item = f64>) -> Vec<f64> {
    let mut sorted: Vec<f64> = values
        .into_iter()
        .filter(|value| value.is_finite())
        .collect();
    sorted.sort_by(f64::total_cmp);
    sorted
}
