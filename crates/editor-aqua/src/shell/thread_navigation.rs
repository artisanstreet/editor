use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use aqua_core::Point;
use aqua_text::{FontFamily, FontWeight, TextStyle, overflow::OverflowMode};
use aqua_ui::{
    AlignItems, JustifyContent,
    collection_runtime::CollectionViewport,
    element::{Box as UiBox, Element, Text},
    style::{Length, Style},
};
use artisan_api::{ItemId, OutlineEntry, Seq};

use crate::{
    conversation::{
        controller::Conversation,
        view::{
            Dispatch,
            navigator::{self, Navigator},
        },
    },
    core::{Core, HostInstance},
    ui::theme::ArtisanTheme,
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

pub fn shifted_in_place(
    navigation: &mut ThreadNavigation,
    previous: Option<CollectionViewport>,
    next: CollectionViewport,
    next_marker: Option<usize>,
    now: Instant,
    reduced_motion: bool,
) -> bool {
    previous.is_some_and(|previous| {
        previous.viewport_height.to_bits() == next.viewport_height.to_bits()
    }) && next_marker.is_some_and(|marker| navigation.shift_active(marker, now, reduced_motion))
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

pub fn jump_to_latest(
    conversation: &Conversation,
    core: &Core,
    composer_present: bool,
    theme: ArtisanTheme,
    dispatch: Dispatch,
) -> Option<Element> {
    if !latest_visible(conversation, core, composer_present) {
        return None;
    }
    let button = crate::ui::button::Button::icon(
        "jump-latest",
        theme,
        editor_assets::AssetId::TABLER_CHEVRON_DOWN,
        "Jump to latest",
    )
    .variant(crate::ui::button::ButtonVariant::Ghost)
    .on_activate(move |_| dispatch(crate::conversation::adapter::Action::JumpLatest))
    .build();
    Some(
        UiBox::new()
            .width(Length::Pct(100.0))
            .style(Style::new().justify(JustifyContent::CENTER))
            .child(crate::ui::glass::glass_handle(&theme, button))
            .build(),
    )
}

fn show_latest(thread: bool, composer: bool, agent: bool, following: bool, current: bool) -> bool {
    thread && composer && !agent && !following && current
}

#[derive(Default)]
struct Interaction {
    scope: Option<(String, HostInstance)>,
    hovered: bool,
    opened: Option<Instant>,
    center: Option<(usize, usize)>,
    epoch: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub changed: bool,
    pub animating: bool,
    pub ticking: bool,
}

#[derive(Clone)]
struct BuildContext {
    window_height: f32,
    composer_bottom: f32,
    theme: ArtisanTheme,
    reduced_motion: bool,
    dispatch: Dispatch,
    invalidate: Rc<dyn Fn()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    InPlace,
    Rebuild,
}

pub const OVERLAY_KEY: &str = "turn-navigation-overlay";

pub struct ThreadNavigation {
    navigator: Rc<RefCell<Navigator>>,
    interaction: Rc<RefCell<Interaction>>,
    context: Option<BuildContext>,
}

impl Default for ThreadNavigation {
    fn default() -> Self {
        Self {
            navigator: Rc::new(RefCell::new(Navigator::default())),
            interaction: Rc::new(RefCell::new(Interaction::default())),
            context: None,
        }
    }
}

impl ThreadNavigation {
    pub fn shift_active(&mut self, active: usize, now: Instant, reduced_motion: bool) -> bool {
        !self.interaction.borrow().hovered
            && self
                .navigator
                .borrow_mut()
                .shift_active(active, now, reduced_motion)
    }

    pub fn build(
        &mut self,
        conversation: &Conversation,
        core: &Core,
        viewport: CollectionViewport,
        window_height: f32,
        composer_height: Option<f32>,
        composer_bottom: f32,
        theme: ArtisanTheme,
        reduced_motion: bool,
        now: Instant,
        dispatch: Dispatch,
        invalidate: Rc<dyn Fn()>,
    ) -> Option<Element> {
        self.context = Some(BuildContext {
            window_height,
            composer_bottom,
            theme,
            reduced_motion,
            dispatch: dispatch.clone(),
            invalidate: invalidate.clone(),
        });
        self.build_with(
            conversation,
            core,
            viewport,
            composer_height,
            now,
            window_height,
            composer_bottom,
            theme,
            reduced_motion,
            dispatch,
            invalidate,
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
            context.window_height,
            context.composer_bottom,
            context.theme,
            context.reduced_motion,
            context.dispatch,
            context.invalidate,
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
        window_height: f32,
        composer_bottom: f32,
        theme: ArtisanTheme,
        reduced_motion: bool,
        dispatch: Dispatch,
        invalidate: Rc<dyn Fn()>,
    ) -> Option<Element> {
        let scope = conversation
            .selection_scope()
            .zip(conversation.host_instance());
        {
            let mut state = self.interaction.borrow_mut();
            if state.scope != scope {
                state.scope = scope;
                state.hovered = false;
                state.opened = None;
                state.center = None;
                state.epoch = state.epoch.wrapping_add(1);
                self.navigator.borrow().clear_hover();
            }
        }
        if !conversation.host_is_current(core) {
            return None;
        }
        let (host, thread) = conversation.open.as_ref()?;
        let summary = core.host(host)?.thread_summary(thread)?.value.as_ref()?;
        let markers = &summary.outline;
        if markers.len() < 2 {
            let mut state = self.interaction.borrow_mut();
            state.hovered = false;
            state.opened = None;
            state.center = None;
            self.navigator.borrow().clear_hover();
            return None;
        }
        let reading = reading_position(conversation, viewport);
        let active = active_index(markers, reading);
        let viewport_height = viewport.viewport_height.max(window_height * 0.5);
        let composer_height = if conversation.agent.is_none() {
            composer_height
        } else {
            None
        };
        let (rail, max_height, _) = self.navigator.borrow_mut().rail(
            markers,
            reading,
            viewport_height,
            composer_height,
            composer_bottom,
            reduced_motion,
            now,
            theme,
            dispatch.clone(),
        )?;
        let (hovered, opened, epoch) = {
            let state = self.interaction.borrow();
            (state.hovered, state.opened, state.epoch)
        };
        let mut region = UiBox::new().style(Style::new().align(AlignItems::CENTER));
        if hovered {
            let progress = if reduced_motion {
                1.0
            } else {
                opened.map_or(1.0, |at| {
                    crate::ui::picker_motion::smooth_out(
                        (now.saturating_duration_since(at).as_secs_f32() / 0.15).clamp(0.0, 1.0),
                    )
                })
            };
            let navigator = self.navigator.clone();
            let state = Rc::downgrade(&self.interaction);
            let invalidate_hover = invalidate.clone();
            let row_hover: Rc<dyn Fn(ItemId)> = Rc::new(move |id| {
                if state
                    .upgrade()
                    .is_some_and(|state| state.borrow().epoch == epoch && state.borrow().hovered)
                {
                    navigator.borrow().hovered(&id);
                    invalidate_hover();
                }
            });
            let label: Rc<dyn Fn(&str, bool, ArtisanTheme) -> Element> =
                Rc::new(|label, active, theme| {
                    Text::new(label.to_owned())
                        .text_style(TextStyle {
                            families: vec![FontFamily::Named(
                                theme.typography.sans.family.to_owned(),
                            )],
                            size_px: theme.typography.control_text,
                            weight: if active {
                                FontWeight::MEDIUM
                            } else {
                                FontWeight::NORMAL
                            },
                            ..TextStyle::default()
                        })
                        .gpui_source_color(theme.colors.text.to_reference_text_color())
                        .overflow(OverflowMode::EndEllipsis)
                        .wrap(false)
                        .style(Style::new().min_width(Length::Px(0.0)).flex_shrink(1.0))
                        .build()
                });
            let menu = navigator::menu(
                markers,
                Some(markers[active].position),
                max_height,
                theme,
                progress,
                &self.navigator.borrow().hover,
                label,
                row_hover,
                dispatch,
            );
            region = region.child(
                UiBox::new()
                    .style(Style::new().padding_insets(aqua_core::Insets {
                        right: 4.0,
                        ..aqua_core::Insets::default()
                    }))
                    .child(menu)
                    .build(),
            );
        }
        region = region.child(rail);
        let interaction = Rc::downgrade(&self.interaction);
        let navigator = Rc::downgrade(&self.navigator);
        let count = markers.len();
        let region = region
            .build()
            .with_key("turn-navigator")
            .on_hover_change(move |over| {
                let Some(interaction) = interaction.upgrade() else {
                    return;
                };
                let mut state = interaction.borrow_mut();
                if state.epoch != epoch || state.hovered == over {
                    return;
                }
                state.hovered = over;
                if over {
                    state.opened = Some(Instant::now());
                    state.center = Some((active, count));
                } else {
                    state.opened = None;
                    state.center = None;
                    if let Some(navigator) = navigator.upgrade() {
                        navigator.borrow().hover.hide();
                    }
                }
                drop(state);
                invalidate();
            });
        let above = composer_height.map_or(0.0, |height| height + composer_bottom) + 8.0;
        Some(
            UiBox::new()
                .layout("column")
                .style(
                    Style::new()
                        .absolute_at(None, Some(8.0), Some(8.0), Some(above))
                        .justify(JustifyContent::CENTER),
                )
                .child(region)
                .build()
                .with_key(OVERLAY_KEY),
        )
    }

    pub fn sync(
        &mut self,
        ui: &mut aqua::UiApp,
        theme: &ArtisanTheme,
        now: Instant,
        seconds: f64,
        reduced_motion: bool,
    ) -> Result<Frame, String> {
        let mut frame = Frame::default();
        let center = self.interaction.borrow().center;
        if let Some((active, count)) = center {
            if let Some(list) = keyed(ui, "turn-navigator-list") {
                if let Some(scroll) = scroll_descendant(ui, list) {
                    if let Some(bounds) = ui.tree().pre_transform_rect(scroll) {
                        if bounds.h > 0.0 {
                            let offset = navigator::menu_scroll_offset(active, count, bounds.h);
                            frame.changed |=
                                ui.tree_mut().scroll_to(scroll, Point { x: 0.0, y: offset });
                            self.interaction.borrow_mut().center = None;
                        }
                    }
                }
            }
        }
        if let Some(surface) = keyed(ui, "turn-navigator-hover-surface") {
            let hover = self.navigator.borrow_mut().advance_hover_frame(
                ui,
                surface,
                theme,
                seconds,
                reduced_motion,
            )?;
            frame.changed |= hover.changed;
            frame.animating |= hover.animating;
        }
        frame.animating |= !reduced_motion
            && self.interaction.borrow().opened.is_some_and(|opened| {
                now.saturating_duration_since(opened) < Duration::from_millis(150)
            });
        let ticks = self.navigator.borrow_mut().advance_ticks(ui, theme, now);
        frame.changed |= ticks.changed;
        frame.ticking = ticks.moving;
        Ok(frame)
    }

    pub fn dispose(&mut self, ui: &mut aqua::UiApp) {
        self.context = None;
        self.navigator.borrow_mut().dispose(ui);
        let mut state = self.interaction.borrow_mut();
        state.scope = None;
        state.hovered = false;
        state.opened = None;
        state.center = None;
        state.epoch = state.epoch.wrapping_add(1);
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
