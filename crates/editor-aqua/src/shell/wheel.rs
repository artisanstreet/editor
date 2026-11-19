use crate::conversation::scroll::{self, Step};
use crate::ui::text::wheel_glide::{WheelGlide, WheelInput};
use aqua::UiApp;
use aqua_core::{InputEvent, NodeId, Point};

pub(crate) const THREADS: &str = "sidebar-threads";
pub use crate::ui::scroll::MODEL_MENU;
const PROJECT_TREE: &str = "artisan-project-tree";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Transcript,
    Threads,
    Model,
    Tree,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Metrics {
    offset: f32,
    x: f32,
    max: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent per-frame outcomes that merge by OR across surfaces, not states of one machine"
)]
pub struct Advance {
    pub moved: bool,
    pub active: bool,
    pub threads_moved: bool,
    pub transcript_moved: bool,
}

impl Advance {
    fn merge(self, other: Self) -> Self {
        Self {
            moved: self.moved || other.moved,
            active: self.active || other.active,
            threads_moved: self.threads_moved || other.threads_moved,
            transcript_moved: self.transcript_moved || other.transcript_moved,
        }
    }
}

#[derive(Debug, Default)]
pub struct WheelGlides {
    transcript: WheelGlide,
    threads: WheelGlide,
    model: WheelGlide,
    tree: WheelGlide,
}

pub use crate::ui::scroll::key_hash;

pub fn keyed_node(ui: &UiApp, key: &str) -> Option<NodeId> {
    ui.tree().sole_node_with_key(key_hash(key))
}

pub fn repaint(ui: &mut UiApp, key: &str) -> bool {
    keyed_node(ui, key).is_some_and(|node| ui.tree_mut().get_mut(node).is_some())
}

#[must_use]
pub(crate) fn viewport_height(ui: &UiApp, key: &str) -> Option<f32> {
    keyed_node(ui, key)
        .and_then(|node| ui.tree().rect(node))
        .map(|rect| rect.h)
}

#[must_use]
pub fn kept_offset(ui: &UiApp, key: &str) -> Point {
    keyed_node(ui, key)
        .and_then(|node| ui.tree().scroll_offset(node))
        .unwrap_or(Point::ZERO)
}

impl Surface {
    const ALL: [Self; 4] = [Self::Model, Self::Tree, Self::Threads, Self::Transcript];

    fn node(self, ui: &UiApp) -> Option<NodeId> {
        match self {
            Self::Transcript => keyed_node(ui, "conversation-transcript"),
            Self::Threads => keyed_node(ui, THREADS),
            Self::Model => keyed_node(ui, MODEL_MENU),
            Self::Tree => keyed_node(ui, PROJECT_TREE),
        }
    }

    fn metrics(ui: &UiApp, node: NodeId) -> Option<Metrics> {
        let offset = ui.tree().scroll_offset(node)?;
        let rect = ui.tree().rect(node)?;
        let content = ui.tree().content_size(node)?;
        Some(Metrics {
            offset: offset.y,
            x: offset.x,
            max: (content.h - rect.h).max(0.0),
        })
    }

    fn write(self, ui: &mut UiApp, metrics: Metrics, offset: f32) -> bool {
        match self {
            Self::Transcript => scroll::apply_step(
                ui,
                Step::Scroll {
                    offset: -offset,
                    continue_animation: true,
                },
                metrics.x,
            ),
            Self::Threads => ui.scroll_collection_to(THREADS, Point::new(metrics.x, offset)),
            Self::Model | Self::Tree => self
                .node(ui)
                .is_some_and(|node| ui.tree_mut().scroll_to(node, Point::new(metrics.x, offset))),
        }
    }
}

fn under(ui: &UiApp, hit: Option<NodeId>, surface: NodeId) -> bool {
    let mut cursor = hit;
    while let Some(node) = cursor {
        if node == surface {
            return true;
        }
        cursor = ui.tree().get(node).and_then(|record| record.parent);
    }
    false
}

impl WheelGlides {
    fn glide(&mut self, surface: Surface) -> &mut WheelGlide {
        match surface {
            Surface::Transcript => &mut self.transcript,
            Surface::Threads => &mut self.threads,
            Surface::Model => &mut self.model,
            Surface::Tree => &mut self.tree,
        }
    }

    pub fn offer(&mut self, ui: &UiApp, input: &InputEvent) -> Option<Surface> {
        let InputEvent::Scroll { at, delta } = input else {
            return None;
        };
        let hit = {
            let _tm = crate::shell::application::Tm::new("wheel.hit");
            aqua_ui::events::hit_test(ui.tree(), *at)
        };
        let surface = Surface::ALL
            .into_iter()
            .find(|surface| surface.node(ui).is_some_and(|node| under(ui, hit, node)))?;
        let _tm = crate::shell::application::Tm::new("wheel.metrics");
        let node = surface.node(ui)?;
        let metrics = Surface::metrics(ui, node)?;
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let glide = self.glide(surface);
        match WheelInput::classify(delta.y) {
            Some(WheelInput::Notch(distance) | WheelInput::Precise(distance))
                if !reduced && delta.x == 0.0 =>
            {
                glide.push_down(metrics.offset, distance, metrics.max);
                Some(surface)
            }
            _ => {
                glide.cancel();
                None
            }
        }
    }

    pub fn advance(&mut self, ui: &mut UiApp) -> Advance {
        Surface::ALL
            .into_iter()
            .fold(Advance::default(), |advance, surface| {
                advance.merge(self.advance_surface(ui, surface))
            })
    }

    fn advance_surface(&mut self, ui: &mut UiApp, surface: Surface) -> Advance {
        let glide = self.glide(surface);
        if !glide.active() {
            return Advance::default();
        }
        let Some(metrics) = surface.node(ui).and_then(|node| Surface::metrics(ui, node)) else {
            self.glide(surface).cancel();
            return Advance::default();
        };
        let glide = self.glide(surface);
        let next = glide.step_down(metrics.offset, metrics.max);
        let active = glide.active();
        let moved = next.is_some_and(|next| surface.write(ui, metrics, next));
        Advance {
            moved,
            active,
            threads_moved: moved && surface == Surface::Threads,
            transcript_moved: moved && surface == Surface::Transcript,
        }
    }

    pub(crate) fn cancel_transcript(&mut self) {
        self.transcript.cancel();
    }

    pub fn active(&self) -> bool {
        self.transcript.active()
            || self.threads.active()
            || self.model.active()
            || self.tree.active()
    }
}
