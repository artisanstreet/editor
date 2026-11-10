use artisan_api::Seq;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetch {
    Tail,
    Older { before: Seq },
    Newer { after: Seq },
    At { position: Seq },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    Latest,
    To(Seq),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Fetch(Fetch),
    ScrollToEnd,
    ScrollTo(Seq),
}

#[derive(Clone, Debug)]
pub enum Input {
    Open,
    TopReached,
    BottomReached,
    ScrolledAway {
        anchor: Seq,
        through: Seq,
    },
    Jump(Jump),
    Loaded {
        fetch: Fetch,
        first: Option<Seq>,
        last: Option<Seq>,
        more: bool,
    },
    LoadFailed,
    EvictedOlder {
        oldest: Seq,
    },
    EvictedNewer {
        last: Seq,
    },
    Present {
        position: Seq,
        loaded: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Viewport {
    state: State,
    pub oldest: Option<Seq>,
    pub exhausted: bool,
    pub in_flight: bool,
    pub requested: Option<Fetch>,
    pub gap: Option<Seq>,
}

impl Viewport {
    pub const fn loaded(&self) -> bool {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Following {},
    Reading { anchor: Seq, through: Seq },
    Jumping { target: Jump, loading: bool },
}

impl Default for State {
    fn default() -> Self {
        Self::Following {}
    }
}

impl Viewport {
    pub const fn state(&self) -> &State {
        &self.state
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one state machine: every (state, input) transition sits in one match"
    )]
    pub fn handle_with_context(&mut self, event: &Input, effects: &mut Vec<Effect>) {
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
                    let anchor = self.oldest.unwrap_or(*position);
                    self.state = State::Reading {
                        anchor,
                        through: anchor,
                    };
                } else if !*loading {
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
    pub const fn is_following(&self) -> bool {
        matches!(self, Self::Following { .. })
    }
}
