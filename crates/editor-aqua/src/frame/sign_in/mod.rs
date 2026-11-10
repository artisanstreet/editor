pub mod card;
pub mod dither;
pub mod model;
pub mod pattern;

use crate::ui::chrome::glide::{CubicBezier, Glide, Transition};
use aqua::images::{ImageAssets, ImageHandle};
use model::{EmailProblem, Feedback, Link, Page, Provider};
use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Arc, time::Duration, time::Instant};

pub const EMAIL_PAGE: &str = "artisan-sign-in-page-email";
pub const CODE_PAGE: &str = "artisan-sign-in-page-code";
pub const EMAIL_FEEDBACK: &str = "artisan-sign-in-feedback-email";
pub const CODE_FEEDBACK: &str = "artisan-sign-in-feedback-code";
pub const ART_BOX: (f32, f32) = (240.0, 96.0);
pub const ART_PARAMS: dither::Params = dither::Params {
    pixel_size: 0.5,
    contrast: 1.15,
    brightness: 1.25,
    threshold_bias: 0.02,
    max_resolution: 1024.0,
    fade_right: 0.3,
};
pub const ART_PAINT: dither::Paint = dither::Paint {
    rgb: [165, 172, 178],
    opacity: 0.7,
    fade_in_left: 0.3,
};

const SMOOTH_OUT: CubicBezier = CubicBezier::new(0.22, 1.0, 0.36, 1.0);
pub const PAGE: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: SMOOTH_OUT,
};
pub const RESIZE: Transition = Transition {
    duration: Duration::from_millis(300),
    easing: SMOOTH_OUT,
};
pub const SWAP: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: CubicBezier::new(0.42, 0.0, 0.58, 1.0),
};
pub const REVEAL: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: SMOOTH_OUT,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    SubmitEmail,
    SubmitCode,
    Edited,
    Provider(Provider),
    Open(Link),
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Slot {
    Email,
    Code,
}

pub struct Motion {
    pub page: Glide,
    pub height: Option<Glide>,
    pub busy: Glide,
    pub email_feedback: Glide,
    pub code_feedback: Glide,
    pub spin: u32,
}

pub const SPIN_STEPS: u32 = 24;
const SPIN_PERIOD_MS: u32 = 800;

#[must_use]
pub const fn spin_step(clock_ms: u32) -> u32 {
    clock_ms % SPIN_PERIOD_MS * SPIN_STEPS / SPIN_PERIOD_MS
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sizes {
    pub email_page: f32,
    pub code_page: f32,
    pub email_feedback: f32,
    pub code_feedback: f32,
}

pub struct SignIn {
    pub problem: Option<EmailProblem>,
    pub reduced: bool,
    pub motion: Motion,
    shown: Page,
    sizes: Option<Sizes>,
    last: (Feedback, Feedback),
    art: Option<Arc<ImageHandle>>,
    events: Rc<RefCell<VecDeque<Event>>>,
}

impl Default for SignIn {
    fn default() -> Self {
        Self {
            problem: None,
            reduced: false,
            motion: Motion {
                page: Glide::Resting(0.0),
                height: None,
                busy: Glide::Resting(0.0),
                email_feedback: Glide::Resting(0.0),
                code_feedback: Glide::Resting(0.0),
                spin: 0,
            },
            shown: Page::Email,
            sizes: None,
            last: (Feedback::None, Feedback::None),
            art: None,
            events: Rc::default(),
        }
    }
}

impl SignIn {
    #[must_use]
    pub fn emit(&self) -> Rc<dyn Fn(Event)> {
        let events = Rc::clone(&self.events);
        Rc::new(move |event| events.borrow_mut().push_back(event))
    }

    pub fn take_events(&self) -> Vec<Event> {
        self.events.borrow_mut().drain(..).collect()
    }

    #[must_use]
    pub fn art(&self) -> Option<Arc<ImageHandle>> {
        self.art.clone()
    }

    pub fn ensure_art(&mut self, assets: &ImageAssets) {
        if self.art.is_some() {
            return;
        }
        let Ok(decoded) = image::load_from_memory(editor_assets::art::MOUNTAIN) else {
            return;
        };
        let rgba = decoded.to_rgba8();
        let source = dither::Source {
            width: rgba.width(),
            height: rgba.height(),
            rgba: rgba.as_raw(),
        };
        let mask = dither::dither(&source, ART_BOX.0, ART_BOX.1, ART_PARAMS);
        let pixels = dither::paint(&mask, ART_PAINT);
        let Ok(image) = aqua::images::DecodedImage::from_rgba(mask.width, mask.height, pixels) else {
            return;
        };
        self.art = assets.insert(image).ok().map(Arc::new);
    }

    #[must_use]
    pub fn height(&self, now: Instant) -> Option<f32> {
        self.motion.height.map(|height| height.value(now, RESIZE))
    }

    #[must_use]
    pub const fn last(&self, slot: Slot) -> &Feedback {
        match slot {
            Slot::Email => &self.last.0,
            Slot::Code => &self.last.1,
        }
    }

    #[must_use]
    pub fn feedback_height(&self, slot: Slot) -> f32 {
        self.sizes.map_or(0.0, |sizes| match slot {
            Slot::Email => sizes.email_feedback,
            Slot::Code => sizes.code_feedback,
        })
    }

    pub fn present(
        &mut self,
        page: Page,
        busy: bool,
        feedback: (&Feedback, &Feedback),
        now: Instant,
        reduced: bool,
    ) -> bool {
        let mut changed = false;
        for (current, last) in [(feedback.0, &mut self.last.0), (feedback.1, &mut self.last.1)] {
            if !matches!(current, Feedback::None) && current != last {
                *last = current.clone();
                changed = true;
            }
        }
        let shown = (
            !matches!(feedback.0, Feedback::None),
            !matches!(feedback.1, Feedback::None),
        );
        let mut aim = |glide: &mut Glide, target: f32, transition| {
            if (glide.target() - target).abs() > f32::EPSILON {
                *glide = glide.retarget(target, now, transition, reduced);
                changed = true;
            }
        };
        aim(
            &mut self.motion.page,
            match page {
                Page::Email => 0.0,
                Page::Code => 1.0,
            },
            PAGE,
        );
        aim(&mut self.motion.busy, f32::from(u8::from(busy)), SWAP);
        aim(&mut self.motion.email_feedback, f32::from(u8::from(shown.0)), REVEAL);
        aim(&mut self.motion.code_feedback, f32::from(u8::from(shown.1)), REVEAL);
        if self.shown != page {
            self.shown = page;
            changed |= self.resize(now, reduced);
        }
        changed
    }

    pub fn measure(&mut self, sizes: Sizes, now: Instant, reduced: bool) -> bool {
        let close = |a: f32, b: f32| (a - b).abs() < 0.5;
        if self.sizes.is_some_and(|known| {
            close(known.email_page, sizes.email_page)
                && close(known.code_page, sizes.code_page)
                && close(known.email_feedback, sizes.email_feedback)
                && close(known.code_feedback, sizes.code_feedback)
        }) {
            return false;
        }
        self.sizes = Some(sizes);
        self.resize(now, reduced) | true
    }

    fn resize(&mut self, now: Instant, reduced: bool) -> bool {
        let Some(sizes) = self.sizes else {
            return false;
        };
        let target = match self.shown {
            Page::Email => sizes.email_page,
            Page::Code => sizes.code_page,
        };
        let next = match self.motion.height {
            None => Glide::Resting(target),
            Some(height) if (height.target() - target).abs() < 0.5 => return false,
            Some(height) => height.retarget(target, now, RESIZE, reduced),
        };
        self.motion.height = Some(next);
        true
    }

    #[must_use]
    pub fn animating(&self) -> bool {
        let motion = &self.motion;
        motion.page.running()
            || motion.busy.running()
            || self.spinning()
            || motion.email_feedback.running()
            || motion.code_feedback.running()
            || motion.height.is_some_and(|height| height.running())
    }

    fn spinning(&self) -> bool {
        !self.reduced && self.motion.busy.target() > 0.5
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        let spun = self.spinning() && {
            let step = spin_step(crate::ui::clock::pulse_ms());
            std::mem::replace(&mut self.motion.spin, step) != step
        };
        let moving = self.motion.page.running()
            || self.motion.busy.running()
            || self.motion.email_feedback.running()
            || self.motion.code_feedback.running()
            || self.motion.height.is_some_and(|height| height.running())
            || spun;
        let motion = &mut self.motion;
        motion.page = motion.page.settle(now);
        motion.busy = motion.busy.settle(now);
        motion.email_feedback = motion.email_feedback.settle(now);
        motion.code_feedback = motion.code_feedback.settle(now);
        motion.height = motion.height.map(|height| height.settle(now));
        moving
    }
}
