use std::time::{Duration, Instant};

use aqua_core::Point;
use aqua_ui::{
    JustifyContent,
    collection_runtime::CollectionViewport,
    element::{Box as UiBox, Element},
    style::{Length, Style},
};
use artisan_api::{OutlineEntry, Seq};

use crate::{
    conversation::{
        controller::Conversation,
        view::{Dispatch, navigator},
    },
    core::{Core, HostInstance},
    frame::strip,
    ui::{
        chrome::glide::{CubicBezier, Glide, Transition},
        kit::{Finish, IconButton, IconButtonSize, Surface},
        theme::ArtisanTheme,
    },
};

pub fn viewport_moves_navigation(
    previous: Option<CollectionViewport>,
    next: CollectionViewport,
    previous_marker: Option<usize>,
    next_marker: Option<usize>,
) -> bool {
    previous.is_none_or(|previous| {
        previous.viewport_height.to_bits() != next.viewport_height.to_bits()
            || previous_marker != next_marker
    })
}

pub fn active_marker(
    conversation: &Conversation,
    core: &Core,
    viewport: CollectionViewport,
) -> Option<usize> {
    let (host, thread) = conversation.open.as_ref()?;
    let summary = core.host(host)?.thread_summary(thread)?.value.as_ref()?;
    Some(active_index(
        &summary.outline,
        reading_position(conversation, viewport),
    ))
}

fn reading_position(conversation: &Conversation, viewport: CollectionViewport) -> Option<Seq> {
    let view = conversation.view.as_ref()?;
    conversation
        .rows
        .iter()
        .take(viewport.visible.start.saturating_add(1))
        .rev()
        .find_map(|(row, _)| row.anchor().and_then(|id| view.item(id)))
        .map(|item| item.position)
}

pub(super) fn latest_visible(
    conversation: &Conversation,
    core: &Core,
    composer_present: bool,
) -> bool {
    conversation.view.as_ref().is_some_and(|view| {
        show_latest(
            matches!(core.workspace_screen(), crate::core::Screen::Thread { host, thread } if conversation.open.as_ref().is_some_and(|(open_host, open_thread)| open_host == host && open_thread == thread)),
            composer_present,
            conversation.agent.is_some(),
            view.viewport.state().is_following(),
            conversation.host_is_current(core),
        )
    })
}

const POP_IN: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: CubicBezier::new(0.22, 1.0, 0.36, 1.0),
};
const POP_OUT: Transition = Transition {
    duration: Duration::from_millis(150),
    easing: CubicBezier::new(0.22, 1.0, 0.36, 1.0),
};
const POP_SCALE: f32 = 0.9;
pub const LATEST_KEY: &str = "jump-latest-presence";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LatestPresence {
    shown: Glide,
}

impl Default for LatestPresence {
    fn default() -> Self {
        Self {
            shown: Glide::Resting(0.0),
        }
    }
}

impl LatestPresence {
    pub fn show(&mut self, visible: bool, now: Instant, reduced_motion: bool) {
        let (target, transition) = match visible {
            true => (1.0, POP_IN),
            false => (0.0, POP_OUT),
        };
        if self.shown.target() != target {
            self.shown = self.shown.retarget(target, now, transition, reduced_motion);
        }
    }

    #[must_use]
    pub fn value(&self, now: Instant) -> f32 {
        self.shown.value(now, POP_IN)
    }

    #[must_use]
    pub const fn shown(&self) -> bool {
        self.shown.target() > 0.5
    }

    pub fn sync(&mut self, ui: &mut aqua::UiApp, now: Instant) -> (bool, bool) {
        let moving = self.shown.running();
        if moving
            && let Some(node) = keyed(ui, LATEST_KEY)
            && let Some(mut paint) = ui.tree().get(node).map(|record| record.paint().clone())
        {
            let presence = self.value(now);
            paint.opacity = Some(presence);
            paint.transform = Some(pop(presence));
            ui.tree_mut().set_paint(node, paint);
        }
        self.shown = self.shown.settle(now);
        let gone = moving && !self.shown.running() && !self.shown();
        (moving, gone)
    }
}

fn pop(presence: f32) -> aqua_core::Transform2D {
    let centre = IconButtonSize::Md.edge() / 2.0;
    let scale = (1.0 - POP_SCALE).mul_add(presence, POP_SCALE);
    aqua_core::Transform2D::translate(-centre, -centre)
        .then(aqua_core::Transform2D::scale(scale, scale))
        .then(aqua_core::Transform2D::translate(centre, centre))
}

pub fn jump_to_latest(theme: ArtisanTheme, presence: f32, dispatch: Dispatch) -> Element {
    let size = IconButtonSize::Md;
    let button = IconButton::new(
        "jump-latest",
        theme,
        editor_assets::AssetId::TABLER_CHEVRON_DOWN,
        "Jump to latest",
        size,
        Finish::Glass,
    )
    .nested(Surface::Pill(size.edge()).radius_px(), 0.0)
    .on_activate(move |_| dispatch(crate::conversation::adapter::Action::JumpLatest))
    .build()
    .style(Style::new().opacity(presence).transform(pop(presence)))
    .with_key(LATEST_KEY);
    UiBox::new()
        .width(Length::Pct(100.0))
        .style(Style::new().justify(JustifyContent::CENTER))
        .child(button)
        .build()
}

fn show_latest(thread: bool, composer: bool, agent: bool, following: bool, current: bool) -> bool {
    thread && composer && !agent && !following && current
}

const EASE: CubicBezier = CubicBezier::new(0.22, 1.0, 0.36, 1.0);
const OPEN: Transition = Transition {
    duration: Duration::from_millis(400),
    easing: EASE,
};
const CLOSE: Transition = Transition {
    duration: Duration::from_millis(350),
    easing: EASE,
};
const FLIP: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: EASE,
};
const INSET: f32 = 8.0;

struct Interaction {
    scope: Option<(String, HostInstance)>,
    panel: Glide,
    chevron: Glide,
    center: Option<(usize, usize)>,
    recentre: bool,
    available: bool,
}

impl Default for Interaction {
    fn default() -> Self {
        Self {
            scope: None,
            panel: Glide::Resting(0.0),
            chevron: Glide::Resting(0.0),
            center: None,
            recentre: false,
            available: false,
        }
    }
}

impl Interaction {
    fn open(&self) -> bool {
        self.panel.target() > 0.5
    }

    fn set_open(&mut self, open: bool, now: Instant, reduced: bool) {
        let target = if open { 1.0 } else { 0.0 };
        let transition = if open { OPEN } else { CLOSE };
        self.panel = self.panel.retarget(target, now, transition, reduced);
        self.chevron = self.chevron.retarget(target, now, FLIP, reduced);
    }

    fn reset(&mut self) {
        self.panel = Glide::Resting(0.0);
        self.chevron = Glide::Resting(0.0);
        self.center = None;
        self.recentre = false;
    }

    fn settle(&mut self, now: Instant) -> bool {
        let moved = self.panel.running() || self.chevron.running();
        self.panel = self.panel.settle(now);
        self.chevron = self.chevron.settle(now);
        moved
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub changed: bool,
    pub animating: bool,
}

#[derive(Clone)]
struct BuildContext {
    composer_bottom: f32,
    theme: ArtisanTheme,
    dispatch: Dispatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    InPlace,
    Rebuild,
}

pub const OVERLAY_KEY: &str = "turn-navigation-overlay";

#[derive(Default)]
pub struct ThreadNavigation {
    interaction: Interaction,
    context: Option<BuildContext>,
}

impl ThreadNavigation {
    pub fn toggle(&mut self, now: Instant, reduced_motion: bool) {
        let state = &mut self.interaction;
        if !state.available {
            return;
        }
        let open = !state.open();
        state.set_open(open, now, reduced_motion);
        state.recentre = open;
    }

    pub fn close(&mut self, now: Instant, reduced_motion: bool) -> bool {
        let state = &mut self.interaction;
        let open = state.open();
        if open {
            state.set_open(false, now, reduced_motion);
            state.recentre = false;
        }
        open
    }

    pub fn chevron(&self, now: Instant) -> Option<f32> {
        self.interaction
            .available
            .then(|| self.interaction.chevron.value(now, FLIP))
    }

    pub fn build(
        &mut self,
        conversation: &Conversation,
        core: &Core,
        viewport: CollectionViewport,
        composer_height: Option<f32>,
        composer_bottom: f32,
        theme: ArtisanTheme,
        now: Instant,
        dispatch: Dispatch,
    ) -> Option<Element> {
        self.context = Some(BuildContext {
            composer_bottom,
            theme,
            dispatch: dispatch.clone(),
        });
        self.build_with(
            conversation,
            core,
            viewport,
            composer_height,
            now,
            composer_bottom,
            theme,
            dispatch,
        )
    }

    pub fn refresh(
        &mut self,
        ui: &mut aqua::UiApp,
        conversation: &Conversation,
        core: &Core,
        viewport: CollectionViewport,
        composer_height: Option<f32>,
        now: Instant,
    ) -> Result<Refresh, String> {
        let Some(context) = self.context.clone() else {
            return Ok(Refresh::Rebuild);
        };
        let mounted = keyed(ui, OVERLAY_KEY).is_some();
        let element = self.build_with(
            conversation,
            core,
            viewport,
            composer_height,
            now,
            context.composer_bottom,
            context.theme,
            context.dispatch,
        );
        match (element, mounted) {
            (None, false) => Ok(Refresh::InPlace),
            (Some(element), true) => ui
                .refresh_subtree(OVERLAY_KEY, element)
                .map(|refreshed| {
                    if refreshed {
                        Refresh::InPlace
                    } else {
                        Refresh::Rebuild
                    }
                })
                .map_err(|error| error.to_string()),
            _ => Ok(Refresh::Rebuild),
        }
    }

    fn build_with(
        &mut self,
        conversation: &Conversation,
        core: &Core,
        viewport: CollectionViewport,
        composer_height: Option<f32>,
        now: Instant,
        composer_bottom: f32,
        theme: ArtisanTheme,
        dispatch: Dispatch,
    ) -> Option<Element> {
        let scope = conversation
            .selection_scope()
            .zip(conversation.host_instance());
        let state = &mut self.interaction;
        if state.scope != scope {
            state.scope = scope;
            state.reset();
        }
        let markers = Self::markers(conversation, core);
        state.available = markers.is_some_and(|markers| markers.len() >= 2);
        if !state.available {
            state.reset();
            return None;
        }
        let markers = markers?;
        let progress = state.panel.value(now, OPEN);
        if progress <= 0.0 && !state.open() {
            return None;
        }
        let active = active_index(markers, reading_position(conversation, viewport));
        if std::mem::take(&mut state.recentre) {
            state.center = Some((active, markers.len()));
        }
        let composer_height = if conversation.agent.is_none() {
            composer_height
        } else {
            None
        };
        let above = composer_height.map_or(0.0, |height| height + composer_bottom) + INSET;
        let panel = navigator::panel(
            markers,
            Some(markers[active].position),
            theme,
            progress,
            dispatch,
        );
        Some(
            UiBox::new()
                .layout("column")
                .style(Style::new().absolute_at(Some(INSET), Some(INSET), None, Some(above)))
                .child(panel)
                .build()
                .with_key(OVERLAY_KEY),
        )
    }

    fn markers<'a>(conversation: &Conversation, core: &'a Core) -> Option<&'a [OutlineEntry]> {
        if !conversation.host_is_current(core) {
            return None;
        }
        let (host, thread) = conversation.open.as_ref()?;
        let summary = core.host(host)?.thread_summary(thread)?.value.as_ref()?;
        Some(&summary.outline)
    }

    pub fn sync(&mut self, ui: &mut aqua::UiApp, now: Instant) -> Frame {
        let mut frame = Frame::default();
        if let Some((active, count)) = self.interaction.center
            && let Some(list) = keyed(ui, "turn-navigator-list")
            && let Some(scroll) = scroll_descendant(ui, list)
            && let Some(bounds) = ui.tree().pre_transform_rect(scroll)
            && bounds.h > 0.0
        {
            let offset = navigator::panel_scroll_offset(active, count, bounds.h);
            frame.changed |= ui.tree_mut().scroll_to(scroll, Point { x: 0.0, y: offset });
            self.interaction.center = None;
        }
        if self.interaction.chevron.running()
            && let Some(node) = keyed(ui, strip::OUTLINE_CHEVRON)
            && let Some(record) = ui.tree().get(node)
        {
            let mut paint = record.paint().clone();
            paint.transform = Some(strip::flip(self.interaction.chevron.value(now, FLIP)));
            frame.changed |= ui.tree_mut().set_paint(node, paint);
        }
        frame.animating = self.interaction.settle(now);
        frame
    }

    pub fn dispose(&mut self) {
        self.context = None;
        self.interaction.scope = None;
        self.interaction.available = false;
        self.interaction.reset();
    }
}

fn keyed(ui: &aqua::UiApp, key: &str) -> Option<aqua_core::NodeId> {
    let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key));
    ui.tree().sole_node_with_key(hash)
}

fn scroll_descendant(ui: &aqua::UiApp, root: aqua_core::NodeId) -> Option<aqua_core::NodeId> {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if ui.tree().scroll_offset(node).is_some() {
            return Some(node);
        }
        pending.extend(ui.tree().get(node)?.children.iter().copied());
    }
    None
}

fn active_index(markers: &[OutlineEntry], reading: Option<Seq>) -> usize {
    markers
        .iter()
        .rposition(|marker| reading.is_some_and(|position| marker.position <= position))
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "../../tests/shell/thread_navigation.rs"]
mod tests;
