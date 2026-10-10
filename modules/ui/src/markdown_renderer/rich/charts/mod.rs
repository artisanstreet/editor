//! Data and chart kinds: `chart`, `concentration`, `heatmap`, `hierarchy`,
//! `distribution`, `funnel`, `sankey`, and `spans`. See `docs/rich-blocks.md`.
//!
//! Every kind draws its marks on one canvas from shapes laid out in plot
//! fractions ([`scene`]), and places its text as divs at the same fractions
//! ([`frame`]), so marks and labels share one scale without measuring text.

use std::rc::Rc;

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
mod slider;
mod spans;

/// Claims this family's kinds; `None` for every other kind.
pub(super) fn parse(kind: &str, value: &Value) -> Option<Result<Rc<dyn RichBlock>, String>> {
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

#[cfg(test)]
mod tests;
