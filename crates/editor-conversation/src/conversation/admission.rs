use super::fold::{Row, RowKey, Slot, TextSection};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub fn immediate(row: &Row) -> bool {
    matches!(
        row.semantic(),
        Row::Work { .. }
            | Row::WorkHeader(_)
            | Row::Text {
                slot: Slot::Text(TextSection::User),
                ..
            }
    )
}

pub fn behind_reveal(index: usize, row: &Row, frontier: Option<usize>) -> bool {
    !immediate(row) && frontier.is_some_and(|frontier| index > frontier)
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
    pub fn begin(
        &mut self,
        key: RowKey,
        order: (u64, u8, usize),
        enabled: bool,
        now: Instant,
    ) -> bool {
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
    pub fn frontier_key(&self, now: Instant) -> Option<RowKey> {
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
    pub fn baseline(blocks: usize, now: Instant) -> Self {
        Self {
            high_water: blocks,
            pending: false,
            fresh: HashMap::new(),
            seen: now,
        }
    }
    pub const fn observed(&mut self, now: Instant) {
        self.pending = true;
        self.seen = now;
    }
    pub fn prepared(
        &mut self,
        blocks: usize,
        caught_up: bool,
        initially_fresh: bool,
        now: Instant,
    ) {
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
