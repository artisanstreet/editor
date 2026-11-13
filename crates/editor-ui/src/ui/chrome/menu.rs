pub use crate::ui::kit::hover::{FAST, Highlight, QUICK, REST_SCALE};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Spot {
    Trigger,
    Identity,
    Theme,
    Settings,
    Help,
    Upgrade,
    SignOut,
}

impl Spot {
    pub const ALL: [Self; 7] = [
        Self::Trigger,
        Self::Identity,
        Self::Theme,
        Self::Settings,
        Self::Help,
        Self::Upgrade,
        Self::SignOut,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Toggle,
    Dismissed,
    Hover(Spot, bool),
    Pick(Spot),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effect {
    FlipTheme,
    OpenSettings,
    SignOut,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Menu {
    open: bool,
    hovered: [bool; Spot::ALL.len()],
    highlights: [Highlight; Spot::ALL.len()],
}

impl Default for Menu {
    fn default() -> Self {
        Self {
            open: false,
            hovered: [false; Spot::ALL.len()],
            highlights: [Highlight::default(); Spot::ALL.len()],
        }
    }
}

impl Menu {
    #[must_use]
    pub const fn open(&self) -> bool {
        self.open
    }

    #[must_use]
    pub const fn highlight(&self, spot: Spot) -> Highlight {
        self.highlights[spot.index()]
    }

    #[must_use]
    pub fn animating(&self) -> bool {
        self.highlights.iter().any(Highlight::running)
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        let moving = self.animating();
        for highlight in &mut self.highlights {
            *highlight = highlight.settle(now);
        }
        moving || self.animating()
    }

    pub fn apply(&mut self, event: Event, now: Instant, reduced: bool) -> Option<Effect> {
        match event {
            Event::Toggle => {
                self.set_open(!self.open, now, reduced);
                None
            }
            Event::Dismissed => {
                self.set_open(false, now, reduced);
                None
            }
            Event::Hover(spot, on) => {
                self.hovered[spot.index()] = on;
                self.relight(now, reduced);
                None
            }
            Event::Pick(spot) => self.pick(spot, now, reduced),
        }
    }

    fn pick(&mut self, spot: Spot, now: Instant, reduced: bool) -> Option<Effect> {
        match spot {
            Spot::Trigger => {
                self.set_open(!self.open, now, reduced);
                None
            }
            Spot::Identity => None,
            Spot::Theme => Some(Effect::FlipTheme),
            Spot::Settings => {
                self.set_open(false, now, reduced);
                Some(Effect::OpenSettings)
            }
            Spot::Help | Spot::Upgrade => {
                self.set_open(false, now, reduced);
                None
            }
            Spot::SignOut => {
                self.set_open(false, now, reduced);
                Some(Effect::SignOut)
            }
        }
    }

    fn set_open(&mut self, open: bool, now: Instant, reduced: bool) {
        if self.open == open {
            return;
        }
        self.open = open;
        if !open {
            let trigger = self.hovered[Spot::Trigger.index()];
            self.hovered = [false; Spot::ALL.len()];
            self.hovered[Spot::Trigger.index()] = trigger;
        }
        self.relight(now, reduced);
    }

    fn relight(&mut self, now: Instant, reduced: bool) {
        for spot in Spot::ALL {
            let on = self.hovered[spot.index()] || spot == Spot::Trigger && self.open;
            let highlight = &mut self.highlights[spot.index()];
            *highlight = highlight.light(on, now, reduced);
        }
    }
}
