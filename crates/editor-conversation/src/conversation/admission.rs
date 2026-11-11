use super::fold::{Row, RowKey, Slot, TextSection};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub fn immediate(row: &Row) -> bool {
    matches!(
        row.semantic(),
        Row::Work { .. }
            | Row::Text {
                slot: Slot::Text(TextSection::User),
                ..
            }
    )
}

pub fn speculative_frontier(
    rows: &[(Row, u64)],
    reduce_motion: bool,
    mut unfolding: impl FnMut(&Row) -> bool,
    mut visible: impl FnMut(&RowKey) -> bool,
    mut mounted_awaiting: impl FnMut(&RowKey) -> Option<bool>,
    mut fresh: impl FnMut(&Row) -> bool,
) -> Option<RowKey> {
    if reduce_motion {
        return None;
    }
    rows.iter()
        .find(|(row, _)| {
            if immediate(row) || unfolding(row) {
                return false;
            }
            let key = row.key();
            if !visible(&key) {
                return false;
            }
            if let Some(awaiting) = mounted_awaiting(&key) {
                return awaiting;
            }
            match row.semantic() {
                Row::Text {
                    slot: Slot::Prose, ..
                }
                | Row::WorkEntry { .. }
                | Row::Chain { .. }
                | Row::Prompt(_)
                | Row::Image(_) => fresh(row),
                Row::Projected { .. }
                | Row::Work { .. }
                | Row::EarlierWork { .. }
                | Row::Text { .. }
                | Row::Compaction(_)
                | Row::Gap
                | Row::Status(_)
                | Row::AgentResult(_)
                | Row::AgentStatus(..) => false,
            }
        })
        .map(|(row, _)| row.key())
}

pub fn behind_reveal(index: usize, row: &Row, frontier: Option<usize>) -> bool {
    !immediate(row) && frontier.is_some_and(|frontier| index > frontier)
}

pub fn revealed_rows(rows: &[(Row, u64)], frontier: Option<usize>) -> &[(Row, u64)] {
    let end = frontier.map_or(rows.len(), |index| index + 1);
    &rows[..end.min(rows.len())]
}

pub fn changed_from(previous: Option<usize>, next: Option<usize>, len: usize) -> Option<usize> {
    let from = previous.unwrap_or(len).min(next.unwrap_or(len)) + 1;
    (from < len).then_some(from)
}

pub fn sync_frontier(
    current: Option<RowKey>,
    queued: Option<RowKey>,
    reduce_motion: bool,
    awaiting: impl FnOnce(&RowKey) -> bool,
    speculative: impl FnOnce() -> Option<RowKey>,
) -> Option<RowKey> {
    if queued == current {
        return current;
    }
    if let Some(key) = queued {
        return Some(key);
    }
    let current = current?;
    if !reduce_motion && awaiting(&current) {
        return Some(current);
    }
    speculative()
}

pub fn publish_frontier(
    queued: Option<RowKey>,
    speculative: impl FnOnce() -> Option<RowKey>,
) -> Option<RowKey> {
    queued.or_else(speculative)
}

#[derive(Default)]
pub struct RevealQueue {
    rows: HashMap<RowKey, RevealGate>,
    rushed_before: u64,
}
struct RevealGate {
    order: (u64, u8, usize),
    seen: Instant,
    settling: bool,
}
impl RevealQueue {
    pub fn begin(&mut self, key: RowKey, order: (u64, u8, usize), enabled: bool) -> bool {
        let now = Instant::now();
        self.rows
            .retain(|_, row| now.saturating_duration_since(row.seen) < Duration::from_millis(250));
        let held = enabled
            && self
                .rows
                .iter()
                .any(|(other, row)| *other != key && row.order < order && row.settling);
        self.rows
            .entry(key)
            .and_modify(|row| {
                row.seen = now;
                row.order = order;
            })
            .or_insert(RevealGate {
                order,
                seen: now,
                settling: false,
            });
        held
    }
    pub fn frontier_key(&self) -> Option<RowKey> {
        let now = Instant::now();
        self.rows
            .iter()
            .filter(|(_, row)| {
                row.settling && now.saturating_duration_since(row.seen) < Duration::from_millis(250)
            })
            .min_by_key(|(_, row)| row.order)
            .map(|(key, _)| key.clone())
    }
    pub fn finish(&mut self, key: &RowKey, settling: bool) {
        if let Some(row) = self.rows.get_mut(key) {
            row.settling = settling;
        }
    }
    pub fn remove(&mut self, key: &RowKey) {
        self.rows.remove(key);
    }
    pub fn rush(&mut self, before: u64) {
        self.rushed_before = self.rushed_before.max(before);
        for row in self.rows.values_mut() {
            if row.order.0 < self.rushed_before {
                row.settling = false;
            }
        }
    }
    pub const fn rushed(&self, position: u64) -> bool {
        position < self.rushed_before
    }
}

pub struct LiveBlockArrivals {
    high_water: usize,
    pending: bool,
    fresh: HashMap<usize, Instant>,
    pub seen: Instant,
}
impl LiveBlockArrivals {
    pub fn is_fresh(&self, block: usize) -> bool {
        self.fresh.contains_key(&block)
    }
    pub fn baseline(blocks: usize) -> Self {
        Self {
            high_water: blocks,
            pending: false,
            fresh: HashMap::new(),
            seen: Instant::now(),
        }
    }
    pub fn observed(&mut self) {
        self.pending = true;
        self.seen = Instant::now();
    }
    pub fn prepared(&mut self, blocks: usize, caught_up: bool, initially_fresh: bool) {
        let now = Instant::now();
        if self.pending || initially_fresh {
            for block in self.high_water.max(blocks.saturating_sub(128))..blocks {
                self.fresh.insert(block, now);
            }
            while self.fresh.len() > 128
                && let Some(oldest) = self.fresh.keys().min().copied()
            {
                self.fresh.remove(&oldest);
            }
        }
        self.high_water = self.high_water.max(blocks);
        if caught_up {
            self.pending = false;
        }
        self.seen = now;
    }
    pub fn claim(&mut self, block: usize) -> bool {
        self.fresh.remove(&block).is_some()
    }
}
