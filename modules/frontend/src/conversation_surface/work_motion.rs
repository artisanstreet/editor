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

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{FontFeatures, Pixels};

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
/// Status line exit: the texts-reveal quiet fade out.
const STATUS_EXIT: Duration = Duration::from_millis(200);

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
    /// Whether anything still moves; the row then asks for the next frame.
    animating: bool,
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
    pub(super) fn begin_frame(&mut self, live: bool, motion: MotionPolicy) {
        self.pace = match (live, motion) {
            (false, _) => Pace::Settled,
            (true, MotionPolicy::Reduced) => Pace::Reduced,
            (true, MotionPolicy::Full) => Pace::Full,
        };
        self.animating = false;
        self.touched.clear();
        self.status_painted = false;
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
        self.animating
    }

    fn full_motion(&self) -> bool {
        self.primed && self.pace == Pace::Full
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
            if self.primed {
                self.reveals.insert(id.clone(), ReplyReveal::begin(now));
            } else if streaming {
                self.reveals
                    .insert(id.clone(), ReplyReveal::resume(body, now));
            }
        }
        let reveal = self.reveals.get_mut(id)?;
        self.touched.insert(id.clone());
        if reveal.advance(body, streaming, motion, now) {
            self.animating = true;
        } else if !streaming && reveal.is_complete(body) {
            self.reveals.remove(id);
            return None;
        }
        Some((reveal.revealed().to_owned(), reveal.fade.clone()))
    }

    /// Whether the reveal of `id`, if any, shows at least one segment.
    pub(super) fn reveal_shows(&self, id: &SceneId) -> Option<bool> {
        self.reveals
            .get(id)
            .map(|reveal| !reveal.revealed().is_empty())
    }

    /// Records the visible length the renderer reported for `id`.
    pub(super) fn reveal_rendered(&mut self, id: &SceneId, visible_len: usize) {
        if let Some(reveal) = self.reveals.get_mut(id) {
            reveal.rendered(visible_len);
        }
    }

    /// The entrance of one work row this frame, `None` at rest.
    pub(super) fn entrance(&mut self, key: &str) -> Option<Entrance> {
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
        WordMotion::Still => div()
            .flex_none()
            .child(word.text.clone())
            .into_any_element(),
        WordMotion::Enter => swap_in(word.text.clone(), word.elapsed),
        WordMotion::Extend { kept } => {
            let (kept, added) = word.text.split_at((*kept).min(word.text.len()));
            let progress = progress_of(word.elapsed, SWAP, MotionCurve::EaseInOut);
            div()
                .flex()
                .flex_none()
                .child(kept.to_owned())
                .child(div().opacity(progress).child(added.to_owned()))
                .into_any_element()
        }
        WordMotion::Roll { from, up } => odometer(from, &word.text, *up, word.elapsed, line_height),
    }
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
        motion.begin_frame(true, MotionPolicy::Full);
        // The first frame of a row paints at rest as plain text.
        assert!(motion.phrase("chain", "Ran a command").is_none());
        motion.end_frame();
        motion.begin_frame(true, MotionPolicy::Full);
        // Once live, a phrase at rest still paints word by word.
        let still = motion
            .phrase("chain", "Ran a command")
            .expect("live phrase");
        assert!(still.iter().all(|word| word.motion == WordMotion::Still));
        assert!(!motion.end_frame(), "a phrase at rest requests no frames");
        motion.begin_frame(true, MotionPolicy::Full);
        let words = motion.phrase("chain", "Ran 2 commands").expect("changed");
        assert_eq!(words[0].motion, WordMotion::Still);
        assert!(matches!(words[1].motion, WordMotion::Roll { up: true, .. }));
        assert_eq!(words[2].motion, WordMotion::Extend { kept: 7 });
    }

    #[test]
    fn rows_on_screen_at_first_render_never_enter() {
        let mut motion = WorkMotion::default();
        motion.begin_frame(true, MotionPolicy::Full);
        assert!(motion.entrance("old-row").is_none());
        motion.end_frame();
        motion.begin_frame(true, MotionPolicy::Full);
        assert!(motion.entrance("old-row").is_none());
        assert!(motion.entrance("new-row").is_some());
        // Settled history paints at rest.
        motion.begin_frame(false, MotionPolicy::Full);
        assert!(motion.entrance("later-row").is_none());
    }
}
