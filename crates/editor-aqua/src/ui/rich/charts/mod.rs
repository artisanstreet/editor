pub mod chart;
#[cfg(feature = "test-support")]
pub use chart::ChartFixture;
pub(crate) mod concentration;
#[cfg(feature = "test-support")]
pub use concentration::ConcentrationFixture;
pub(crate) mod distribution;
#[cfg(feature = "test-support")]
pub use distribution::DistributionFixture;
pub(crate) mod frame;
pub(crate) mod funnel;
#[cfg(feature = "test-support")]
pub use funnel::FunnelFixture;
pub(crate) mod heatmap;
#[cfg(feature = "test-support")]
pub use heatmap::HeatmapFixture;
pub(crate) mod hierarchy;
#[cfg(feature = "test-support")]
pub use hierarchy::HierarchyFixture;
pub(crate) mod sankey;
#[cfg(feature = "test-support")]
pub use sankey::SankeyFixture;
pub mod scale;
pub(crate) mod scene;
pub(crate) mod spans;
#[cfg(feature = "test-support")]
pub use spans::SpansFixture;

#[derive(Debug)]
pub(crate) enum Prepared {
    Chart(
        std::sync::Arc<chart::ChartStyle>,
        std::sync::Arc<chart::model::Model>,
    ),
    Concentration(concentration::Concentration),
    Distribution(std::sync::Arc<distribution::Distribution>),
    Funnel(funnel::Funnel),
    Heatmap(heatmap::Heatmap),
    Hierarchy(hierarchy::Prepared),
    Sankey(std::sync::Arc<sankey::Graph>),
    Spans(std::sync::Arc<spans::Layout>),
}
impl Prepared {
    pub(crate) fn resident_bytes(&self) -> usize {
        match self {
            Self::Chart(style, model) => style.resident_bytes() + model.resident_bytes(),
            Self::Concentration(value) => value.resident_bytes(),
            Self::Distribution(value) => value.resident_bytes(),
            Self::Funnel(value) => value.resident_bytes(),
            Self::Heatmap(value) => value.resident_bytes(),
            Self::Hierarchy(value) => value.resident_bytes(),
            Self::Sankey(value) => value.resident_bytes(),
            Self::Spans(value) => value.resident_bytes(),
        }
    }
}

pub(crate) fn prepare(kind: &str, value: &serde_json::Value) -> Option<Result<Prepared, String>> {
    if !matches!(
        kind,
        "chart"
            | "concentration"
            | "distribution"
            | "funnel"
            | "heatmap"
            | "hierarchy"
            | "sankey"
            | "spans"
    ) {
        return None;
    }
    if let Err(error) = preparation_budget(value) {
        return Some(Err(error));
    }
    let parse = || -> Result<Prepared, String> {
        use serde::Deserialize;
        Ok(match kind {
            "chart" => {
                let (style, model) = chart::prepare(value)?;
                Prepared::Chart(std::sync::Arc::new(style), std::sync::Arc::new(model))
            }
            "concentration" => Prepared::Concentration(
                concentration::Concentration::deserialize(value)
                    .map_err(|error| error.to_string())?,
            ),
            "distribution" => {
                Prepared::Distribution(std::sync::Arc::new(distribution::prepare(value)?))
            }
            "funnel" => Prepared::Funnel(
                funnel::Funnel::deserialize(value).map_err(|error| error.to_string())?,
            ),
            "heatmap" => Prepared::Heatmap(
                heatmap::Heatmap::deserialize(value).map_err(|error| error.to_string())?,
            ),
            "hierarchy" => Prepared::Hierarchy(hierarchy::prepare(value)?),
            "sankey" => Prepared::Sankey(std::sync::Arc::new(sankey::prepare(value)?)),
            "spans" => Prepared::Spans(std::sync::Arc::new(spans::prepare(value)?)),
            _ => return Err(format!("`{kind}` is not a rich block kind")),
        })
    };
    Some(parse())
}

fn preparation_budget(value: &serde_json::Value) -> Result<(), String> {
    let mut stack = vec![(value, 0usize)];
    let mut count = 0usize;
    while let Some((value, depth)) = stack.pop() {
        count += 1;
        if count > 16_384 || depth > 32 {
            return Err("chart preview exceeds its 16384-value or 32-level preparation budget; provide a summarized dataset".to_owned());
        }
        match value {
            serde_json::Value::Array(values) => {
                stack.extend(values.iter().map(|value| (value, depth + 1)))
            }
            serde_json::Value::Object(fields) => {
                stack.extend(fields.values().map(|value| (value, depth + 1)))
            }
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_) => {}
        }
    }
    Ok(())
}
