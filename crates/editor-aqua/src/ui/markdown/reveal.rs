use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ui::motion::MotionCurve;

const FADE: Duration = Duration::from_millis(120);
const MAX_LEAVES: usize = 128;

struct Leaf {
    text: Arc<str>,
    visible: usize,
    marks: VecDeque<(Range<usize>, Instant)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fade {
    pub range: Range<usize>,
    pub opacity: f32,
}

pub struct PreparedReveal {
    leaves: RefCell<HashMap<usize, Leaf>>,
    presented: Cell<bool>,
    fresh: Cell<bool>,
    began_fresh: Cell<Option<bool>>,
    streaming: Cell<bool>,
    tail: Cell<Option<usize>>,
    reduced: Cell<bool>,
    swift: Cell<bool>,
    animating: Cell<bool>,
    now: Cell<Instant>,
    paused: Cell<bool>,
    rushed: Cell<bool>,
    settling: Cell<bool>,
    blocks: RefCell<HashMap<usize, Option<Instant>>>,
    block_heights: RefCell<HashMap<usize, f32>>,
    frontier: Cell<Option<f32>>,
    visible_height: Cell<f32>,
    fully_visible: Cell<bool>,
}

impl Default for PreparedReveal {
    fn default() -> Self {
        Self {
            leaves: RefCell::new(HashMap::new()),
            presented: Cell::new(false),
            fresh: Cell::new(false),
            began_fresh: Cell::new(None),
            streaming: Cell::new(false),
            tail: Cell::new(None),
            reduced: Cell::new(false),
            swift: Cell::new(false),
            animating: Cell::new(false),
            now: Cell::new(Instant::now()),
            paused: Cell::new(false),
            rushed: Cell::new(false),
            settling: Cell::new(false),
            blocks: RefCell::new(HashMap::new()),
            block_heights: RefCell::new(HashMap::new()),
            frontier: Cell::new(None),
            visible_height: Cell::new(0.0),
            fully_visible: Cell::new(true),
        }
    }
}

impl PreparedReveal {
    pub fn has_presented(&self) -> bool {
        self.presented.get()
    }
    pub fn mark_presented(&self) {
        self.presented.set(true);
    }
    pub fn begin_frame(
        &self,
        now: Instant,
        streaming: bool,
        fresh: bool,
        reduced: bool,
        swift: bool,
    ) {
        self.streaming.set(streaming);
        let began = self.began_fresh.get().unwrap_or(fresh);
        self.began_fresh.set(Some(began));
        self.fresh.set(began);
        self.reduced.set(reduced);
        self.swift.set(swift);
        self.animating.set(false);
        self.settling.set(false);
        self.now.set(now);
        self.fully_visible.set(true);
        self.frontier.set(None);
    }
    pub fn set_tail(&self, slot: Option<usize>) {
        self.tail.set(slot);
    }
    pub fn set_paused(&self, paused: bool) {
        self.paused.set(paused);
    }
    pub fn set_rushed(&self, rushed: bool) {
        self.rushed.set(rushed);
    }
    pub fn is_animating(&self) -> bool {
        self.animating.get()
    }
    pub fn is_settling(&self) -> bool {
        self.settling.get()
    }
    pub fn fully_visible(&self) -> bool {
        self.fully_visible.get()
    }
    pub fn visible_end(&self, slot: usize, text: &str) -> usize {
        self.leaves
            .borrow()
            .get(&slot)
            .map_or(text.len(), |leaf| leaf.visible)
    }
    pub fn report_text_frontier(
        &self,
        app: &aqua::UiApp,
        node: aqua_core::NodeId,
        text: &str,
        end: usize,
    ) -> bool {
        if end > text.len() || !text.is_char_boundary(end) {
            return false;
        }
        let Some((index, _)) = text[..end].char_indices().next_back() else {
            return true;
        };
        let Some(frontier) = app.text_frontier(node, index, aqua_text::CaretAffinity::Downstream)
        else {
            return false;
        };
        self.report_frontier(Some(frontier.content_origin.y + frontier.line_bottom));
        true
    }

    pub fn report_frontier(&self, bottom: Option<f32>) {
        if let Some(bottom) = bottom {
            self.frontier.set(Some(
                self.frontier
                    .get()
                    .map_or(bottom, |previous| previous.max(bottom)),
            ));
        }
    }
    pub fn update_visible_height(&self, content_top: f32, content_height: f32) -> bool {
        let height = self.frontier.get().map_or(0.0, |frontier| {
            (frontier - content_top).max(0.0).min(content_height)
        });
        self.visible_height.replace(height) != height
    }
    pub fn visible_height(&self) -> f32 {
        self.visible_height.get()
    }
    pub fn update_block_height(&self, slot: usize, height: f32) -> bool {
        self.block_heights
            .borrow_mut()
            .insert(slot, height)
            .unwrap_or(0.0)
            != height
    }
    pub fn block_panel_height(&self, slot: usize) -> f32 {
        self.block_heights
            .borrow()
            .get(&slot)
            .copied()
            .unwrap_or(0.0)
            * self.block_growth(slot)
    }
    pub fn block_entrance(&self, slot: usize) -> Option<(f32, f32)> {
        if self.reduced.get() || self.swift.get() || self.rushed.get() {
            return None;
        }
        let mut blocks = self.blocks.borrow_mut();
        if blocks.len() >= MAX_LEAVES && !blocks.contains_key(&slot) {
            return None;
        }
        let started = blocks.entry(slot).or_insert_with(|| {
            (!self.fresh.get())
                .then(|| self.now.get().checked_sub(Duration::from_secs(1)))
                .flatten()
        });
        if self.paused.get() && started.is_none() {
            return Some((0.0, 4.0));
        }
        let started = *started.get_or_insert(self.now.get());
        let elapsed = self.now.get().saturating_duration_since(started);
        if elapsed >= FADE {
            return None;
        }
        self.animating.set(true);
        self.settling.set(self.settling.get() || elapsed < FADE / 2);
        let progress = MotionCurve::EaseOut
            .sample_f32((elapsed.as_secs_f64() / FADE.as_secs_f64()).clamp(0.0, 1.0));
        let alpha = 0.65_f32.mul_add(progress, 0.35);
        Some((alpha, 4.0 * (1.0 - alpha)))
    }
    pub fn block_growth(&self, slot: usize) -> f32 {
        if self.reduced.get() || self.swift.get() || self.rushed.get() {
            return 1.0;
        }
        self.blocks
            .borrow()
            .get(&slot)
            .copied()
            .flatten()
            .map_or(0.0, |started| {
                MotionCurve::SmoothOut.sample_f32(
                    (self
                        .now
                        .get()
                        .saturating_duration_since(started)
                        .as_secs_f64()
                        / FADE.as_secs_f64())
                    .clamp(0.0, 1.0),
                )
            })
    }
    pub fn fades(&self, slot: usize, text: Arc<str>) -> Vec<Fade> {
        if self.reduced.get() || self.swift.get() || self.rushed.get() {
            self.leaves.borrow_mut().clear();
            return Vec::new();
        }
        let mut leaves = self.leaves.borrow_mut();
        if !leaves.contains_key(&slot) && leaves.len() >= MAX_LEAVES {
            return Vec::new();
        }
        let now = self.now.get();
        let leaf = leaves.entry(slot).or_insert_with(|| Leaf {
            text: text.clone(),
            visible: if self.fresh.get() { 0 } else { text.len() },
            marks: VecDeque::new(),
        });
        if !Arc::ptr_eq(&leaf.text, &text) {
            if !text.starts_with(leaf.text.as_ref()) {
                leaf.visible = text.len();
                leaf.marks.clear();
            }
            leaf.text = text.clone();
        }
        if !self.paused.get() && leaf.visible < text.len() {
            leaf.marks.push_back((leaf.visible..text.len(), now));
            leaf.visible = text.len();
        }
        while leaf
            .marks
            .front()
            .is_some_and(|(_, at)| now.saturating_duration_since(*at) >= FADE)
        {
            leaf.marks.pop_front();
        }
        let waiting = leaf.visible < text.len();
        if waiting {
            self.fully_visible.set(false);
        }
        self.settling.set(self.settling.get() || waiting);
        self.animating
            .set(self.animating.get() || (!self.paused.get() && waiting) || !leaf.marks.is_empty());
        let mut fades = Vec::with_capacity(leaf.marks.len() + usize::from(waiting));
        for (range, started) in &leaf.marks {
            let progress =
                now.saturating_duration_since(*started).as_secs_f64() / FADE.as_secs_f64();
            fades.push(Fade {
                range: range.clone(),
                opacity: 0.65_f32.mul_add(
                    MotionCurve::EaseOut.sample_f32(progress.clamp(0.0, 1.0)),
                    0.35,
                ),
            });
        }
        if waiting {
            fades.push(Fade {
                range: leaf.visible..text.len(),
                opacity: 0.0,
            });
        }
        fades
    }
}
