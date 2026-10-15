//! Data and chart kinds: `chart`, `concentration`, `heatmap`, `hierarchy`,
//! `distribution`, `funnel`, `sankey`, and `spans`. See `docs/rich-blocks.md`.
//!
//! Every kind draws its marks on one canvas from shapes laid out in plot
//! fractions ([`scene`]), and places its text as divs at the same fractions
//! ([`frame`]), so marks and labels share one scale without measuring text.

use std::sync::Arc;

use serde_json::Value;

use super::{RichBlock, block};

mod chart;
mod concentration;
mod distribution;
mod frame;
mod funnel;
mod heatmap;
mod hierarchy;
mod sankey;
mod scale;
mod scene;
mod spans;

/// Claims this family's kinds; `None` for every other kind.
pub(super) fn parse(kind: &str, value: &Value) -> Option<Result<Arc<dyn RichBlock>, String>> {
    if !matches!(
        kind,
        "chart"
            | "concentration"
            | "heatmap"
            | "hierarchy"
            | "distribution"
            | "funnel"
            | "sankey"
            | "spans"
    ) {
        return None;
    }
    if let Err(error) = preparation_budget(value) {
        return Some(Err(error));
    }
    Some(match kind {
        "chart" => chart::parse(value),
        "concentration" => block::<concentration::Concentration>(value),
        "heatmap" => block::<heatmap::Heatmap>(value),
        "hierarchy" => hierarchy::parse(value),
        "distribution" => distribution::parse(value),
        "funnel" => block::<funnel::Funnel>(value),
        "sankey" => sankey::parse(value),
        "spans" => spans::parse(value),
        _ => return None,
    })
}

/// Work is bounded before a format-specific layout runs. The code fallback
/// retains the original source when a dataset needs an explicit summary.
fn preparation_budget(value: &Value) -> Result<(), String> {
    let mut stack = vec![(value, 0usize)];
    let mut count = 0usize;
    while let Some((value, depth)) = stack.pop() {
        count += 1;
        if count > 16_384 || depth > 32 {
            return Err("chart preview exceeds its 16384-value or 32-level preparation budget; provide a summarized dataset".to_owned());
        }
        match value {
            Value::Array(values) => stack.extend(values.iter().map(|value| (value, depth + 1))),
            Value::Object(fields) => stack.extend(fields.values().map(|value| (value, depth + 1))),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
