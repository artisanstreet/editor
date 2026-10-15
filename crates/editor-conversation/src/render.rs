//! The conversation frame owns the native list, composer and navigation:
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
use editor_ui::button::{Button, ButtonVariant};

use editor_ui::hover_card::{glass_handle, glass_navigator_card};
use editor_ui::motion::MotionDuration;
use editor_ui::motion::smooth_out;
use editor_ui::picker_motion::{HoverRect, sliding_hover_pill};
use editor_ui::theme::{ArtisanTheme, Oklch, RadiusStep, RadiusTokens};
use editor_ui::wordmark_loader::WordmarkLoader;
use gpui::{
    Animation, AnimationExt as _, AnyElement, AppContext as _, Context, FontWeight,
    InteractiveElement as _, IntoElement, MouseMoveEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, list,
    prelude::FluentBuilder as _, px, rgb, svg,
};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
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
const NAVIGATOR_MENU_ENTRANCE_MS: u64 = 150;
const NAVIGATOR_MENU_ENTRANCE_SHIFT_PX: f32 = 4.0;
/// Delay before the loading wordmark appears (the
/// `THREAD_SCREEN_LOADING_MARK_DELAY` did, there 1 s): a thread that opens
/// quickly shows no flash of it.
const LOADING_MARK_DELAY: std::time::Duration = std::time::Duration::from_millis(500);
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
        self.sync_reveal_admission(cx);
        if self.advance_disclosures(cx) {
            window.request_animation_frame();
        }
        // Judged on last frame's layout; the wordmark stays until it holds.
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
            // The composer and project tab identify the new thread.
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
                    centered(
                        WordmarkLoader::new("conversation-loading", theme)
                            .appear_after(LOADING_MARK_DELAY),
                    )
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
                    cx.processor(|this, index, window, cx| this.row_element(index, window, cx)),
                )
                .size_full();
                div()
                    .relative()
                    .size_full()
                    // Initial loading ends as soon as usable content arrives.
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
                            div().absolute().inset_0().child(centered(
                                WordmarkLoader::new("conversation-landing", theme)
                                    .appear_after(LOADING_MARK_DELAY),
                            )),
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
                    .child(self.transcript_observer(following, cx))
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
            if let Some(view) = &self.view {
                let thread = view.thread.clone();
                if let Some(host) = self.host(cx) {
                    let error = host
                        .read(cx)
                        .thread_summary(&thread)
                        .and_then(|summary| summary.error.clone());
                    if let Some(error) = error {
                        shell = shell.child(
                            editor_ui::page::notice(
                                &theme,
                                format!("Couldn't refresh questions and goals: {error}"),
                            )
                            .action(
                                Button::text("retry-thread-controls", theme, "Retry")
                                    .variant(ButtonVariant::Ghost)
                                    .on_activate(move |_, _, cx| {
                                        host.update(cx, |host, cx| {
                                            host.load_thread_summary(&thread, true, cx)
                                        });
                                    }),
                            ),
                        );
                    }
                }
            }
            shell = shell.child(
                div()
                    .relative()
                    .w_full()
                    .child(composer.clone())
                    .child(measure),
            );
            if let Screen::NewThread { host, .. } = &screen {
                // The new-thread composer card is centred in
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
#[derive(Clone)]
pub(crate) struct Marker {
    index: usize,
    id: ItemId,
    label: SharedString,
}

/// New-thread heading size.
const HOME_HEADLINE_TEXT_PX: f32 = 28.0;
/// New-thread emblem size.
const HOME_EMBLEM_SIZE_PX: f32 = 30.0;

impl Conversation {
    /// The heading above the new-thread composer (the
    /// `new_thread_surface_section`): nothing while Forge is reachable;
    /// otherwise the emblem, a heading and what to do.
    fn new_thread_status(
        &self,
        host: &editor_core::HostId,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let host = self.core.read(cx).host(host)?.clone();
        let status = host.read(cx).status.clone();
        let desktop = ArtisanTheme::default().workspace;
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
        let (reason, code) = match status {
            ConnectionStatus::Live => return None,
            ConnectionStatus::Connecting => {
                return Some(
                    section
                        .child(heading("Reconnecting to Forge…"))
                        .into_any_element(),
                );
            }
            ConnectionStatus::Offline { reason, code, .. } => (reason, code),
        };
        let refused = code == Some(artisan_api::ErrorCode::Unauthorized);
        let (title, detail) = if refused {
            (
                "Could not authenticate with Forge",
                "The paired credential was rejected. Add a fresh Forge invitation to pair this client again.",
            )
        } else if code == Some(artisan_api::ErrorCode::VersionMismatch) {
            (
                "Forge and Editor versions differ",
                "Update Forge or the Editor so both use the same protocol.",
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

    /// Read-ahead and frame timing run after native list layout.
    fn transcript_observer(&self, following: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let list = self.list.clone();
        // A collapsed latest page can be shorter than the native viewport.
        // Admit one bounded page of initial leeway before the reader scrolls.
        let last_body_ready = self
            .rows
            .iter()
            .rev()
            .find_map(|(row, _)| {
                matches!(
                    row,
                    Row::Reply(_)
                        | Row::ProseBlock { .. }
                        | Row::TextBlock { .. }
                        | Row::PlainBlock { .. }
                )
                .then(|| self.read_evidence(row, cx).is_some())
            })
            .unwrap_or(false);
        let initial_leeway = following
            && last_body_ready
            && self
                .view
                .as_ref()
                .is_some_and(|view| view.items.len() < crate::thread::PAGE as usize * 2);
        let more = (!following || initial_leeway)
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
            move |_, (), _window, cx| {
                // Render to here is the transcript's build and layout.
                if let Some(started) = frame_started.take() {
                    let ms = started.elapsed().as_millis();
                    if ms >= SLOW_FRAME_MS {
                        tracing::info!(ms, rows, "slow transcript frame");
                    }
                }
                if more
                    && ((!following && wants_older(&list))
                        || (initial_leeway && list.max_offset_for_scrollbar().y <= px(0.0)))
                {
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

    /// One visible native list entry. ListState alone owns its geometry.
    pub(super) fn row_element(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((definition, _)) = self.rows.get(index).cloned() else {
            return div().into_any_element();
        };
        // A zero-height queue must not force the native list to construct every
        // later source owner merely to fill its viewport. Keep those descriptors
        // cheap until the single active native frontier releases them.
        let group_deferred = !self
            .shared
            .disclosure_fractions
            .borrow()
            .contains_key(&definition.key())
            && definition
                .anchor()
                .is_some_and(|id| self.shared.disclosure_hidden.borrow().contains(id))
            && !self
                .shared
                .disclosure_clips
                .borrow()
                .contains_key(&definition.key());
        let bypass = matches!(
            definition.semantic(),
            Row::User(_)
                | Row::Work { .. }
                | Row::PlainBlock {
                    section: crate::fold::PlainSection::User,
                    ..
                }
        );
        let queue_deferred = !bypass
            && self
                .shared
                .reveal_queue
                .borrow()
                .frontier_key()
                .and_then(|key| self.row_context.key_rows.get(&key).copied())
                .is_some_and(|frontier| index > frontier);
        if queue_deferred {
            // A detached scroll can discover a new frontier during this native
            // layout pass, after structural admission ran. One temporary native
            // viewport fills the remainder, bounding that pass to one deferred
            // descriptor. The next root render removes it via the keyed splice;
            // it owns no source, measured row metric, or persistent tail height.
            return div()
                .h(window.viewport_size().height.max(px(1.0)))
                .into_any_element();
        }
        if group_deferred {
            return div().into_any_element();
        }
        let images = self.row_images(&definition, cx);
        if self.row_views.get(index).is_some_and(Option::is_none) {
            let context = &self.row_context;
            let model = self.model(
                &definition,
                definition
                    .run()
                    .and_then(|run| context.runs.get(run))
                    .cloned(),
                images.clone(),
                context.now,
                context.gaps.get(index).copied().unwrap_or_default(),
                if index + 1 == self.rows.len() {
                    context.end_space
                } else {
                    0.0
                },
                &context.agents,
            );
            let shared = self.shared.clone();
            let version = context.versions.get(index).copied().unwrap_or_default();
            self.row_views[index] = Some(cx.new(|_| RowView::new(model, version, shared)));
        }
        self.row_cache_order.retain(|cached| *cached != index);
        self.row_cache_order.push_back(index);
        while self.row_cache_order.len() > 128 {
            if let Some(oldest) = self.row_cache_order.pop_front() {
                self.row_views[oldest] = None;
            }
        }
        self.prune_literal_sources();
        let Some(row) = self.row_views.get(index).and_then(Clone::clone) else {
            return div().into_any_element();
        };
        row.update(cx, |row, cx| row.set_images(images, cx));
        let evidence_row = row.downgrade();
        let content = row.into_any_element();
        let observed = self
            .read_evidence(&definition, cx)
            .map(|seq| (self.open.clone(), definition.key(), seq));
        let source_row = matches!(
            definition.semantic(),
            Row::ProseBlock { .. } | Row::TextBlock { .. } | Row::PlainBlock { .. }
        )
        .then_some(definition);
        let this = cx.entity().downgrade();
        div()
            .relative()
            .w_full()
            .child(content)
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        let clip = window.content_mask().bounds;
                        let revealed = evidence_row
                            .upgrade()
                            .is_some_and(|row| row.read(cx).ready_to_read());
                        if clip.intersects(&bounds) && bounds.size.height > px(0.0) && revealed {
                            if let Some(source_row) = source_row {
                                let owner = this.clone();
                                cx.defer(move |cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        // Native ListState holds its mutable layout borrow while
                                        // building rows. Read its scroll state only after paint.
                                        if let Some((key, start)) =
                                            this.native_window_prefetch(&source_row)
                                        {
                                            let anchor = this
                                                .rows
                                                .get(this.list.logical_scroll_top().item_ix)
                                                .map(|(row, _)| row.key());
                                            if let Some(anchor) = anchor {
                                                if let Some(state) = this
                                                    .shared
                                                    .native_windows
                                                    .borrow_mut()
                                                    .get_mut(&key)
                                                {
                                                    state.frontier =
                                                        Some((anchor, start > state.window.start));
                                                }
                                            }
                                            this.native_window_edge(key, start, cx);
                                        } else if let Some((id, slot, revision, offset)) =
                                            this.source_window_prefetch(&source_row)
                                        {
                                            if let Some(anchor) = this
                                                .rows
                                                .get(this.list.logical_scroll_top().item_ix)
                                                .map(|(row, _)| row.key())
                                            {
                                                let (reference, newer) = {
                                                    let sources =
                                                        this.shared.stored_sources.borrow();
                                                    let source =
                                                        sources.get(&(id.clone(), slot)).unwrap();
                                                    (
                                                        source.source_ref().clone(),
                                                        offset > source.loaded_range().start,
                                                    )
                                                };
                                                this.shared.source_frontiers.borrow_mut().insert(
                                                    (id.clone(), slot),
                                                    (reference, anchor, newer),
                                                );
                                            }
                                            this.source_window_edge(id, slot, revision, offset, cx);
                                        }
                                    });
                                });
                            }
                        }
                        // The final line must actually paint inside the transcript's native clip.
                        if !window.is_window_active()
                            || bounds.size.height <= px(0.0)
                            || !revealed
                            || !clip.intersects(&bounds)
                            || bounds.bottom() > clip.bottom()
                            || bounds.bottom() <= clip.top()
                        {
                            return;
                        }
                        if let Some((target, key, seq)) = observed {
                            cx.defer(move |cx| {
                                let _ = this.update(cx, |this, cx| {
                                    if this.open != target {
                                        return;
                                    }
                                    let valid = this
                                        .rows
                                        .iter()
                                        .find(|(row, _)| row.key() == key)
                                        .is_some_and(|(row, _)| {
                                            this.read_evidence(row, cx) == Some(seq)
                                        });
                                    if valid {
                                        if let (Some(host), Some((_, thread))) =
                                            (this.host(cx), target)
                                        {
                                            host.update(cx, |host, cx| {
                                                host.acknowledge_read(thread, seq, cx)
                                            });
                                        }
                                    }
                                });
                            });
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }

    /// Read evidence belongs to complete current leaf content, independently of controls.
    fn native_window_prefetch(&self, row: &Row) -> Option<(crate::fold::RowKey, usize)> {
        let visual = row.visual()?;
        let key = row.semantic().key();
        let states = self.shared.native_windows.borrow();
        let state = states.get(&key)?;
        if !state.painted.contains(&visual) {
            return None;
        }
        let start = if state.window.start > 0
            && visual.saturating_sub(state.window.start) < 4
            && !self.list.is_following_tail()
            && self
                .view
                .as_ref()
                .is_some_and(|view| !view.viewport.state().is_following())
            && self
                .rows
                .get(self.list.logical_scroll_top().item_ix)
                .is_some_and(|(top, _)| top.key() == row.key())
        {
            state.window.start.saturating_sub(32)
        } else if state.window.end < state.count && state.window.end.saturating_sub(visual) <= 4 {
            state.window.start.saturating_add(32)
        } else {
            return None;
        };
        let top = self
            .rows
            .get(self.list.logical_scroll_top().item_ix)?
            .0
            .key();
        if reverses_at_same_anchor(
            state
                .frontier
                .as_ref()
                .map(|(anchor, newer)| (anchor, *newer)),
            &top,
            start > state.window.start,
        ) {
            return None;
        }
        Some((key, start))
    }

    fn source_window_prefetch(
        &self,
        row: &Row,
    ) -> Option<(artisan_api::ItemId, &'static str, u64, usize)> {
        if let Some(visual) = row.visual() {
            let states = self.shared.native_windows.borrow();
            let state = states.get(&row.semantic().key())?;
            if !state.painted.contains(&visual) || (visual != 0 && visual + 1 != state.count) {
                return None;
            }
        }
        let native_key = row.key();
        let row = row.semantic();
        let (id, slot, block) = match row {
            Row::ProseBlock { id, block } => (id, "prose", *block),
            Row::TextBlock {
                id, section, block, ..
            } => (id, section.slot(), *block),
            Row::PlainBlock { id, section, block } => (id, section.slot(), *block),
            _ => return None,
        };
        let sources = self.shared.stored_sources.borrow();
        let source = sources.get(&(id.clone(), slot))?;
        let loaded = source.loaded_range();
        if !loaded.contains(&block) {
            return None;
        }
        let body_ready = if source.format() == artisan_api::SourceIndexFormat::Literal {
            self.shared
                .literal_sources
                .borrow()
                .get(&(id.clone(), slot, block))
                .is_some_and(|entry| entry.leaf.source_ref == *source.source_ref())
        } else {
            let key = format!("{id}:{slot}:leaf:{block}").into();
            self.shared
                .markdown
                .is_stored_leaf_prepared(&key, source, block)
        };
        if !body_ready {
            return None;
        }
        // Overlap half of the old page so ListState's surviving RowKeys keep
        // their native geometry and scroll anchor when a source page shifts.
        let offset = if loaded.start > 0
            && !self.list.is_following_tail()
            && self
                .view
                .as_ref()
                .is_some_and(|view| !view.viewport.state().is_following())
            && block.saturating_sub(loaded.start) < 4
            && self
                .rows
                .get(self.list.logical_scroll_top().item_ix)
                .is_some_and(|(top, _)| top.key() == native_key)
        {
            loaded.start.saturating_sub(32)
        } else if loaded.end < source.block_count() && loaded.end.saturating_sub(block) <= 4 {
            loaded.start.saturating_add(32)
        } else {
            return None;
        };
        let top = self
            .rows
            .get(self.list.logical_scroll_top().item_ix)?
            .0
            .key();
        let frontiers = self.shared.source_frontiers.borrow();
        if let Some((reference, anchor, newer)) = frontiers.get(&(id.clone(), slot)) {
            if reference == source.source_ref()
                && reverses_at_same_anchor(Some((anchor, *newer)), &top, offset > loaded.start)
            {
                return None;
            }
        }
        Some((id.clone(), slot, source.revision(), offset))
    }

    pub(crate) fn read_evidence(&self, row: &Row, cx: &gpui::App) -> Option<artisan_api::Seq> {
        let visual = row.visual();
        let row = row.semantic();
        let view = self.view.as_ref()?;
        let id = row.anchor()?;
        let item = view.item(id)?;
        let stored = match row {
            Row::Reply(_) | Row::ProseBlock { .. } => matches!(
                &item.kind,
                ItemKind::AssistantMessage {
                    text: artisan_api::TextContent::Stored(_),
                    ..
                }
            ),
            Row::TextBlock { section, .. } => section
                .text(item)
                .is_some_and(|text| matches!(text, artisan_api::TextContent::Stored(_))),
            Row::PlainBlock { section, .. } => section
                .text(item)
                .is_some_and(|text| matches!(text, artisan_api::TextContent::Stored(_))),
            _ => false,
        };
        let current_live = item.status == artisan_api::Status::Running
            && view.live.contains_key(&artisan_api::LiveTarget::Text {
                item_id: id.clone(),
            })
            && matches!(row, Row::Reply(_) | Row::ProseBlock { .. });
        if stored && !current_live {
            let (slot, block, reference) = match row {
                Row::ProseBlock { block, .. } => {
                    let ItemKind::AssistantMessage {
                        text: artisan_api::TextContent::Stored(reference),
                        ..
                    } = &item.kind
                    else {
                        return None;
                    };
                    ("prose", *block, reference)
                }
                Row::TextBlock { section, block, .. } => {
                    let artisan_api::TextContent::Stored(reference) = section.text(item)? else {
                        return None;
                    };
                    (section.slot(), *block, reference)
                }
                Row::PlainBlock { section, block, .. } => {
                    let artisan_api::TextContent::Stored(reference) = section.text(item)? else {
                        return None;
                    };
                    (section.slot(), *block, reference)
                }
                _ => return None,
            };
            let sources = self.shared.stored_sources.borrow();
            let index = sources.get(&(id.clone(), slot))?;
            let key: gpui::SharedString = format!("{id}:{slot}:leaf:{block}").into();
            let literal = matches!(row, Row::PlainBlock { .. });
            let literal_sources = self.shared.literal_sources.borrow();
            let literal_source = literal_sources.get(&(id.clone(), slot, block));
            let projected = literal_source.is_some_and(|entry| entry.visible.len() > 16 * 1024);
            let native = self.shared.native_windows.borrow();
            let projection_complete = if projected {
                native.get(&row.key()).is_some_and(|state| {
                    let ordinal = visual.unwrap_or(0);
                    state.reference == *reference
                        && state.error.is_none()
                        && ordinal + 1 == state.count
                        && state.painted.contains(&ordinal)
                })
            } else {
                visual.is_none()
            };
            let complete = reference == index.source_ref()
                && projection_complete
                && block + 1 == index.block_count()
                && if literal {
                    index.format() == artisan_api::SourceIndexFormat::Literal
                        && literal_source.is_some_and(|entry| &entry.leaf.source_ref == reference)
                        && self
                            .shared
                            .markdown
                            .is_stored_literal_leaf_prepared(&key, index, block)
                } else {
                    index.format() == artisan_api::SourceIndexFormat::Markdown
                        && self
                            .shared
                            .markdown
                            .is_stored_leaf_prepared(&key, index, block)
                        && self
                            .shared
                            .markdown
                            .prepared_source(&key)
                            .is_some_and(|prepared| !prepared.is_preview())
                };
            return complete.then(|| {
                self.row_context
                    .read_boundary
                    .as_ref()
                    .filter(|(boundary, _)| boundary == id)
                    .map_or(item.seq, |(_, seq)| *seq)
            });
        }
        let complete = match row {
            Row::ProseBlock { block, .. } => {
                let key: gpui::SharedString = id.to_string().into();
                let current = if item.status == artisan_api::Status::Running {
                    view.live
                        .get(&artisan_api::LiveTarget::Text {
                            item_id: id.clone(),
                        })
                        .map(|source| {
                            let covered = self.host(cx).is_some_and(|host| {
                                host.read(cx)
                                    .live_snapshot(
                                        &view.thread,
                                        &artisan_api::LiveTarget::Text {
                                            item_id: id.clone(),
                                        },
                                    )
                                    .is_some_and(|snapshot| {
                                        std::sync::Arc::ptr_eq(&snapshot.text, source)
                                            && snapshot.coverage.is_complete(source.len_bytes())
                                    })
                            });
                            covered && self.shared.markdown.is_rope_source_prepared(&key, source)
                        })
                } else {
                    None
                };
                let current = current.unwrap_or_else(|| {
                    self.shared
                        .sources
                        .borrow()
                        .get(&(id.clone(), "prose"))
                        .is_some_and(|(seq, source)| {
                            *seq == item.seq
                                && self.shared.markdown.is_source_prepared(&key, source)
                        })
                });
                current
                    && self
                        .shared
                        .markdown
                        .prepared_source(&key)
                        .is_some_and(|prepared| {
                            !prepared.is_preview() && *block + 1 == prepared.block_count()
                        })
            }
            Row::TextBlock { section, block, .. } => {
                let key: gpui::SharedString = format!("{id}:{}", section.slot()).into();
                self.shared
                    .sources
                    .borrow()
                    .get(&(id.clone(), section.slot()))
                    .is_some_and(|(seq, source)| {
                        *seq == item.seq && self.shared.markdown.is_source_prepared(&key, source)
                    })
                    && self
                        .shared
                        .markdown
                        .prepared_source(&key)
                        .is_some_and(|prepared| {
                            !prepared.is_preview() && *block + 1 == prepared.block_count()
                        })
            }
            Row::PlainBlock { section, block, .. }
                if *section != crate::fold::PlainSection::AgentTask =>
            {
                self.shared
                    .plain_sources
                    .borrow()
                    .get(&(id.clone(), section.slot()))
                    .is_some_and(|body| body.seq == item.seq && *block + 1 == body.chunks.len())
            }
            _ => false,
        };
        complete.then(|| {
            self.row_context
                .read_boundary
                .as_ref()
                .filter(|(id, _)| id == &item.id)
                .map_or(item.seq, |(_, seq)| *seq)
        })
    }

    /// The round glass chip with a chevron, above the composer.
    fn jump_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let this = cx.entity().downgrade();
        let button = Button::icon(
            "jump-latest",
            theme,
            AssetId::TABLER_CHEVRON_DOWN,
            "Jump to latest",
        )
        .variant(ButtonVariant::Ghost);
        glass_handle(
            &theme,
            button.on_activate(move |_, _, cx| {
                let _ = this.update(cx, Conversation::jump_to_latest);
            }),
        )
        .into_any_element()
    }

    /// The user messages in the window, in order, as navigator markers.
    fn markers(&mut self) -> Arc<[Marker]> {
        if let Some(markers) = &self.navigator_markers {
            return markers.clone();
        }
        let Some(view) = self.view.as_ref() else {
            return Arc::from([]);
        };
        let mut ids = HashSet::new();
        let mut markers = Vec::new();
        for (index, (row, _)) in self.rows.iter().enumerate() {
            let id = match row.semantic() {
                Row::User(id)
                | Row::PlainBlock {
                    id,
                    section: crate::fold::PlainSection::User,
                    ..
                } => id,
                _ => continue,
            };
            let Some(item) = view.item(id) else { continue };
            let ItemKind::UserMessage { text, .. } = &item.kind else {
                continue;
            };
            if !ids.insert(id.clone()) {
                continue;
            }
            let label = self.navigator_labels.entry(id.clone()).or_insert_with(|| {
                (
                    item.seq,
                    match text {
                        artisan_api::TextContent::Inline(text) => navigator_label(text),
                        artisan_api::TextContent::Stored(_) => "Stored message".into(),
                    },
                )
            });
            if label.0 != item.seq {
                *label = (
                    item.seq,
                    match text {
                        artisan_api::TextContent::Inline(text) => navigator_label(text),
                        artisan_api::TextContent::Stored(_) => "Stored message".into(),
                    },
                );
            }
            markers.push(Marker {
                index,
                id: id.clone(),
                label: label.1.clone(),
            });
        }
        self.navigator_labels.retain(|id, _| ids.contains(id));
        let markers: Arc<[Marker]> = markers.into();
        self.navigator_markers = Some(markers.clone());
        markers
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
        let tick_indices = navigator_tick_indices(
            markers.len(),
            ((rail_height - NAVIGATOR_PAD_PX * 2.0) / NAVIGATOR_TICK_PITCH_PX).floor() as usize,
            active.and_then(|index| markers.iter().position(|marker| marker.index == index)),
        );
        let tick_ids: HashSet<_> = tick_indices
            .iter()
            .map(|index| markers[*index].id.clone())
            .collect();
        self.navigator_ticks.retain(|id, _| tick_ids.contains(id));
        let mut ticks_moving = false;
        let mut ticks = div()
            .flex()
            .flex_col()
            .items_end()
            .w(px(NAVIGATOR_RAIL_WIDTH_PX))
            .h(px(rail_height))
            .p(px(NAVIGATOR_PAD_PX))
            .overflow_hidden();
        for tick_index in tick_indices {
            let marker = &markers[tick_index];
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
                        markers.clone(),
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

    /// Only visible native menu rows are constructed; the original shared
    /// hover pill flies behind their labels.
    fn navigator_menu(
        &self,
        markers: Arc<[Marker]>,
        active: Option<usize>,
        viewport_height: f32,
        reduce_motion: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let pitch = NAVIGATOR_MENU_ROW_PX + 4.0;
        let content_height = markers.len() as f32 * pitch - 4.0;
        let height = content_height.min(
            (viewport_height * NAVIGATOR_HEIGHT_CAP - NAVIGATOR_PAD_PX * 2.0)
                .max(NAVIGATOR_MENU_ROW_PX),
        );
        let this = cx.entity().downgrade();
        let hover = self.navigator_hover.clone();
        let surface = self.navigator_hover_surface.clone();
        let rows = gpui::uniform_list("turn-navigator-list", markers.len(), move |range, _, _| {
            range
                .map(|position| {
                    let marker = &markers[position];
                    let index = marker.index;
                    let this = this.clone();
                    let move_hover = hover.clone();
                    let move_id = marker.id.to_string();
                    let probe_hover = hover.clone();
                    let probe_surface = surface.clone();
                    let probe_id = move_id.clone();
                    div().h(px(pitch)).w_full().child(
                        div()
                            .id(SharedString::from(format!("navigator-row-{}", marker.id)))
                            .relative()
                            .cursor_pointer()
                            .w_full()
                            .h(px(NAVIGATOR_MENU_ROW_PX))
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
                                        if let Some(surface) = *probe_surface.borrow() {
                                            if probe_hover.borrow_mut().measure(
                                                &probe_id,
                                                HoverRect::within(bounds, surface),
                                            ) {
                                                window.defer(cx, |window, _| window.refresh());
                                            }
                                        }
                                    },
                                )
                                .absolute()
                                .inset_0(),
                            ),
                    )
                })
                .collect()
        })
        .h(px(height))
        .w_full();
        let surface = self.navigator_hover_surface.clone();
        let content = div()
            .relative()
            .p(px(NAVIGATOR_PAD_PX))
            .child(sliding_hover_pill(
                "turn-navigator-hover",
                &theme,
                &self.navigator_hover,
                RadiusTokens::value(RadiusStep::Lg),
                false,
                reduce_motion,
            ))
            .child(rows)
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), _, _| {
                        *surface.borrow_mut() = Some(bounds);
                    },
                )
                .absolute()
                .inset_0(),
            );
        let card = div()
            .w(px(NAVIGATOR_MENU_WIDTH_PX))
            .child(glass_navigator_card(&theme, content));
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
    Oklch::new(
        from.l + (to.l - from.l) * fraction,
        from.c + (to.c - from.c) * fraction,
        from.h + (to.h - from.h) * fraction,
    )
    .with_alpha(from.a + (to.a - from.a) * fraction)
}

/// Scan a bounded prefix and retain a bounded label, even for a giant unbroken message.
fn navigator_label(text: &str) -> SharedString {
    let prefix: String = text.chars().take(1024).collect();
    let line = prefix
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Image");
    let mut label: String = line.chars().take(160).collect();
    if line.chars().nth(160).is_some() {
        label.push('…');
    }
    label.into()
}

/// Sample the entire window into the available pixels, retaining the active turn.
fn navigator_tick_indices(count: usize, slots: usize, active: Option<usize>) -> Vec<usize> {
    let slots = slots.max(1).min(count);
    if count == 0 {
        return Vec::new();
    }
    if count <= slots {
        return (0..count).collect();
    }
    let mut indices: Vec<usize> = (0..slots)
        .map(|slot| {
            if slots == 1 {
                0
            } else {
                slot * (count - 1) / (slots - 1)
            }
        })
        .collect();
    if let Some(active) = active.filter(|active| *active < count) {
        if !indices.contains(&active) {
            let nearest = indices
                .iter()
                .enumerate()
                .min_by_key(|(_, index)| index.abs_diff(active))
                .unwrap()
                .0;
            indices[nearest] = active;
            indices.sort_unstable();
        }
    }
    indices
}

#[cfg(test)]
mod navigator_tests {
    use super::{navigator_label, navigator_tick_indices};
    #[test]
    fn rail_samples_entire_history_instead_of_clipping_tail_and_keeps_active() {
        let indices = navigator_tick_indices(10_000, 50, Some(6234));
        assert_eq!(indices.len(), 50);
        assert_eq!(indices[0], 0);
        assert_eq!(indices[49], 9999);
        assert!(indices.contains(&6234));
        assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(navigator_tick_indices(0, 0, None).is_empty());
    }
    #[test]
    fn labels_bound_giant_unicode_messages_and_use_first_nonempty_line() {
        assert_eq!(navigator_label("\n  \n  hello  \nsecond").as_ref(), "hello");
        let label = navigator_label(&"界".repeat(10_000));
        assert_eq!(label.chars().count(), 161);
        assert!(label.ends_with('…'));
    }
}

/// A page splice is not a reader movement. Keep its opposite edge from undoing
/// the fetch while the same canonical row remains at the top of the viewport.
fn reverses_at_same_anchor(
    previous: Option<(&crate::fold::RowKey, bool)>,
    anchor: &crate::fold::RowKey,
    newer: bool,
) -> bool {
    previous.is_some_and(|(old_anchor, old_newer)| old_anchor == anchor && old_newer != newer)
}
