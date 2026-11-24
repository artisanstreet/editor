use crate::conversation::scroll::{self, Step};
use crate::ui::text::wheel_glide::{WheelGlide, WheelInput};
use aqua::Instant;
use aqua::UiApp;
use aqua_core::{InputEvent, NodeId, Point};

pub(crate) const THREADS: &str = "sidebar-threads";
pub use crate::ui::scroll::MODEL_MENU;
const PROJECT_TREE: &str = "artisan-project-tree";
pub(crate) const SETTINGS_PAGE: &str = "settings-main-scroll";
const SETTINGS_NAV: &str = "settings-dialog-nav";
const SETTINGS_LICENCES: &str = "settings-licences";
const SETTINGS_ARCHIVE: &str = "archived-threads";
const SETTINGS_PROMPT: &str = "global-prompt-scroll";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Transcript,
    Threads,
    Model,
    Tree,
    SettingsPage,
    SettingsNav,
    SettingsLicences,
    SettingsArchive,
    SettingsPrompt,
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
    glides: [WheelGlide; Surface::ALL.len()],
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
    const ALL: [Self; 9] = [
        Self::Model,
        Self::Tree,
        Self::SettingsLicences,
        Self::SettingsArchive,
        Self::SettingsPrompt,
        Self::SettingsNav,
        Self::SettingsPage,
        Self::Threads,
        Self::Transcript,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Model => 0,
            Self::Tree => 1,
            Self::SettingsLicences => 2,
            Self::SettingsArchive => 3,
            Self::SettingsPrompt => 4,
            Self::SettingsNav => 5,
            Self::SettingsPage => 6,
            Self::Threads => 7,
            Self::Transcript => 8,
        }
    }

    const fn key(self) -> &'static str {
        match self {
            Self::Transcript => "conversation-transcript",
            Self::Threads => THREADS,
            Self::Model => MODEL_MENU,
            Self::Tree => PROJECT_TREE,
            Self::SettingsPage => SETTINGS_PAGE,
            Self::SettingsNav => SETTINGS_NAV,
            Self::SettingsLicences => SETTINGS_LICENCES,
            Self::SettingsArchive => SETTINGS_ARCHIVE,
            Self::SettingsPrompt => SETTINGS_PROMPT,
        }
    }

    fn node(self, ui: &UiApp) -> Option<NodeId> {
        keyed_node(ui, self.key())
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
            Self::Threads | Self::SettingsLicences | Self::SettingsArchive => {
                ui.scroll_collection_to(self.key(), Point::new(metrics.x, offset))
            }
            Self::Model
            | Self::Tree
            | Self::SettingsPage
            | Self::SettingsNav
            | Self::SettingsPrompt => self
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
        &mut self.glides[surface.index()]
    }

    pub fn offer(&mut self, ui: &UiApp, input: &InputEvent, now: Instant) -> Option<Surface> {
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
                glide.push_down(metrics.offset, distance, metrics.max, now);
                Some(surface)
            }
            _ => {
                glide.cancel();
                None
            }
        }
    }

    pub fn advance(&mut self, ui: &mut UiApp, now: Instant) -> Advance {
        Surface::ALL
            .into_iter()
            .fold(Advance::default(), |advance, surface| {
                advance.merge(self.advance_surface(ui, surface, now))
            })
    }

    fn advance_surface(&mut self, ui: &mut UiApp, surface: Surface, now: Instant) -> Advance {
        let glide = self.glide(surface);
        if !glide.active() {
            return Advance::default();
        }
        let Some(metrics) = surface.node(ui).and_then(|node| Surface::metrics(ui, node)) else {
            self.glide(surface).cancel();
            return Advance::default();
        };
        let glide = self.glide(surface);
        let next = glide.step_down(metrics.offset, metrics.max, now);
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
        self.glide(Surface::Transcript).cancel();
    }

    pub fn active(&self) -> bool {
        self.glides.iter().any(WheelGlide::active)
    }
}
