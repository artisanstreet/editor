use crate::frame::scramble::Rows;
use crate::frame::view::{INK_X, WORDMARK_INK_LEFT, WORDMARK_WIDTH};
use crate::frame::wordmark::Wordmark;
use crate::ui::chrome::glide::{CubicBezier, Glide, Transition};
use crate::ui::chrome::menu::{self, Effect, Menu};
use artisan_api::{ProjectId, ThreadId};
use std::collections::VecDeque;
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
const SEARCH_OPEN: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: CubicBezier::new(0.22, 1.0, 0.36, 1.0),
};
const SEARCH_CLOSE: Transition = Transition {
    duration: Duration::from_millis(150),
    easing: CubicBezier::new(0.22, 1.0, 0.36, 1.0),
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Hover(bool),
    Toggle,
    Search(bool),
    Account(menu::Event),
    Navigate(Command),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Open(ThreadId),
    OpenUrl(String),
    Project(ProjectId),
    Agent(Option<artisan_api::ItemId>),
    Outline,
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
    refreshed: Option<Instant>,
    search: Glide,
    search_motion: Transition,
    focus_search: bool,
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
            refreshed: None,
            search: Glide::Resting(0.0),
            search_motion: SEARCH_OPEN,
            focus_search: false,
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
        let threshold = if self.forced {
            RESTORE_AT
        } else {
            COLLAPSE_BELOW
        };
        self.forced = window_width < threshold;
        if self.open() {
            self.search = Glide::Resting(0.0);
        }
        self.width_limit = (window_width - THREAD_MIN_WIDTH - crate::frame::view::SHEET_PADDING)
            .clamp(COLLAPSED_WIDTH, OPEN_WIDTH);
        let target = if self.open() {
            OPEN_WIDTH
        } else {
            COLLAPSED_WIDTH
        };
        if self.width.target() != target || (reduced && self.width.running()) {
            if self.width.value(now, RESIZE) > visible_width {
                self.width = Glide::Resting(visible_width);
            }
            self.width = self.width.retarget(target, now, RESIZE, reduced);
        }
    }

    #[must_use]
    pub(crate) fn search(&self, now: Instant) -> f32 {
        self.search.value(now, self.search_motion)
    }

    #[must_use]
    pub(crate) fn searching(&self) -> bool {
        self.search.target() > 0.5 || self.search.running()
    }

    pub(crate) fn take_search_focus(&mut self) -> bool {
        std::mem::take(&mut self.focus_search)
    }

    fn show_search(&mut self, shown: bool, now: Instant, reduced: bool) {
        let target = f32::from(u8::from(shown && !self.open()));
        if self.search.target() == target {
            return;
        }
        self.search_motion = if target > 0.5 {
            SEARCH_OPEN
        } else {
            SEARCH_CLOSE
        };
        self.focus_search = target > 0.5;
        self.search = self
            .search
            .retarget(target, now, self.search_motion, reduced);
    }

    #[must_use]
    pub(crate) fn wordmark(&self, now: Instant) -> Rows {
        self.wordmark.shown(now)
    }

    #[must_use]
    pub(crate) const fn menu(&self) -> &Menu {
        &self.menu
    }

    pub(crate) fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    pub(crate) fn refresh_due(&mut self, now: Instant) -> bool {
        let due = self
            .refreshed
            .is_none_or(|last| now.saturating_duration_since(last) >= REFRESH);
        if due {
            self.refreshed = Some(now);
        }
        due
    }

    pub(crate) fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
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
        self.search = self.search.settle(now);
        let turning = self.menu.tick(now);
        moving || turning || self.animating()
    }

    #[must_use]
    pub fn animating(&self) -> bool {
        self.width.running()
            || self.search.running()
            || self.wordmark.scrambling()
            || self.menu.animating()
    }

    pub(crate) fn resizing(&self) -> bool {
        self.width.running()
    }

    #[must_use]
    pub(crate) const fn deadline(&self) -> Option<Instant> {
        self.wordmark.deadline()
    }

    fn apply(&mut self, event: Event, now: Instant, reduced: bool) {
        match event {
            Event::Hover(true) => self.wordmark.enter(now, reduced),
            Event::Hover(false) => self.wordmark.leave(now),
            Event::Toggle if !self.forced => {
                self.open = !self.open;
                self.search = Glide::Resting(0.0);
                let target = if self.open {
                    OPEN_WIDTH
                } else {
                    COLLAPSED_WIDTH
                };
                self.width = self.width.retarget(target, now, RESIZE, reduced);
            }
            Event::Toggle => {}
            Event::Search(shown) => self.show_search(shown, now, reduced),
            Event::Account(event) => {
                self.effects.extend(self.menu.apply(event, now, reduced));
            }
            Event::Navigate(command) => {
                self.show_search(false, now, reduced);
                self.commands.push(command);
            }
        }
    }
}
