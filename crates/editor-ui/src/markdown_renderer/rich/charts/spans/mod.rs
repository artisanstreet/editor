//! `spans`: intervals on a shared time axis: a Gantt chart, a request
//! waterfall, swimlanes, or a flame chart of trace spans.
//!
//! Every span has a `start` and an `end`, both numbers in `unit` or both ISO
//! dates (`2026-10-07`). With `lane`s, spans sharing a lane share a row and
//! overlapping ones stack beneath it, so nested trace spans read as a flame
//! chart; without lanes, every span gets its own row in the order given,
//! which reads as a waterfall. Spans color by `group`, else by lane.
//! `markers` draw labelled rules across every row.

use crate::markdown_renderer::rich::budget;
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AnyElement, IntoElement as _, SharedString};
use serde::Deserialize;
use serde_json::Value;

use super::super::{BlockContext, RichBlock, kit, spec};
use super::scale::Domain;

pub(super) mod time;
mod view;

pub(super) use time::Time;

#[derive(Clone, Debug, Deserialize)]
struct SpanSpec {
    #[serde(default)]
    label: String,
    start: Value,
    end: Value,
    #[serde(default)]
    lane: Option<String>,
    #[serde(default)]
    group: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct MarkerSpec {
    at: Value,
    #[serde(default)]
    label: String,
}

/// The `spans` spec.
#[derive(Clone, Debug, Deserialize)]
struct SpansSpec {
    spans: Vec<SpanSpec>,
    #[serde(default)]
    markers: Vec<MarkerSpec>,
    /// The unit of numeric times ("ms").
    #[serde(default)]
    unit: Option<String>,
}

/// One span, resolved.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Span {
    pub(super) label: SharedString,
    pub(super) start: f64,
    pub(super) end: f64,
    /// The drawn row, counting every lane's stacked rows.
    pub(super) row: usize,
    /// Index into [`Layout::lanes`]; `None` without lanes.
    pub(super) lane: Option<usize>,
    /// The palette index.
    pub(super) color: usize,
    pub(super) group: Option<SharedString>,
}

/// A labelled lane and the rows it covers.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Lane {
    pub(super) name: SharedString,
    pub(super) first_row: usize,
    pub(super) rows: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Marker {
    pub(super) at: f64,
    pub(super) label: SharedString,
}

/// The spans placed in rows over one time domain.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Layout {
    pub(super) spans: Vec<Span>,
    pub(super) row_spans: Vec<Vec<usize>>,
    pub(super) lanes: Vec<Lane>,
    pub(super) markers: Vec<Marker>,
    pub(super) rows: usize,
    pub(super) domain: Domain,
    /// Whether times are calendar days.
    pub(super) dates: bool,
    pub(super) unit: Option<SharedString>,
}

/// A span's spec with its resolved start and end, `start <= end`.
type Timed<'a> = (&'a SpanSpec, f64, f64);

/// Every span's row and lane.
struct Rows {
    /// Per span: the row within its lane (or overall, without lanes) and
    /// its lane.
    placed: Vec<(usize, Option<usize>)>,
    lanes: Vec<Lane>,
    count: usize,
}

/// Resolves every time in `spec`; all numbers or all dates.
fn resolve(spec: &SpansSpec) -> Result<(Vec<Timed<'_>>, Vec<Marker>, bool), String> {
    let mut dates = None;
    let mut time = |value: &Value, what: &str| -> Result<f64, String> {
        let time = Time::parse(value)
            .ok_or_else(|| format!("{what} must be a number or an ISO date, got {value}"))?;
        match dates {
            None => dates = Some(time.is_date()),
            Some(was) if was != time.is_date() => {
                return Err("mixes numeric times and dates".to_owned());
            }
            Some(_) => {}
        }
        Ok(time.value())
    };
    let mut timed = Vec::with_capacity(spec.spans.len());
    for (index, span) in spec.spans.iter().enumerate() {
        let start = time(&span.start, &format!("spans[{index}].start"))?;
        let end = time(&span.end, &format!("spans[{index}].end"))?;
        if end < start {
            return Err(format!("spans[{index}].end precedes its start"));
        }
        timed.push((span, start, end));
    }
    let markers = spec
        .markers
        .iter()
        .enumerate()
        .map(|(index, marker)| {
            Ok(Marker {
                at: time(&marker.at, &format!("markers[{index}].at"))?,
                label: SharedString::from(marker.label.trim().to_owned()),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((timed, markers, dates.unwrap_or(false)))
}

/// Places spans in rows. Without lanes, one row per span in order. With
/// lanes, spans stack into the first row of their lane free at their start,
/// earlier and longer first so a parent sits over its children; a span
/// without a lane among laned ones becomes a lane named by its label.
fn place(timed: &[Timed<'_>]) -> Rows {
    fn lane_of(span: &SpanSpec) -> Option<&str> {
        span.lane
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }
    let mut lane_names: Vec<&str> = Vec::new();
    let mut lane_indices = HashMap::new();
    for (span, _, _) in timed {
        if let Some(lane) = lane_of(span) {
            lane_indices.entry(lane).or_insert_with(|| {
                let index = lane_names.len();
                lane_names.push(lane);
                index
            });
        }
    }
    if lane_names.is_empty() {
        return Rows {
            placed: (0..timed.len()).map(|row| (row, None)).collect(),
            lanes: Vec::new(),
            count: timed.len(),
        };
    }
    let mut order: Vec<usize> = (0..timed.len()).collect();
    order.sort_by(|a, b| {
        timed[*a]
            .1
            .total_cmp(&timed[*b].1)
            .then(timed[*b].2.total_cmp(&timed[*a].2))
    });
    let mut lane_counts = vec![0; lane_names.len()];
    for (span, _, _) in timed {
        if let Some(lane) = lane_of(span).and_then(|name| lane_indices.get(name)) {
            lane_counts[*lane] += 1;
        }
    }
    let mut stack_ends: Vec<RowEnds> = lane_counts.into_iter().map(RowEnds::new).collect();
    let mut placed: Vec<(usize, Option<usize>)> = vec![(0, None); timed.len()];
    for index in order {
        let (span, start, end) = timed[index];
        let lane = lane_of(span).and_then(|lane| lane_indices.get(lane).copied());
        if let Some(lane) = lane {
            placed[index] = (stack(&mut stack_ends[lane], start, end), Some(lane));
        }
    }
    let mut lanes: Vec<Lane> = lane_names
        .iter()
        .zip(&stack_ends)
        .map(|(name, ends)| Lane {
            name: SharedString::from((*name).to_owned()),
            first_row: 0,
            rows: ends.used.max(1),
        })
        .collect();
    for (index, (span, _, _)) in timed.iter().enumerate() {
        if placed[index].1.is_none() {
            lanes.push(Lane {
                name: SharedString::from(span.label.trim().to_owned()),
                first_row: 0,
                rows: 1,
            });
            placed[index].1 = Some(lanes.len() - 1);
        }
    }
    let mut count = 0;
    for lane in &mut lanes {
        lane.first_row = count;
        count += lane.rows;
    }
    Rows {
        placed,
        lanes,
        count,
    }
}

impl Layout {
    fn from_spec(spec: &SpansSpec) -> Result<Self, String> {
        if spec.spans.is_empty() {
            return Err("needs at least one span".to_owned());
        }
        if spec.spans.len() > 512 {
            return Err("preview supports at most 512 spans; summarize this trace".to_owned());
        }
        let (timed, markers, dates) = resolve(spec)?;
        let Rows {
            placed,
            lanes,
            count,
        } = place(&timed);
        let mut groups: HashMap<&str, usize> = HashMap::new();
        let spans = timed
            .iter()
            .zip(&placed)
            .map(|((span, start, end), (row, lane))| {
                let group = span
                    .group
                    .as_deref()
                    .map(str::trim)
                    .filter(|group| !group.is_empty());
                let group_color = group.map(|group| {
                    let next = groups.len();
                    *groups.entry(group).or_insert(next)
                });
                Span {
                    label: SharedString::from(span.label.trim().to_owned()),
                    start: *start,
                    end: *end,
                    row: lane.map_or(*row, |lane| lanes[lane].first_row + row),
                    lane: *lane,
                    color: group_color.or(*lane).unwrap_or(0),
                    group: group.map(|group| SharedString::from(group.to_owned())),
                }
            })
            .collect::<Vec<_>>();
        let domain = Domain::of(
            spans
                .iter()
                .flat_map(|span| [span.start, span.end])
                .chain(markers.iter().map(|marker| marker.at)),
        )
        .ok_or("needs at least one span")?;
        let domain = if dates {
            let lo = spans
                .iter()
                .map(|span| span.start)
                .chain(markers.iter().map(|marker| marker.at))
                .fold(f64::INFINITY, f64::min);
            let hi = spans
                .iter()
                .map(|span| span.end)
                .chain(markers.iter().map(|marker| marker.at))
                .fold(f64::NEG_INFINITY, f64::max);
            Domain {
                lo: lo - 0.5,
                hi: hi + 0.5,
            }
        } else {
            domain
        };
        let mut row_spans = vec![Vec::new(); count];
        for (index, span) in spans.iter().enumerate() {
            row_spans[span.row].push(index);
        }
        Ok(Self {
            row_spans,
            spans,
            lanes,
            markers,
            rows: count,
            domain,
            dates,
            unit: spec.unit.clone().map(SharedString::from),
        })
    }

    /// A time as written on the axis.
    pub(super) fn time(&self, value: f64) -> String {
        if self.dates {
            Time::Date(value).to_string()
        } else {
            kit::with_unit(&kit::format_number(value), self.unit.as_deref())
        }
    }

    /// A span's length.
    pub(super) fn duration(&self, span: &Span) -> String {
        let length = span.end - span.start;
        if self.dates {
            let days = length.round();
            if (days - 1.0).abs() < f64::EPSILON {
                "1 day".to_owned()
            } else {
                format!("{} days", kit::format_number(days))
            }
        } else {
            kit::with_unit(&kit::format_number(length), self.unit.as_deref())
        }
    }
}

/// A minimum tree finds the first reusable row in logarithmic time while
/// preserving authored first-row placement. Unused leaves stay at infinity.
struct RowEnds {
    minimum: Vec<f64>,
    leaves: usize,
    used: usize,
}
impl RowEnds {
    fn new(capacity: usize) -> Self {
        let leaves = capacity.max(1).next_power_of_two();
        Self {
            minimum: vec![f64::INFINITY; leaves * 2],
            leaves,
            used: 0,
        }
    }
}
fn stack(ends: &mut RowEnds, start: f64, end: f64) -> usize {
    let row = if ends.minimum[1] <= start {
        let mut branch = 1;
        while branch < ends.leaves {
            branch = branch * 2 + usize::from(ends.minimum[branch * 2] > start);
        }
        branch - ends.leaves
    } else {
        let row = ends.used;
        ends.used += 1;
        row
    };
    let mut branch = ends.leaves + row;
    ends.minimum[branch] = end;
    while branch > 1 {
        branch /= 2;
        ends.minimum[branch] = ends.minimum[branch * 2].min(ends.minimum[branch * 2 + 1]);
    }
    row
}

#[derive(Debug)]
pub(super) struct Spans {
    layout: Arc<Layout>,
}

pub(super) fn parse(value: &Value) -> Result<Arc<dyn RichBlock>, String> {
    let spec = spec::<SpansSpec>(value)?;
    Ok(Arc::new(Spans {
        layout: Arc::new(Layout::from_spec(&spec)?),
    }))
}

impl RichBlock for Spans {
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<Layout>()
            + budget::vector(&self.layout.row_spans)
            + self
                .layout
                .row_spans
                .iter()
                .map(budget::vector)
                .sum::<usize>()
            + budget::vector(&self.layout.spans)
            + self
                .layout
                .spans
                .iter()
                .map(|span| span.label.len() + span.group.as_ref().map_or(0, |group| group.len()))
                .sum::<usize>()
            + budget::vector(&self.layout.lanes)
            + self
                .layout
                .lanes
                .iter()
                .map(|lane| lane.name.len())
                .sum::<usize>()
            + budget::vector(&self.layout.markers)
            + self
                .layout
                .markers
                .iter()
                .map(|marker| marker.label.len())
                .sum::<usize>()
            + self.layout.unit.as_ref().map_or(0, |unit| unit.len())
    }

    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        view::SpansView {
            id: cx.key("spans"),
            theme: *cx.theme,
            layout: Arc::clone(&self.layout),
        }
        .into_any_element()
    }
}

#[cfg(test)]
pub(super) fn layout_for_tests(value: &Value) -> Result<Layout, String> {
    Layout::from_spec(&spec::<SpansSpec>(value)?)
}

#[cfg(test)]
mod row_stack_tests {
    use super::*;
    #[test]
    fn minimum_tree_preserves_first_available_row() {
        let mut indexed = RowEnds::new(128);
        let mut linear: Vec<f64> = Vec::new();
        for index in 0..128 {
            let start = f64::from(index / 3);
            let end = start + f64::from(index % 7);
            let expected = if let Some(row) = linear.iter().position(|end| *end <= start) {
                linear[row] = end;
                row
            } else {
                linear.push(end);
                linear.len() - 1
            };
            assert_eq!(stack(&mut indexed, start, end), expected);
        }
        assert_eq!(indexed.used, linear.len());
    }
}
