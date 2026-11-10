use super::fold::{self, Row, RowKey};
use super::thread::ThreadView;
use editor_ui::motion::MotionCurve;
use artisan_api::ItemId;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct DisclosureState {
    pub disclosures: HashMap<RowKey, Flight>,
    pub disclosure_clips: HashMap<RowKey, RowClip>,
    pub disclosure_hidden: HashSet<ItemId>,
    pub disclosure_fractions: HashMap<RowKey, f32>,
    pub disclosure_gaps: HashMap<RowKey, f32>,
}

#[derive(Default)]
pub struct DisclosureFrame {
    pub active: bool,
    pub refold: bool,
    pub changed: Vec<RowKey>,
    pub released: Vec<RowKey>,
}

const CLOCK: Duration = Duration::from_millis(250);
const MAX_FLIGHTS: usize = 8;
const MAX_ROWS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowClip {
    pub height: f32,
    pub opacity: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RowDisclosure {
    pub clip: Option<RowClip>,
    pub fraction: Option<f32>,
    pub hidden: bool,
    pub gap: Option<f32>,
}
pub struct Flight {
    members: HashSet<ItemId>,
    retained: Vec<(Row, u64)>,
    keys: Vec<RowKey>,
    heights: HashMap<RowKey, f32>,
    open: bool,
    from: f32,
    started: Option<Instant>,
    landed: bool,
}
impl Flight {
    fn fraction(&self, now: Instant) -> f32 {
        let run = self.started.map_or(0.0, |started| {
            now.saturating_duration_since(started).as_secs_f64() / CLOCK.as_secs_f64()
        });
        let target = if self.open { 1.0 } else { 0.0 };
        if run >= 1.0 {
            return target;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "an eased fraction in 0..=1 needs no f64 precision"
        )]
        let eased = MotionCurve::SmoothOut.sample(run.max(0.0)) as f32;
        (target - self.from).mul_add(eased, self.from)
    }
    pub(crate) fn includes(&self, row: &Row) -> bool {
        row.member().is_some_and(|id| self.members.contains(id))
    }
}
impl DisclosureState {
    pub fn start_disclosure(
        &mut self,
        rows: &[(Row, u64)],
        row_heights: &HashMap<RowKey, f32>,
        header: RowKey,
        open: bool,
        now: Instant,
    ) {
        if let Some(flight) = self.disclosures.get_mut(&header) {
            flight.from = self
                .disclosure_fractions
                .get(&header)
                .copied()
                .unwrap_or_else(|| flight.fraction(now));
            flight.open = open;
            flight.started = None;
            flight.landed = false;
            return;
        }
        if self.disclosures.len() >= MAX_FLIGHTS {
            return;
        }
        let Some(index) = rows.iter().position(|(row, _)| row.key() == header) else {
            return;
        };
        let members: HashSet<_> = match &rows[index].0 {
            Row::Work {
                entries, unloaded, ..
            } => entries
                .iter()
                .cloned()
                .chain(unloaded.iter().map(|first| fold::span_member(*first)))
                .collect(),
            Row::Chain { ids, .. } => ids.iter().cloned().collect(),
            Row::Projected { .. }
            | Row::EarlierWork { .. }
            | Row::WorkEntry { .. }
            | Row::Text { .. }
            | Row::Prompt(_)
            | Row::Compaction(_)
            | Row::Image(_)
            | Row::Gap
            | Row::Status(_)
            | Row::AgentResult(_)
            | Row::AgentStatus(..) => return,
        };
        let heights = row_heights;
        let mut retained = Vec::new();
        let mut captured = HashMap::new();
        let mut seen = HashSet::new();
        for (row, version) in &rows[index + 1..] {
            if !row.member().is_some_and(|id| members.contains(id)) {
                break;
            }
            let Some(height) = heights.get(&row.key()).copied() else {
                continue;
            };
            if captured.len() >= MAX_ROWS {
                break;
            }
            captured.insert(row.key(), height);
            let semantic = row.semantic().clone();
            if seen.insert(semantic.key()) {
                retained.push((semantic, *version));
            }
            if retained.len() >= MAX_ROWS {
                break;
            }
        }
        self.disclosures.insert(
            header,
            Flight {
                members,
                retained,
                keys: Vec::new(),
                heights: captured,
                open,
                from: if open { 0.0 } else { 1.0 },
                started: None,
                landed: false,
            },
        );
    }
    pub fn retain_disclosure_rows(&mut self, mut rows: Vec<(Row, u64)>) -> Vec<(Row, u64)> {
        for (header, flight) in &mut self.disclosures {
            let Some(index) = rows.iter().position(|(row, _)| row.key() == *header) else {
                continue;
            };
            if flight.open {
                flight.retained = rows[index + 1..]
                    .iter()
                    .take_while(|(row, _)| flight.includes(row))
                    .take(MAX_ROWS)
                    .cloned()
                    .collect();
            } else {
                let present: HashSet<_> = rows.iter().map(|(row, _)| row.key()).collect();
                let retained = flight
                    .retained
                    .iter()
                    .filter(|(row, _)| !present.contains(&row.key()))
                    .cloned();
                rows.splice((index + 1)..=index, retained);
            }
        }
        rows
    }
    pub fn index_disclosure_rows(&mut self, rows: &[(Row, u64)]) {
        for (header, flight) in &mut self.disclosures {
            let Some(index) = rows.iter().position(|(row, _)| row.key() == *header) else {
                continue;
            };
            flight.keys = rows[index + 1..]
                .iter()
                .take_while(|(row, _)| flight.includes(row))
                .take(MAX_ROWS)
                .map(|(row, _)| row.key())
                .collect();
        }
    }
    pub fn advance_disclosures(
        &mut self,
        rows: &[(Row, u64)],
        row_heights: &HashMap<RowKey, f32>,
        view: Option<&ThreadView>,
        now: Instant,
        reduce_motion: bool,
    ) -> DisclosureFrame {
        if self.disclosures.is_empty() && self.disclosure_clips.is_empty() {
            return DisclosureFrame::default();
        }
        for flight in self.disclosures.values_mut() {
            flight.started.get_or_insert(now);
        }
        let completed: Vec<_> = self
            .disclosures
            .iter()
            .filter(|(_, flight)| reduce_motion || flight.landed)
            .map(|(key, _)| key.clone())
            .collect();
        let refold = !completed.is_empty();
        let mut released = Vec::new();
        for key in completed {
            if let Some(flight) = self.disclosures.remove(&key) {
                released.extend(flight.keys);
                released.extend(flight.heights.into_keys());
            }
        }
        let mut clips: HashMap<RowKey, RowClip> = HashMap::new();
        let mut hidden = HashSet::new();
        let mut fractions = HashMap::new();
        let mut gaps = HashMap::new();
        {
            let heights = row_heights;
            for (header, flight) in &mut self.disclosures {
                if flight.open {
                    for key in &flight.keys {
                        if let Some(height) = heights.get(key) {
                            flight.heights.insert(key.clone(), *height);
                        }
                    }
                }
                let fraction = flight.fraction(now);
                fractions.insert(header.clone(), fraction);
                if let Some((key, gap)) = trailing_gap(view, rows, header, flight, fraction) {
                    gaps.insert(key, gap);
                }
                hidden.extend(flight.members.iter().cloned());
                let full: f32 = flight
                    .keys
                    .iter()
                    .map(|key| flight.heights.get(key).copied().unwrap_or(0.0))
                    .sum();
                let frontier = full * fraction;
                let mut top = 0.0;
                for key in &flight.keys {
                    let height = flight.heights.get(key).copied().unwrap_or(0.0);
                    let clip = RowClip {
                        height: (frontier - top).max(0.0).min(height),
                        opacity: fraction,
                    };
                    clips
                        .entry(key.clone())
                        .and_modify(|old| {
                            old.height = old.height.min(clip.height);
                            old.opacity = old.opacity.min(clip.opacity);
                        })
                        .or_insert(clip);
                    top += height;
                }
            }
        }
        let changed: HashSet<_> = {
            let old = &self.disclosure_clips;
            old.keys()
                .chain(clips.keys())
                .filter(|key| old.get(*key) != clips.get(*key))
                .cloned()
                .collect()
        };
        let respaced: Vec<_> = {
            let old = &self.disclosure_gaps;
            old.keys()
                .chain(gaps.keys())
                .filter(|key| old.get(*key) != gaps.get(*key))
                .cloned()
                .collect()
        };
        for (header, flight) in &mut self.disclosures {
            let target = if flight.open { 1.0 } else { 0.0 };
            flight.landed = fractions.get(header) == Some(&target)
                && self.disclosure_fractions.get(header) == Some(&target)
                && !flight.keys.iter().any(|key| changed.contains(key))
                && !respaced.iter().any(|key| gaps.contains_key(key));
        }
        self.disclosure_clips = clips;
        self.disclosure_gaps = gaps;
        self.disclosure_hidden = hidden;
        self.disclosure_fractions = fractions;
        released.extend(changed.iter().cloned());
        DisclosureFrame {
            active: !self.disclosures.is_empty(),
            refold,
            changed: changed.into_iter().chain(respaced).collect(),
            released,
        }
    }
    pub fn row_disclosure(&self, row: &Row) -> RowDisclosure {
        let key = row.key();
        RowDisclosure {
            clip: self.disclosure_clips.get(&key).copied(),
            fraction: self.disclosure_fractions.get(&key).copied(),
            hidden: row
                .member()
                .is_some_and(|id| self.disclosure_hidden.contains(id)),
            gap: self.disclosure_gaps.get(&key).copied(),
        }
    }
}

#[expect(
    clippy::float_cmp,
    reason = "both gaps come from the same spacing table; only an exact match means no motion"
)]
fn trailing_gap(
    view: Option<&ThreadView>,
    rows: &[(Row, u64)],
    header: &RowKey,
    flight: &Flight,
    fraction: f32,
) -> Option<(RowKey, f32)> {
    let view = view?;
    let index = rows.iter().position(|(row, _)| row.key() == *header)?;
    let members = rows[index + 1..]
        .iter()
        .take_while(|(row, _)| flight.includes(row))
        .count();
    let after = index + 1 + members;
    let trailing = rows.get(after)?;
    let closed = fold::gap(view, &[rows[index].clone(), trailing.clone()], 1);
    let open = fold::gap(view, &rows[after - 1..=after], 1);
    (closed != open).then(|| (trailing.0.key(), (open - closed).mul_add(fraction, closed)))
}
