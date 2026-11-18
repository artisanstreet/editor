use std::time::{Duration, Instant};

use aqua_core::Transform2D;
use artisan_api::ThreadId;

use crate::composer::draft_session::Posting;
use crate::ui::chrome::glide::{CubicBezier, Glide, Transition};

pub(crate) const DOCK_KEY: &str = "composer-dock";
pub(crate) const STAGE_KEY: &str = "new-thread-stage";
const MOVE: Transition = Transition {
    duration: Duration::from_millis(400),
    easing: CubicBezier::new(0.22, 1.0, 0.36, 1.0),
};
const DUSK: Transition = Transition {
    duration: Duration::from_millis(900),
    easing: CubicBezier::new(0.42, 0.0, 0.58, 1.0),
};
const SET: f32 = 72.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Centered {
    composer_bottom: f32,
    stage_bottom: f32,
}

#[derive(Clone, Debug, PartialEq)]
enum Stage {
    Away,
    Centered(Option<Centered>),
    Launched { from: f32, thread: ThreadId },
    Docked(ThreadId),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ComposerGlide {
    stage: Stage,
    shift: Glide,
    dusk: Glide,
}

impl Default for ComposerGlide {
    fn default() -> Self {
        Self {
            stage: Stage::Away,
            shift: Glide::Resting(0.0),
            dusk: Glide::Resting(0.0),
        }
    }
}

pub(crate) enum Route<'a> {
    NewThread(Posting),
    Thread {
        thread: &'a ThreadId,
        sent: Option<ThreadId>,
    },
    Elsewhere,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Synced {
    pub(crate) moving: bool,
    pub(crate) sky_gone: bool,
}

impl ComposerGlide {
    pub(crate) fn follow(
        &mut self,
        route: Route<'_>,
        lift: f32,
        now: Instant,
        reduced_motion: bool,
    ) {
        match route {
            Route::NewThread(posting) => self.new_thread(posting, lift, now, reduced_motion),
            Route::Thread { thread, sent } => {
                if let Some(sent) = sent.filter(|sent| sent == thread) {
                    self.new_thread(Posting::Sending(sent), lift, now, reduced_motion);
                }
                self.leave(Some(thread));
            }
            Route::Elsewhere => self.leave(None),
        }
    }

    fn observe_centered(&mut self, composer_bottom: f32, stage_bottom: f32) {
        if let Stage::Centered(_) = self.stage {
            self.stage = Stage::Centered(Some(Centered {
                composer_bottom,
                stage_bottom,
            }));
        }
    }

    fn new_thread(&mut self, posting: Posting, lift: f32, now: Instant, reduced_motion: bool) {
        match posting {
            Posting::Sending(thread) => self.launch(thread, lift, now, reduced_motion),
            Posting::Blocked(thread) | Posting::Accepted(thread) if self.launched_into(&thread) => {
            }
            Posting::Blocked(_) | Posting::Accepted(_) | Posting::Idle | Posting::Failed => {
                self.center(now, reduced_motion);
            }
        }
    }

    #[must_use]
    pub(crate) fn awaits(&self, posting: &Posting) -> bool {
        self.wants_launch(posting) != self.launched()
    }

    fn wants_launch(&self, posting: &Posting) -> bool {
        match posting {
            Posting::Sending(_) => true,
            Posting::Blocked(thread) | Posting::Accepted(thread) => self.launched_into(thread),
            Posting::Idle | Posting::Failed => false,
        }
    }

    fn center(&mut self, now: Instant, reduced_motion: bool) {
        match &self.stage {
            Stage::Centered(_) => {}
            Stage::Away | Stage::Docked(_) => {
                self.stage = Stage::Centered(None);
                self.shift = Glide::Resting(0.0);
                self.dusk = Glide::Resting(0.0);
            }
            Stage::Launched { from, .. } => {
                let back = self.shift(now) - from;
                self.stage = Stage::Centered(None);
                self.shift = Glide::Resting(back).retarget(0.0, now, MOVE, reduced_motion);
                self.dusk = self.dusk.retarget(0.0, now, DUSK, reduced_motion);
            }
        }
    }

    fn launched_into(&self, thread: &ThreadId) -> bool {
        matches!(&self.stage, Stage::Launched { thread: sent, .. } if sent == thread)
    }

    fn launch(&mut self, thread: ThreadId, lift: f32, now: Instant, reduced_motion: bool) {
        let centered = match &self.stage {
            Stage::Launched { .. } => return,
            Stage::Away | Stage::Docked(_) => None,
            Stage::Centered(centered) => *centered,
        };
        match centered {
            Some(centered) => {
                let from = centered.composer_bottom + lift - centered.stage_bottom;
                let start = from + self.shift(now);
                self.stage = Stage::Launched { from, thread };
                self.shift = Glide::Resting(start).retarget(0.0, now, MOVE, reduced_motion);
                self.dusk = self.dusk.retarget(1.0, now, DUSK, reduced_motion);
            }
            None => {
                self.stage = Stage::Launched { from: 0.0, thread };
                self.shift = Glide::Resting(0.0);
                self.dusk = Glide::Resting(1.0);
            }
        }
    }

    fn leave(&mut self, thread: Option<&ThreadId>) {
        match (&self.stage, thread) {
            (Stage::Launched { thread: sent, .. }, Some(thread)) if sent == thread => {
                self.stage = Stage::Docked(thread.clone());
            }
            (Stage::Docked(docked), Some(thread)) if docked == thread => {}
            _ => *self = Self::default(),
        }
    }

    #[must_use]
    pub(crate) const fn launched(&self) -> bool {
        matches!(self.stage, Stage::Launched { .. })
    }

    #[must_use]
    pub(crate) fn shift(&self, now: Instant) -> f32 {
        self.shift.value(now, MOVE)
    }

    #[must_use]
    fn transform(&self, now: Instant) -> Transform2D {
        Transform2D::translate(0.0, self.shift(now))
    }

    #[must_use]
    fn dusk(&self, now: Instant) -> f32 {
        self.dusk.value(now, DUSK)
    }

    #[must_use]
    pub(crate) fn sky_shown(&self, now: Instant) -> bool {
        self.dusk(now) < 1.0
    }

    #[must_use]
    pub(crate) fn sky_setting(&self, now: Instant) -> bool {
        self.dusk.target() >= 1.0 && self.sky_shown(now)
    }

    #[must_use]
    pub(crate) fn sky_opacity(&self, now: Instant) -> f32 {
        1.0 - self.dusk(now)
    }

    #[must_use]
    pub(crate) fn sky_transform(&self, now: Instant) -> Transform2D {
        let dusk = self.dusk(now);
        Transform2D::translate(0.0, SET * dusk * dusk)
    }

    pub(crate) fn sync(&mut self, ui: &mut aqua::UiApp, now: Instant) -> Synced {
        let keyed = |ui: &aqua::UiApp, key: &str| {
            ui.tree()
                .sole_node_with_key(aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key)))
        };
        if let Stage::Centered(_) = self.stage
            && let (Some(dock), Some(stage)) = (keyed(ui, DOCK_KEY), keyed(ui, STAGE_KEY))
            && let (Some(composer), Some(stage)) = (
                ui.tree().pre_transform_rect(dock),
                ui.tree().pre_transform_rect(stage),
            )
        {
            self.observe_centered(composer.y + composer.h, stage.y + stage.h);
        }
        let moving = self.shift.running();
        if moving && let Some(dock) = keyed(ui, DOCK_KEY) {
            Self::paint(ui, dock, None, Some(self.transform(now)));
        }
        let dusking = self.dusk.running();
        if dusking && let Some(sky) = keyed(ui, crate::ui::sky::KEY) {
            Self::paint(
                ui,
                sky,
                Some(self.sky_opacity(now)),
                Some(self.sky_transform(now)),
            );
        }
        self.shift = self.shift.settle(now);
        self.dusk = self.dusk.settle(now);
        Synced {
            moving: moving || dusking,
            sky_gone: dusking && !self.dusk.running() && !self.sky_shown(now),
        }
    }

    fn paint(
        ui: &mut aqua::UiApp,
        node: aqua_core::NodeId,
        opacity: Option<f32>,
        transform: Option<Transform2D>,
    ) {
        let Some(record) = ui.tree().get(node) else {
            return;
        };
        let mut paint = record.paint().clone();
        if opacity.is_some() {
            paint.opacity = opacity;
        }
        paint.transform = transform;
        ui.tree_mut().set_paint(node, paint);
    }
}

#[cfg(test)]
#[path = "../../tests/shell/composer_glide.rs"]
mod tests;
