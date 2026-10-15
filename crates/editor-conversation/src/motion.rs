//! Row entrances, counting labels, disclosure controls and compaction animate locally.
//! Native ListState owns measurement, disclosure rows and scroll anchors.
use editor_ui::motion::{MotionCurve, MotionDuration, MotionPlan, MotionPolicy, MotionRecipe};
use gpui::{
    AnyElement, Div, Filter, FontFeatures, IntoElement, ParentElement as _, Pixels, SharedString,
    Styled as _, StyledText, div, px,
};
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
const ENTRANCE: Duration = Duration::from_millis(400);
const ENTRANCE_RISE_PX: f32 = 4.0;
const ROLL: Duration = Duration::from_millis(500);
const ROLL_STAGGER: Duration = Duration::from_millis(40);
const SWAP: Duration = Duration::from_millis(150);
const SWAP_RISE_PX: f32 = 4.0;
const SWAP_BLUR_PX: f32 = 2.0;
const HEADER_HOLD: Duration = MotionDuration::Quick.as_duration();
// The compaction separator.
const COMPACTION_OPEN: Duration = Duration::from_millis(250);
const COMPACTION_DRAW_DELAY: Duration = Duration::from_millis(150);
const COMPACTION_DRAW: Duration = Duration::from_millis(400);
const COMPACTION_LABEL_DELAY: Duration = Duration::from_millis(230);
const COMPACTION_LABEL_RISE: Duration = Duration::from_millis(250);
const COMPACTION_SWAP_HALF: Duration = Duration::from_millis(150);
const COMPACTION_PULSE: Duration = Duration::from_millis(500);

#[derive(Default)]
pub(crate) struct WorkMotion {
    live: bool,
    policy: Option<MotionPolicy>,
    animating: bool,
    settling: bool,
    entrance: Option<Option<Instant>>,
    entrance_height: Rc<Cell<Pixels>>,
    compactions: HashMap<String, CompactionMotion>,
    phrases: HashMap<String, PhraseState>,
    header: Option<(bool, Instant)>,
    chevrons: HashMap<String, ChevronMotion>,
}
impl WorkMotion {
    pub(crate) fn begin_frame(&mut self, live: bool, policy: MotionPolicy) {
        self.live = live;
        self.policy = Some(policy);
        self.animating = false;
        self.settling = false;
    }
    pub(crate) fn is_settling(&self) -> bool {
        self.settling
    }
    pub(crate) fn end_frame(&mut self) -> bool {
        self.animating
    }
    fn policy(&self) -> MotionPolicy {
        self.policy.unwrap_or(MotionPolicy::Full)
    }
    pub(crate) fn entrance_started(&self) -> bool {
        self.entrance.is_some()
    }

    pub(crate) fn entrance(&mut self, _key: &str, fresh: bool) -> Option<Entrance> {
        let animate = fresh && self.live && self.policy() == MotionPolicy::Full;
        let started = *self
            .entrance
            .get_or_insert_with(|| animate.then(Instant::now));
        let elapsed = started?.elapsed();
        if elapsed >= ENTRANCE || self.policy() != MotionPolicy::Full {
            return None;
        }
        self.animating = true;
        self.settling |= elapsed < Duration::from_millis(205);
        Some(Entrance {
            grow: progress_of(elapsed, Duration::from_millis(250), MotionCurve::SmoothOut),
            measured: self.entrance_height.clone(),
            reveal: progress_of(
                elapsed.saturating_sub(Duration::from_millis(80)),
                Duration::from_millis(250),
                MotionCurve::EaseOut,
            ),
        })
    }
    pub(crate) fn phrase(&mut self, key: &str, text: &str) -> Option<Vec<PhraseWord>> {
        let animate = self.policy() == MotionPolicy::Full;
        let words: Vec<String> = text.split(' ').map(str::to_owned).collect();
        let now = Instant::now();
        let state = self
            .phrases
            .entry(key.to_owned())
            .or_insert_with(|| PhraseState {
                current: words.clone(),
                previous: words.clone(),
                changed: None,
            });
        if state.current != words {
            state.previous = std::mem::replace(&mut state.current, words);
            state.changed = animate.then_some(now);
        }
        if !animate {
            state.changed = None;
            return None;
        }
        let longest = ROLL + ROLL_STAGGER * 4;
        let elapsed = state
            .changed
            .map(|changed| now.saturating_duration_since(changed))
            .filter(|elapsed| *elapsed < longest);
        if elapsed.is_none() {
            state.changed = None;
        }
        self.animating |= elapsed.is_some();
        let previous = if elapsed.is_some() {
            &state.previous
        } else {
            &state.current
        };
        let elapsed = elapsed.unwrap_or(longest);
        Some(
            state
                .current
                .iter()
                .enumerate()
                .map(|(index, word)| PhraseWord {
                    text: word.clone(),
                    motion: word_motion(previous.get(index).map(String::as_str), word),
                    elapsed,
                })
                .collect(),
        )
    }

    /// How far the work header's entrance has played, `None` at rest. Only a
    /// header first mounted while its run worked enters (frozen at that
    /// mount: history that later goes live never does), and only once.
    pub(crate) fn header_entrance(&mut self) -> Option<f32> {
        let live = self.live;
        let (working, mounted) = *self.header.get_or_insert_with(|| (live, Instant::now()));
        let MotionPlan::Animate(animation) = self.policy().resolve(MotionRecipe::TextSwap) else {
            return None;
        };
        if !working {
            return None;
        }
        let elapsed = Instant::now().saturating_duration_since(mounted);
        if elapsed >= HEADER_HOLD + animation.duration() {
            return None;
        }
        self.animating = true;
        Some(match elapsed.checked_sub(HEADER_HOLD) {
            None => 0.0,
            Some(run) => sample(MotionCurve::EaseInOut, run, animation.duration()),
        })
    }

    /// Rotate from the fraction currently painted when a disclosure reverses.
    pub(crate) fn chevron(&mut self, key: &str, open: bool) -> f32 {
        let target = if open { 1.0 } else { 0.0 };
        let motion = self
            .chevrons
            .entry(key.to_owned())
            .or_insert(ChevronMotion {
                from: target,
                target,
                started: Instant::now(),
            });
        let elapsed = motion.started.elapsed();
        let progress = progress_of(elapsed, Duration::from_millis(250), MotionCurve::SmoothOut);
        let current = motion.from + (motion.target - motion.from) * progress;
        if motion.target != target {
            motion.from = current;
            motion.target = target;
            motion.started = Instant::now();
        }
        if self.policy.unwrap_or(MotionPolicy::Full) != MotionPolicy::Full {
            motion.from = target;
            return target;
        }
        self.animating |= (current - target).abs() > 0.001;
        current
    }
    pub(crate) fn compaction(
        &mut self,
        key: &str,
        phase: CompactionPhase,
        fresh: bool,
    ) -> CompactionFrame {
        let full = self.live && self.policy() == MotionPolicy::Full;
        let now = Instant::now();
        let motion = self
            .compactions
            .entry(key.to_owned())
            .or_insert_with(|| CompactionMotion::first_seen(phase, full && fresh, now));
        let (frame, moving) = motion.advance(phase, full, now);
        self.animating |= moving;
        frame
    }
}
pub(crate) struct Entrance {
    grow: f32,
    reveal: f32,
    measured: Rc<Cell<Pixels>>,
}

/// Wraps a row in its entrance: the height opens from nothing (clipped, so
/// what follows glides down) and the content fades in a beat later while it
/// settles 4 px into place. `gap` is the space before the row, which closes
/// with the height so no empty band shows first.
pub(crate) fn with_entrance(
    element: AnyElement,
    entrance: Option<Entrance>,
    gap: Pixels,
) -> AnyElement {
    let Some(Entrance {
        grow,
        reveal,
        measured,
    }) = entrance
    else {
        return element;
    };
    let height = measured.get() * grow;
    let content = div()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .relative()
        .top(px(ENTRANCE_RISE_PX * (1.0 - reveal)))
        .opacity(reveal)
        .child(element);
    div()
        .w_full()
        .min_w_0()
        .h(height)
        .mt(-(gap * (1.0 - grow)))
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(content)
        .on_children_prepainted(move |bounds, _, _| {
            if let Some(bounds) = bounds.first() {
                measured.set(bounds.size.height);
            }
        })
        .into_any_element()
}

/// The header entrance at `progress`: fading in, rising 4 px and
/// un-blurring from 2 px, all on one eased clock.
pub(crate) fn header_entering(header: Div, progress: f32) -> Div {
    header
        .opacity(progress)
        .top(px(-4.0 * (1.0 - progress)))
        .filter(vec![Filter::Blur(px(2.0 * (1.0 - progress)))])
}

/// One changing phrase's words now and before its latest change.
struct PhraseState {
    current: Vec<String>,
    previous: Vec<String>,
    changed: Option<Instant>,
}

/// How one word of a phrase moves this frame.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum WordMotion {
    Still,
    /// A number rolling from `from`; `up` when the value grew.
    Roll {
        from: String,
        up: bool,
    },
    /// The word grew: its first `kept` bytes stay, the rest fade in.
    Extend {
        kept: usize,
    },
    /// A new or replaced word rising in.
    Enter,
}

/// One word of a phrase with its motion and elapsed time.
#[derive(Clone, Debug)]
pub(crate) struct PhraseWord {
    text: String,
    motion: WordMotion,
    elapsed: Duration,
}

/// How `word` moves given the word it replaced at the same position.
fn word_motion(previous: Option<&str>, word: &str) -> WordMotion {
    let Some(previous) = previous else {
        return WordMotion::Enter;
    };
    if previous == word {
        return WordMotion::Still;
    }
    if let (Some((from_suffix, from_value)), Some((to_suffix, to_value))) =
        (number_word(previous), number_word(word))
        && from_suffix == to_suffix
    {
        return WordMotion::Roll {
            from: previous.to_owned(),
            up: to_value >= from_value,
        };
    }
    if word.starts_with(previous) {
        return WordMotion::Extend {
            kept: previous.len(),
        };
    }
    WordMotion::Enter
}

/// Splits a counting word into its unit suffix and value. The article
/// that stands for one ("a command", "an edit") counts as `1`.
fn number_word(word: &str) -> Option<(&str, u64)> {
    if matches!(word, "a" | "an") {
        return Some(("", 1));
    }
    let digits_end = word
        .char_indices()
        .find(|(_, character)| !character.is_ascii_digit())
        .map_or(word.len(), |(index, _)| index);
    if digits_end == 0 {
        return None;
    }
    let (digits, suffix) = word.split_at(digits_end);
    Some((suffix, digits.parse().ok()?))
}

struct ChevronMotion {
    from: f32,
    target: f32,
    started: Instant,
}

fn sample(curve: MotionCurve, elapsed: Duration, duration: Duration) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the easing curve samples in f64 and feeds f32 paint values"
    )]
    let value = curve.sample(elapsed.as_secs_f64() / duration.as_secs_f64()) as f32;
    value
}

fn progress_of(elapsed: Duration, duration: Duration, curve: MotionCurve) -> f32 {
    if elapsed >= duration {
        1.0
    } else {
        sample(curve, elapsed, duration)
    }
}

/// Tabular figures (`tnum`): a counting number keeps its width as its
/// digits change.
pub(crate) fn tabular_figures() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// A phrase as plain text, or word by word while it moves.
pub(crate) fn phrase_or_text(
    words: Option<Vec<PhraseWord>>,
    text: String,
    line_height: Pixels,
) -> AnyElement {
    match words {
        Some(words) => phrase_element(&words, line_height),
        None => text.into_any_element(),
    }
}

/// Paints a changing phrase word by word on one line, in tabular figures.
fn phrase_element(words: &[PhraseWord], line_height: Pixels) -> AnyElement {
    let mut row = div()
        .font_features(tabular_figures())
        .flex()
        .flex_row()
        .flex_nowrap()
        .items_center()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap();
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            row = row.child(" ");
        }
        row = row.child(word_element(word, line_height));
    }
    row.into_any_element()
}

fn word_element(word: &PhraseWord, line_height: Pixels) -> AnyElement {
    match &word.motion {
        // A number rests in the same per-character columns it rolls in, so
        // it never shifts between ticks.
        WordMotion::Still if word.text.chars().any(|c| c.is_ascii_digit()) => {
            character_columns(&word.text)
        }
        WordMotion::Still => div()
            .flex_none()
            .child(word.text.clone())
            .into_any_element(),
        WordMotion::Enter => swap_in(word.text.clone(), word.elapsed),
        // One text run while its added letters fade in: the layout it
        // settles in.
        WordMotion::Extend { kept } => {
            let kept = (*kept).min(word.text.len());
            let progress = progress_of(word.elapsed, SWAP, MotionCurve::EaseInOut);
            let text = SharedString::from(word.text.clone());
            let added = kept..text.len();
            let styled = if added.is_empty() {
                StyledText::new(text)
            } else {
                StyledText::new(text).with_highlights([(
                    added,
                    gpui::HighlightStyle {
                        fade_out: Some(1.0 - progress),
                        ..Default::default()
                    },
                )])
            };
            div().flex_none().child(styled).into_any_element()
        }
        WordMotion::Roll { from, up } => odometer(from, &word.text, *up, word.elapsed, line_height),
    }
}

/// A word as one column per character, the layout the odometer rolls in.
fn character_columns(text: &str) -> AnyElement {
    let mut row = div().flex().flex_row().flex_none();
    for character in text.chars() {
        row = row.child(div().child(character.to_string()));
    }
    row.into_any_element()
}

/// A word rising 4 px into place while it fades in (text swap enter).
fn swap_in(text: String, elapsed: Duration) -> AnyElement {
    let progress = progress_of(elapsed, SWAP, MotionCurve::EaseInOut);
    div()
        .flex_none()
        .relative()
        .top(px(SWAP_RISE_PX * (1.0 - progress)))
        .opacity(progress)
        .filter(vec![Filter::Blur(px(SWAP_BLUR_PX * (1.0 - progress)))])
        .child(text)
        .into_any_element()
}

/// A number rolling from `from` to `to`: right-aligned digit columns, each
/// a clipped reel through every digit between its old and new value,
/// rightmost first.
fn odometer(from: &str, to: &str, up: bool, elapsed: Duration, line_height: Pixels) -> AnyElement {
    let from_chars: Vec<char> = from.chars().collect();
    let to_chars: Vec<char> = to.chars().collect();
    let mut row = div().flex().flex_row().flex_none();
    for (column, &target) in to_chars.iter().enumerate() {
        let from_right = to_chars.len() - 1 - column;
        let source = from_chars
            .len()
            .checked_sub(1 + from_right)
            .map(|index| from_chars[index]);
        let delay = ROLL_STAGGER * u32::try_from(from_right).unwrap_or(0);
        let local = elapsed.saturating_sub(delay);
        row = row.child(match source {
            Some(source) if source == target => div().child(target.to_string()).into_any_element(),
            Some(source) => reel(source, target, up, local, line_height),
            // A column the number grew into rises in like a new word.
            None => swap_in(target.to_string(), local),
        });
    }
    row.into_any_element()
}

/// One clipped digit column scrolling from `source` to `target`.
fn reel(
    source: char,
    target: char,
    up: bool,
    elapsed: Duration,
    line_height: Pixels,
) -> AnyElement {
    let mut cells = vec![source];
    if let (Some(from), Some(to)) = (source.to_digit(10), target.to_digit(10)) {
        let step = if up { 1 } else { 9 };
        let mut digit = from;
        while digit != to {
            digit = (digit + step) % 10;
            cells.push(char::from_digit(digit, 10).unwrap_or(target));
        }
    } else {
        cells.push(target);
    }
    // Counting down, the strip is laid out in reverse and moves down.
    if !up {
        cells.reverse();
    }
    let steps = f32::from(u16::try_from(cells.len() - 1).unwrap_or(u16::MAX));
    let travel = f32::from(line_height) * steps;
    let progress = progress_of(elapsed, ROLL, MotionCurve::SmoothOut);
    let offset = if up {
        -travel * progress
    } else {
        -travel * (1.0 - progress)
    };
    let mut strip = div()
        .absolute()
        .left_0()
        .right_0()
        .top(px(offset))
        .flex()
        .flex_col();
    for cell in cells {
        strip = strip.child(
            div()
                .h(line_height)
                .flex()
                .items_center()
                .justify_center()
                .child(cell.to_string()),
        );
    }
    // The invisible target sizes the column; the reel scrolls inside it.
    div()
        .relative()
        .flex_none()
        .h(line_height)
        .overflow_hidden()
        .child(div().opacity(0.0).child(target.to_string()))
        .child(strip)
        .into_any_element()
}

/// What a compaction separator says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompactionPhase {
    Running,
    Done,
    Stopped,
}

impl CompactionPhase {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Running => "Compacting",
            Self::Done => "Compacted",
            Self::Stopped => "Compaction stopped",
        }
    }
}

/// One separator's motion.
#[derive(Debug)]
struct CompactionMotion {
    phase: CompactionPhase,
    /// When it appeared live, while its entrance plays.
    entered: Option<Instant>,
    /// When it settled live, and the phase it settled from.
    settled: Option<(Instant, CompactionPhase)>,
}

/// One frame of a separator: every progress runs 0 to 1 and rests at 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CompactionFrame {
    /// Row height.
    pub open: f32,
    /// Rule length, from the label outward.
    pub draw: f32,
    /// Label rise-in.
    pub label: f32,
    /// The label swapped out and the swap's progress over both halves.
    pub swap: Option<(CompactionPhase, f32)>,
    /// The checkpoint highlight's run along the rules.
    pub pulse: Option<f32>,
}

impl CompactionFrame {
    const REST: Self = Self {
        open: 1.0,
        draw: 1.0,
        label: 1.0,
        swap: None,
        pulse: None,
    };
}

impl CompactionMotion {
    fn first_seen(phase: CompactionPhase, enter: bool, now: Instant) -> Self {
        Self {
            phase,
            entered: (enter && phase == CompactionPhase::Running).then_some(now),
            settled: None,
        }
    }

    fn advance(
        &mut self,
        phase: CompactionPhase,
        full_motion: bool,
        now: Instant,
    ) -> (CompactionFrame, bool) {
        if phase != self.phase {
            // A stop has no checkpoint and swaps at once.
            self.settled = (full_motion
                && self.phase == CompactionPhase::Running
                && phase == CompactionPhase::Done)
                .then_some((now, self.phase));
            self.phase = phase;
        }
        if !full_motion {
            self.entered = None;
            self.settled = None;
        }
        let mut frame = CompactionFrame::REST;
        if let Some(entered) = self.entered {
            let elapsed = now.saturating_duration_since(entered);
            frame.open = eased(
                MotionCurve::SmoothOut,
                linear(elapsed, Duration::ZERO, COMPACTION_OPEN),
            );
            frame.draw = eased(
                MotionCurve::SmoothOut,
                linear(elapsed, COMPACTION_DRAW_DELAY, COMPACTION_DRAW),
            );
            frame.label = eased(
                MotionCurve::SmoothOut,
                linear(elapsed, COMPACTION_LABEL_DELAY, COMPACTION_LABEL_RISE),
            );
            if elapsed
                >= (COMPACTION_DRAW_DELAY + COMPACTION_DRAW)
                    .max(COMPACTION_LABEL_DELAY + COMPACTION_LABEL_RISE)
            {
                self.entered = None;
            }
        }
        if let Some((settled, from)) = self.settled {
            let elapsed = now.saturating_duration_since(settled);
            let swap = linear(elapsed, Duration::ZERO, COMPACTION_SWAP_HALF * 2);
            if swap < 1.0 {
                frame.swap = Some((from, swap));
            }
            let pulse = linear(elapsed, COMPACTION_SWAP_HALF * 2, COMPACTION_PULSE);
            if pulse > 0.0 && pulse < 1.0 {
                frame.pulse = Some(pulse);
            }
            if elapsed >= COMPACTION_SWAP_HALF * 2 + COMPACTION_PULSE {
                self.settled = None;
            }
        }
        (
            frame,
            frame != CompactionFrame::REST || self.entered.is_some() || self.settled.is_some(),
        )
    }
}

/// Linear progress of a clock that starts `delay` in and runs `duration`.
fn linear(elapsed: Duration, delay: Duration, duration: Duration) -> f32 {
    let Some(run) = elapsed.checked_sub(delay) else {
        return 0.0;
    };
    (run.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}

#[expect(clippy::cast_possible_truncation, reason = "progress stays in 0..=1")]
pub(crate) fn eased(curve: MotionCurve, value: f32) -> f32 {
    curve.sample(f64::from(value)) as f32
}

/// Only mounted native rows participate in the top-to-bottom reveal queue.
/// Offscreen cached rows expire rather than keeping visible work waiting.
#[derive(Default)]
pub(crate) struct RevealQueue {
    rows: HashMap<crate::fold::RowKey, RevealGate>,
}
struct RevealGate {
    order: (u64, u8, usize),
    seen: Instant,
    settling: bool,
}
impl RevealQueue {
    pub(crate) fn begin(
        &mut self,
        key: crate::fold::RowKey,
        order: (u64, u8, usize),
        enabled: bool,
    ) -> bool {
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
    pub(crate) fn frontier_key(&self) -> Option<crate::fold::RowKey> {
        let now = Instant::now();
        self.rows
            .iter()
            .filter(|(_, row)| {
                row.settling && now.saturating_duration_since(row.seen) < Duration::from_millis(250)
            })
            .min_by_key(|(_, row)| row.order)
            .map(|(key, _)| key.clone())
    }
    pub(crate) fn finish(&mut self, key: &crate::fold::RowKey, settling: bool) {
        if let Some(row) = self.rows.get_mut(key) {
            row.settling = settling;
        }
    }
    pub(crate) fn remove(&mut self, key: &crate::fold::RowKey) {
        self.rows.remove(key);
    }
}

#[cfg(test)]
mod restored_motion_tests {
    use super::*;
    use crate::fold::RowKey;
    #[test]
    fn queue_holds_later_rows_but_releases_after_midpoint_or_eviction() {
        let mut queue = RevealQueue::default();
        let first = RowKey::Item(artisan_api::ItemId::new());
        let next = RowKey::Item(artisan_api::ItemId::new());
        assert!(!queue.begin(first.clone(), (1, 1, 0), true));
        queue.finish(&first, true);
        assert!(queue.begin(next.clone(), (2, 1, 0), true));
        assert!(!queue.begin(next.clone(), (2, 1, 0), false));
        queue.finish(&first, false);
        assert!(!queue.begin(next.clone(), (2, 1, 0), true));
        queue.finish(&first, true);
        queue.rows.get_mut(&first).unwrap().seen = Instant::now() - Duration::from_secs(1);
        assert!(!queue.begin(next, (2, 1, 0), true));
        assert!(!queue.rows.contains_key(&first));
    }
    #[test]
    fn phrase_counts_roll_and_new_words_enter() {
        assert_eq!(
            word_motion(Some("a"), "2"),
            WordMotion::Roll {
                from: "a".into(),
                up: true
            }
        );
        assert_eq!(
            word_motion(Some("9s"), "10s"),
            WordMotion::Roll {
                from: "9s".into(),
                up: true
            }
        );
        assert_eq!(word_motion(Some("1m"), "1m"), WordMotion::Still);
        assert_eq!(word_motion(None, "commands"), WordMotion::Enter);
    }
}

/// First-seen live semantic ordinals. The loaded Item owns this record across
/// native row eviction. Only observed visible source events admit fresh rows;
/// page/index preparation alone establishes a history baseline.
pub(crate) struct LiveBlockArrivals {
    high_water: usize,
    pending: bool,
    fresh: HashMap<usize, Instant>,
    pub(crate) seen: Instant,
}
impl LiveBlockArrivals {
    pub(crate) fn is_fresh(&self, block: usize) -> bool {
        self.fresh.contains_key(&block)
    }
    pub(crate) fn baseline(blocks: usize) -> Self {
        Self {
            high_water: blocks,
            pending: false,
            fresh: HashMap::new(),
            seen: Instant::now(),
        }
    }
    pub(crate) fn observed(&mut self) {
        self.pending = true;
        self.seen = Instant::now();
    }
    pub(crate) fn prepared(&mut self, blocks: usize, caught_up: bool, initially_fresh: bool) {
        let now = Instant::now();
        if self.pending || initially_fresh {
            // At most the native owner cache's128 latest ordinals remain eligible.
            for block in self.high_water.max(blocks.saturating_sub(128))..blocks {
                self.fresh.insert(block, now);
            }
            while self.fresh.len() > 128 {
                let oldest = *self.fresh.keys().min().unwrap();
                self.fresh.remove(&oldest);
            }
        }
        self.high_water = self.high_water.max(blocks);
        if caught_up {
            self.pending = false;
        }
        self.seen = now;
    }
    pub(crate) fn claim(&mut self, block: usize) -> bool {
        self.fresh.remove(&block).is_some()
    }
}

#[cfg(test)]
mod live_block_arrival_tests {
    use super::*;
    #[test]
    fn a_late_live_block_is_fresh_once_but_loading_and_remounts_are_history() {
        let mut arrivals = LiveBlockArrivals::baseline(3);
        arrivals.prepared(3, true, false);
        assert!(!arrivals.claim(2));
        arrivals.observed();
        arrivals.prepared(4, true, false);
        assert!(arrivals.claim(3));
        assert!(!arrivals.claim(3));
        arrivals.prepared(8, true, false);
        assert!(
            !arrivals.claim(7),
            "index or page preparation is not a live event"
        );
    }
    #[test]
    fn intermediate_preparation_does_not_consume_a_newer_observed_update() {
        let mut arrivals = LiveBlockArrivals::baseline(1);
        arrivals.observed();
        arrivals.prepared(2, false, false);
        assert!(arrivals.claim(1));
        arrivals.prepared(3, true, false);
        assert!(arrivals.claim(2));
        arrivals.prepared(4, true, false);
        assert!(!arrivals.claim(3));
    }
}
