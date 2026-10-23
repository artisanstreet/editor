//! The Viewport: where the reader is in the open thread, and which pages of
//! it to load. Following pins the view to the live tail; Reading holds an
//! anchor while history loads above it; Jumping waits for a target to load.
//!
//! The machine keeps the window's bookkeeping (oldest loaded position,
//! whether older items exist, a gap below the window) and pushes effects;
//! the conversation performs them.

use artisan_api::Seq;

/// What to fetch with `GET /threads/{id}/items`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// The newest page.
    Tail,
    /// The page before `before` (a position).
    Older { before: Seq },
    /// The page after `after`, to close a gap below the window.
    Newer { after: Seq },
    /// The page starting at `position`: a jump far from the window loads
    /// there at once and replaces it.
    At { position: Seq },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    Latest,
    /// An item's position.
    To(Seq),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Fetch(Fetch),
    ScrollToEnd,
    /// Scroll so the item at this position is at the top.
    ScrollTo(Seq),
}

#[derive(Clone, Debug)]
pub enum Input {
    /// The thread was opened, or Forge reset the stream: load the tail.
    Open,
    /// The reader reached the top edge of the loaded window.
    TopReached,
    /// The reader is at the bottom edge.
    BottomReached,
    /// The reader scrolled away from the tail; `anchor` is the position at
    /// the top of the view, `through` the newest position the view shows.
    ScrolledAway {
        anchor: Seq,
        through: Seq,
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
pub struct Viewport {
    state: State,
    /// Position of the oldest loaded item.
    pub oldest: Option<Seq>,
    /// No items exist before `oldest`.
    pub exhausted: bool,
    /// A page request is out.
    pub in_flight: bool,
    /// The page asked for last; a page arriving for any other request is
    /// stale and dropped.
    pub requested: Option<Fetch>,
    /// The window stops at this position; newer items exist but are not
    /// shown until the gap is fetched.
    pub gap: Option<Seq>,
}

impl Viewport {
    /// A page has arrived since the thread opened.
    pub(crate) const fn loaded(&self) -> bool {
        !self.in_flight && (self.oldest.is_some() || self.exhausted)
    }

    fn note_loaded(&mut self, fetch: Fetch, first: Option<Seq>, last: Option<Seq>, more: bool) {
        self.in_flight = false;
        self.requested = None;
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
            Fetch::At { .. } => {
                self.oldest = first;
                self.exhausted = false;
                self.gap = if more { last } else { None };
            }
        }
    }

    fn load_older(&mut self, effects: &mut Vec<Effect>) {
        if self.in_flight || self.exhausted {
            return;
        }
        let Some(before) = self.oldest else { return };
        self.fetch(Fetch::Older { before }, effects);
    }

    fn fetch(&mut self, fetch: Fetch, effects: &mut Vec<Effect>) {
        self.in_flight = true;
        self.requested = Some(fetch);
        effects.push(Effect::Fetch(fetch));
    }
}

/// The reader's current mode; all page edges live on `Viewport`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Following {},
    /// `anchor..=through` is on screen: the window never trims it.
    Reading {
        anchor: Seq,
        through: Seq,
    },
    Jumping {
        target: Jump,
        loading: bool,
    },
}

impl Default for State {
    fn default() -> Self {
        Self::Following {}
    }
}

impl Viewport {
    pub(crate) const fn state(&self) -> &State {
        &self.state
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one state machine: every (state, input) transition sits in one match"
    )]
    pub(crate) fn handle_with_context(&mut self, event: &Input, effects: &mut Vec<Effect>) {
        // Mode-specific events finish here. The rest share page bookkeeping.
        match (&mut self.state, event) {
            (
                State::Following {} | State::Reading { .. } | State::Jumping { loading: false, .. },
                Input::ScrolledAway { anchor, through },
            ) => {
                self.state = State::Reading {
                    anchor: *anchor,
                    through: *through,
                };
                return;
            }
            (
                State::Following {},
                Input::Loaded {
                    fetch: Fetch::Tail, ..
                },
            ) => {
                effects.push(Effect::ScrollToEnd);
            }
            (State::Reading { .. }, Input::BottomReached) => {
                match self.gap {
                    Some(after) if !self.in_flight => self.fetch(Fetch::Newer { after }, effects),
                    Some(_) => {}
                    None => self.state = State::Following {},
                }
                return;
            }
            (
                State::Jumping { target, loading },
                Input::Loaded {
                    fetch,
                    first,
                    last,
                    more,
                },
            ) => {
                let latest = *target == Jump::Latest;
                *loading = false;
                self.note_loaded(*fetch, *first, *last, *more);
                if latest {
                    effects.push(Effect::ScrollToEnd);
                    self.state = State::Following {};
                }
                return;
            }
            (State::Jumping { loading, .. }, Input::Present { position, loaded }) => {
                if *loaded {
                    effects.push(Effect::ScrollTo(*position));
                    self.state = State::Reading {
                        anchor: *position,
                        through: *position,
                    };
                } else if self
                    .oldest
                    .is_some_and(|oldest| oldest <= *position || self.exhausted)
                {
                    // Inside the loaded stretch, or older than the thread's
                    // first item, and still not there: nothing to go to.
                    let anchor = self.oldest.unwrap_or(*position);
                    self.state = State::Reading {
                        anchor,
                        through: anchor,
                    };
                } else if !*loading {
                    // Far above the window: load where it is, in one page.
                    *loading = true;
                    self.fetch(
                        Fetch::At {
                            position: *position,
                        },
                        effects,
                    );
                }
                return;
            }
            (State::Jumping { .. }, Input::LoadFailed) => {
                self.in_flight = false;
                self.requested = None;
                let anchor = self.oldest.unwrap_or_default();
                self.state = State::Reading {
                    anchor,
                    through: anchor,
                };
                return;
            }
            _ => {}
        }

        match event {
            Input::Open => {
                *self = Self::default();
                self.fetch(Fetch::Tail, effects);
            }
            Input::TopReached => self.load_older(effects),
            Input::Loaded {
                fetch,
                first,
                last,
                more,
            } => {
                self.note_loaded(*fetch, *first, *last, *more);
            }
            Input::LoadFailed => {
                self.in_flight = false;
                self.requested = None;
            }
            Input::EvictedOlder { oldest } => {
                self.oldest = Some(*oldest);
                self.exhausted = false;
            }
            Input::EvictedNewer { last } => self.gap = Some(*last),
            Input::Jump(Jump::Latest) => {
                if self.gap.is_some() {
                    self.fetch(Fetch::Tail, effects);
                    self.state = State::Jumping {
                        target: Jump::Latest,
                        loading: true,
                    };
                } else {
                    effects.push(Effect::ScrollToEnd);
                    self.state = State::Following {};
                }
            }
            Input::Jump(Jump::To(position)) => {
                self.state = State::Jumping {
                    target: Jump::To(*position),
                    loading: false,
                };
            }
            Input::BottomReached => self.state = State::Following {},
            Input::ScrolledAway { .. } | Input::Present { .. } => {}
        }
    }
}

impl State {
    #[must_use]
    pub(crate) const fn is_following(&self) -> bool {
        matches!(self, Self::Following { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(
        clippy::needless_pass_by_value,
        reason = "tests read as a sequence of inputs handed to the viewport"
    )]
    fn send(viewport: &mut Viewport, input: Input) -> Vec<Effect> {
        let mut effects = Vec::new();
        viewport.handle_with_context(&input, &mut effects);
        effects
    }

    #[test]
    fn reading_preserves_anchor_and_fetches_a_gap_once() {
        let mut viewport = Viewport::default();
        assert_eq!(
            send(&mut viewport, Input::Open),
            [Effect::Fetch(Fetch::Tail)]
        );
        assert_eq!(
            send(
                &mut viewport,
                Input::Loaded {
                    fetch: Fetch::Tail,
                    first: Some(100),
                    last: Some(199),
                    more: true,
                }
            ),
            [Effect::ScrollToEnd]
        );
        assert!(
            send(
                &mut viewport,
                Input::ScrolledAway {
                    anchor: 150,
                    through: 160
                }
            )
            .is_empty()
        );
        assert_eq!(
            send(&mut viewport, Input::TopReached),
            [Effect::Fetch(Fetch::Older { before: 100 })]
        );
        assert!(send(&mut viewport, Input::TopReached).is_empty());
        assert!(
            send(
                &mut viewport,
                Input::Loaded {
                    fetch: Fetch::At { position: 75 },
                    first: Some(75),
                    last: Some(174),
                    more: true,
                }
            )
            .is_empty()
        );
        assert_eq!((viewport.oldest, viewport.gap), (Some(75), Some(174)));
        assert_eq!(
            viewport.state(),
            &State::Reading {
                anchor: 150,
                through: 160
            }
        );
        assert!(send(&mut viewport, Input::EvictedNewer { last: 175 }).is_empty());
        assert_eq!(
            send(&mut viewport, Input::BottomReached),
            [Effect::Fetch(Fetch::Newer { after: 175 })]
        );
        assert!(send(&mut viewport, Input::BottomReached).is_empty());
        assert!(
            send(
                &mut viewport,
                Input::Loaded {
                    fetch: Fetch::Newer { after: 175 },
                    first: Some(176),
                    last: Some(199),
                    more: false,
                }
            )
            .is_empty()
        );
        assert!(send(&mut viewport, Input::BottomReached).is_empty());
        assert!(viewport.state().is_following());
    }

    #[test]
    fn a_jump_loads_the_page_it_lands_on_then_scrolls_to_the_item() {
        let mut viewport = Viewport::default();
        send(
            &mut viewport,
            Input::Loaded {
                fetch: Fetch::Tail,
                first: Some(100),
                last: Some(199),
                more: true,
            },
        );
        send(&mut viewport, Input::Jump(Jump::To(75)));
        assert_eq!(
            send(
                &mut viewport,
                Input::Present {
                    position: 75,
                    loaded: false
                }
            ),
            [Effect::Fetch(Fetch::At { position: 75 })],
            "the page it is on, at once"
        );
        assert!(
            send(
                &mut viewport,
                Input::ScrolledAway {
                    anchor: 150,
                    through: 160
                }
            )
            .is_empty()
        );
        assert!(matches!(
            viewport.state(),
            State::Jumping { loading: true, .. }
        ));
        assert!(
            send(
                &mut viewport,
                Input::Loaded {
                    fetch: Fetch::At { position: 75 },
                    first: Some(75),
                    last: Some(174),
                    more: true,
                }
            )
            .is_empty()
        );
        assert_eq!((viewport.oldest, viewport.gap), (Some(75), Some(174)));
        assert_eq!(
            send(
                &mut viewport,
                Input::Present {
                    position: 75,
                    loaded: true
                }
            ),
            [Effect::ScrollTo(75)]
        );
        assert_eq!(
            viewport.state(),
            &State::Reading {
                anchor: 75,
                through: 75
            }
        );
    }

    #[test]
    fn latest_jump_reloads_an_evicted_tail_and_failure_returns_to_history() {
        let mut viewport = Viewport::default();
        send(
            &mut viewport,
            Input::Loaded {
                fetch: Fetch::Tail,
                first: Some(100),
                last: Some(199),
                more: true,
            },
        );
        send(&mut viewport, Input::EvictedNewer { last: 175 });
        assert_eq!(
            send(&mut viewport, Input::Jump(Jump::Latest)),
            [Effect::Fetch(Fetch::Tail)]
        );
        assert!(send(&mut viewport, Input::LoadFailed).is_empty());
        assert_eq!(
            viewport.state(),
            &State::Reading {
                anchor: 100,
                through: 100
            }
        );
        assert!(!viewport.in_flight);
        assert_eq!(
            send(&mut viewport, Input::Jump(Jump::Latest)),
            [Effect::Fetch(Fetch::Tail)]
        );
        assert_eq!(
            send(
                &mut viewport,
                Input::Loaded {
                    fetch: Fetch::Tail,
                    first: Some(200),
                    last: Some(299),
                    more: true,
                }
            ),
            [Effect::ScrollToEnd]
        );
        assert!(viewport.state().is_following());
        assert!(viewport.gap.is_none());
    }
}
