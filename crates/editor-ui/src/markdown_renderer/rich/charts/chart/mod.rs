//! `chart`: line, area, bar, and scatter over categories or numbers.
//!
//! The spec resolves once, at parse time, into a [`model::Model`] of sorted
//! series over one x scale. Rendering recomputes the visible geometry from
//! the window state (hidden series, hover, zoom, thresholds) every frame.

use std::fmt::Write as _;
use std::rc::Rc;

use gpui::{AnyElement, IntoElement as _, SharedString};
use serde::Deserialize;
use serde_json::Value;

use super::super::{BlockContext, RichBlock, kit, spec};
use super::scale::exact;

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

impl XValue {
    fn display(&self) -> String {
        match self {
            Self::Number(value) => exact(*value),
            Self::Text(text) => text.clone(),
        }
    }
}

/// One series: `values` along the categories, or `[x, y]` points.
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SeriesSpec {
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) values: Option<Vec<Option<f64>>>,
    #[serde(default)]
    pub(super) points: Option<Vec<(XValue, Option<f64>)>>,
}

/// An axis's label, unit, and pinned ends.
#[derive(Clone, Debug, Default, Deserialize)]
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
#[derive(Clone, Debug, Deserialize)]
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
#[derive(Clone, Debug, Deserialize)]
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
#[derive(Clone, Debug, Deserialize)]
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

impl ChartSpec {
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
    spec: Rc<ChartSpec>,
    model: Rc<model::Model>,
}

/// Parses a `chart` spec.
pub(super) fn parse(value: &Value) -> Result<Rc<dyn RichBlock>, String> {
    let spec = spec::<ChartSpec>(value)?;
    let model = model::Model::from_spec(&spec);
    Ok(Rc::new(Chart {
        spec: Rc::new(spec),
        model: Rc::new(model),
    }))
}

impl RichBlock for Chart {
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        view::ChartView {
            id: cx.key("chart"),
            theme: *cx.theme,
            spec: Rc::clone(&self.spec),
            model: Rc::clone(&self.model),
        }
        .into_any_element()
    }

    fn markdown(&self) -> String {
        markdown(&self.spec, &self.model)
    }
}

/// The data as a table (x by series), then annotations and thresholds.
pub(super) fn markdown(spec: &ChartSpec, model: &model::Model) -> String {
    let x_header = spec
        .x
        .label
        .clone()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| {
            if model.is_categorical() {
                "Category"
            } else {
                "x"
            }
            .to_owned()
        });
    let mut header = vec![x_header];
    header.extend(model.series.iter().map(|series| series.name.clone()));
    let aligned: Vec<Vec<Option<f64>>> = (0..model.series.len())
        .map(|index| model.aligned(index))
        .collect();
    let rows: Vec<Vec<String>> = model
        .xs
        .iter()
        .enumerate()
        .map(|(row, x)| {
            let mut cells = vec![model.x_label(*x)];
            cells.extend(
                aligned
                    .iter()
                    .map(|values| values[row].map(exact).unwrap_or_default()),
            );
            cells
        })
        .collect();
    let mut out = if model.series.is_empty() {
        "_No data._\n".to_owned()
    } else {
        kit::markdown_table(&header, &rows)
    };
    let units: Vec<String> = [("x", &spec.x), ("y", &spec.y)]
        .iter()
        .filter_map(|(axis, spec)| {
            let unit = spec.unit.as_deref()?.trim();
            (!unit.is_empty()).then(|| format!("{axis} in {unit}"))
        })
        .collect();
    if !units.is_empty() {
        let _ = write!(out, "\n_{}_\n", units.join(", "));
    }
    let notes: Vec<String> = spec
        .annotations
        .iter()
        .filter_map(annotation_line)
        .chain(spec.thresholds().map(|(_, param)| {
            let label = if param.label.trim().is_empty() {
                "Threshold"
            } else {
                param.label.trim()
            };
            format!("- {label}: {}", exact(param.value))
        }))
        .collect();
    if !notes.is_empty() {
        out.push('\n');
        out.push_str(&notes.join("\n"));
        out.push('\n');
    }
    out
}

fn annotation_line(annotation: &Annotation) -> Option<String> {
    let label = if annotation.label.trim().is_empty() {
        "Annotation"
    } else {
        annotation.label.trim()
    };
    let at = match (&annotation.x, &annotation.x_end, annotation.y) {
        (Some(x), Some(end), _) => format!("x {} to {}", x.display(), end.display()),
        (Some(x), None, _) => format!("x = {}", x.display()),
        (None, _, Some(y)) => format!("y = {}", exact(y)),
        (None, _, None) => return None,
    };
    Some(format!("- {label} ({at})"))
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
