//! `chart`: line, area, bar, and scatter over categories or numbers.
//!
//! The spec resolves once, at parse time, into a [`model::Model`] of sorted
//! series over one x scale. Rendering recomputes the visible geometry from
//! the window state (hidden series, hover, zoom, thresholds) every frame.

use crate::markdown_renderer::rich::budget;
use std::sync::Arc;

use gpui::{AnyElement, IntoElement as _, SharedString};
use serde::Deserialize;
use serde_json::Value;

use super::super::{BlockContext, RichBlock, spec};

pub(super) mod model;
mod plot;
mod view;

/// How each series is drawn.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Mark {
    #[default]
    Line,
    Area,
    Bar,
    Scatter,
}

/// An x value: a number, or a category or time-like string.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub(super) enum XValue {
    Number(f64),
    Text(String),
}

/// One series: `values` along the categories, or `[x, y]` points.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct SeriesSpec {
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) values: Option<Vec<Option<f64>>>,
    #[serde(default)]
    pub(super) points: Option<Vec<(XValue, Option<f64>)>>,
}

/// An axis's label, unit, and pinned ends.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(super) struct Axis {
    #[serde(default)]
    pub(super) label: Option<String>,
    #[serde(default)]
    pub(super) unit: Option<String>,
    #[serde(default)]
    pub(super) min: Option<f64>,
    #[serde(default)]
    pub(super) max: Option<f64>,
}

/// A vertical rule (`x`), a horizontal rule (`y`), or a band (`x`..`x_end`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct Annotation {
    #[serde(default)]
    pub(super) x: Option<XValue>,
    #[serde(default)]
    pub(super) y: Option<f64>,
    #[serde(default)]
    pub(super) x_end: Option<XValue>,
    #[serde(default)]
    pub(super) label: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum ParamKind {
    #[default]
    Threshold,
    #[serde(other)]
    Unknown,
}

/// A declared control; `threshold` moves a horizontal rule.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct Param {
    #[serde(default)]
    pub(super) id: String,
    #[serde(default)]
    pub(super) kind: ParamKind,
    #[serde(default)]
    pub(super) label: String,
    pub(super) min: f64,
    pub(super) max: f64,
    #[serde(default)]
    pub(super) step: Option<f64>,
    pub(super) value: f64,
}

/// The `chart` spec.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct ChartSpec {
    #[serde(default)]
    pub(super) mark: Mark,
    #[serde(default)]
    pub(super) stacked: bool,
    #[serde(default)]
    pub(super) normalize: bool,
    #[serde(default)]
    pub(super) categories: Option<Vec<String>>,
    pub(super) series: Vec<SeriesSpec>,
    #[serde(default)]
    pub(super) x: Axis,
    #[serde(default)]
    pub(super) y: Axis,
    #[serde(default)]
    pub(super) annotations: Vec<Annotation>,
    #[serde(default)]
    pub(super) params: Vec<Param>,
}

/// Render configuration after raw samples/categories move into the indexed model.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ChartStyle {
    pub(super) mark: Mark,
    pub(super) stacked: bool,
    pub(super) normalize: bool,
    pub(super) x: Axis,
    pub(super) y: Axis,
    pub(super) annotations: Vec<Annotation>,
    pub(super) params: Vec<Param>,
}
impl From<ChartSpec> for ChartStyle {
    fn from(spec: ChartSpec) -> Self {
        Self {
            mark: spec.mark,
            stacked: spec.stacked,
            normalize: spec.normalize,
            x: spec.x,
            y: spec.y,
            annotations: spec.annotations,
            params: spec.params,
        }
    }
}

impl ChartStyle {
    /// Stacks when asked to, and always when normalizing to shares.
    pub(super) fn stacks(&self) -> bool {
        self.stacked || self.normalize
    }

    /// The threshold sliders this chart declares.
    pub(super) fn thresholds(&self) -> impl Iterator<Item = (usize, &Param)> {
        self.params
            .iter()
            .enumerate()
            .filter(|(_, param)| param.kind == ParamKind::Threshold)
    }
}

/// A parsed chart: its spec and the model resolved from it.
#[derive(Debug)]
pub(super) struct Chart {
    spec: Arc<ChartStyle>,
    model: Arc<model::Model>,
}

/// Parses a `chart` spec.
pub(super) fn parse(value: &Value) -> Result<Arc<dyn RichBlock>, String> {
    let spec = spec::<ChartSpec>(value)?;
    let model = model::Model::from_spec(&spec);
    if model.series.len() > 128
        || model
            .series
            .iter()
            .map(|series| series.points.len())
            .sum::<usize>()
            > 16384
    {
        return Err("chart preview supports at most 128 series and 16384 populated points".into());
    }
    let spec = ChartStyle::from(spec);
    if spec.stacks() && model.series.len().saturating_mul(model.xs.len()) > 16_384 {
        return Err(
            "stacked chart preview exceeds 16384 aligned cells; use fewer series or points"
                .to_owned(),
        );
    }
    if spec.stacks() && !spec.normalize {
        let tracks = model.tracks(&vec![true; model.series.len()], true, false);
        if tracks
            .iter()
            .flatten()
            .filter_map(|(_, extent)| *extent)
            .any(|(start, end)| !start.is_finite() || !end.is_finite())
        {
            return Err("stacked values exceed the finite chart range".to_owned());
        }
    }
    Ok(Arc::new(Chart {
        spec: Arc::new(spec),
        model: Arc::new(model),
    }))
}

impl RichBlock for Chart {
    fn resident_bytes(&self) -> usize {
        let x = |value: &XValue| match value {
            XValue::Text(text) => text.capacity(),
            XValue::Number(_) => 0,
        };
        let axis = |axis: &Axis| budget::optional(&axis.label) + budget::optional(&axis.unit);
        std::mem::size_of::<Self>()
            + std::mem::size_of::<ChartStyle>()
            + std::mem::size_of::<model::Model>()
            + axis(&self.spec.x)
            + axis(&self.spec.y)
            + budget::vector(&self.spec.annotations)
            + self
                .spec
                .annotations
                .iter()
                .map(|note| {
                    note.label.capacity()
                        + note.x.as_ref().map_or(0, x)
                        + note.x_end.as_ref().map_or(0, x)
                })
                .sum::<usize>()
            + budget::vector(&self.spec.params)
            + self
                .spec
                .params
                .iter()
                .map(|param| param.id.capacity() + param.label.capacity())
                .sum::<usize>()
            + budget::vector(&self.model.xs)
            + budget::vector(&self.model.series)
            + self
                .model
                .series
                .iter()
                .map(|series| series.name.capacity() + budget::vector(&series.points))
                .sum::<usize>()
            + match &self.model.x {
                model::XScale::Category(names) => budget::strings(names),
                model::XScale::Number => 0,
            }
    }
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        view::ChartView {
            id: cx.key("chart"),
            theme: *cx.theme,
            spec: Arc::clone(&self.spec),
            model: Arc::clone(&self.model),
        }
        .into_any_element()
    }
}

/// The name drawn for a series: its own, or its position.
pub(super) fn series_name(name: &str, index: usize) -> SharedString {
    let name = name.trim();
    if name.is_empty() {
        SharedString::from(format!("Series {}", index + 1))
    } else {
        SharedString::from(name.to_owned())
    }
}
