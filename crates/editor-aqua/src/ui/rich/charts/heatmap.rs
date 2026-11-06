use super::scale::{Domain, f32_of};
use serde::Deserialize;
use std::sync::Arc;

mod view;
#[cfg(feature = "test-support")]
pub use view::HeatmapFixture;
pub(crate) use view::{HeatmapState, TextRole, envelope};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Scale {
    #[default]
    Sequential,
    Diverging,
}

#[derive(Debug, Deserialize)]
struct HeatmapSpec {
    #[serde(default)]
    x: Vec<String>,
    #[serde(default)]
    y: Vec<String>,
    values: Vec<Vec<Option<f64>>>,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    scale: Scale,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "HeatmapSpec")]
pub(crate) struct Heatmap(Arc<HeatmapData>);
#[derive(Debug, PartialEq)]
pub(crate) struct HeatmapData {
    x: Vec<String>,
    y: Vec<String>,
    values: Vec<Vec<Option<f64>>>,
    unit: Option<String>,
    scale: Scale,
    domain: Domain,
    reach: f64,
    rows: usize,
    columns: usize,
    has_data: bool,
}
impl std::ops::Deref for Heatmap {
    type Target = HeatmapData;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Heatmap {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::{retained_optional, retained_strings, retained_vec};
        std::mem::size_of::<Self>()
            + std::mem::size_of::<HeatmapData>()
            + retained_strings(&self.x)
            + retained_strings(&self.y)
            + retained_vec(&self.values)
            + self.values.iter().map(retained_vec).sum::<usize>()
            + retained_optional(self.unit.as_ref())
    }
}
impl TryFrom<HeatmapSpec> for Heatmap {
    type Error = String;
    fn try_from(spec: HeatmapSpec) -> Result<Self, String> {
        let finite = || {
            spec.values
                .iter()
                .flatten()
                .filter_map(|value| *value)
                .filter(|value| value.is_finite())
        };
        let rows = spec.values.len().max(spec.y.len());
        let columns = spec
            .values
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(spec.x.len());
        let has_data = finite().next().is_some();
        let domain = Domain::of(finite()).unwrap_or_else(|| Domain::new(0.0, 1.0));
        let reach = finite().map(f64::abs).fold(0.0, f64::max);
        if !domain.lo.is_finite() || !domain.hi.is_finite() || !domain.span().is_finite() {
            return Err("heatmap range exceeds the finite preview domain".to_owned());
        }
        Ok(Self(Arc::new(HeatmapData {
            x: spec.x,
            y: spec.y,
            values: spec.values,
            unit: spec.unit,
            scale: spec.scale,
            domain,
            reach,
            rows,
            columns,
            has_data,
        })))
    }
}

impl Heatmap {
    pub(crate) fn rows(&self) -> usize {
        self.0.rows
    }
    pub(crate) fn columns(&self) -> usize {
        self.0.columns
    }

    pub(crate) fn cell(&self, row: usize, column: usize) -> Option<f64> {
        self.values
            .get(row)
            .and_then(|cells| cells.get(column).copied().flatten())
            .filter(|value| value.is_finite())
    }

    fn label(labels: &[String], index: usize) -> String {
        labels
            .get(index)
            .cloned()
            .unwrap_or_else(|| (index + 1).to_string())
    }

    fn x_label(&self, column: usize) -> String {
        Self::label(&self.x, column)
    }

    fn y_label(&self, row: usize) -> String {
        Self::label(&self.y, row)
    }

    pub(crate) fn position(&self, value: f64) -> f32 {
        match self.scale {
            Scale::Sequential => self.domain.fraction(value).clamp(0.0, 1.0),
            Scale::Diverging => {
                let reach = self.reach;
                if reach > 0.0 {
                    f32_of(value / reach).clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            }
        }
    }
}
