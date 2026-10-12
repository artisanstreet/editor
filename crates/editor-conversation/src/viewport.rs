//! The Viewport: where the reader is in the open thread, and which pages of
//! it to load. Following pins the view to the live tail; Reading holds an
//! anchor while history loads above it; Jumping waits for a target to load.
//!
//! The machine keeps the window's bookkeeping (oldest loaded position,
//! whether older items exist, a gap below the window) and pushes effects;
//! the conversation performs them.

use artisan_api::Seq;
use statig::blocking::{self, Outcome};

/// What to fetch with `GET /threads/{id}/items`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fetch {
    /// The newest page.
    Tail,
    /// The page before `before` (a position).
    Older { before: Seq },
    /// The page after `after`, to close a gap below the window.
    Newer { after: Seq },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Jump {
    Latest,
    /// An item's position.
    To(Seq),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    Fetch(Fetch),
    ScrollToEnd,
    /// Scroll so the item at this position is at the top.
    ScrollTo(Seq),
}

#[derive(Clone, Debug)]
pub(crate) enum Input {
    /// The thread was opened, or Forge reset the stream: load the tail.
    Open,
    /// The reader reached the top edge of the loaded window.
    TopReached,
    /// The reader is at the bottom edge.
    BottomReached,
    /// The reader scrolled away from the tail; `anchor` is the position at
    /// the top of the view.
    ScrolledAway {
        anchor: Seq,
    },
    Jump(Jump),
    /// A page arrived. `first`/`last` are its positions (none when empty).
    Loaded {
        fetch: Fetch,
        first: Option<Seq>,
        last: Option<Seq>,
        more: bool,
    },
    /// A page failed; the reader can retry by scrolling again.
    LoadFailed,
    /// Items were dropped from the top of the window.
    EvictedOlder {
        oldest: Seq,
    },
    /// Items were dropped from the bottom; the window ends at `last`.
    EvictedNewer {
        last: Seq,
    },
    /// Whether an item at this position is loaded (asked while jumping).
    Present {
        position: Seq,
        loaded: bool,
    },
}

/// Lifecycle data only: the edges of the loaded window.
#[derive(Debug, Default)]
pub(crate) struct Viewport {
    /// Position of the oldest loaded item.
    pub oldest: Option<Seq>,
    /// No items exist before `oldest`.
    pub exhausted: bool,
    /// A page request is out.
    pub in_flight: bool,
    /// The window stops at this position; newer items exist but are not
    /// shown until the gap is fetched.
    pub gap: Option<Seq>,
}

impl Viewport {
    fn note_loaded(&mut self, fetch: Fetch, first: Option<Seq>, last: Option<Seq>, more: bool) {
        self.in_flight = false;
        match fetch {
            Fetch::Tail => {
                self.oldest = first;
                self.exhausted = !more;
                self.gap = None;
            }
            Fetch::Older { .. } => {
                if first.is_some() {
                    self.oldest = first;
                }
                self.exhausted = !more;
            }
            Fetch::Newer { .. } => {
                self.gap = if more { last.or(self.gap) } else { None };
            }
        }
    }

    fn load_older(&mut self, effects: &mut Vec<Effect>) {
        if self.in_flight || self.exhausted {
            return;
        }
        let Some(before) = self.oldest else { return };
        self.in_flight = true;
        effects.push(Effect::Fetch(Fetch::Older { before }));
    }

    fn fetch(&mut self, fetch: Fetch, effects: &mut Vec<Effect>) {
        self.in_flight = true;
        effects.push(Effect::Fetch(fetch));
    }
}

#[blocking::state_machine(
    initial = "State::following()",
    state(derive(Debug, Clone, PartialEq, Eq))
)]
impl Viewport {
    /// Pinned to the live tail.
    #[state(superstate = "window")]
    fn following(context: &mut Vec<Effect>, event: &Input) -> Outcome<State> {
        match event {
            Input::ScrolledAway { anchor } => Outcome::Transition(State::reading(*anchor)),
            Input::Loaded {
                fetch: Fetch::Tail, ..
            } => {
                context.push(Effect::ScrollToEnd);
                Outcome::Super
            }
            _ => Outcome::Super,
        }
    }

    /// Reading history; `anchor` stays put while pages load around it.
    #[state(superstate = "window")]
    fn reading(
        &mut self,
        anchor: &mut Seq,
        context: &mut Vec<Effect>,
        event: &Input,
    ) -> Outcome<State> {
        match event {
            Input::ScrolledAway { anchor: next } => {
                *anchor = *next;
                Outcome::Handled
            }
            Input::BottomReached => match self.gap {
                Some(after) if !self.in_flight => {
                    self.fetch(Fetch::Newer { after }, context);
                    Outcome::Handled
                }
                Some(_) => Outcome::Handled,
                None => Outcome::Transition(State::following()),
            },
            _ => Outcome::Super,
        }
    }

    /// Waiting for `target` to load (older pages, or the tail again).
    #[state(superstate = "window")]
    fn jumping(
        &mut self,
        target: &mut Jump,
        loading: &mut bool,
        context: &mut Vec<Effect>,
        event: &Input,
    ) -> Outcome<State> {
        match event {
            Input::Loaded {
                fetch,
                first,
                last,
                more,
            } => {
                self.note_loaded(*fetch, *first, *last, *more);
                *loading = false;
                match *target {
                    Jump::Latest => {
                        context.push(Effect::ScrollToEnd);
                        Outcome::Transition(State::following())
                    }
                    // The owner answers with `Present` once the page is in.
                    Jump::To(_) => Outcome::Handled,
                }
            }
            Input::Present { position, loaded } => {
                if *loaded {
                    context.push(Effect::ScrollTo(*position));
                    return Outcome::Transition(State::reading(*position));
                }
                if self.exhausted || self.oldest.is_none_or(|oldest| oldest <= *position) {
                    // Not in the thread (or already above it): stay where we are.
                    return Outcome::Transition(State::reading(self.oldest.unwrap_or(*position)));
                }
                *loading = true;
                self.load_older(context);
                Outcome::Handled
            }
            Input::LoadFailed => {
                self.in_flight = false;
                Outcome::Transition(State::reading(self.oldest.unwrap_or_default()))
            }
            Input::ScrolledAway { anchor } if !*loading => {
                Outcome::Transition(State::reading(*anchor))
            }
            _ => Outcome::Super,
        }
    }

    /// History loading and jumps, the same in every state.
    #[superstate]
    fn window(&mut self, context: &mut Vec<Effect>, event: &Input) -> Outcome<State> {
        match event {
            Input::Open => {
                *self = Self::default();
                self.fetch(Fetch::Tail, context);
                Outcome::Transition(State::following())
            }
            Input::TopReached => {
                self.load_older(context);
                Outcome::Handled
            }
            Input::Loaded {
                fetch,
                first,
                last,
                more,
            } => {
                self.note_loaded(*fetch, *first, *last, *more);
                Outcome::Handled
            }
            Input::LoadFailed => {
                self.in_flight = false;
                Outcome::Handled
            }
            Input::EvictedOlder { oldest } => {
                self.oldest = Some(*oldest);
                self.exhausted = false;
                Outcome::Handled
            }
            Input::EvictedNewer { last } => {
                self.gap = Some(*last);
                Outcome::Handled
            }
            Input::Jump(Jump::Latest) => {
                if self.gap.is_some() {
                    self.fetch(Fetch::Tail, context);
                    Outcome::Transition(State::jumping(Jump::Latest, true))
                } else {
                    context.push(Effect::ScrollToEnd);
                    Outcome::Transition(State::following())
                }
            }
            Input::Jump(Jump::To(position)) => {
                Outcome::Transition(State::jumping(Jump::To(*position), false))
            }
            Input::BottomReached => Outcome::Transition(State::following()),
            _ => Outcome::Handled,
        }
    }
}

impl State {
    #[must_use]
    pub(crate) fn is_following(&self) -> bool {
        matches!(self, Self::Following { .. })
    }
}
