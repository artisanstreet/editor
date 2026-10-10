//! Motion of a live turn's work: streamed prose, entering rows, counting
//! phrases, and the status line's exit.
//!
//! Every animation here belongs to one turn row and runs only while that
//! turn is live: settled history and a row's first render paint at rest, so
//! scrolling back or opening a thread never replays anything. Motion follows
//! the transitions-dev tokens:
//!
//! - Prose inside the work streams exactly like the reply ([`ReplyReveal`]).
//! - A work row (a tool chain, prose, a chain step, the status line) enters
//!   with the texts-reveal recipe: 500 ms smooth-out, rising 12 px and
//!   un-blurring from 3 px while it fades in.
//! - A phrase that keeps changing (a chain's "Ran 2 commands, read a file",
//!   the "Working for 1m 4s" header) animates per word. Its first paint is
//!   the row's entrance; after that a changed number rolls like an odometer,
//!   digit columns sliding up as it counts up and down as it counts down; a
//!   word that grows ("command" → "commands") fades its new letters in; a
//!   replaced word and appended words rise in with the text-swap recipe
//!   (150 ms ease-in-out, 4 px, 2 px blur).
//! - The status line leaves by fading out while its height closes, so
//!   nothing below it jumps.
//!
//! A turn reveals strictly top to bottom through one reveal queue. The row
//! renders its blocks in paint order, and each frame carries a gate: while
//! something painted above is still settling (prose with words left to
//! reveal or a segment not yet halfway faded in, or a row whose entrance
//! fade has not reached its midpoint), every block below waits. A waiting
//! block paints nothing, keeps no scroll identity, and starts no entrance
//! clock; prose below the gate keeps its reveal paused at what it shows.
//! The row keeps requesting frames while anything waits, so the first frame
//! the gate clears releases the next block with its normal entrance, and
//! the blocks after it cascade in at half-fade intervals. While a block
//! waits, the prose above it drains at a hurried pace. Actionable blocks
//! (approvals, questions) wait at most [`ACTIONABLE_HOLD_CAP`]. Nothing waits
//! outside full live motion: history, a first render, a caught-up frame,
//! and reduced motion paint everything at once, as before.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use artisan_ui::markdown_reveal::stable_reveal_end;
use gpui::{FontFeatures, Pixels, StyledText};

use super::reply_reveal::ReplyReveal;
use super::*;

/// Row entrance: `--duration-very-slow` (texts reveal).
const ENTRANCE: Duration = Duration::from_millis(400);
/// The row's height opening (`--duration-fast`, accordion/card resize).
const ENTRANCE_GROW: Duration = MotionDuration::Fast.as_duration();
/// The content's fade waits this long, so it fades into opened space.
const ENTRANCE_FADE_DELAY: Duration = Duration::from_millis(80);
/// The content's fade (`--duration-fast`).
const ENTRANCE_FADE: Duration = MotionDuration::Fast.as_duration();
/// The content's rise into place (`--distance-micro`).
const ENTRANCE_RISE_PX: f32 = 4.0;
/// Odometer roll of one digit column (`--duration-very-slow`).
const ROLL: Duration = Duration::from_millis(500);
/// Offset between neighbouring digit columns (`--duration-stagger`).
const ROLL_STAGGER: Duration = Duration::from_millis(40);
/// In-place word swap: `--duration-quick` (text swap).
const SWAP: Duration = Duration::from_millis(150);
/// In-place word swap rise: `--distance-micro` (text swap).
const SWAP_RISE_PX: f32 = 4.0;
/// In-place word swap blur: `--blur-small` (text swap).
const SWAP_BLUR_PX: f32 = 2.0;
/// A new thinking summary swapping into the status line (`--duration-fast`).
const SUMMARY_SWAP: Duration = MotionDuration::Fast.as_duration();
/// Status line exit: the texts-reveal quiet fade out.
const STATUS_EXIT: Duration = Duration::from_millis(200);
/// The longest an actionable block (an approval, a question) waits behind
/// settling content above it: the run is blocked on the reader, so it must
/// not sit behind a long drain.
pub(super) const ACTIONABLE_HOLD_CAP: Duration = Duration::from_millis(300);

/// How long an entering row holds the queue: until its content fade passes
/// its midpoint, so the blocks after it cascade instead of arriving at once.
fn entrance_handoff() -> Duration {
    ENTRANCE_FADE_DELAY + ENTRANCE_FADE / 2
}

/// One turn row's work motion, retained across its renders.
#[derive(Default)]
pub(super) struct WorkMotion {
    /// Streaming reveals of live prose, by scene id.
    reveals: HashMap<SceneId, ReplyReveal>,
    /// Reveals touched by the current frame; the rest are dropped.
    touched: HashSet<SceneId>,
    /// Every prose body this row has painted, revealed or not.
    seen_prose: HashSet<SceneId>,
    /// When each work row first painted while the turn was live.
    entrances: HashMap<String, Option<Instant>>,
    /// Natural heights of rows mid-entrance, measured each frame.
    entrance_heights: HashMap<String, Rc<Cell<Pixels>>>,
    /// Set after the row's first render: rows already on screen then never
    /// enter.
    primed: bool,
    /// Whether the turn is live this frame, and how much it may move.
    pace: Pace,
    /// Changing phrases, by key.
    phrases: HashMap<String, PhraseState>,
    /// The status line painted last frame and its measured height.
    status: Option<(String, Pixels)>,
    /// Whether this frame painted a status line.
    status_painted: bool,
    /// A status line on its way out.
    status_exit: Option<StatusExit>,
    /// The status line's thinking summary and its swap, if one plays.
    summary: Option<SummaryState>,
    /// Whether anything still moves; the row then asks for the next frame.
    animating: bool,
    /// The turn's top-to-bottom reveal queue.
    queue: RevealQueue,
    /// Disclosures last painted folded, by selector.
    folded: HashSet<String>,
    /// Compaction separators, by scene id.
    compactions: HashMap<SceneId, super::compaction_separator::CompactionMotion>,
    /// How many disclosures the reader just opened are rendering their rows
    /// right now. Their rows already happened, so they paint at rest.
    opening: u32,
}

/// One turn's reveal queue (see the module docs).
#[derive(Default)]
struct RevealQueue {
    /// The gate this frame: set once something painted earlier in paint
    /// order is still settling, so everything after it waits.
    settling: bool,
    /// Blocks waiting behind the gate and when each started waiting.
    waiting: HashMap<String, Instant>,
    /// Blocks that waited this frame; the rest leave [`Self::waiting`].
    waited: HashSet<String>,
    /// Whether this frame held anything back.
    held: bool,
    /// Whether the previous frame held anything back: settling prose then
    /// drains at the hurried pace.
    hurry: bool,
}

/// How much a frame of the row may move.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Pace {
    /// The turn has settled: everything paints at rest.
    #[default]
    Settled,
    /// The turn is live under reduced motion: prose shows whole segments
    /// at once and nothing animates.
    Reduced,
    /// The turn is live with full motion.
    Full,
}

/// A status line fading out after it left the scene.
struct StatusExit {
    copy: String,
    height: Pixels,
    started: Instant,
}

/// One changing phrase's words now and before its latest change.
struct PhraseState {
    current: Vec<String>,
    previous: Vec<String>,
    changed: Option<Instant>,
}

/// How one word of a phrase moves this frame.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum WordMotion {
    /// At rest.
    Still,
    /// A number rolling from `from`; `up` when the value grew.
    Roll { from: String, up: bool },
    /// The word grew: its first `kept` bytes stay, the rest fade in.
    Extend { kept: usize },
    /// A new or replaced word rising in.
    Enter,
}

/// One word of a phrase with its motion and elapsed time.
#[derive(Clone, Debug)]
pub(super) struct PhraseWord {
    pub(super) text: String,
    pub(super) motion: WordMotion,
    pub(super) elapsed: Duration,
}

impl WorkMotion {
    /// Starts one render of the row.
    ///
    /// A `caught_up` frame paints what arrived while nobody watched (a
    /// minimized or hidden window) at rest, like a first render: running
    /// reveals end, and prose still arriving shows what landed and streams
    /// only the rest.
    pub(super) fn begin_frame(&mut self, live: bool, motion: MotionPolicy, caught_up: bool) {
        if caught_up {
            self.primed = false;
            for id in self.reveals.keys() {
                self.seen_prose.remove(id);
            }
            self.reveals.clear();
            for phrase in self.phrases.values_mut() {
                phrase.changed = None;
            }
            self.queue.waiting.clear();
            self.queue.hurry = false;
        }
        self.pace = match (live, motion) {
            (false, _) => Pace::Settled,
            (true, MotionPolicy::Reduced) => Pace::Reduced,
            (true, MotionPolicy::Full) => Pace::Full,
        };
        self.animating = false;
        self.touched.clear();
        self.status_painted = false;
        self.queue.settling = false;
        self.queue.held = false;
        self.queue.waited.clear();
    }

    /// Ends one render; returns whether the row must render the next frame.
    pub(super) fn end_frame(&mut self) -> bool {
        self.primed = true;
        let touched = std::mem::take(&mut self.touched);
        self.reveals.retain(|id, _| touched.contains(id));
        self.touched = touched;
        if !self.status_painted {
            self.status = None;
        }
        let waited = std::mem::take(&mut self.queue.waited);
        self.queue.waiting.retain(|key, _| waited.contains(key));
        self.queue.waited = waited;
        self.queue.hurry = self.queue.held;
        self.animating
    }

    /// Advances one compaction separator to this frame's phase.
    pub(super) fn compaction(
        &mut self,
        id: &SceneId,
        phase: CompactionPhase,
    ) -> super::compaction_separator::CompactionFrame {
        let full = self.full_motion();
        let now = Instant::now();
        let motion = self.compactions.entry(id.clone()).or_insert_with(|| {
            super::compaction_separator::CompactionMotion::first_seen(phase, full, now)
        });
        let (frame, moving) = motion.advance(phase, full, now);
        self.animating |= moving;
        // Like any entering row, it holds the blocks below it in the reveal
        // queue until its label is halfway in.
        if frame.label < 0.5 {
            self.queue.settling = true;
        }
        frame
    }

    fn full_motion(&self) -> bool {
        self.fresh() && self.pace == Pace::Full
    }

    /// Whether what this row has not painted before is new to the reader:
    /// not on the row's first render, and not rows a disclosure the reader
    /// just opened reveals.
    fn fresh(&self) -> bool {
        self.primed && self.opening == 0
    }

    /// Notes one disclosure's state before its rows render; returns whether
    /// the reader just opened it. Rows it reveals then paint at rest until
    /// [`Self::end_disclosure`]: opening work that already happened never
    /// replays its entrances or reveals. Only rows that arrive while the
    /// disclosure stays open enter.
    pub(super) fn begin_disclosure(&mut self, selector: &str, mounted: bool) -> bool {
        if !mounted {
            self.folded.insert(selector.to_owned());
            return false;
        }
        let opened = self.folded.remove(selector);
        if opened {
            self.opening += 1;
        }
        opened
    }

    /// Ends the scope [`Self::begin_disclosure`] opened.
    pub(super) fn end_disclosure(&mut self, opened: bool) {
        if opened {
            self.opening = self.opening.saturating_sub(1);
        }
    }

    /// Whether the reveal queue holds back what renders next this frame.
    fn gate_closed(&self) -> bool {
        self.queue.settling && self.full_motion()
    }

    /// Whether the block keyed `key` waits behind the reveal queue this
    /// frame. A waiting block must paint nothing and must not call
    /// [`Self::entrance`], whose clock starts on its first call; the row
    /// keeps rendering frames until it is released.
    ///
    /// A block already on screen never waits again, so nothing painted
    /// disappears. An `actionable` block waits at most
    /// [`ACTIONABLE_HOLD_CAP`], then enters whatever still settles above.
    pub(super) fn hold(&mut self, key: &str, actionable: bool) -> bool {
        if !self.gate_closed() || self.entrances.contains_key(key) {
            self.queue.waiting.remove(key);
            return false;
        }
        let now = Instant::now();
        let since = *self.queue.waiting.entry(key.to_owned()).or_insert(now);
        if actionable && now.saturating_duration_since(since) >= ACTIONABLE_HOLD_CAP {
            self.queue.waiting.remove(key);
            return false;
        }
        self.queue.waited.insert(key.to_owned());
        self.animating = true;
        self.queue.held = true;
        true
    }

    /// Whether the prose row `id` waits behind the reveal queue this frame.
    ///
    /// Prose that has shown nothing yet waits like any block: it paints
    /// nothing while its reveal stays registered and paused, so it streams
    /// in from its first word once released. Prose already showing words
    /// renders on and pauses inside [`Self::reveal`] instead.
    pub(super) fn hold_prose(&mut self, id: &SceneId) -> bool {
        if !self.gate_closed() || self.entrances.contains_key(id.as_str()) {
            return false;
        }
        let now = Instant::now();
        if self.seen_prose.insert(id.clone()) {
            self.reveals.insert(id.clone(), ReplyReveal::begin(now));
        }
        let Some(reveal) = self.reveals.get_mut(id) else {
            return false;
        };
        if !reveal.revealed().is_empty() {
            return false;
        }
        reveal.wait(now);
        self.touched.insert(id.clone());
        self.animating = true;
        self.queue.held = true;
        true
    }

    /// Advances the streaming reveal of one prose body.
    ///
    /// Returns the prefix to paint and its fade while a reveal runs, or
    /// `None` to paint `body` whole. Prose that appears after the row first
    /// painted streams in whether or not it is still arriving: a reply that
    /// lands whole as the turn ends drains in segments instead of appearing
    /// at once. Prose already mid-stream at the row's first paint shows what
    /// arrived and streams the rest; settled history never animates. A
    /// reveal ends once its last segment has faded in, and the same id keeps
    /// its reveal when the scene moves the prose between the reply slot and
    /// the work.
    pub(super) fn reveal(
        &mut self,
        id: &SceneId,
        body: &str,
        streaming: bool,
    ) -> Option<(String, RevealFade)> {
        let motion = if self.pace == Pace::Reduced {
            MotionPolicy::Reduced
        } else {
            MotionPolicy::Full
        };
        let now = Instant::now();
        if self.seen_prose.insert(id.clone()) {
            if self.fresh() {
                self.reveals.insert(id.clone(), ReplyReveal::begin(now));
            } else if streaming {
                self.reveals
                    .insert(id.clone(), ReplyReveal::resume(body, now));
            }
        }
        let gated = self.gate_closed();
        let queued = self.full_motion();
        let hurry = self.queue.hurry;
        let reveal = self.reveals.get_mut(id)?;
        self.touched.insert(id.clone());
        if gated {
            // Something above still settles: what shows stays, its fades
            // finish, and nothing new reveals until the queue clears.
            reveal.wait(now);
            self.animating = true;
            self.queue.held = true;
            return Some((reveal.revealed().to_owned(), reveal.fade.clone()));
        }
        reveal.hurry = hurry;
        if reveal.advance(body, streaming, motion, now) {
            self.animating = true;
        } else if !streaming && reveal.is_complete(body) {
            self.reveals.remove(id);
            return None;
        }
        if queued && reveal.settling(body, streaming, now) {
            self.queue.settling = true;
        }
        Some((reveal.revealed().to_owned(), reveal.fade.clone()))
    }

    /// How many words of prose `id` are on screen, read without advancing
    /// its reveal: the turn decides whether its work folds before the
    /// reply renders in paint order (see `TurnRowView::advance_reply_reveal`).
    /// Prose this frame will begin revealing shows nothing yet; prose this
    /// frame resumes shows what arrived.
    pub(super) fn shown_words(&self, id: &SceneId, body: &str, streaming: bool) -> usize {
        let words = |text: &str| text.split_whitespace().count();
        if let Some(reveal) = self.reveals.get(id) {
            return words(reveal.revealed());
        }
        if self.seen_prose.contains(id) {
            return words(body);
        }
        if self.fresh() {
            0
        } else if streaming {
            words(&body[..stable_reveal_end(body)])
        } else {
            words(body)
        }
    }

    /// Records that the row keyed `key` is already on screen, so it never
    /// enters later: prose shown as the reply keeps its place when the scene
    /// moves it into the work.
    pub(super) fn mark_present(&mut self, key: &str) {
        self.entrances.entry(key.to_owned()).or_insert(None);
    }

    /// How far the status line's thinking summary has swapped in, while a
    /// new one does: it fades in and rises into place, like a text swap.
    /// The status line's first copy arrives with its own entrance, and the
    /// counting verb never swaps.
    ///
    /// Only a genuinely new summary swaps. One that grows or is restated in
    /// place (the live title settling to its buffered form, a public summary
    /// streaming its first line) updates without restarting, and a new one
    /// landing mid-swap continues from the opacity already reached: restarting
    /// from nothing read as the line fading, then flashing out and in again.
    /// A summary returning after the verb stood in for it swaps in too, never
    /// popping in at full opacity.
    pub(super) fn summary_swap(&mut self, summary: Option<&str>) -> Option<f32> {
        let animate = self.full_motion();
        let now = Instant::now();
        let Some(state) = self.summary.as_mut() else {
            // The status line's first copy rides its entrance.
            self.summary = Some(SummaryState {
                shown: summary.map(str::to_owned),
                swap: None,
            });
            return None;
        };
        let current = state.progress(now);
        match (state.shown.as_deref(), summary) {
            (Some(shown), Some(next)) if shown == next => {}
            (Some(shown), Some(next)) if summary_restates(shown, next) => {
                state.shown = Some(next.to_owned());
            }
            (_, Some(next)) => {
                state.shown = Some(next.to_owned());
                state.swap = animate.then(|| SummarySwap {
                    started: now,
                    from: current.unwrap_or(0.0),
                });
            }
            // The verb stands in for a summary without swapping.
            (_, None) => {
                state.shown = None;
                state.swap = None;
            }
        }
        let progress = state.progress(now)?;
        self.animating = true;
        Some(progress)
    }

    /// Records the visible length the renderer reported for `id`.
    pub(super) fn reveal_rendered(&mut self, id: &SceneId, visible_len: usize) {
        if let Some(reveal) = self.reveals.get_mut(id) {
            reveal.rendered(visible_len);
        }
    }

    /// The entrance of one row this frame, `None` at rest. Until its fade
    /// passes its midpoint, an entering row holds the reveal queue for
    /// everything rendered after it.
    pub(super) fn entrance(&mut self, key: &str) -> Option<Entrance> {
        self.enter(key, true)
    }

    /// The status line's entrance. The status line trails the turn and
    /// narrates it live, so it neither waits in nor holds the reveal queue.
    pub(super) fn status_entrance(&mut self) -> Option<Entrance> {
        self.enter("status", false)
    }

    fn enter(&mut self, key: &str, queued: bool) -> Option<Entrance> {
        let animate = self.full_motion();
        let started = *self
            .entrances
            .entry(key.to_owned())
            .or_insert_with(|| animate.then(Instant::now));
        let elapsed = started.map(|started| Instant::now().saturating_duration_since(started));
        let Some(elapsed) = elapsed.filter(|elapsed| *elapsed < ENTRANCE) else {
            self.entrance_heights.remove(key);
            return None;
        };
        if self.pace == Pace::Settled {
            self.entrance_heights.remove(key);
            return None;
        }
        self.animating = true;
        if queued && elapsed < entrance_handoff() {
            self.queue.settling = true;
        }
        Some(Entrance {
            grow: progress_of(elapsed, ENTRANCE_GROW, MotionCurve::SmoothOut),
            reveal: progress_of(
                elapsed.saturating_sub(ENTRANCE_FADE_DELAY),
                ENTRANCE_FADE,
                MotionCurve::EaseOut,
            ),
            measured: Rc::clone(
                self.entrance_heights
                    .entry(key.to_owned())
                    .or_insert_with(|| Rc::new(Cell::new(px(0.0)))),
            ),
        })
    }

    /// The words of a changing phrase with their motion this frame.
    ///
    /// While the turn is live with full motion the phrase always paints word
    /// by word, at rest between changes too: switching to plain text between
    /// ticks would swap two layouts that do not measure alike, and a ticking
    /// timer would visibly resize every second. Returns `None` (plain text)
    /// outside a live turn and under reduced motion.
    pub(super) fn phrase(&mut self, key: &str, text: &str) -> Option<Vec<PhraseWord>> {
        let animate = self.full_motion();
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

    /// Records the status line painted this frame and its measured height.
    pub(super) fn status_painted(&mut self, copy: &str, height: Pixels) {
        self.status_painted = true;
        self.status_exit = None;
        self.status = Some((copy.to_owned(), height));
    }

    /// The status line on its way out: its copy, height, and progress.
    ///
    /// Called after the turn's blocks painted, so a status line that left
    /// this frame starts its exit in the same frame, with no blank frame
    /// between them. A status line that returns later enters again.
    pub(super) fn status_exit(&mut self) -> Option<(String, Pixels, f32)> {
        if !self.status_painted
            && let Some((copy, height)) = self.status.take()
        {
            self.entrances.remove("status");
            if self.full_motion() {
                self.status_exit = Some(StatusExit {
                    copy,
                    height,
                    started: Instant::now(),
                });
            }
        }
        let exit = self.status_exit.as_ref()?;
        let elapsed = Instant::now().saturating_duration_since(exit.started);
        if elapsed >= STATUS_EXIT {
            self.status_exit = None;
            return None;
        }
        self.animating = true;
        Some((
            exit.copy.clone(),
            exit.height,
            eased(MotionCurve::EaseInOut, elapsed, STATUS_EXIT),
        ))
    }
}

/// The status line's thinking summary and its swap, if one plays.
#[derive(Debug)]
struct SummaryState {
    /// The summary painted, `None` while the verb stands in for it.
    shown: Option<String>,
    swap: Option<SummarySwap>,
}

/// One summary swap: when it began and the opacity it began from.
#[derive(Clone, Copy, Debug)]
struct SummarySwap {
    started: Instant,
    from: f32,
}

impl SummaryState {
    /// The swap's progress at `now`, `None` once it has settled.
    fn progress(&self, now: Instant) -> Option<f32> {
        let swap = self.swap?;
        let eased = progress_of(
            now.saturating_duration_since(swap.started),
            SUMMARY_SWAP,
            MotionCurve::EaseOut,
        );
        (eased < 1.0).then(|| swap.from + (1.0 - swap.from) * eased)
    }
}

/// Whether `next` restates `shown` rather than replacing it: one grows into
/// the other, ignoring the trailing ellipsis or stop a live line may carry.
fn summary_restates(shown: &str, next: &str) -> bool {
    let trim = |text: &str| {
        text.trim_end_matches(|c: char| c == '…' || c == '.' || c.is_whitespace())
            .to_owned()
    };
    let (shown, next) = (trim(shown), trim(next));
    !shown.is_empty() && !next.is_empty() && (next.starts_with(&shown) || shown.starts_with(&next))
}

/// How `word` moves given the word it replaced at the same position.
fn word_motion(previous: Option<&str>, word: &str) -> WordMotion {
    let Some(previous) = previous else {
        return WordMotion::Enter;
    };
    if previous == word {
        return WordMotion::Still;
    }
    if let (Some((_, from_suffix, from_value)), Some((_, to_suffix, to_value))) =
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

/// Splits a counting word into its digits, unit suffix, and value. The
/// article that stands for one ("a command", "an edit") counts as `1`.
fn number_word(word: &str) -> Option<(&str, &str, u64)> {
    if matches!(word, "a" | "an") {
        return Some((word, "", 1));
    }
    let digits_end = word
        .char_indices()
        .find(|(_, character)| !character.is_ascii_digit())
        .map_or(word.len(), |(index, _)| index);
    if digits_end == 0 {
        return None;
    }
    let (digits, suffix) = word.split_at(digits_end);
    Some((digits, suffix, digits.parse().ok()?))
}

fn eased(curve: MotionCurve, elapsed: Duration, duration: Duration) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the shared easing curve samples in f64 and feeds f32 opacity and offsets; the narrowing is the intended precision"
    )]
    let value = curve.sample(elapsed.as_secs_f64() / duration.as_secs_f64()) as f32;
    value
}

/// One row's entrance this frame.
pub(super) struct Entrance {
    /// How far the row's height has opened, `0.0..=1.0`.
    grow: f32,
    /// How far its content has faded in, `0.0..=1.0`.
    reveal: f32,
    /// The content's natural height, measured each frame.
    measured: Rc<Cell<Pixels>>,
}

/// Wraps a work row in its entrance: the row's height opens from nothing
/// (clipped, so the rows below glide down instead of jumping), and the
/// content fades in a beat later while it settles 4 px into place. `gap` is
/// the container's gap before the row, which closes with the height so no
/// empty band appears first. No blur: a filtered layer softened the text.
pub(super) fn with_entrance(
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

/// Tabular figures (`tnum`): every digit takes the same advance, so a
/// counting number never changes width as its digits change. Spline Sans
/// digits are proportional by default ("1" is a third narrower than "8").
pub(super) fn tabular_figures() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// Paints a changing phrase word by word on one line of `line_height`, in
/// tabular figures.
pub(super) fn phrase_element(words: &[PhraseWord], line_height: Pixels) -> AnyElement {
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
        // A number rests in the same per-character columns it rolls in:
        // one text run spaces its characters by the font's tracking, columns
        // do not, so swapping between the two moved it a few pixels on
        // every tick.
        WordMotion::Still if word.text.chars().any(|c| c.is_ascii_digit()) => {
            character_columns(&word.text)
        }
        WordMotion::Still => div()
            .flex_none()
            .child(word.text.clone())
            .into_any_element(),
        WordMotion::Enter => swap_in(word.text.clone(), word.elapsed),
        // The word stays one text run while its added letters fade in, the
        // layout it settles in: split into a kept run and an added run, the
        // runs measured apart and nudged every later word a pixel or two
        // until the swap ended (a new clause's comma, "commands" →
        // "commands,").
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

fn progress_of(elapsed: Duration, duration: Duration, curve: MotionCurve) -> f32 {
    if elapsed >= duration {
        1.0
    } else {
        eased(curve, elapsed, duration)
    }
}

/// A number rolling from `from` to `to` like an odometer: right-aligned
/// digit columns, each a clipped reel passing through every digit between
/// its old and new value, rightmost column first.
fn odometer(from: &str, to: &str, up: bool, elapsed: Duration, line_height: Pixels) -> AnyElement {
    let from_chars: Vec<char> = from.chars().collect();
    let to_chars: Vec<char> = to.chars().collect();
    let mut row = div().flex().flex_row().flex_none();
    for column in 0..to_chars.len() {
        let from_right = to_chars.len() - 1 - column;
        let target = to_chars[column];
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
    // Counting up the reel moves up through ascending cells; counting down
    // it moves down, so the strip is laid out in reverse.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_move_by_what_changed() {
        assert_eq!(word_motion(Some("ran"), "ran"), WordMotion::Still);
        assert_eq!(
            word_motion(Some("a"), "2"),
            WordMotion::Roll {
                from: "a".to_owned(),
                up: true
            }
        );
        assert_eq!(
            word_motion(Some("12s"), "11s"),
            WordMotion::Roll {
                from: "12s".to_owned(),
                up: false
            }
        );
        assert_eq!(
            word_motion(Some("command"), "commands"),
            WordMotion::Extend { kept: 7 }
        );
        // A unit change is a different word, not a count.
        assert_eq!(word_motion(Some("59s"), "1m"), WordMotion::Enter);
        assert_eq!(word_motion(None, "read"), WordMotion::Enter);
    }

    #[test]
    fn a_live_phrase_keeps_one_layout_and_moves_on_change() {
        let mut motion = WorkMotion::default();
        motion.begin_frame(true, MotionPolicy::Full, false);
        // The first frame of a row paints at rest as plain text.
        assert!(motion.phrase("chain", "Ran a command").is_none());
        motion.end_frame();
        motion.begin_frame(true, MotionPolicy::Full, false);
        // Once live, a phrase at rest still paints word by word.
        let still = motion
            .phrase("chain", "Ran a command")
            .expect("live phrase");
        assert!(still.iter().all(|word| word.motion == WordMotion::Still));
        assert!(!motion.end_frame(), "a phrase at rest requests no frames");
        motion.begin_frame(true, MotionPolicy::Full, false);
        let words = motion.phrase("chain", "Ran 2 commands").expect("changed");
        assert_eq!(words[0].motion, WordMotion::Still);
        assert!(matches!(words[1].motion, WordMotion::Roll { up: true, .. }));
        assert_eq!(words[2].motion, WordMotion::Extend { kept: 7 });
    }

    #[test]
    fn rows_on_screen_at_first_render_never_enter() {
        let mut motion = WorkMotion::default();
        motion.begin_frame(true, MotionPolicy::Full, false);
        assert!(motion.entrance("old-row").is_none());
        motion.end_frame();
        motion.begin_frame(true, MotionPolicy::Full, false);
        assert!(motion.entrance("old-row").is_none());
        assert!(motion.entrance("new-row").is_some());
        // Settled history paints at rest.
        motion.begin_frame(false, MotionPolicy::Full, false);
        assert!(motion.entrance("later-row").is_none());
    }

    #[test]
    fn a_restated_summary_never_restarts_its_swap() {
        let mut motion = primed();
        motion.begin_frame(true, MotionPolicy::Full, false);
        assert!(
            motion.summary_swap(Some("Checking sums")).is_none(),
            "first copy rides the entrance"
        );
        let first = motion
            .summary_swap(Some("Planning the layout"))
            .expect("a new summary swaps");
        assert!(first < 0.05);
        age(&mut motion, Duration::from_millis(150));
        let midway = motion
            .summary_swap(Some("Planning the layout"))
            .expect("still swapping");
        // The live title settling to its fuller buffered form keeps going.
        let settled = motion
            .summary_swap(Some("Planning the layout of the card…"))
            .expect("still swapping");
        assert!(settled >= midway, "{settled} restarted below {midway}");
    }

    #[test]
    fn a_new_summary_mid_swap_continues_from_the_reached_opacity() {
        let mut motion = primed();
        motion.begin_frame(true, MotionPolicy::Full, false);
        motion.summary_swap(Some("Reading the composer"));
        motion.summary_swap(Some("Planning the layout"));
        age(&mut motion, Duration::from_millis(150));
        let reached = motion
            .summary_swap(Some("Planning the layout"))
            .expect("swapping");
        let next = motion
            .summary_swap(Some("Checking the tests"))
            .expect("a different summary swaps");
        assert!(
            next >= reached - f32::EPSILON,
            "{next} dropped below {reached}"
        );
        age(&mut motion, SUMMARY_SWAP);
        assert!(
            motion.summary_swap(Some("Checking the tests")).is_none(),
            "settles"
        );
    }

    #[test]
    fn a_summary_returning_after_the_verb_swaps_in() {
        let mut motion = primed();
        motion.begin_frame(true, MotionPolicy::Full, false);
        motion.summary_swap(Some("Reading the composer"));
        assert!(motion.summary_swap(None).is_none(), "the verb never swaps");
        let back = motion
            .summary_swap(Some("Planning the layout"))
            .expect("swaps in instead of popping");
        assert!(back < 0.05);
    }

    const LONG: &str = "word word word word word word word word word word word word \
        word word word word word word word word word word word word word word word ";

    fn prose_id(name: &str) -> SceneId {
        SceneId::parse(name).expect("scene id")
    }

    /// Moves every clock the motion holds `by` into the past.
    fn age(motion: &mut WorkMotion, by: Duration) {
        let back = |at: Instant| at.checked_sub(by).unwrap_or(at);
        for started in motion.entrances.values_mut().flatten() {
            *started = back(*started);
        }
        for since in motion.queue.waiting.values_mut() {
            *since = back(*since);
        }
        for reveal in motion.reveals.values_mut() {
            reveal.age(by);
        }
        if let Some(swap) = motion
            .summary
            .as_mut()
            .and_then(|summary| summary.swap.as_mut())
        {
            swap.started = back(swap.started);
        }
    }

    /// Paints one prose body in paint order, reporting what it showed.
    fn paint_prose(motion: &mut WorkMotion, id: &SceneId, body: &str) -> Option<String> {
        let (revealed, _) = motion.reveal(id, body, false)?;
        motion.reveal_rendered(id, revealed.len());
        Some(revealed)
    }

    /// A primed live row with full motion, as after its first render.
    fn primed() -> WorkMotion {
        let mut motion = WorkMotion::default();
        motion.begin_frame(true, MotionPolicy::Full, false);
        motion.end_frame();
        motion
    }

    #[test]
    fn opening_a_folded_section_shows_its_work_at_rest() {
        let mut motion = primed();
        // The section paints folded while its work happens.
        motion.begin_frame(true, MotionPolicy::Full, false);
        assert!(!motion.begin_disclosure("work-a", false));
        motion.end_frame();

        // The reader opens it: everything inside already happened.
        motion.begin_frame(true, MotionPolicy::Full, false);
        let opened = motion.begin_disclosure("work-a", true);
        assert!(opened);
        assert!(!motion.hold("chain:tool-1", false), "nothing queues");
        assert!(
            motion.entrance("chain:tool-1").is_none(),
            "no entrance replays"
        );
        let prose = prose_id("prose-1");
        assert!(
            motion.reveal(&prose, "Done. ", false).is_none(),
            "finished prose paints whole"
        );
        motion.end_disclosure(opened);
        assert!(!motion.end_frame(), "nothing animates");

        // Work that arrives while it stays open still enters.
        motion.begin_frame(true, MotionPolicy::Full, false);
        assert!(!motion.begin_disclosure("work-a", true));
        assert!(motion.entrance("chain:tool-2").is_some());
        motion.end_frame();
    }

    #[test]
    fn a_block_waits_for_the_prose_above_it_then_enters() {
        let mut motion = primed();
        let prose = prose_id("prose-1");
        let body = "Let me check the tests first. ";
        motion.begin_frame(true, MotionPolicy::Full, false);
        assert_eq!(
            paint_prose(&mut motion, &prose, body).as_deref(),
            Some("Let ")
        );
        assert!(motion.hold("chain:tool-1", false), "the tool waits");
        assert!(
            !motion.entrances.contains_key("chain:tool-1"),
            "a waiting block starts no entrance clock"
        );
        assert!(motion.end_frame(), "a waiting block keeps frames coming");

        let mut frames = 0;
        let released = loop {
            frames += 1;
            assert!(frames < 400, "the tool is released");
            age(&mut motion, Duration::from_millis(16));
            motion.begin_frame(true, MotionPolicy::Full, false);
            let shown = paint_prose(&mut motion, &prose, body);
            if !motion.hold("chain:tool-1", false) {
                break shown;
            }
            assert!(motion.end_frame());
        };
        // Released only once every word shows and the last has faded
        // halfway in (or the reveal has ended).
        assert!(released.is_none_or(|shown| shown == body));
        let entrance = motion.entrance("chain:tool-1").expect("it enters now");
        assert!(entrance.grow < 0.05, "the entrance starts at release");
        assert!(motion.end_frame());
    }

    #[test]
    fn prose_below_waits_paused_and_streams_from_its_first_word() {
        let mut motion = primed();
        let (above, below) = (prose_id("prose-above"), prose_id("prose-below"));
        motion.begin_frame(true, MotionPolicy::Full, false);
        paint_prose(&mut motion, &above, LONG);
        assert!(motion.hold_prose(&below), "unshown prose waits");
        assert_eq!(motion.shown_words(&below, "Reply text here. ", false), 0);
        assert!(motion.end_frame());
        // However long it waits, nothing banks: release shows one word.
        age(&mut motion, Duration::from_secs(3));
        motion.begin_frame(true, MotionPolicy::Full, false);
        paint_prose(&mut motion, &above, LONG);
        assert!(motion.hold_prose(&below), "it still waits");
        motion.end_frame();
        // The prose above finishes.
        motion.reveals.remove(&above);
        motion.seen_prose.insert(above.clone());
        motion.begin_frame(true, MotionPolicy::Full, false);
        assert!(
            motion.reveal(&above, LONG, false).is_none(),
            "above is done"
        );
        assert!(!motion.hold_prose(&below));
        let shown = paint_prose(&mut motion, &below, "Reply text here. ");
        assert_eq!(shown.as_deref(), Some("Reply "));
    }

    #[test]
    fn a_waiting_block_hurries_the_prose_above() {
        let mut motion = primed();
        let prose = prose_id("prose-1");
        motion.begin_frame(true, MotionPolicy::Full, false);
        paint_prose(&mut motion, &prose, LONG);
        assert!(!motion.reveals[&prose].hurry);
        assert!(motion.hold("chain:tool-1", false));
        motion.end_frame();
        motion.begin_frame(true, MotionPolicy::Full, false);
        paint_prose(&mut motion, &prose, LONG);
        assert!(motion.reveals[&prose].hurry, "a waiting block hurries it");
    }

    #[test]
    fn an_actionable_block_waits_at_most_the_cap() {
        let mut motion = primed();
        let prose = prose_id("prose-1");
        motion.begin_frame(true, MotionPolicy::Full, false);
        paint_prose(&mut motion, &prose, LONG);
        assert!(motion.hold("block:approval", true));
        motion.end_frame();
        age(&mut motion, ACTIONABLE_HOLD_CAP / 2);
        motion.begin_frame(true, MotionPolicy::Full, false);
        paint_prose(&mut motion, &prose, LONG);
        assert!(
            motion.hold("block:approval", true),
            "within the cap it waits"
        );
        motion.end_frame();
        age(&mut motion, ACTIONABLE_HOLD_CAP);
        motion.begin_frame(true, MotionPolicy::Full, false);
        assert!(
            paint_prose(&mut motion, &prose, LONG).is_some_and(|shown| shown != LONG),
            "the prose still settles"
        );
        assert!(
            !motion.hold("block:approval", true),
            "past the cap it enters"
        );
        assert!(motion.entrance("block:approval").is_some());
        // What follows the approval still waits its turn.
        assert!(motion.hold("block:plan", false));
    }

    #[test]
    fn entering_rows_cascade_at_half_fade() {
        let mut motion = primed();
        motion.begin_frame(true, MotionPolicy::Full, false);
        assert!(!motion.hold("first", false));
        assert!(motion.entrance("first").is_some());
        assert!(
            motion.hold("second", false),
            "it waits for the first's fade"
        );
        motion.end_frame();
        age(&mut motion, entrance_handoff());
        motion.begin_frame(true, MotionPolicy::Full, false);
        assert!(!motion.hold("first", false), "a painted row never waits");
        assert!(motion.entrance("first").is_some(), "still entering");
        assert!(!motion.hold("second", false), "released at the midpoint");
    }

    #[test]
    fn nothing_waits_outside_full_live_motion() {
        let prose = prose_id("prose-1");
        // A first render paints everything at rest.
        let mut motion = WorkMotion::default();
        motion.begin_frame(true, MotionPolicy::Full, false);
        paint_prose(&mut motion, &prose, LONG);
        assert!(!motion.hold("chain:tool-1", false));
        assert!(motion.entrance("chain:tool-1").is_none());
        motion.end_frame();
        // Reduced motion shows everything at once.
        let mut motion = WorkMotion::default();
        motion.begin_frame(true, MotionPolicy::Reduced, false);
        motion.end_frame();
        motion.begin_frame(true, MotionPolicy::Reduced, false);
        paint_prose(&mut motion, &prose, LONG);
        assert!(!motion.hold("chain:tool-1", false));
        assert!(!motion.hold_prose(&prose_id("prose-2")));
        // Settled history never waits.
        let mut motion = primed();
        motion.begin_frame(false, MotionPolicy::Full, false);
        paint_prose(&mut motion, &prose, LONG);
        assert!(!motion.hold("chain:tool-1", false));
        // A caught-up frame shows what waited, at rest.
        let mut motion = primed();
        motion.begin_frame(true, MotionPolicy::Full, false);
        paint_prose(&mut motion, &prose, LONG);
        assert!(motion.hold("chain:tool-1", false));
        motion.end_frame();
        motion.begin_frame(true, MotionPolicy::Full, true);
        assert!(
            paint_prose(&mut motion, &prose, LONG).is_none(),
            "prose at rest"
        );
        assert!(!motion.hold("chain:tool-1", false));
        assert!(motion.entrance("chain:tool-1").is_none(), "no entrance");
        assert!(!motion.end_frame());
    }

    #[test]
    fn the_status_line_neither_waits_nor_holds_the_queue() {
        let mut motion = primed();
        motion.begin_frame(true, MotionPolicy::Full, false);
        assert!(motion.status_entrance().is_some());
        assert!(!motion.hold("block:after", false));
    }
}
