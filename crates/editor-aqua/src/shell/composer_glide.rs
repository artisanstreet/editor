use std::time::{Duration, Instant};

use aqua_core::Transform2D;

use crate::ui::chrome::glide::{CubicBezier, Glide, Transition};

pub const DOCK_KEY: &str = "composer-dock";
pub const STAGE_KEY: &str = "new-thread-stage";
const MOVE: Transition = Transition {
    duration: Duration::from_millis(400),
    easing: CubicBezier::new(0.22, 1.0, 0.36, 1.0),
};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Centered {
    composer_bottom: f32,
    stage_bottom: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ComposerGlide {
    centered: Option<Centered>,
    shift: Glide,
}

impl Default for ComposerGlide {
    fn default() -> Self {
        Self {
            centered: None,
            shift: Glide::Resting(0.0),
        }
    }
}

impl ComposerGlide {
    pub fn observe_centered(&mut self, composer_bottom: f32, stage_bottom: f32) {
        self.centered = Some(Centered {
            composer_bottom,
            stage_bottom,
        });
        self.shift = Glide::Resting(0.0);
    }

    pub fn forget(&mut self) {
        self.centered = None;
    }

    pub fn dock(&mut self, lift: f32, now: Instant, reduced_motion: bool) {
        let Some(centered) = self.centered.take() else {
            return;
        };
        let from = centered.composer_bottom + lift - centered.stage_bottom;
        self.shift = Glide::Resting(from).retarget(0.0, now, MOVE, reduced_motion);
    }

    #[must_use]
    pub fn shift(&self, now: Instant) -> f32 {
        self.shift.value(now, MOVE)
    }

    #[must_use]
    pub fn transform(&self, now: Instant) -> Transform2D {
        Transform2D::translate(0.0, self.shift(now))
    }

    pub fn sync(&mut self, ui: &mut aqua::UiApp, now: Instant) -> bool {
        let keyed = |key: &str| {
            ui.tree()
                .sole_node_with_key(aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key)))
        };
        let dock = keyed(DOCK_KEY);
        if let (Some(dock), Some(stage)) = (dock, keyed(STAGE_KEY))
            && let (Some(composer), Some(stage)) = (
                ui.tree().pre_transform_rect(dock),
                ui.tree().pre_transform_rect(stage),
            )
        {
            self.observe_centered(composer.y + composer.h, stage.y + stage.h);
            return false;
        }
        let moving = self.shift.running();
        if moving
            && let Some(dock) = dock
            && let Some(record) = ui.tree().get(dock)
        {
            let mut paint = record.paint().clone();
            paint.transform = Some(self.transform(now));
            ui.tree_mut().set_paint(dock, paint);
        }
        self.shift = self.shift.settle(now);
        moving
    }
}

#[cfg(test)]
#[path = "../../tests/shell/composer_glide.rs"]
mod tests;
