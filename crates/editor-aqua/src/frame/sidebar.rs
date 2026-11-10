use crate::ui::chrome::glide::{CubicBezier, Glide, Transition};
use crate::ui::chrome::menu::{self, Effect, Menu};
use crate::frame::projects::{Metrics, Section, pinned};
use crate::frame::scramble::Rows;
use crate::frame::tabs::Tabs;
use crate::frame::view::{INK_X, WORDMARK_INK_LEFT, WORDMARK_WIDTH};
use crate::frame::wordmark::Wordmark;
use artisan_api::{ProjectId, ThreadId};
use std::collections::{HashMap, VecDeque};
use std::{cell::RefCell, rc::Rc, time::Duration, time::Instant};

pub const OPEN_WIDTH: f32 = 336.0;
const COLLAPSE_BELOW: f32 = 960.0;
const RESTORE_AT: f32 = 992.0;
const THREAD_MIN_WIDTH: f32 = 600.0;
pub const COLLAPSED_WIDTH: f32 = 2.0 * INK_X + WORDMARK_WIDTH - 2.0 * WORDMARK_INK_LEFT;
pub const RESIZE: Transition = Transition {
    duration: Duration::from_millis(300),
    easing: CubicBezier::new(0.22, 1.0, 0.36, 1.0),
};
const REFRESH: Duration = Duration::from_secs(1);
pub const ACCORDION: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: CubicBezier::new(0.22, 1.0, 0.36, 1.0),
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Hover(bool),
    Toggle,
    Account(menu::Event),
    Project(ProjectId),
    Navigate(Command),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Open(ThreadId),
    OpenUrl(String),
    Settle {
        host: crate::core::HostId,
        instance: crate::core::HostInstance,
        thread: ThreadId,
        observed_seq: artisan_api::Seq,
    },
    New,
}

pub struct Sidebar {
    open: bool,
    forced: bool,
    width_limit: f32,
    width: Glide,
    wordmark: Wordmark,
    menu: Menu,
    effects: Vec<Effect>,
    commands: Vec<Command>,
    expanded: HashMap<ProjectId, Glide>,
    sections: Vec<Section>,
    pinned: Option<(ProjectId, f32)>,
    metrics: Metrics,
    followed: Option<ThreadId>,
    tabs: Tabs,
    refreshed: Option<Instant>,
    fades: [Glide; 2],
    events: Rc<RefCell<VecDeque<Event>>>,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            open: true,
            forced: false,
            width_limit: OPEN_WIDTH,
            width: Glide::Resting(OPEN_WIDTH),
            wordmark: Wordmark::default(),
            menu: Menu::default(),
            effects: Vec::new(),
            commands: Vec::new(),
            expanded: HashMap::new(),
            sections: Vec::new(),
            pinned: None,
            metrics: Metrics::at(1.0),
            followed: None,
            tabs: Tabs::default(),
            refreshed: None,
            fades: [Glide::Resting(0.0); 2],
            events: Rc::default(),
        }
    }
}

impl Sidebar {
    #[must_use]
    pub fn emit(&self) -> Rc<dyn Fn(Event)> {
        let events = Rc::clone(&self.events);
        Rc::new(move |event| events.borrow_mut().push_back(event))
    }

    #[must_use]
    pub const fn open(&self) -> bool {
        self.open && !self.forced
    }

    #[must_use]
    pub fn width(&self, now: Instant) -> f32 {
        self.width.value(now, RESIZE).min(self.width_limit)
    }

    pub fn fit(&mut self, window_width: f32, now: Instant, reduced: bool) {
        let visible_width = self.width(now);
        let threshold = match self.forced {
            true => RESTORE_AT,
            false => COLLAPSE_BELOW,
        };
        self.forced = window_width < threshold;
        self.width_limit = (window_width - THREAD_MIN_WIDTH - crate::frame::view::SHEET_PADDING)
            .clamp(COLLAPSED_WIDTH, OPEN_WIDTH);
        let target = match self.open() {
            true => OPEN_WIDTH,
            false => COLLAPSED_WIDTH,
        };
        if self.width.target() != target || (reduced && self.width.running()) {
            if self.width.value(now, RESIZE) > visible_width {
                self.width = Glide::Resting(visible_width);
            }
            self.width = self.width.retarget(target, now, RESIZE, reduced);
        }
    }

    #[must_use]
    pub fn wordmark(&self, now: Instant) -> Rows {
        self.wordmark.shown(now)
    }

    #[must_use]
    pub const fn menu(&self) -> &Menu {
        &self.menu
    }

    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    pub fn refresh_due(&mut self, now: Instant) -> bool {
        let due = self
            .refreshed
            .is_none_or(|last| now.saturating_duration_since(last) >= REFRESH);
        if due {
            self.refreshed = Some(now);
        }
        due
    }

    #[must_use]
    pub fn fades(&self, now: Instant) -> (f32, f32) {
        let [above, below] = self.fades;
        (above.value(now, menu::QUICK), below.value(now, menu::QUICK))
    }

    pub fn edges(&mut self, above: bool, below: bool, now: Instant, reduced: bool) -> bool {
        let mut changed = false;
        for (fade, shown) in self.fades.iter_mut().zip([above, below]) {
            let target = f32::from(u8::from(shown));
            if fade.target() != target {
                *fade = fade.retarget(target, now, menu::QUICK, reduced);
                changed = true;
            }
        }
        changed
    }

    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }

    #[must_use]
    pub fn openness(&self, project: &ProjectId, now: Instant) -> f32 {
        self.expanded
            .get(project)
            .map_or(0.0, |glide| glide.value(now, ACCORDION))
    }

    #[must_use]
    pub fn pinned(&self) -> Option<&(ProjectId, f32)> {
        self.pinned.as_ref()
    }

    pub fn lay_out(&mut self, sections: Vec<Section>, metrics: Metrics, offset: f32) {
        self.metrics = metrics;
        self.pinned = pinned(&sections, metrics, offset);
        self.sections = sections;
    }

    pub fn scrolled(&mut self, offset: f32) -> bool {
        let next = pinned(&self.sections, self.metrics, offset);
        std::mem::replace(&mut self.pinned, next) != self.pinned
    }

    #[must_use]
    pub const fn tabs(&self) -> &Tabs {
        &self.tabs
    }

    pub fn follow(&mut self, working: &[ThreadId], open: Option<&ThreadId>) -> bool {
        let moved = self.followed.as_ref() != open;
        if moved {
            self.followed = open.cloned();
        }
        self.tabs.sync(working, open) || moved
    }

    pub fn take_events(&mut self, now: Instant, reduced: bool) -> bool {
        let events: Vec<Event> = self.events.borrow_mut().drain(..).collect();
        let changed = !events.is_empty();
        for event in events {
            self.apply(event, now, reduced);
        }
        changed
    }

    pub fn tick(&mut self, now: Instant, reduced: bool) -> bool {
        let moving = self.animating();
        self.wordmark.advance(now, reduced);
        self.width = self.width.settle(now);
        for fade in &mut self.fades {
            *fade = fade.settle(now);
        }
        for glide in self.expanded.values_mut() {
            *glide = glide.settle(now);
        }
        self.expanded
            .retain(|_, glide| glide.running() || glide.target() > 0.5);
        let turning = self.menu.tick(now);
        moving || turning || self.animating()
    }

    #[must_use]
    pub fn animating(&self) -> bool {
        self.width.running()
            || self.wordmark.scrambling()
            || self.menu.animating()
            || self.fades.iter().any(Glide::running)
            || self.expanded.values().any(Glide::running)
    }

    pub fn resizing(&self) -> bool {
        self.width.running()
    }

    #[must_use]
    pub const fn deadline(&self) -> Option<Instant> {
        self.wordmark.deadline()
    }

    fn apply(&mut self, event: Event, now: Instant, reduced: bool) {
        match event {
            Event::Hover(true) => self.wordmark.enter(now, reduced),
            Event::Hover(false) => self.wordmark.leave(now),
            Event::Toggle if !self.forced => {
                self.open = !self.open;
                let target = if self.open {
                    OPEN_WIDTH
                } else {
                    COLLAPSED_WIDTH
                };
                self.width = self.width.retarget(target, now, RESIZE, reduced);
            }
            Event::Toggle => {}
            Event::Account(event) => {
                self.effects.extend(self.menu.apply(event, now, reduced));
            }
            Event::Project(project) => {
                let glide = self.expanded.entry(project).or_insert(Glide::Resting(0.0));
                let target = if glide.target() > 0.5 { 0.0 } else { 1.0 };
                *glide = glide.retarget(target, now, ACCORDION, reduced);
            }
            Event::Navigate(command) => self.commands.push(command),
        }
    }
}
