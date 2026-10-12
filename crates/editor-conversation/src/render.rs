//! The conversation's frame (the old `conversation_surface` shell): the
//! full-height transcript list of cached row views on the black shell, the
//! composer floating over its bottom, the jump-to-latest chip above the
//! composer, and the turn navigator on the right edge.

use crate::fold::Row;
use crate::row::{COLUMN, GUTTER, RowView};
use crate::viewport::Input;
use crate::{Conversation, wants_older};
use artisan_api::{ItemId, ItemKind};
use editor_core::{ConnectionStatus, Screen};
use editor_ui::AssetId;
use editor_ui::button::{AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant};
use editor_ui::glass::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use editor_ui::motion::{MotionDuration, MotionPolicy};
use editor_ui::picker_motion::{HoverRect, sliding_hover_pill, smooth_out};
use editor_ui::theme::{DesktopTheme, Oklch, RadiusStep, RadiusTokens};
use editor_ui::wordmark_loader::WordmarkLoader;
use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, FontWeight, InteractiveElement as _,
    IntoElement, MouseMoveEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Window, canvas, div, list,
    prelude::FluentBuilder as _, px, rgb, svg,
};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The composer's bottom inset (`pb-4`).
pub(crate) const COMPOSER_BOTTOM: f32 = 16.0;
/// The composer's bottom inset once the window is wide (`sm:pb-6`).
const COMPOSER_BOTTOM_WIDE: f32 = 24.0;
/// The width from which the wider inset applies (`sm`).
const WIDE_FROM: f32 = 640.0;
/// The air between the latest words and the composer card.
pub(crate) const COMPOSER_GAP: f32 = 24.0;
/// The shell under the transcript (`SHELL_BLACK`).
const SHELL_BLACK: u32 = 0x0000_0000;

const NAVIGATOR_RAIL_WIDTH_PX: f32 = 40.0;
const NAVIGATOR_RAIL_INSET_PX: f32 = 8.0;
const NAVIGATOR_PAD_PX: f32 = 8.0;
const NAVIGATOR_TICK_PITCH_PX: f32 = 5.0;
const NAVIGATOR_MENU_WIDTH_PX: f32 = 320.0;
const NAVIGATOR_MENU_GAP_PX: f32 = 4.0;
const NAVIGATOR_MENU_ROW_PX: f32 = 32.0;
const NAVIGATOR_MENU_ROW_GAP_PX: f32 = 4.0;
const NAVIGATOR_MENU_ENTRANCE_MS: u64 = 150;
const NAVIGATOR_MENU_ENTRANCE_SHIFT_PX: f32 = 4.0;
/// A transcript frame slower than this is logged.
const SLOW_FRAME_MS: u128 = 50;
/// The rail and menu never exceed this share of the transcript's height.
const NAVIGATOR_HEIGHT_CAP: f32 = 0.7;

impl Render for Conversation {
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the transcript, its overlays and the floating composer in visual order"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame_started.set(Some(std::time::Instant::now()));
        // Judged on last frame's layout; the wordmark stays until it holds.
        if self.advance_landing(cx) {
            window.request_animation_frame();
        }
        let theme = self.theme;
        let screen = self.core.read(cx).screen().clone();
        let wide = f32::from(window.viewport_size().width) >= WIDE_FROM;
        let bottom = if wide {
            COMPOSER_BOTTOM_WIDE
        } else {
            COMPOSER_BOTTOM
        };
        let mut root = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(SHELL_BLACK))
            .text_color(theme.colors.foreground.to_paint())
            .font_family(theme.typography.sans.family);
        let mut following = true;
        let body: AnyElement = match (&screen, &self.view) {
            // The composer and its project tab say it all (master).
            (Screen::NewThread { .. }, _) => div().size_full().into_any_element(),
            (Screen::Thread { .. }, Some(view)) if self.rows.is_empty() => {
                let window_state = view.window();
                if let Some(error) = &self.load_error {
                    centered(
                        div()
                            .text_color(theme.colors.destructive.to_paint())
                            .child(error.clone()),
                    )
                } else if window_state.in_flight
                    || window_state.oldest.is_none() && !window_state.exhausted
                {
                    centered(WordmarkLoader::new("conversation-loading", theme))
                } else {
                    centered(
                        div()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child("No messages yet."),
                    )
                }
            }
            (Screen::Thread { .. }, Some(view)) => {
                following = view.viewport.state().is_following();
                let landing = self.landing();
                let arrival = self.arrival_offset();
                if arrival != 0.0 {
                    window.request_animation_frame();
                }
                let transcript = list(
                    self.list.clone(),
                    cx.processor(|this, index, _window, cx| this.row_element(index, cx)),
                )
                .size_full();
                div()
                    .relative()
                    .size_full()
                    // Landing, the list lays out and reads ahead unseen.
                    .child(
                        div()
                            .size_full()
                            .relative()
                            .top(px(arrival))
                            .when(landing, |list| list.opacity(0.0))
                            .child(transcript),
                    )
                    .when(landing, |this| {
                        this.child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(centered(WordmarkLoader::new("conversation-landing", theme))),
                        )
                    })
                    // Over the list and painted after it, so its wheel
                    // listener runs first and can glide a tick.
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .on_scroll_wheel(cx.listener(Self::transcript_wheel)),
                    )
                    // Landing unseen, the end is reached at once, not glided to.
                    .child(self.follow_driver(following, cx.reduce_motion() || landing, cx))
                    .when_some(self.load_error.clone(), |this, error| {
                        this.child(
                            div()
                                .absolute()
                                .top(px(8.0))
                                .w_full()
                                .flex()
                                .justify_center()
                                .text_color(theme.colors.destructive.to_paint())
                                .child(error),
                        )
                    })
                    .children(self.navigator(window, cx))
                    .into_any_element()
            }
            _ => div().flex_1().into_any_element(),
        };
        root = root.child(div().size_full().child(body));
        // A subagent's view is read-only: the composer talks to the thread.
        if let Some(composer) = self.composer.as_ref().filter(|_| self.agent.is_none())
            && matches!(screen, Screen::Thread { .. } | Screen::NewThread { .. })
        {
            let this = cx.entity().downgrade();
            let measure = canvas(
                move |bounds, _, cx| {
                    let height = f32::from(bounds.size.height);
                    let this = this.clone();
                    cx.defer(move |cx| {
                        let _ = this.update(cx, |this, cx| this.composer_measured(height, cx));
                    });
                },
                |_, (), _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            let mut shell = div()
                .w_full()
                .max_w(px(COLUMN))
                .flex()
                .flex_col()
                .gap(px(8.0));
            if !following {
                shell = shell.child(
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(self.jump_button(cx)),
                );
            }
            shell = shell.child(
                div()
                    .relative()
                    .w_full()
                    .child(composer.clone())
                    .child(measure),
            );
            if let Screen::NewThread { host, .. } = &screen {
                // Master's new-thread layout: the composer card centred in
                // the column, any connection status above it.
                let status = self.new_thread_status(host, cx);
                let card = div()
                    .relative()
                    .w_full()
                    .max_w(px(COLUMN))
                    .child(shell)
                    .when_some(status, |card, status| {
                        card.child(
                            div()
                                .absolute()
                                .bottom_full()
                                .left_0()
                                .right_0()
                                .flex()
                                .justify_center()
                                .child(status),
                        )
                    });
                root = root.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .px(px(GUTTER))
                        .child(card),
                );
            } else {
                root = root.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .flex()
                        .justify_center()
                        .px(px(GUTTER))
                        .pb(px(bottom))
                        .child(shell),
                );
            }
        }
        root
    }
}

fn centered(child: impl IntoElement) -> AnyElement {
    div()
        .flex_1()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(child)
        .into_any_element()
}

/// One user message on the turn navigator.
struct Marker {
    index: usize,
    id: ItemId,
    label: String,
}

/// Master's home heading size (`HOME_HEADLINE_TEXT_PX`).
const HOME_HEADLINE_TEXT_PX: f32 = 28.0;
/// Master's muted emblem above it (`HOME_EMBLEM_SIZE_PX`).
const HOME_EMBLEM_SIZE_PX: f32 = 30.0;

impl Conversation {
    /// What sits above the new-thread composer (master's
    /// `new_thread_surface_section`): nothing while Forge is reachable;
    /// otherwise the emblem, a heading and what to do.
    fn new_thread_status(
        &self,
        host: &editor_core::HostId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let host = self.core.read(cx).host(host)?.clone();
        let status = host.read(cx).status.clone();
        let desktop = DesktopTheme::neutral_dark();
        let section = div()
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .pb(px(32.0))
            .debug_selector(|| "new-thread-status".to_owned())
            .child(
                div().mb(px(16.0)).child(
                    svg()
                        .path(AssetId::TABLER_TERMINAL_2.as_str())
                        .size(px(HOME_EMBLEM_SIZE_PX))
                        .text_color(desktop.secondary),
                ),
            );
        let heading = |text: &'static str| {
            div()
                .text_size(px(HOME_HEADLINE_TEXT_PX))
                .font_weight(FontWeight::NORMAL)
                .text_color(desktop.foreground)
                .child(text)
        };
        let reason = match status {
            ConnectionStatus::Live => return None,
            ConnectionStatus::Connecting => {
                return Some(
                    section
                        .child(heading("Reconnecting to Forge…"))
                        .into_any_element(),
                );
            }
            ConnectionStatus::Offline { reason, .. } => reason,
        };
        let refused = ["401", "403", "Unauthorized", "Forbidden"]
            .iter()
            .any(|code| reason.contains(code));
        let (title, detail) = if refused {
            (
                "Could not authenticate with Forge",
                "The saved connection credential is no longer usable. Restart Forge on the host, then retry here, or add a fresh host invitation.",
            )
        } else {
            (
                "Forge is offline",
                "The editor could not connect to Forge. Existing project and task data will remain available when it reconnects.",
            )
        };
        Some(
            section
                .child(heading(title))
                .child(
                    div()
                        .mt(px(8.0))
                        .max_w(px(440.0))
                        .text_size(px(15.0))
                        .text_color(desktop.secondary)
                        .child(detail)
                        .child(div().mt(px(8.0)).text_size(px(12.0)).child(reason)),
                )
                .child(
                    div()
                        .id("retry-forge-connection")
                        .debug_selector(|| "retry-forge-connection".to_owned())
                        .cursor_pointer()
                        .mt(px(16.0))
                        .px(px(16.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(desktop.line)
                        .text_color(desktop.foreground)
                        .child("Retry connection")
                        .on_click(move |_, _, cx| host.read(cx).retry_now()),
                )
                .into_any_element(),
        )
    }

    /// Moves the list toward its end each frame while the reader follows.
    /// It runs in paint, after the list's layout, so this frame's growth is
    /// in the end it glides to; frames keep coming until it settles.
    ///
    /// It also reads ahead: whenever the view's top comes within a few rows
    /// of the oldest loaded one and older items exist, the next page loads.
    /// Waiting for the reader's scroll left a short transcript on screen
    /// that pages then landed under, moving it each time.
    fn follow_driver(
        &self,
        following: bool,
        reduce_motion: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (follow, list) = (Rc::clone(&self.follow), self.list.clone());
        let more = self.agent.is_none()
            && self
                .view
                .as_ref()
                .is_some_and(|view| !view.window().exhausted && !view.window().in_flight)
            // A failed page waits for the reader to scroll again.
            && self.load_error.is_none();
        let this = cx.entity().downgrade();
        let frame_started = Rc::clone(&self.frame_started);
        let rows = self.rows.len();
        canvas(
            |_, _, _| {},
            move |_, (), window, cx| {
                // Render to here is the transcript's build and layout.
                if let Some(started) = frame_started.take() {
                    let ms = started.elapsed().as_millis();
                    if ms >= SLOW_FRAME_MS {
                        tracing::info!(ms, rows, "slow transcript frame");
                    }
                }
                if follow.borrow_mut().drive(&list, following, reduce_motion) {
                    window.request_animation_frame();
                }
                if more && wants_older(&list) {
                    let this = this.clone();
                    cx.defer(move |cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.viewport(vec![Input::TopReached], cx);
                        });
                    });
                }
            },
        )
        .absolute()
        .size_0()
    }

    /// One list entry: the row's view, drawn from cache at its measured
    /// height unless its content changed or its reveal is running.
    fn row_element(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.row_views.get(index).cloned() else {
            return div().into_any_element();
        };
        let (measured, animating) = {
            let view: &RowView = row.read(cx);
            (view.measured, view.animating)
        };
        match measured {
            Some(height) if !animating => row
                .cached(StyleRefinement::default().w_full().h(height))
                .into_any_element(),
            _ => row.into_any_element(),
        }
    }

    /// The round glass chip with a chevron, above the composer.
    fn jump_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let this = cx.entity().downgrade();
        let Ok(label) = AccessibleLabel::new("Jump to latest") else {
            return div().into_any_element();
        };
        let Ok(button) = Button::new(
            "jump-latest",
            self.focus.clone(),
            theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::IconSmall,
            ButtonContent::icon_only(AssetId::TABLER_CHEVRON_DOWN, label),
        ) else {
            return div().into_any_element();
        };
        let radius = px(16.0);
        div()
            .relative()
            .size(px(32.0))
            .rounded_full()
            .overflow_hidden()
            .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
            .bg(glass_foreground_base(&theme))
            .shadow(glass_card_shadows())
            .child(glass_material_layer(GlassStrength::Quiet, radius))
            .child(glass_highlight_layer(GlassStrength::Quiet, radius))
            .child(button.corner_radius(radius).on_activate(move |_, _, cx| {
                let _ = this.update(cx, Conversation::jump_to_latest);
            }))
            .into_any_element()
    }

    /// The user messages in the window, in order, as navigator markers.
    fn markers(&self) -> Vec<Marker> {
        let Some(view) = self.view.as_ref() else {
            return Vec::new();
        };
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(index, (row, _))| {
                let Row::User(id) = row else {
                    return None;
                };
                let ItemKind::UserMessage { text, .. } = &view.item(id)?.kind else {
                    return None;
                };
                let label = text
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .unwrap_or("Image")
                    .to_owned();
                Some(Marker {
                    index,
                    id: id.clone(),
                    label,
                })
            })
            .collect()
    }

    /// The turn navigator (`render_turn_navigator`): one tick per user
    /// message on the right edge, the one in view drawn long and bright;
    /// hovering opens the glass menu of their first lines.
    #[allow(clippy::cast_precision_loss)]
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder keeps the rail, its ticks and their tick motion in visual order"
    )]
    fn navigator(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let markers = self.markers();
        if markers.len() < 2 {
            self.navigator_hovered = false;
            return None;
        }
        let theme = self.theme;
        let top = self.list.logical_scroll_top().item_ix;
        let active = markers
            .iter()
            .rev()
            .find(|marker| marker.index <= top)
            .or(markers.first())
            .map(|marker| marker.index);
        let viewport_height = f32::from(self.list.viewport_bounds().size.height)
            .max(f32::from(window.viewport_size().height) * 0.5);
        let count = markers.len() as f32;
        let capped = |content: f32| content.min(viewport_height * NAVIGATOR_HEIGHT_CAP);
        let rail_height = capped(NAVIGATOR_PAD_PX * 2.0 + count * NAVIGATOR_TICK_PITCH_PX);
        let reduce_motion = cx.reduce_motion();
        // A tick entering or leaving the active range eases between the two
        // looks; one seen for the first time starts at rest.
        let animate_ticks = !reduce_motion && self.navigator_had_active;
        self.navigator_had_active = active.is_some();
        let now = Instant::now();
        self.navigator_ticks
            .retain(|id, _| markers.iter().any(|marker| &marker.id == id));
        let mut ticks_moving = false;
        let mut ticks = div()
            .flex()
            .flex_col()
            .items_end()
            .w(px(NAVIGATOR_RAIL_WIDTH_PX))
            .h(px(rail_height))
            .p(px(NAVIGATOR_PAD_PX))
            .overflow_hidden();
        for marker in &markers {
            let current = active == Some(marker.index);
            let entry = self
                .navigator_ticks
                .entry(marker.id.clone())
                .or_insert((current, None));
            if entry.0 != current {
                *entry = (current, animate_ticks.then_some(now));
            }
            let toward = entry.1.map_or(1.0, |changed| {
                let elapsed = now.saturating_duration_since(changed).as_secs_f32();
                let total = MotionDuration::Fast.as_duration().as_secs_f32();
                smooth_out((elapsed / total).clamp(0.0, 1.0))
            });
            ticks_moving |= toward < 1.0;
            let presence = if current { toward } else { 1.0 - toward };
            let paint = blend_oklch(
                theme.colors.muted_foreground.with_alpha(0.5),
                theme.colors.foreground,
                presence,
            )
            .to_paint();
            let tick_width = 16.0 + 8.0 * presence;
            let index = marker.index;
            let this = cx.entity().downgrade();
            ticks = ticks.child(
                div()
                    .id(SharedString::from(format!("navigator-tick-{index}")))
                    .cursor_pointer()
                    .w_full()
                    .h(px(NAVIGATOR_TICK_PITCH_PX))
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .on_click(move |_, _, cx| {
                        let _ = this.update(cx, |this, cx| this.navigate(index, cx));
                    })
                    .child(div().h(px(1.0)).w(px(tick_width)).rounded_full().bg(paint)),
            );
        }
        if ticks_moving {
            window.request_animation_frame();
        }
        let mut region = div()
            .id("turn-navigator")
            .flex()
            .flex_row()
            .items_center()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.navigator_hovered != *hovered {
                    this.navigator_hovered = *hovered;
                    if !*hovered {
                        this.navigator_hover.borrow_mut().hide();
                    }
                    cx.notify();
                }
            }));
        if self.navigator_hovered {
            region = region.child(
                div()
                    .pr(px(NAVIGATOR_MENU_GAP_PX))
                    .child(self.navigator_menu(
                        &markers,
                        active,
                        viewport_height,
                        reduce_motion,
                        cx,
                    )),
            );
        }
        region = region.child(ticks);
        Some(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(NAVIGATOR_RAIL_INSET_PX))
                .flex()
                .flex_col()
                .justify_center()
                .child(region)
                .into_any_element(),
        )
    }

    /// The navigator's glass menu: the messages' first lines, with the
    /// shared hover pill flying between rows behind them.
    #[allow(clippy::cast_precision_loss)]
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the menu rows, hover pill, probes and entrance in visual order"
    )]
    fn navigator_menu(
        &self,
        markers: &[Marker],
        active: Option<usize>,
        viewport_height: f32,
        reduce_motion: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let count = markers.len() as f32;
        let content = NAVIGATOR_PAD_PX * 2.0
            + count * NAVIGATOR_MENU_ROW_PX
            + (count - 1.0).max(0.0) * NAVIGATOR_MENU_ROW_GAP_PX;
        let height = content.min(viewport_height * NAVIGATOR_HEIGHT_CAP);
        let ids: Vec<String> = markers.iter().map(|marker| marker.id.to_string()).collect();
        self.navigator_hover.borrow_mut().clear_if_missing(&ids);
        let surface_bounds = Rc::clone(&self.navigator_hover_surface);
        let mut rows = div()
            .id("turn-navigator-list")
            .relative()
            .flex()
            .flex_col()
            .gap(px(NAVIGATOR_MENU_ROW_GAP_PX))
            .p(px(NAVIGATOR_PAD_PX))
            .w_full()
            .h(px(height))
            .overflow_y_scroll()
            // The list is the hit target for the gaps between rows: a move
            // alone schedules no frame.
            .on_mouse_move(|_: &MouseMoveEvent, window, _| window.refresh())
            .child(sliding_hover_pill(
                "turn-navigator-hover",
                &theme,
                &self.navigator_hover,
                RadiusTokens::value(RadiusStep::Lg),
                false,
                reduce_motion,
            ))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), _, _| {
                        *surface_bounds.borrow_mut() = Some(bounds);
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
        for marker in markers {
            let index = marker.index;
            let this = cx.entity().downgrade();
            let row_id = marker.id.to_string();
            let (move_hover, move_id) = (Rc::clone(&self.navigator_hover), row_id.clone());
            let (probe_hover, probe_surface, probe_id) = (
                Rc::clone(&self.navigator_hover),
                Rc::clone(&self.navigator_hover_surface),
                row_id,
            );
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("navigator-row-{index}")))
                    .relative()
                    .cursor_pointer()
                    .w_full()
                    .h(px(NAVIGATOR_MENU_ROW_PX))
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .rounded(RadiusTokens::value(RadiusStep::Lg))
                    .px(px(12.0))
                    .on_mouse_move(move |_: &MouseMoveEvent, window, _| {
                        move_hover.borrow_mut().set_active(move_id.clone());
                        window.refresh();
                    })
                    .on_click(move |_, _, cx| {
                        let _ = this.update(cx, |this, cx| this.navigate(index, cx));
                    })
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(theme.typography.control_text)
                            .text_color(theme.colors.foreground.to_paint())
                            .when(active == Some(index), |label| {
                                label.font_weight(FontWeight::MEDIUM)
                            })
                            .child(marker.label.clone()),
                    )
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, (), window, cx| {
                                let Some(surface) = *probe_surface.borrow() else {
                                    return;
                                };
                                let rect = HoverRect::within(bounds, surface);
                                if probe_hover.borrow_mut().measure(&probe_id, rect) {
                                    window.defer(cx, |window, _| window.refresh());
                                }
                            },
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    ),
            );
        }
        let radius = RadiusTokens::value(RadiusStep::Xl);
        let card = div()
            .relative()
            .w(px(NAVIGATOR_MENU_WIDTH_PX))
            .rounded(radius)
            .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
            .bg(glass_foreground_base(&theme))
            .shadow(glass_card_shadows())
            .child(glass_material_layer(GlassStrength::Strong, radius))
            .child(glass_highlight_layer(GlassStrength::Strong, radius))
            .child(rows);
        if reduce_motion {
            return card.into_any_element();
        }
        card.opacity(0.0)
            .left(px(-NAVIGATOR_MENU_ENTRANCE_SHIFT_PX))
            .with_animation(
                "turn-navigator-menu-entrance",
                Animation::new(Duration::from_millis(NAVIGATOR_MENU_ENTRANCE_MS))
                    .with_easing(smooth_out),
                |card, progress| {
                    card.opacity(progress)
                        .left(px(-NAVIGATOR_MENU_ENTRANCE_SHIFT_PX * (1.0 - progress)))
                },
            )
            .into_any_element()
    }

    /// Scrolls the transcript to a user message picked on the navigator.
    fn navigate(&mut self, index: usize, cx: &mut Context<Self>) {
        let item = self
            .rows
            .get(index)
            .and_then(|(row, _)| row.anchor().cloned())
            .and_then(|id| self.view.as_ref()?.item(&id).cloned());
        if let Some(item) = item {
            self.jump_to(&item, cx);
        }
    }
}

/// The colour `fraction` of the way from `from` to `to`, blended in OKLCH
/// so the step looks even.
fn blend_oklch(from: Oklch, to: Oklch, fraction: f32) -> Oklch {
    let mix = |a: f32, b: f32| a + (b - a) * fraction;
    Oklch {
        l: mix(from.l, to.l),
        c: mix(from.c, to.c),
        h: mix(from.h, to.h),
        a: mix(from.a, to.a),
    }
}
