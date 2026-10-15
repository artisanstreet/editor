//! Paced opacity on mounted, already prepared inline leaves. Text and native
//! geometry stay complete; no growing prefix is reparsed or copied.
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::motion::MotionCurve;
use gpui::{
    AnyElement, HighlightStyle, IntoElement as _, ParentElement as _, Pixels, SharedString,
    Styled as _, div, px,
};

const FADE: Duration = Duration::from_millis(400);
const INTERVAL: Duration = Duration::from_millis(150);
const MAX_LEAVES: usize = 128;

#[derive(Debug)]
struct Leaf {
    text: SharedString,
    visible: usize,
    remaining_words: usize,
    marks: VecDeque<(Range<usize>, Instant)>,
    paced_at: Instant,
    next_tick: Instant,
    rate: f64,
    budget: f64,
}

#[derive(Debug, Default)]
struct RevealGeometry {
    frontier: Cell<Option<Pixels>>,
    visible_height: Cell<Pixels>,
}

/// Reveal ownership follows one mounted native transcript row. Retained text is
/// a shared prepared leaf, never an independently copied Item or history body.
#[derive(Debug)]
pub struct PreparedReveal {
    leaves: RefCell<HashMap<usize, Leaf>>,
    presented: Cell<bool>,
    fresh: Cell<bool>,
    began_fresh: Cell<Option<bool>>,
    streaming: Cell<bool>,
    reduced: Cell<bool>,
    animating: Cell<bool>,
    now: Cell<Instant>,
    paused: Cell<bool>,
    settling: Cell<bool>,
    blocks: RefCell<HashMap<usize, Option<Instant>>>,
    geometry: Rc<RevealGeometry>,
    block_heights: RefCell<HashMap<usize, Rc<Cell<Pixels>>>>,
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
            reduced: Cell::new(false),
            animating: Cell::new(false),
            now: Cell::new(Instant::now()),
            paused: Cell::new(false),
            settling: Cell::new(false),
            blocks: RefCell::default(),
            geometry: Rc::default(),
            block_heights: RefCell::default(),
            fully_visible: Cell::new(true),
        }
    }
}
impl PreparedReveal {
    /// Whether this mounted native leaf has presented prepared content yet.
    pub fn has_presented(&self) -> bool {
        self.presented.get()
    }
    pub fn mark_presented(&self) {
        self.presented.set(true);
    }
    pub fn begin_frame(&self, streaming: bool, fresh: bool, reduced: bool) {
        self.streaming.set(streaming);
        let began = self.began_fresh.get().unwrap_or(fresh);
        self.began_fresh.set(Some(began));
        self.fresh.set(began);
        self.reduced.set(reduced);
        self.animating.set(false);
        self.settling.set(false);
        self.now.set(Instant::now());
        self.fully_visible.set(true);
        self.geometry.frontier.set(None);
    }
    pub(super) fn block_panel(&self, slot: usize, content: gpui::Div) -> gpui::Div {
        let measured = self
            .block_heights
            .borrow_mut()
            .entry(slot)
            .or_default()
            .clone();
        let started = self.blocks.borrow().get(&slot).copied().flatten();
        let grow = started.map_or(0.0, |started| {
            MotionCurve::SmoothOut.sample(
                (self
                    .now
                    .get()
                    .saturating_duration_since(started)
                    .as_secs_f64()
                    / 0.25)
                    .clamp(0.0, 1.0),
            ) as f32
        });
        div()
            .relative()
            .w_full()
            .min_w_0()
            .h(measured.get() * grow)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(content.flex_shrink_0())
            .on_children_prepainted(move |bounds, window, _| {
                if let Some(bounds) = bounds.first()
                    && measured.replace(bounds.size.height) != bounds.size.height
                {
                    window.request_animation_frame();
                }
            })
    }

    pub(super) fn visible_end(&self, slot: usize, text: &SharedString) -> usize {
        self.leaves
            .borrow()
            .get(&slot)
            .map_or(text.len(), |leaf| leaf.visible)
    }
    pub(super) fn frontier_reporter(&self) -> crate::selectable_text::FrontierReport {
        let geometry = self.geometry.clone();
        Rc::new(move |bottom, _| {
            if let Some(bottom) = bottom {
                geometry.frontier.set(Some(
                    geometry
                        .frontier
                        .get()
                        .map_or(bottom, |previous| previous.max(bottom)),
                ));
            }
        })
    }
    /// Clip to the original native line containing the reveal frontier while
    /// the complete child keeps its width, glyphs and intrinsic height.
    pub(super) fn mask(&self, content: AnyElement) -> AnyElement {
        self.mark_presented();
        if self.fully_visible.get() || self.reduced.get() {
            return content;
        }
        let geometry = self.geometry.clone();
        let height = geometry.visible_height.get();
        div()
            .w_full()
            .min_w_0()
            .h(height)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(div().w_full().min_w_0().flex_shrink_0().child(content))
            .on_children_prepainted(move |bounds, window, _| {
                if let Some(bounds) = bounds.first() {
                    let visible = geometry.frontier.get().map_or(px(0.0), |frontier| {
                        (frontier - bounds.top())
                            .max(px(0.0))
                            .min(bounds.size.height)
                    });
                    if geometry.visible_height.replace(visible) != visible {
                        window.request_animation_frame();
                    }
                }
            })
            .into_any_element()
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.set(paused);
    }
    pub fn is_settling(&self) -> bool {
        self.settling.get()
    }

    pub(super) fn block_entrance(&self, slot: usize) -> Option<(f32, f32)> {
        if self.reduced.get() {
            return None;
        }
        let mut blocks = self.blocks.borrow_mut();
        if blocks.len() >= MAX_LEAVES && !blocks.contains_key(&slot) {
            return None;
        }
        let started = blocks.entry(slot).or_insert_with(|| {
            (!self.fresh.get()).then_some(self.now.get() - Duration::from_secs(1))
        });
        if self.paused.get() && started.is_none() {
            return Some((0.0, 4.0));
        }
        let started = *started.get_or_insert(self.now.get());
        let elapsed = self.now.get().saturating_duration_since(started);
        if elapsed >= Duration::from_millis(400) {
            return None;
        }
        self.animating.set(true);
        self.settling
            .set(self.settling.get() || elapsed < Duration::from_millis(205));
        let alpha = MotionCurve::EaseOut.sample(
            (elapsed
                .saturating_sub(Duration::from_millis(80))
                .as_secs_f64()
                / 0.25)
                .clamp(0.0, 1.0),
        ) as f32;
        Some((alpha, 4.0 * (1.0 - alpha)))
    }

    pub fn is_animating(&self) -> bool {
        self.animating.get()
    }

    pub(super) fn highlights(
        &self,
        slot: usize,
        text: &SharedString,
        base: Rc<[(Range<usize>, HighlightStyle)]>,
    ) -> Rc<[(Range<usize>, HighlightStyle)]> {
        self.highlights_at(slot, text, base, self.now.get())
    }
    fn highlights_at(
        &self,
        slot: usize,
        text: &SharedString,
        base: Rc<[(Range<usize>, HighlightStyle)]>,
        now: Instant,
    ) -> Rc<[(Range<usize>, HighlightStyle)]> {
        if self.reduced.get() {
            self.leaves.borrow_mut().clear();
            return base;
        }
        let mut leaves = self.leaves.borrow_mut();
        if !leaves.contains_key(&slot) && leaves.len() >= MAX_LEAVES {
            // A caller rendering an unexpectedly large range cannot retain an
            // unbounded second copy of its presentation ownership.
            return base;
        }
        let leaf = leaves.entry(slot).or_insert_with(|| {
            let visible = if self.fresh.get() { 0 } else { text.len() };
            Leaf {
                text: text.clone(),
                visible,
                remaining_words: text[visible..].split_whitespace().count(),
                marks: VecDeque::new(),
                paced_at: now,
                next_tick: now,
                rate: 5.0,
                budget: 1.0,
            }
        });
        if leaf.text.as_ptr() != text.as_ptr() || leaf.text.len() != text.len() {
            if !text.starts_with(leaf.text.as_ref()) {
                leaf.visible = text.len();
                leaf.marks.clear();
            }
            leaf.text = text.clone();
            leaf.remaining_words = text[leaf.visible..].split_whitespace().count();
        }
        let elapsed = now.saturating_duration_since(leaf.paced_at).as_secs_f64();
        leaf.paced_at = now;
        let target = (leaf.remaining_words as f64 / 1.5).clamp(5.0, 15.0);
        leaf.rate += (target - leaf.rate) * (1.0 - (-elapsed / 0.5).exp());
        if !self.paused.get() {
            leaf.budget = (leaf.budget + leaf.rate * elapsed).min(6.0);
        }
        if !self.paused.get() && now >= leaf.next_tick && leaf.budget >= 1.0 {
            let start = leaf.visible;
            while leaf.visible < text.len() && leaf.budget >= 1.0 {
                let rest = &text[leaf.visible..];
                let mut end = leaf.visible;
                let mut word = false;
                let mut complete = false;
                for (offset, ch) in rest.char_indices() {
                    if ch.is_whitespace() && word {
                        end = leaf.visible + offset + ch.len_utf8();
                        complete = true;
                        break;
                    }
                    word |= !ch.is_whitespace();
                }
                if !complete {
                    if self.streaming.get() {
                        break;
                    }
                    end = text.len();
                }
                if end <= leaf.visible {
                    break;
                }
                leaf.visible = end;
                leaf.remaining_words = leaf.remaining_words.saturating_sub(1);
                leaf.budget -= 1.0;
            }
            if leaf.visible > start {
                // The original reveal swept each admitted segment from left to
                // right in three-byte runs spread over one150ms interval.
                let chunk_bytes = (leaf.visible - start).div_ceil(512).max(3);
                let count = (leaf.visible - start).div_ceil(chunk_bytes).max(1);
                let step = INTERVAL / u32::try_from(count).unwrap_or(u32::MAX);
                let mut from = start;
                let mut mark = 0u32;
                for (offset, ch) in text[start..leaf.visible].char_indices() {
                    let end = start + offset + ch.len_utf8();
                    if end - from >= chunk_bytes || end == leaf.visible {
                        leaf.marks.push_back((from..end, now + step * mark));
                        from = end;
                        mark += 1;
                    }
                }
                leaf.next_tick = now + INTERVAL;
            }
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
        let pending_words = if self.streaming.get() {
            text[leaf.visible..]
                .rfind(char::is_whitespace)
                .is_some_and(|end| {
                    text[leaf.visible..leaf.visible + end]
                        .split_whitespace()
                        .next()
                        .is_some()
                })
        } else {
            waiting
        };
        self.settling.set(
            self.settling.get()
                || pending_words
                || leaf
                    .marks
                    .back()
                    .is_some_and(|(_, started)| now.saturating_duration_since(*started) < FADE / 2),
        );
        self.animating.set(
            self.animating.get() || (!self.paused.get() && pending_words) || !leaf.marks.is_empty(),
        );
        if !waiting && leaf.marks.is_empty() {
            return base;
        }
        let mut highlights = base.to_vec();
        for (range, started) in &leaf.marks {
            let progress =
                now.saturating_duration_since(*started).as_secs_f64() / FADE.as_secs_f64();
            highlights.push((
                range.clone(),
                HighlightStyle {
                    fade_out: Some(
                        1.0 - MotionCurve::EaseOut.sample(progress.clamp(0.0, 1.0)) as f32,
                    ),
                    ..Default::default()
                },
            ));
        }
        if waiting {
            highlights.push((
                leaf.visible..text.len(),
                HighlightStyle {
                    fade_out: Some(1.0),
                    ..Default::default()
                },
            ));
        }
        highlights.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_unicode_words_fade_without_changing_the_prepared_text() {
        let reveal = PreparedReveal::default();
        reveal.begin_frame(false, true, false);
        let text: SharedString = "héllo 世界 more text".into();
        let now = Instant::now();
        let fades = reveal.highlights_at(1, &text, Rc::from([]), now);
        assert!(fades.iter().all(
            |(range, _)| text.is_char_boundary(range.start) && text.is_char_boundary(range.end)
        ));
        assert!(reveal.is_animating());
        assert_eq!(reveal.leaves.borrow()[&1].visible, "héllo ".len());
        let done = reveal.highlights_at(1, &text, Rc::from([]), now + Duration::from_secs(5));
        assert!(done.iter().any(|(_, style)| style.fade_out.is_some()));
        assert_eq!(reveal.leaves.borrow()[&1].text, text);
    }
    #[test]
    fn history_resumes_whole_and_reduced_motion_drops_reveal_ownership() {
        let reveal = PreparedReveal::default();
        reveal.begin_frame(true, false, false);
        let text: SharedString = "Already streaming words".into();
        assert!(reveal.highlights(1, &text, Rc::from([])).is_empty());
        assert!(!reveal.is_animating());
        reveal.begin_frame(true, true, true);
        assert!(reveal.highlights(1, &text, Rc::from([])).is_empty());
        assert!(reveal.leaves.borrow().is_empty());
    }
}
