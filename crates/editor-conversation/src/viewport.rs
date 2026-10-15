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
    state: State,
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

/// The reader's current mode; all page edges live on `Viewport`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Following {},
    Reading { anchor: Seq },
    Jumping { target: Jump, loading: bool },
}

impl Default for State {
    fn default() -> Self {
        Self::Following {}
    }
}

impl Viewport {
    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    pub(crate) fn handle_with_context(&mut self, event: &Input, effects: &mut Vec<Effect>) {
        // Mode-specific events finish here. The rest share page bookkeeping.
        match (&mut self.state, event) {
            (State::Following {}, Input::ScrolledAway { anchor })
            | (State::Reading { .. }, Input::ScrolledAway { anchor }) => {
                self.state = State::Reading { anchor: *anchor };
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
                    self.state = State::Reading { anchor: *position };
                } else if self.exhausted || self.oldest.is_none_or(|oldest| oldest <= *position) {
                    self.state = State::Reading {
                        anchor: self.oldest.unwrap_or(*position),
                    };
                } else {
                    *loading = true;
                    self.load_older(effects);
                }
                return;
            }
            (State::Jumping { .. }, Input::LoadFailed) => {
                self.in_flight = false;
                self.state = State::Reading {
                    anchor: self.oldest.unwrap_or_default(),
                };
                return;
            }
            (State::Jumping { loading: false, .. }, Input::ScrolledAway { anchor }) => {
                self.state = State::Reading { anchor: *anchor };
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
            Input::LoadFailed => self.in_flight = false,
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
    pub(crate) fn is_following(&self) -> bool {
        matches!(self, Self::Following { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(send(&mut viewport, Input::ScrolledAway { anchor: 150 }).is_empty());
        assert_eq!(
            send(&mut viewport, Input::TopReached),
            [Effect::Fetch(Fetch::Older { before: 100 })]
        );
        assert!(send(&mut viewport, Input::TopReached).is_empty());
        assert!(
            send(
                &mut viewport,
                Input::Loaded {
                    fetch: Fetch::Older { before: 100 },
                    first: Some(50),
                    last: Some(99),
                    more: false,
                }
            )
            .is_empty()
        );
        assert_eq!(viewport.state(), &State::Reading { anchor: 150 });
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
    fn a_jump_waits_for_its_page_then_scrolls_to_the_present_item() {
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
            [Effect::Fetch(Fetch::Older { before: 100 })]
        );
        assert!(send(&mut viewport, Input::ScrolledAway { anchor: 150 }).is_empty());
        assert!(matches!(
            viewport.state(),
            State::Jumping { loading: true, .. }
        ));
        assert!(
            send(
                &mut viewport,
                Input::Loaded {
                    fetch: Fetch::Older { before: 100 },
                    first: Some(50),
                    last: Some(99),
                    more: false,
                }
            )
            .is_empty()
        );
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
        assert_eq!(viewport.state(), &State::Reading { anchor: 75 });
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
        assert_eq!(viewport.state(), &State::Reading { anchor: 100 });
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
