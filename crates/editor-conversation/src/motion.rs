//! Motion of a row's work (`conversation_surface/work_motion.rs`,
//! `disclosure.rs` and `compaction_separator.rs`): disclosures that open
//! and close their measured height, phrases that count like an odometer,
//! the live header's entrance, entering rows and the compaction separator.
//!
//! Every animation belongs to one row view and runs only while what it
//! shows is new: a row's first render and settled history paint at rest,
//! so scrolling back or opening a thread never replays anything. Reduced
//! motion paints every final state at once. Durations and curves are the
//! transitions-dev tokens the old Editor used:
//!
//! - A disclosure opens and closes on the 250 ms smooth-out accordion
//!   clock; the panel clips to the animated share of its measured height
//!   while its content fades with it, and an interrupted flight reverses
//!   from the fraction on screen.
//! - A work row (a chain, prose, a card) enters by opening its height
//!   (250 ms smooth-out) while its content fades in 80 ms later (250 ms
//!   ease-out) and settles 4 px into place.
//! - A phrase that keeps changing (a chain's "Ran 2 commands, read a file",
//!   the "Working for 1m 4s" header) animates per word: a changed number
//!   rolls like an odometer (500 ms, columns 40 ms apart), a word that grows
//!   fades its new letters in, and a new word rises in with the text-swap
//!   recipe (150 ms ease-in-out, 4 px, 2 px blur).
//! - A header mounted while its run works enters after a 150 ms hold with
//!   the text-swap recipe.
//! - The compaction separator opens, draws its rules outward and lifts its
//!   label in; settling, the word swaps and a highlight runs along the
//!   rules.
//!
//! A row reveals strictly top to bottom through one reveal queue (master's
//! `work_motion`): blocks render in paint order, and while something above
//! still settles (prose with words left to reveal, or an entrance whose fade
//! hasn't reached its midpoint) every block below waits. A waiting block
//! paints nothing and starts no entrance clock; prose already showing words
//! pauses where it is. So text never keeps streaming beside a tool call that
//! arrived after it. Nothing waits outside full live motion.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use editor_ui::motion::{MotionCurve, MotionDuration, MotionPlan, MotionPolicy, MotionRecipe};
use gpui::{
    AnyElement, Div, Filter, FontFeatures, IntoElement, ParentElement as _, Pixels, SharedString,
    Styled as _, StyledText, div, px,
};

/// Row entrance: `--duration-very-slow` (texts reveal).
const ENTRANCE: Duration = Duration::from_millis(400);
/// The row's height opening (`--duration-fast`).
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
/// In-place word swap rise: `--distance-micro`.
const SWAP_RISE_PX: f32 = 4.0;
/// In-place word swap blur: `--blur-small`.
const SWAP_BLUR_PX: f32 = 2.0;
/// The header entrance: a quick hold, then the text swap.
const HEADER_HOLD: Duration = MotionDuration::Quick.as_duration();

// The compaction separator.
const COMPACTION_OPEN: Duration = Duration::from_millis(250);
const COMPACTION_DRAW_DELAY: Duration = Duration::from_millis(150);
const COMPACTION_DRAW: Duration = Duration::from_millis(400);
const COMPACTION_LABEL_DELAY: Duration = Duration::from_millis(230);
const COMPACTION_LABEL_RISE: Duration = Duration::from_millis(250);
const COMPACTION_SWAP_HALF: Duration = Duration::from_millis(150);
const COMPACTION_PULSE: Duration = Duration::from_millis(500);

/// One row's motion, retained across its renders.
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent per-frame flags; packing them would conflate them"
)]
pub(crate) struct WorkMotion {
    /// Set after the row's first render: what it showed then never moves.
    primed: bool,
    /// Whether the row's run is live this frame.
    live: bool,
    /// The reader's motion preference this frame.
    policy: Option<MotionPolicy>,
    /// Whether anything still moves; the row then renders the next frame.
    animating: bool,
    phrases: HashMap<String, PhraseState>,
    /// When each row first painted while new, `None` at rest.
    entrances: HashMap<String, Option<Instant>>,
    /// Natural heights of rows mid-entrance, measured each frame.
    entrance_heights: HashMap<String, Rc<Cell<Pixels>>>,
    disclosures: HashMap<String, DisclosurePanel>,
    compactions: HashMap<String, CompactionMotion>,
    /// Whether the header first mounted while its run worked, and when.
    header: Option<(bool, Instant)>,
    /// Disclosures last painted folded, by selector.
    folded: HashSet<String>,
    /// Disclosures the reader just opened are rendering their rows: those
    /// rows already happened, so they paint at rest.
    opening: u32,
    /// Something painted above is still settling this frame: what renders
    /// next waits (the reveal queue).
    settling: bool,
}

impl WorkMotion {
    /// Starts one render.
    pub(crate) fn begin_frame(&mut self, live: bool, policy: MotionPolicy) {
        self.live = live;
        self.policy = Some(policy);
        self.animating = false;
        self.settling = false;
    }

    /// Whether the reveal queue holds back what renders next this frame.
    pub(crate) fn gate_closed(&self) -> bool {
        self.settling && self.full_motion()
    }

    /// Something just rendered is still settling: what follows waits.
    pub(crate) fn note_settling(&mut self) {
        self.settling = true;
    }

    /// Whether the block keyed `key` waits behind the reveal queue this
    /// frame. A waiting block must paint nothing and must not call
    /// [`Self::entrance`], whose clock starts on its first call; frames keep
    /// coming until it is released. A block already on screen never waits
    /// again, so nothing painted disappears.
    pub(crate) fn hold(&mut self, key: &str) -> bool {
        if !self.gate_closed() || self.entrances.contains_key(key) {
            return false;
        }
        self.animating = true;
        true
    }

    /// Ends one render; returns whether the row must render the next frame.
    pub(crate) fn end_frame(&mut self) -> bool {
        self.primed = true;
        self.animating
    }

    fn policy(&self) -> MotionPolicy {
        self.policy.unwrap_or(MotionPolicy::Full)
    }

    /// New content of a live run under full motion moves.
    fn full_motion(&self) -> bool {
        self.primed && self.opening == 0 && self.live && self.policy() == MotionPolicy::Full
    }

    /// Notes one disclosure before its rows render; returns whether the
    /// reader just opened it. Its rows then paint at rest until
    /// [`Self::end_disclosure`].
    pub(crate) fn begin_disclosure(&mut self, selector: &str, mounted: bool) -> bool {
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

    pub(crate) fn end_disclosure(&mut self, opened: bool) {
        if opened {
            self.opening = self.opening.saturating_sub(1);
        }
    }

    /// Samples one disclosure's open fraction once for its chevron, height
    /// and opacity. A mount rests at its state; only a change of `open`
    /// arms a flight, and reduced motion records it without one.
    pub(crate) fn disclosure(&mut self, selector: &str, open: bool) -> DisclosureFrame {
        let policy = self.policy();
        let now = Instant::now();
        let panel = self
            .disclosures
            .entry(selector.to_owned())
            .or_insert_with(|| DisclosurePanel::new(open));
        if panel.painted_open != open {
            let from = panel.progress.clamp(0.0, 1.0);
            panel.flight = match policy {
                MotionPolicy::Full => Some(Flight {
                    open,
                    from,
                    started: now,
                    collapse_height: panel.content.get(),
                }),
                MotionPolicy::Reduced => None,
            };
            panel.painted_open = open;
        }
        let mut fraction = if open { 1.0 } else { 0.0 };
        match (panel.flight, policy.resolve(MotionRecipe::AccordionExpand)) {
            (Some(flight), MotionPlan::Animate(animation)) => {
                let elapsed = now.saturating_duration_since(flight.started);
                if elapsed >= animation.duration() {
                    panel.flight = None;
                } else {
                    let eased = sample(animation.curve(), elapsed, animation.duration());
                    fraction = flight.from + (flight.target() - flight.from) * eased;
                }
            }
            (_, MotionPlan::Immediate) => panel.flight = None,
            (None, _) => {}
        }
        panel.progress = fraction;
        self.animating |= panel.flight.is_some();
        DisclosureFrame {
            fraction,
            flight: panel.flight,
            content: Rc::clone(&panel.content),
            open,
        }
    }

    /// The entrance of one row this frame, `None` at rest. A row enters when
    /// it first paints after the row view's first render, or when its item
    /// arrived while the thread was open (`fresh`): a row view made for new
    /// work enters too.
    pub(crate) fn entrance(&mut self, key: &str, fresh: bool) -> Option<Entrance> {
        let animate = (self.primed || fresh)
            && self.opening == 0
            && self.live
            && self.policy() == MotionPolicy::Full;
        let started = *self
            .entrances
            .entry(key.to_owned())
            .or_insert_with(|| animate.then(Instant::now));
        let elapsed = started
            .map(|started| Instant::now().saturating_duration_since(started))
            .filter(|elapsed| *elapsed < ENTRANCE && self.live);
        let Some(elapsed) = elapsed else {
            self.entrance_heights.remove(key);
            return None;
        };
        self.animating = true;
        let reveal = progress_of(
            elapsed.saturating_sub(ENTRANCE_FADE_DELAY),
            ENTRANCE_FADE,
            MotionCurve::EaseOut,
        );
        // An entering block holds the queue until it is halfway in, so the
        // blocks after it cascade rather than arrive at once.
        if reveal < 0.5 {
            self.settling = true;
        }
        Some(Entrance {
            grow: progress_of(elapsed, ENTRANCE_GROW, MotionCurve::SmoothOut),
            reveal,
            measured: Rc::clone(
                self.entrance_heights
                    .entry(key.to_owned())
                    .or_insert_with(|| Rc::new(Cell::new(px(0.0)))),
            ),
        })
    }

    /// The words of a changing phrase with their motion this frame, or
    /// `None` (plain text) outside a live run and under reduced motion.
    ///
    /// While the run is live with full motion the phrase paints word by
    /// word at rest too: switching to plain text between ticks would swap
    /// two layouts that do not measure alike.
    pub(crate) fn phrase(&mut self, key: &str, text: &str) -> Option<Vec<PhraseWord>> {
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

    /// Advances one compaction separator to this frame's phase. It enters
    /// only when it appears new and running, under full motion; it settles
    /// with motion only when watched while it ran.
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

/// One disclosure transition: direction plus start fraction, on the
/// 250 ms smooth-out accordion clock.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Flight {
    open: bool,
    /// The fraction on screen when it began, so an interrupted flight
    /// reverses from there.
    from: f32,
    started: Instant,
    /// A closing panel keeps its size whatever its rows do meanwhile.
    collapse_height: f32,
}

impl Flight {
    const fn target(self) -> f32 {
        if self.open { 1.0 } else { 0.0 }
    }
}

/// One disclosure's retained panel state.
struct DisclosurePanel {
    /// Intrinsic height of the mounted content, measured each paint.
    content: Rc<Cell<f32>>,
    painted_open: bool,
    flight: Option<Flight>,
    progress: f32,
}

impl DisclosurePanel {
    fn new(open: bool) -> Self {
        Self {
            content: Rc::new(Cell::new(0.0)),
            painted_open: open,
            flight: None,
            progress: if open { 1.0 } else { 0.0 },
        }
    }
}

/// One sample shared by a disclosure's chevron, height and opacity.
pub(crate) struct DisclosureFrame {
    pub fraction: f32,
    flight: Option<Flight>,
    content: Rc<Cell<f32>>,
    open: bool,
}

impl DisclosureFrame {
    /// Whether the content is painted: open, or closing still.
    pub(crate) fn content_mounted(&self) -> bool {
        self.open || self.flight.is_some()
    }

    /// Attaches the accordion panel to `root`: the content clipped to the
    /// flight's share of its measured height and faded with it, or at rest.
    pub(crate) fn panel(self, root: Div, content: Option<AnyElement>) -> Div {
        let measured = Rc::clone(&self.content);
        let panel = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(bounds) = bounds.first() {
                    measured.set(f32::from(bounds.size.height));
                }
            });
        let content = match content.filter(|_| self.content_mounted()) {
            Some(content) => div().w_full().min_w_0().flex_shrink_0().child(content),
            None => return root.child(panel.h(px(0.0)).overflow_hidden()),
        };
        match self.flight {
            Some(flight) => {
                let full = if self.open {
                    self.content.get()
                } else {
                    flight.collapse_height
                };
                root.child(
                    panel
                        .child(content.opacity(self.fraction))
                        .overflow_hidden()
                        .h(px(full * self.fraction)),
                )
            }
            None => root.child(panel.child(content)),
        }
    }
}

/// One row's entrance this frame.
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
