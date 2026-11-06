use super::scale::f64_of;
use serde::Deserialize;
use std::sync::Arc;

mod view;
#[cfg(feature = "test-support")]
pub use view::ConcentrationFixture;
pub(crate) use view::ConcentrationState;

#[derive(Debug, Deserialize)]
struct ConcentrationSpec {
    values: Vec<f64>,
    #[serde(default)]
    items: Option<String>,
    #[serde(default)]
    total: Option<String>,
    #[serde(default)]
    highlight: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(from = "ConcentrationSpec")]
pub(crate) struct Concentration(Arc<ConcentrationData>);
#[derive(Debug)]
pub(crate) struct ConcentrationData {
    items: Option<String>,
    total: Option<String>,
    highlight: Option<f64>,
    curve: Option<Arc<[(f64, f64)]>>,
}
impl std::ops::Deref for Concentration {
    type Target = ConcentrationData;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Concentration {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::retained_optional;
        std::mem::size_of::<Self>()
            + std::mem::size_of::<ConcentrationData>()
            + retained_optional(self.items.as_ref())
            + retained_optional(self.total.as_ref())
            + self.curve.as_deref().map_or(0, std::mem::size_of_val)
    }
}
impl From<ConcentrationSpec> for Concentration {
    fn from(spec: ConcentrationSpec) -> Self {
        let mut values: Vec<f64> = spec
            .values
            .into_iter()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .collect();
        values.sort_by(|a, b| b.total_cmp(a));
        let scale = values.first().copied().unwrap_or(0.0);
        let curve = if scale > 0.0 {
            let total: f64 = values.iter().map(|value| value / scale).sum();
            let mut running = 0.0;
            let mut points = Vec::with_capacity(values.len() + 1);
            points.push((0.0, 0.0));
            for (index, value) in values.iter().enumerate() {
                running += value / scale;
                points.push((f64_of(index + 1) / f64_of(values.len()), running / total));
            }
            Some(Arc::from(points))
        } else {
            None
        };
        Self(Arc::new(ConcentrationData {
            items: spec.items,
            total: spec.total,
            highlight: spec.highlight,
            curve,
        }))
    }
}

impl Concentration {
    fn items(&self) -> &str {
        self.items.as_deref().map_or("items", str::trim)
    }

    fn total(&self) -> &str {
        self.total.as_deref().map_or("the total", str::trim)
    }

    fn highlight(&self) -> f64 {
        self.highlight
            .filter(|share| share.is_finite() && *share > 0.0 && *share < 1.0)
            .unwrap_or(0.1)
    }

    pub(crate) fn curve(&self) -> Option<Arc<[(f64, f64)]>> {
        self.0.curve.clone()
    }

    pub(crate) fn share_of_total(curve: &[(f64, f64)], share: f64) -> f64 {
        let share = share.clamp(0.0, 1.0);
        let after = curve
            .partition_point(|(x, _)| *x < share)
            .min(curve.len() - 1);
        if after == 0 {
            return curve[0].1;
        }
        let (x0, y0) = curve[after - 1];
        let (x1, y1) = curve[after];
        if x1 - x0 <= f64::EPSILON {
            y1
        } else {
            y0 + (y1 - y0) * (share - x0) / (x1 - x0)
        }
    }
}
