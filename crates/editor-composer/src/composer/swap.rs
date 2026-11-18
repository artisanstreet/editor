use std::time::{Duration, Instant};

use aqua_core::Transform2D;
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::Style;
use editor_assets::AssetId;
use editor_ui::chrome::card::POLAR_INK;
use editor_ui::kit::IconSize;
use editor_ui::motion::MotionCurve;

use super::view::Primary;

pub const DURATION: Duration = Duration::from_millis(250);
pub const INCOMING: &str = "composer-primary-in";
pub const OUTGOING: &str = "composer-primary-out";
pub const PROMPT: &str = "composer-prompt-settle";
pub const SETTLE: Duration = Duration::from_millis(250);
const SETTLE_RISE: f32 = 6.0;
const BLUR: f32 = 2.0;
const SHRINK: f32 = 0.25;
const EDGE: IconSize = IconSize::Md;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    pub opacity: f32,
    pub scale: f32,
    pub blur: f32,
}

impl Layer {
    #[must_use]
    pub fn incoming(progress: f32) -> Self {
        Self {
            opacity: progress,
            scale: SHRINK.mul_add(progress, 1.0 - SHRINK),
            blur: BLUR * (1.0 - progress),
        }
    }

    #[must_use]
    pub fn outgoing(progress: f32) -> Self {
        Self::incoming(1.0 - progress)
    }

    #[must_use]
    pub fn transform(self) -> Transform2D {
        let centre = EDGE.px() / 2.0;
        Transform2D::translate(-centre, -centre)
            .then(Transform2D::scale(self.scale, self.scale))
            .then(Transform2D::translate(centre, centre))
    }

    fn style(self) -> Style {
        Style::new()
            .opacity(self.opacity)
            .content_blur(self.blur)
            .transform(self.transform())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Swap {
    previous: Primary,
    started: Instant,
    previous_level: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwapFrame {
    pub previous: Primary,
    pub progress: f32,
    previous_level: f32,
}

impl SwapFrame {
    #[must_use]
    pub fn incoming(self) -> Layer {
        Layer::incoming(self.progress)
    }

    #[must_use]
    pub fn outgoing(self) -> Layer {
        Layer::incoming(self.previous_level * (1.0 - self.progress))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PrimarySwap {
    face: Option<Primary>,
    swap: Option<Swap>,
}

impl PrimarySwap {
    pub fn observe(&mut self, face: Primary, now: Instant, reduced_motion: bool) {
        match self.face.replace(face) {
            Some(previous) if previous != face && !reduced_motion => {
                let previous_level = self.progress(now).map_or(1.0, |frame| frame.progress);
                self.swap = Some(Swap {
                    previous,
                    started: now,
                    previous_level,
                });
            }
            Some(previous) if previous != face => self.swap = None,
            _ => {}
        }
    }

    #[must_use]
    pub fn progress(&self, now: Instant) -> Option<SwapFrame> {
        let swap = self.swap?;
        let elapsed = now.saturating_duration_since(swap.started);
        (elapsed < DURATION).then(|| SwapFrame {
            previous: swap.previous,
            progress: MotionCurve::EaseInOut
                .sample_f32(elapsed.as_secs_f64() / DURATION.as_secs_f64()),
            previous_level: swap.previous_level,
        })
    }

    pub fn settle(&mut self, now: Instant) -> bool {
        let moving = self.swap.is_some();
        if self.progress(now).is_none() {
            self.swap = None;
        }
        moving
    }
}

fn glyph(icon: AssetId, key: &'static str, layer: Layer) -> Element {
    let edge = EDGE.px();
    UiBox::new()
        .width(edge)
        .height(edge)
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(0.0), None, None)
                .with(&layer.style()),
        )
        .child(EDGE.glyph(icon, POLAR_INK.to_reference_paint()))
        .build()
        .with_key(key)
}

#[must_use]
pub fn face(icon: AssetId, outgoing: Option<(AssetId, SwapFrame)>) -> Element {
    let edge = EDGE.px();
    let mut stack = UiBox::new().width(edge).height(edge);
    let incoming = match outgoing {
        Some((previous, frame)) => {
            stack = stack.child(glyph(previous, OUTGOING, frame.outgoing()));
            frame.incoming()
        }
        None => Layer::incoming(1.0),
    };
    stack.child(glyph(icon, INCOMING, incoming)).build()
}

pub fn advance(app: &mut aqua::UiApp, frame: Option<SwapFrame>) {
    let (incoming, outgoing) = frame.map_or_else(
        || (Layer::incoming(1.0), Layer::outgoing(1.0)),
        |frame| (frame.incoming(), frame.outgoing()),
    );
    for (key, layer) in [(INCOMING, incoming), (OUTGOING, outgoing)] {
        let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key));
        let Some(node) = app.tree().sole_node_with_key(hash) else {
            continue;
        };
        let Some(mut paint) = app.tree().get(node).map(|record| record.paint().clone()) else {
            continue;
        };
        paint.opacity = Some(layer.opacity);
        paint.content_blur = Some(layer.blur);
        paint.transform = Some(layer.transform());
        app.tree_mut().set_paint(node, paint);
    }
}

#[must_use]
pub fn settle_progress(cleared: Instant, now: Instant) -> Option<f32> {
    let elapsed = now.saturating_duration_since(cleared);
    (elapsed < SETTLE)
        .then(|| MotionCurve::EaseOut.sample_f32(elapsed.as_secs_f64() / SETTLE.as_secs_f64()))
}

pub fn advance_prompt(app: &mut aqua::UiApp, progress: f32) {
    let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(PROMPT));
    let Some(node) = app.tree().sole_node_with_key(hash) else {
        return;
    };
    let Some(mut paint) = app.tree().get(node).map(|record| record.paint().clone()) else {
        return;
    };
    paint.opacity = Some(progress);
    paint.transform = Some(Transform2D::translate(0.0, SETTLE_RISE * (1.0 - progress)));
    app.tree_mut().set_paint(node, paint);
}

#[cfg(test)]
#[path = "../../tests/composer/swap.rs"]
mod tests;
