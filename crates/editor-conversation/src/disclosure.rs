//! Native disclosure flights retain only already admitted source rows. One
//! cumulative height frontier clips the group; children never compress together.
use crate::{
    Conversation,
    fold::{self, Row, RowKey},
};
use artisan_api::ItemId;
use editor_ui::motion::MotionCurve;
use gpui::{Context, Pixels, px};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

const CLOCK: Duration = Duration::from_millis(250);
const MAX_FLIGHTS: usize = 8;
const MAX_ROWS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowClip {
    pub height: Pixels,
    pub opacity: f32,
}

/// What the disclosures in flight do to one row this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RowDisclosure {
    /// The clip a flight draws it at.
    pub clip: Option<RowClip>,
    /// A flight header's open fraction (its chevron turns with it).
    pub fraction: Option<f32>,
    /// The row belongs to a group in flight.
    pub hidden: bool,
    /// The row follows a group in flight: the space above it, eased from
    /// its closed spacing (under the header) to its open spacing (under
    /// the group's last row), so nothing below moves when the flight ends.
    pub gap: Option<f32>,
}
pub struct Flight {
    members: HashSet<ItemId>,
    retained: Vec<(Row, u64)>,
    keys: Vec<RowKey>,
    heights: HashMap<RowKey, Pixels>,
    open: bool,
    from: f32,
    started: Option<Instant>,
    /// The last frame painted the end state with the clips of the frame
    /// before, so the list's layout has caught up with it.
    landed: bool,
}
impl Flight {
    fn fraction(&self, now: Instant) -> f32 {
        let run = self.started.map_or(0.0, |started| {
            now.saturating_duration_since(started).as_secs_f64() / CLOCK.as_secs_f64()
        });
        let target = if self.open { 1.0 } else { 0.0 };
        if run >= 1.0 {
            // Exactly the end state, which ends the flight.
            return target;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "an eased fraction in 0..=1 needs no f64 precision"
        )]
        let eased = MotionCurve::SmoothOut.sample(run.max(0.0)) as f32;
        (target - self.from).mul_add(eased, self.from)
    }
    pub(crate) fn admits(&self, header: &RowKey, row: &Row) -> bool {
        let key = row.key();
        key == *header || !self.includes(row) || self.keys.contains(&key)
    }
    pub(crate) fn includes(&self, row: &Row) -> bool {
        row.member().is_some_and(|id| self.members.contains(id))
    }
}
impl Conversation {
    pub(crate) fn start_disclosure(&mut self, header: RowKey, open: bool) {
        let now = Instant::now();
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
        let Some(index) = self.rows.iter().position(|(row, _)| row.key() == header) else {
            return;
        };
        let members: HashSet<_> = match &self.rows[index].0 {
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
        let heights = &self.row_heights;
        let mut retained = Vec::new();
        let mut captured = HashMap::new();
        let mut seen = HashSet::new();
        for (row, version) in &self.rows[index + 1..] {
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
    /// Called before native projections/owner pruning so a closing page keeps
    /// its exact original window and source ordinals until its flight ends.
    pub(crate) fn retain_disclosure_rows(&mut self, mut rows: Vec<(Row, u64)>) -> Vec<(Row, u64)> {
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
    pub(crate) fn index_disclosure_rows(&mut self, rows: &[(Row, u64)]) {
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
    /// Every changed row uses ListState's anchor-preserving, bounded invalidation.
    #[expect(
        clippy::too_many_lines,
        reason = "one frame of every disclosure flight, measured and spliced together"
    )]
    pub(crate) fn advance_disclosures(&mut self, cx: &mut Context<Self>) -> bool {
        if self.disclosures.is_empty() && self.disclosure_clips.is_empty() {
            return false;
        }
        let now = Instant::now();
        // Structural rebuilding must not consume the visible transition.
        for flight in self.disclosures.values_mut() {
            flight.started.get_or_insert(now);
        }
        let completed: Vec<_> = self
            .disclosures
            .iter()
            // A flight ends only once the list laid out its end state, so
            // the refold that follows moves nothing.
            .filter(|(_, flight)| cx.reduce_motion() || flight.landed)
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
            let heights = &self.row_heights;
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
                if let Some((key, gap)) =
                    trailing_gap(self.view.as_ref(), &self.rows, header, flight, fraction)
                {
                    gaps.insert(key, gap);
                }
                hidden.extend(flight.members.iter().cloned());
                let full: Pixels = flight
                    .keys
                    .iter()
                    .map(|key| flight.heights.get(key).copied().unwrap_or(px(0.0)))
                    .sum();
                let frontier = full * fraction;
                let mut top = px(0.0);
                for key in &flight.keys {
                    let height = flight.heights.get(key).copied().unwrap_or(px(0.0));
                    let clip = RowClip {
                        height: (frontier - top).max(px(0.0)).min(height),
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
        // Old clip keys can vanish during native-window paging. Their dropped
        // owners pinned metrics while the old clip existed; release that bounded
        // old/new union as soon as the replacement clip no longer owns it.
        released.extend(changed.iter().cloned());
        // Every mounted row learns where it is clipped; those whose clip
        // changed measure again.
        for index in 0..self.row_views.len() {
            let Some(view) = self.row_views[index].clone() else {
                continue;
            };
            let disclosure = self.row_disclosure(&self.rows[index].0);
            view.update(cx, |view, cx| view.set_disclosure(disclosure, cx));
        }
        for key in changed.into_iter().chain(respaced) {
            if let Some(index) = self.row_context.key_rows.get(&key).copied() {
                self.list.remeasure_items(index..index + 1);
            }
        }
        if refold {
            self.reexpand(cx);
        }
        // A flight temporarily owns cached metrics for evicted descendants.
        // Release only that bounded set when the flight ends; idle frames scan nothing.
        for key in released {
            let owned = self
                .row_context
                .key_rows
                .get(&key)
                .is_some_and(|index| self.row_views.get(*index).is_some_and(Option::is_some));
            if !owned && !self.disclosure_clips.contains_key(&key) {
                self.row_heights.remove(&key);
            }
        }
        !self.disclosures.is_empty()
    }
    /// What the disclosures in flight do to `row` now.
    pub(crate) fn row_disclosure(&self, row: &Row) -> RowDisclosure {
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

    pub(crate) fn row_height_changed(&self, key: &RowKey, cx: &mut Context<Self>) {
        if let Some(index) = self.row_context.key_rows.get(key).copied() {
            self.list.remeasure_items(index..index + 1);
            cx.notify();
        }
    }
}

/// The row after `header`'s group and the space above it at `fraction`:
/// its spacing under the group's last row, eased toward its spacing under
/// the header alone. `None` when nothing follows or the two agree.
#[expect(
    clippy::float_cmp,
    reason = "both gaps come from the same spacing table; only an exact match means no motion"
)]
fn trailing_gap(
    view: Option<&crate::thread::ThreadView>,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "a reversed flight starts from exactly the fraction it painted"
    )]
    fn interrupted_flight_reverses_from_the_painted_fraction() {
        let now = Instant::now();
        let mut flight = Flight {
            members: HashSet::new(),
            retained: Vec::new(),
            keys: Vec::new(),
            heights: HashMap::new(),
            open: false,
            from: 1.0,
            started: Some(now.checked_sub(Duration::from_millis(100)).unwrap()),
            landed: false,
        };
        let painted = flight.fraction(now);
        flight.from = painted;
        flight.open = true;
        flight.started = None;
        assert_eq!(flight.fraction(now), painted);
        assert_eq!(flight.fraction(now + CLOCK), painted);
        flight.started = Some(now + CLOCK);
        assert_eq!(flight.fraction(now + CLOCK * 2), 1.0);
    }
}
