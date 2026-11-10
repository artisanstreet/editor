use crate::ui::chrome::glide::{CubicBezier, Glide, Transition};
use std::time::{Duration, Instant};

const SMOOTH_OUT: CubicBezier = CubicBezier::new(0.22, 1.0, 0.36, 1.0);
pub const QUICK: Transition = Transition {
    duration: Duration::from_millis(150),
    easing: SMOOTH_OUT,
};
pub const FAST: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: SMOOTH_OUT,
};
pub const REST_SCALE: f32 = 0.4;

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
pub struct Highlight {
    opacity: Glide,
    scale: Glide,
}

impl Default for Highlight {
    fn default() -> Self {
        Self {
            opacity: Glide::Resting(0.0),
            scale: Glide::Resting(REST_SCALE),
        }
    }
}

impl Highlight {
    #[must_use]
    pub fn opacity(&self, now: Instant) -> f32 {
        self.opacity.value(now, QUICK)
    }

    #[must_use]
    pub fn scale(&self, now: Instant) -> f32 {
        self.scale.value(now, QUICK)
    }

    #[must_use]
    pub fn lit(&self) -> bool {
        self.opacity.target() > 0.5
    }

    pub fn light(self, on: bool, now: Instant, reduced: bool) -> Self {
        if self.lit() == on {
            return self;
        }
        let (opacity, scale, grow) = if on {
            (1.0, 1.0, FAST)
        } else {
            (0.0, REST_SCALE, QUICK)
        };
        Self {
            opacity: self.opacity.retarget(opacity, now, QUICK, reduced),
            scale: self.scale.retarget(scale, now, grow, reduced),
        }
    }

    pub fn settle(self, now: Instant) -> Self {
        Self {
            opacity: self.opacity.settle(now),
            scale: self.scale.settle(now),
        }
    }

    pub const fn running(&self) -> bool {
        self.opacity.running() || self.scale.running()
    }
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
