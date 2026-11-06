use super::scale::f32_of;
use serde::Deserialize;
use std::sync::Arc;

mod view;
#[cfg(feature = "test-support")]
pub use view::FunnelFixture;
pub(crate) use view::{FunnelState, TextRole};

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Step {
    #[serde(default)]
    label: String,
    value: f64,
}

#[derive(Debug, Deserialize)]
struct FunnelSpec {
    steps: Vec<Step>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "FunnelSpec")]
pub(crate) struct Funnel {
    steps: Arc<[Step]>,
    widest: f64,
}
impl TryFrom<FunnelSpec> for Funnel {
    type Error = String;
    fn try_from(spec: FunnelSpec) -> Result<Self, String> {
        if spec
            .steps
            .iter()
            .any(|step| !step.value.is_finite() || step.value < 0.0)
        {
            return Err("funnel values must be finite and nonnegative".to_owned());
        }
        if spec.steps.len() > 64 {
            return Err("funnel preview supports at most 64 steps".to_owned());
        }
        let widest = spec.steps.iter().map(|step| step.value).fold(0.0, f64::max);
        Ok(Self {
            steps: Arc::from(spec.steps),
            widest,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StepReading {
    pub(crate) value: f64,
    pub(crate) of_first: Option<f64>,
    pub(crate) drop: Option<(f64, Option<f64>)>,
}

impl Funnel {
    pub(crate) fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of_val(self.steps.as_ref())
            + self
                .steps
                .iter()
                .map(|step| step.label.capacity())
                .sum::<usize>()
    }
    fn value(&self, index: usize) -> f64 {
        self.steps
            .get(index)
            .map(|step| step.value)
            .filter(|value| value.is_finite())
            .unwrap_or(0.0)
    }

    fn label(&self, index: usize) -> String {
        let label = self.steps.get(index).map_or("", |step| step.label.trim());
        if label.is_empty() {
            format!("Step {}", index + 1)
        } else {
            label.to_owned()
        }
    }

    pub(crate) fn reading(&self, index: usize) -> StepReading {
        let value = self.value(index);
        let first = self.value(0);
        let share = |part: f64, whole: f64| (whole > 0.0).then(|| part / whole);
        StepReading {
            value,
            of_first: share(value, first),
            drop: index.checked_sub(1).map(|previous| {
                let before = self.value(previous);
                (before - value, share(before - value, before))
            }),
        }
    }

    fn width(&self, index: usize) -> f32 {
        if self.widest > 0.0 {
            f32_of(self.value(index) / self.widest)
        } else {
            0.0
        }
    }
}
