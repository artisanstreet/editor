mod view;
#[cfg(feature = "test-support")]
pub use view::SpansFixture;
pub(crate) use view::{BarLabel, Services, SpansState, Tick, view};

use super::super::kit;
use super::scale::Domain;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
mod time;
use time::Time;

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

#[derive(Clone, Debug, Deserialize)]
struct SpansSpec {
    spans: Vec<SpanSpec>,
    #[serde(default)]
    markers: Vec<MarkerSpec>,
    #[serde(default)]
    unit: Option<String>,
}

pub(crate) fn prepare(value: &Value) -> Result<Layout, String> {
    let spec = SpansSpec::deserialize(value).map_err(|error| error.to_string())?;
    Layout::from_spec(&spec)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Span {
    pub(crate) label: String,
    pub(crate) start: f64,
    pub(crate) end: f64,
    pub(crate) row: usize,
    pub(crate) lane: Option<usize>,
    pub(crate) color: usize,
    pub(crate) group: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Lane {
    pub(crate) name: String,
    pub(crate) first_row: usize,
    pub(crate) rows: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Marker {
    pub(crate) at: f64,
    pub(crate) label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    pub(crate) spans: Vec<Span>,
    pub(crate) row_spans: Vec<Vec<usize>>,
    pub(crate) lanes: Vec<Lane>,
    pub(crate) markers: Vec<Marker>,
    pub(crate) rows: usize,
    pub(crate) domain: Domain,
    pub(crate) dates: bool,
    pub(crate) unit: Option<String>,
}
impl Layout {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::{retained_optional, retained_vec};
        std::mem::size_of::<Self>()
            + retained_vec(&self.spans)
            + self
                .spans
                .iter()
                .map(|span| span.label.capacity() + retained_optional(span.group.as_ref()))
                .sum::<usize>()
            + retained_vec(&self.row_spans)
            + self.row_spans.iter().map(retained_vec).sum::<usize>()
            + retained_vec(&self.lanes)
            + self
                .lanes
                .iter()
                .map(|lane| lane.name.capacity())
                .sum::<usize>()
            + retained_vec(&self.markers)
            + self
                .markers
                .iter()
                .map(|marker| marker.label.capacity())
                .sum::<usize>()
            + retained_optional(self.unit.as_ref())
    }
}

type Timed<'a> = (&'a SpanSpec, f64, f64);

struct Rows {
    placed: Vec<(usize, Option<usize>)>,
    lanes: Vec<Lane>,
    count: usize,
}

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
                label: String::from(marker.label.trim().to_owned()),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((timed, markers, dates.unwrap_or(false)))
}

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
            name: String::from((*name).to_owned()),
            first_row: 0,
            rows: ends.used.max(1),
        })
        .collect();
    for (index, (span, _, _)) in timed.iter().enumerate() {
        if placed[index].1.is_none() {
            lanes.push(Lane {
                name: String::from(span.label.trim().to_owned()),
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
                    label: String::from(span.label.trim().to_owned()),
                    start: *start,
                    end: *end,
                    row: lane.map_or(*row, |lane| lanes[lane].first_row + row),
                    lane: *lane,
                    color: group_color.or(*lane).unwrap_or(0),
                    group: group.map(|group| String::from(group.to_owned())),
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
            unit: spec.unit.clone().map(String::from),
        })
    }

    pub(crate) fn time(&self, value: f64) -> String {
        if self.dates {
            Time::Date(value).to_string()
        } else {
            kit::with_unit(&kit::format_number(value), self.unit.as_deref())
        }
    }

    pub(crate) fn duration(&self, span: &Span) -> String {
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
