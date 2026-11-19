pub mod card;
pub mod dither;
pub mod model;
pub mod pattern;

use crate::ui::chrome::glide::{CubicBezier, Glide, Transition};
use aqua::images::{ImageAssets, ImageHandle};
use model::{EmailProblem, Feedback, Link, Page, Provider};
use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Arc, time::Duration, time::Instant};

pub(crate) const EMAIL_PAGE: &str = "artisan-sign-in-page-email";
pub(crate) const CODE_PAGE: &str = "artisan-sign-in-page-code";
pub(crate) const EMAIL_FEEDBACK: &str = "artisan-sign-in-feedback-email";
pub(crate) const CODE_FEEDBACK: &str = "artisan-sign-in-feedback-code";
const ART_BOX: (f32, f32) = (240.0, 96.0);
const ART_PARAMS: dither::Params = dither::Params {
    pixel_size: 0.5,
    contrast: 1.15,
    brightness: 1.25,
    threshold_bias: 0.02,
    max_resolution: 1024.0,
    fade_right: 0.3,
};
const ART_PAINT: dither::Paint = dither::Paint {
    rgb: [165, 172, 178],
    opacity: 0.7,
    fade_in_left: 0.3,
};

const SMOOTH_OUT: CubicBezier = CubicBezier::new(0.22, 1.0, 0.36, 1.0);
pub(crate) const PAGE: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: SMOOTH_OUT,
};
const RESIZE: Transition = Transition {
    duration: Duration::from_millis(300),
    easing: SMOOTH_OUT,
};
pub(crate) const SWAP: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: CubicBezier::new(0.42, 0.0, 0.58, 1.0),
};
pub(crate) const REVEAL: Transition = Transition {
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
    page: Glide,
    height: Option<Glide>,
    busy: Glide,
    email_feedback: Glide,
    code_feedback: Glide,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sizes {
    pub(crate) email_page: f32,
    pub(crate) code_page: f32,
    pub(crate) email_feedback: f32,
    pub(crate) code_feedback: f32,
}

pub struct SignIn {
    pub(crate) problem: Option<EmailProblem>,
    pub reduced: bool,
    motion: Motion,
    shown: Page,
    sizes: Option<Sizes>,
    last: (Feedback, Feedback),
    art: Option<Arc<ImageHandle>>,
    art_refused: Option<aqua::images::ImageAssetError>,
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
            },
            shown: Page::Email,
            sizes: None,
            last: (Feedback::None, Feedback::None),
            art: None,
            art_refused: None,
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

    pub(crate) fn take_events(&self) -> Vec<Event> {
        self.events.borrow_mut().drain(..).collect()
    }

    #[must_use]
    pub(crate) fn art(&self) -> Option<Arc<ImageHandle>> {
        self.art.clone()
    }

    pub(crate) fn ensure_art(&mut self, assets: &ImageAssets) {
        if self.art.is_some() {
            return;
        }
        let rgba = image::load_from_memory(editor_assets::art::MOUNTAIN)
            .expect("invariant: the bundled sign-in artwork decodes")
            .to_rgba8();
        let mask = dither::dither(dither::Source::new(&rgba), ART_BOX.0, ART_BOX.1, ART_PARAMS);
        let pixels = dither::paint(&mask, ART_PAINT);
        let image = aqua::images::DecodedImage::from_rgba(
            pixels.width(),
            pixels.height(),
            pixels.into_raw(),
        )
        .expect("invariant: the dithered sign-in artwork has one RGBA pixel per cell");
        match assets.insert(image) {
            Ok(handle) => {
                self.art = Some(Arc::new(handle));
                self.art_refused = None;
            }
            Err(error) => {
                if self.art_refused.as_ref() != Some(&error) {
                    tracing::warn!(%error, "the sign-in artwork could not be uploaded");
                }
                self.art_refused = Some(error);
            }
        }
    }

    #[must_use]
    fn height(&self, now: Instant) -> Option<f32> {
        self.motion.height.map(|height| height.value(now, RESIZE))
    }

    #[must_use]
    const fn last(&self, slot: Slot) -> &Feedback {
        match slot {
            Slot::Email => &self.last.0,
            Slot::Code => &self.last.1,
        }
    }

    #[must_use]
    fn feedback_height(&self, slot: Slot) -> f32 {
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
        for (current, last) in [
            (feedback.0, &mut self.last.0),
            (feedback.1, &mut self.last.1),
        ] {
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
        aim(
            &mut self.motion.email_feedback,
            f32::from(u8::from(shown.0)),
            REVEAL,
        );
        aim(
            &mut self.motion.code_feedback,
            f32::from(u8::from(shown.1)),
            REVEAL,
        );
        if self.shown != page {
            self.shown = page;
            changed |= self.resize(now, reduced);
        }
        changed
    }

    pub(crate) fn measure(&mut self, sizes: Sizes, now: Instant, reduced: bool) -> bool {
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
            || motion.email_feedback.running()
            || motion.code_feedback.running()
            || motion.height.is_some_and(|height| height.running())
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        let moving = self.motion.page.running()
            || self.motion.busy.running()
            || self.motion.email_feedback.running()
            || self.motion.code_feedback.running()
            || self.motion.height.is_some_and(|height| height.running());
        let motion = &mut self.motion;
        motion.page = motion.page.settle(now);
        motion.busy = motion.busy.settle(now);
        motion.email_feedback = motion.email_feedback.settle(now);
        motion.code_feedback = motion.code_feedback.settle(now);
        motion.height = motion.height.map(|height| height.settle(now));
        moving
    }
}
