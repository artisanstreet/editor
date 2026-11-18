use serde::Deserialize;

pub mod model;
pub(crate) mod plot;
mod view;

#[cfg(feature = "test-support")]
pub use view::ChartFixture;
pub(crate) use view::{ChartState, Services, view};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mark {
    #[default]
    Line,
    Area,
    Bar,
    Scatter,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub(crate) enum XValue {
    Number(f64),
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SeriesData {
    Values(Vec<Option<f64>>),
    Points(Vec<(XValue, Option<f64>)>),
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(try_from = "RawSeries")]
pub(crate) struct SeriesSpec {
    name: String,
    data: SeriesData,
}

#[derive(Deserialize)]
struct RawSeries {
    #[serde(default)]
    name: String,
    #[serde(default)]
    values: Option<Vec<Option<f64>>>,
    #[serde(default)]
    points: Option<Vec<(XValue, Option<f64>)>>,
}

impl TryFrom<RawSeries> for SeriesSpec {
    type Error = String;

    fn try_from(raw: RawSeries) -> Result<Self, String> {
        let data = match (raw.values, raw.points) {
            (Some(values), None) => SeriesData::Values(values),
            (None, Some(points)) => SeriesData::Points(points),
            (Some(_), Some(_)) => {
                return Err(format!(
                    "series `{}` gives both `values` and `points`; give one",
                    raw.name
                ));
            }
            (None, None) => {
                return Err(format!(
                    "series `{}` gives neither `values` nor `points`",
                    raw.name
                ));
            }
        };
        Ok(Self {
            name: raw.name,
            data,
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct Axis {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct Annotation {
    #[serde(default)]
    x: Option<XValue>,
    #[serde(default)]
    y: Option<f64>,
    #[serde(default)]
    x_end: Option<XValue>,
    #[serde(default)]
    label: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ParamKind {
    #[default]
    Threshold,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct Param {
    #[serde(default)]
    id: String,
    #[serde(default)]
    kind: ParamKind,
    #[serde(default)]
    label: String,
    min: f64,
    max: f64,
    #[serde(default)]
    step: Option<f64>,
    value: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct ChartSpec {
    #[serde(default)]
    mark: Mark,
    #[serde(default)]
    stacked: bool,
    #[serde(default)]
    normalize: bool,
    #[serde(default)]
    categories: Option<Vec<String>>,
    series: Vec<SeriesSpec>,
    #[serde(default)]
    x: Axis,
    #[serde(default)]
    y: Axis,
    #[serde(default)]
    annotations: Vec<Annotation>,
    #[serde(default)]
    params: Vec<Param>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChartStyle {
    mark: Mark,
    stacked: bool,
    normalize: bool,
    x: Axis,
    y: Axis,
    annotations: Vec<Annotation>,
    params: Vec<Param>,
}
impl ChartStyle {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::{retained_optional, retained_vec};
        let axis = |axis: &Axis| {
            retained_optional(axis.label.as_ref()) + retained_optional(axis.unit.as_ref())
        };
        let x = |value: &XValue| match value {
            XValue::Text(text) => text.capacity(),
            XValue::Number(_) => 0,
        };
        std::mem::size_of::<Self>()
            + axis(&self.x)
            + axis(&self.y)
            + retained_vec(&self.annotations)
            + self
                .annotations
                .iter()
                .map(|note| {
                    note.label.capacity()
                        + note.x.as_ref().map_or(0, &x)
                        + note.x_end.as_ref().map_or(0, &x)
                })
                .sum::<usize>()
            + retained_vec(&self.params)
            + self
                .params
                .iter()
                .map(|param| param.id.capacity() + param.label.capacity())
                .sum::<usize>()
    }
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
    const fn stacks(&self) -> bool {
        self.stacked || self.normalize
    }

    fn thresholds(&self) -> impl Iterator<Item = (usize, &Param)> {
        self.params
            .iter()
            .enumerate()
            .filter(|(_, param)| param.kind == ParamKind::Threshold)
    }
}

pub(crate) fn prepare(value: &serde_json::Value) -> Result<(ChartStyle, model::Model), String> {
    let spec: ChartSpec =
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
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
    let style = ChartStyle::from(spec);
    if style.stacks() && model.series.len().saturating_mul(model.xs.len()) > 16_384 {
        return Err(
            "stacked chart preview exceeds 16384 aligned cells; use fewer series or points".into(),
        );
    }
    if style.stacks() && !style.normalize {
        let tracks = model.tracks(&vec![true; model.series.len()], true, false);
        if tracks
            .iter()
            .flatten()
            .filter_map(|(_, extent)| *extent)
            .any(|(start, end)| !start.is_finite() || !end.is_finite())
        {
            return Err("stacked values exceed the finite chart range".into());
        }
    }
    Ok((style, model))
}

pub(crate) fn series_name(name: &str, index: usize) -> String {
    let name = name.trim();
    if name.is_empty() {
        format!("Series {}", index + 1)
    } else {
        name.to_owned()
    }
}

pub(crate) fn press(
    element: aqua_ui::element::Element,
    handler: std::rc::Rc<dyn Fn(aqua_core::Point, usize)>,
) -> aqua_ui::element::Element {
    use aqua_core::{InputEvent, PointerButton};
    use aqua_ui::input::{CaptureRequest, EventPhase, InputKind, InputResponse, RoutedInput};
    element.on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
        let RoutedInput::Native(InputEvent::PointerDown(pointer, PointerButton::Primary)) =
            &event.input
        else {
            return InputResponse::default();
        };
        let Some(click_count) = event.click_count else {
            return InputResponse::default();
        };
        handler(pointer.position, click_count);
        InputResponse {
            pointer_capture: CaptureRequest::Capture,
            ..InputResponse::default()
        }
    })
}

#[cfg(test)]
#[path = "../../../../../tests/ui/rich/charts/chart.rs"]
mod tests;
