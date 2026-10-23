//! The conversation frame owns the native list, composer and navigation:
//! full-height transcript list of cached row views on the black shell, the
//! composer floating over its bottom, the jump-to-latest chip above the
//! composer, and the turn navigator on the right edge.

use crate::fold::Row;
use crate::fold::Slot;
use crate::row::{COLUMN, GUTTER};
use crate::viewport::Input;
use crate::{Conversation, wants_older};
use artisan_api::{ItemId, ItemKind};
use editor_core::{ConnectionStatus, Screen};
use editor_ui::AssetId;
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::theme::ActiveTheme as _;

use editor_ui::hover_card::{glass_handle, glass_navigator_card};
use editor_ui::motion::MotionDuration;
use editor_ui::motion::smooth_out;
use editor_ui::picker_motion::{HoverRect, sliding_hover_pill};
use editor_ui::theme::{RadiusStep, RadiusTokens};
use editor_ui::wordmark_loader::WordmarkLoader;
use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, FontWeight, InteractiveElement as _,
    IntoElement, MouseMoveEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Window, canvas, div, list,
    prelude::FluentBuilder as _, px,
};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The composer's bottom inset (`pb-4`).
pub const COMPOSER_BOTTOM: f32 = 16.0;
/// The composer's bottom inset once the window is wide (`sm:pb-6`).
const COMPOSER_BOTTOM_WIDE: f32 = 24.0;
/// The width from which the wider inset applies (`sm`).
const WIDE_FROM: f32 = 640.0;
/// The air between the latest words and the composer card.
pub const COMPOSER_GAP: f32 = 24.0;

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
/// Earlier work this many rows above the view loads before the reader
/// reaches it.
const EARLIER_WORK_ROWS: usize = 8;

/// The least air between the navigator and the transcript's top edge or
/// the composer card's top.
const NAVIGATOR_EDGE_PX: f32 = 8.0;

impl Render for Conversation {
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the transcript, its overlays and the floating composer in visual order"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame_started.set(Some(std::time::Instant::now()));
        self.sync_reveal(cx);
        if self.advance_disclosures(cx) {
            window.request_animation_frame();
        }
        // The mode can flip between frames: rows and overlays read this.
        self.theme = *cx.theme();
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
            .text_color(theme.colors.text.to_paint())
            .font_family(theme.typography.sans.family);
        let mut following = true;
        let body: AnyElement = match (&screen, &self.view) {
            // The composer and project tab identify the new thread.
            (Screen::NewThread { .. }, _) => div().size_full().into_any_element(),
            (Screen::Thread { .. }, Some(view)) if self.rows.is_empty() => {
                let window_state = view.window();
                let loading = window_state.in_flight
                    || window_state.oldest.is_none() && !window_state.exhausted;
                match &self.load_error {
                    Some(error) => centered(
                        div()
                            .text_color(theme.colors.destructive_text.to_paint())
                            .child(error.clone()),
                    ),
                    None if loading => centered(
                        WordmarkLoader::new("conversation-loading", theme)
                            .appear_after(LOADING_MARK_DELAY),
                    ),
                    None => centered(
                        div()
                            .text_color(theme.colors.text_muted.to_paint())
                            .child("No messages yet."),
                    ),
                }
            }
            (Screen::Thread { .. }, Some(view)) => {
                following = view.viewport.state().is_following();
                let landing = self.landing();
                let arrival = self.arrival_offset();
                if arrival != 0.0 {
                    window.request_animation_frame();
                }
                let transcript =
                    list(self.list.clone(), cx.processor(Self::row_element)).size_full();
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
                    .children(self.scroll_edges())
                    .when_some(self.load_error.clone(), |this, error| {
                        this.child(
                            div()
                                .absolute()
                                .top(px(8.0))
                                .w_full()
                                .flex()
                                .justify_center()
                                .text_color(theme.colors.destructive_text.to_paint())
                                .child(error),
                        )
                    })
                    .children(self.navigator(bottom, window, cx))
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
                    let this = this;
                    cx.defer(move |cx| {
                        this.update(cx, |this, cx| this.composer_measured(height, cx))
                            .ok();
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
                    // Offline, the connection notice says why.
                    let error = host
                        .read(cx)
                        .thread_summary(&thread)
                        .and_then(|summary| summary.error.clone())
                        .filter(|_| host.read(cx).is_live());
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
                                            host.load_thread_summary(&thread, true, cx);
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
                // Until Forge is live the wordmark stands alone: a composer
                // that can't load a draft or a model only adds errors.
                let status = self.new_thread_status(host, cx);
                let card = div()
                    .relative()
                    .w_full()
                    .max_w(px(COLUMN))
                    .map(|card| match status {
                        Some(status) => card.child(status),
                        None => card.child(shell),
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
pub struct Marker {
    pub(crate) position: artisan_api::Seq,
    pub(crate) id: ItemId,
    pub(crate) label: SharedString,
}

/// How long one connection phase lasts before the line under the wordmark
/// fades in, as the Electron app did.
const REASSURANCE_DELAY: std::time::Duration = std::time::Duration::from_secs(5);
/// The line's fade-in.
const REASSURANCE_FADE: std::time::Duration = std::time::Duration::from_millis(500);
/// New-thread heading size.
const HOME_HEADLINE_TEXT_PX: f32 = 28.0;

impl Conversation {
    /// What stands in for the new-thread composer while Forge isn't live: the
    /// shimmering wordmark, and after [`REASSURANCE_DELAY`] in one phase a
    /// quiet line under it (with a way to start WSL when that's the cause).
    /// A rejected pairing or a version mismatch says so in words instead.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder keeps each connection phase's wordmark and words in visual order"
    )]
    fn new_thread_status(
        &mut self,
        host: &editor_core::HostId,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let host = self.core.read(cx).host(host)?.clone();
        let (status, wsl, reached) = {
            let host = host.read(cx);
            (
                host.status.clone(),
                host.wsl.clone(),
                host.forge_version.is_some(),
            )
        };
        let theme = *cx.theme();
        let desktop = theme.colors;
        let code = match &status {
            ConnectionStatus::Live => {
                self.waiting = None;
                return None;
            }
            ConnectionStatus::Connecting => None,
            ConnectionStatus::Offline { code, .. } => *code,
        };
        let failure = match code {
            Some(artisan_api::ErrorCode::Unauthorized) => Some((
                "Could not authenticate with Forge",
                "The paired credential was rejected. Add a fresh Forge invitation to pair this client again.",
            )),
            Some(artisan_api::ErrorCode::VersionMismatch) => Some((
                "Forge and Editor versions differ",
                "Update Forge or the Editor so both use the same protocol.",
            )),
            _ => None,
        };
        if let Some((title, detail)) = failure {
            self.waiting = None;
            return Some(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .text_center()
                    .debug_selector(|| "new-thread-status".to_owned())
                    .child(
                        div()
                            .text_size(px(HOME_HEADLINE_TEXT_PX))
                            .text_color(desktop.text.to_paint())
                            .child(title),
                    )
                    .child(
                        div()
                            .mt(px(8.0))
                            .max_w(px(440.0))
                            .text_size(px(15.0))
                            .text_color(desktop.text_muted.to_paint())
                            .child(detail),
                    )
                    .into_any_element(),
            );
        }

        // Re-entering a phase restarts the delay; WSL's state doesn't.
        let since = match &self.waiting {
            Some((phase, since)) if *phase == reached => *since,
            _ => {
                let since = std::time::Instant::now();
                self.waiting = Some((reached, since));
                self.waiting_generation = self.waiting_generation.wrapping_add(1);
                self.waiting_timer = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(REASSURANCE_DELAY).await;
                    this.update(cx, |_, cx| cx.notify()).ok();
                }));
                since
            }
        };
        let mark = WordmarkLoader::new("forge-connecting", theme).appear_after(LOADING_MARK_DELAY);
        let mut section = div()
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .gap(px(40.0))
            .debug_selector(|| "new-thread-status".to_owned())
            .child(mark);
        if since.elapsed() < REASSURANCE_DELAY {
            return Some(section.into_any_element());
        }

        let (detail, action): (String, Option<Button>) = match &wsl {
            Some(wsl) => {
                let start = {
                    let host = host;
                    move |_: &_, _: &mut Window, cx: &mut gpui::App| {
                        host.update(cx, editor_core::HostData::start_wsl);
                    }
                };
                let (detail, label) = if wsl.running {
                    (
                        format!("Forge isn't running in {}.", wsl.distro),
                        if wsl.starting {
                            "Starting Forge…".to_owned()
                        } else {
                            "Start Forge".to_owned()
                        },
                    )
                } else {
                    (
                        format!(
                            "WSL isn't running, so Forge in {} can't be reached.",
                            wsl.distro
                        ),
                        if wsl.starting {
                            format!("Starting {}…", wsl.distro)
                        } else {
                            format!("Start {}", wsl.distro)
                        },
                    )
                };
                (
                    detail,
                    Some(
                        Button::text("start-wsl-forge", theme, label)
                            .disabled(wsl.starting)
                            .debug_selector("start-wsl-forge")
                            .on_activate(start),
                    ),
                )
            }
            None if reached => ("Connection dropped — getting you back…".to_owned(), None),
            None => ("Connecting to Artisan Forge…".to_owned(), None),
        };
        let start_error = wsl.and_then(|wsl| wsl.error);
        let line = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.0))
            .max_w(px(440.0))
            .child(
                div()
                    .text_sm()
                    .text_color(desktop.text_muted.to_paint())
                    .child(
                        gpui::StyledText::new(format!(
                            "This is taking more time than expected… {detail}"
                        ))
                        .with_highlights([(
                            0.."This is taking more time than expected…".len(),
                            gpui::HighlightStyle {
                                font_weight: Some(FontWeight::MEDIUM),
                                ..Default::default()
                            },
                        )]),
                    ),
            )
            .children(start_error.map(|error| {
                div()
                    .text_sm()
                    .text_color(desktop.destructive_text.to_paint())
                    .child(format!("Couldn't start it: {error}"))
            }))
            .children(action);
        let line = if cx.reduce_motion() {
            line.into_any_element()
        } else {
            line.with_animation(
                ("reassurance", self.waiting_generation),
                Animation::new(REASSURANCE_FADE).with_easing(gpui::ease_in_out),
                gpui::Styled::opacity,
            )
            .into_any_element()
        };
        section = section.child(line);
        Some(section.into_any_element())
    }

    /// Read-ahead, earlier work, landing and frame timing run after native
    /// list layout: pages are rows now, so the transcript reads on until
    /// the view and the read-ahead above it are filled, wherever it is.
    #[expect(
        clippy::too_many_lines,
        reason = "one observer runs the post-layout steps in the order they must happen"
    )]
    fn transcript_observer(&self, following: bool, cx: &Context<Self>) -> impl IntoElement {
        let list = self.list.clone();
        // Bodies above the tail measure before more loads above them.
        let last_body_ready = self
            .revealed_rows()
            .iter()
            .rev()
            .find_map(|(row, _)| {
                matches!(row, Row::Text { .. }).then(|| self.read_evidence(row, cx).is_some())
            })
            .unwrap_or(false);
        let more = (!following || last_body_ready)
            && self
                .view
                .as_ref()
                .is_some_and(|view| !view.window().exhausted && !view.window().in_flight)
            // A failed page waits for the reader to scroll again.
            && self.load_error.is_none();
        // Open sections' unloaded work, by row, and whether a read is out.
        let earlier: Vec<(usize, artisan_api::Seq)> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, (row, _))| match row {
                Row::EarlierWork { first, .. } => Some((index, *first)),
                Row::Projected { .. }
                | Row::Work { .. }
                | Row::WorkEntry { .. }
                | Row::Chain { .. }
                | Row::Text { .. }
                | Row::Prompt(_)
                | Row::Compaction(_)
                | Row::Image(_)
                | Row::Gap
                | Row::Status(_)
                | Row::AgentResult(_)
                | Row::AgentStatus(..) => None,
            })
            .collect();
        let work_idle = self.work_reading.is_none() && self.load_error.is_none();
        let landing = self.landing.is_some();
        let settled = self
            .view
            .as_ref()
            .is_some_and(|view| view.window().loaded());
        // Rows not drawn yet, or whose text is still preparing: they must
        // not show at the placeholder's height.
        let preparing: Vec<usize> = if landing {
            (0..self.rows.len())
                .filter(|index| {
                    self.row_views
                        .get(*index)
                        .and_then(Option::as_ref)
                        .is_none_or(|view| view.read(cx).preparing())
                })
                .collect()
        } else {
            Vec::new()
        };
        let exhausted = self
            .view
            .as_ref()
            .is_none_or(|view| view.window().exhausted);
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
                let viewport = list.viewport_bounds();
                let near = |index: usize, reach: f32| {
                    list.bounds_for_item(index).is_some_and(|bounds| {
                        bounds.bottom() > viewport.top() - px(reach)
                            && bounds.top() < viewport.bottom() + px(reach)
                    })
                };
                let older = more && wants_older(&list);
                // The nearest earlier work under the view loads first: it
                // is what the reader meets scrolling up. The list places
                // only rows from its top one down, so work just above the
                // view counts by rows; it loads before it shows, and what
                // it loads goes in above the top row, which keeps its place.
                let top = list.logical_scroll_top().item_ix;
                let work = earlier
                    .iter()
                    .rev()
                    .find(|(index, _)| {
                        (*index < top && top - index <= EARLIER_WORK_ROWS)
                            || near(*index, crate::READ_AHEAD_PX)
                    })
                    .map(|(_, first)| *first);
                let ready = settled
                    && work_idle
                    && work.is_none()
                    && (exhausted || !wants_older(&list))
                    && !preparing.iter().any(|index| near(*index, 0.0));
                if older || (work_idle && work.is_some()) || (landing && ready) {
                    let this = this.clone();
                    cx.defer(move |cx| {
                        this.update(cx, |this, cx| {
                            if older {
                                this.viewport(vec![Input::TopReached], cx);
                            }
                            if let Some(first) = work.filter(|_| work_idle) {
                                this.load_work(first, cx);
                            }
                            if landing && ready {
                                this.landed(cx);
                            }
                        })
                        .ok();
                    });
                }
            },
        )
        .absolute()
        .size_0()
    }

    /// One visible native list entry. ListState alone owns its geometry.
    #[expect(
        clippy::too_many_lines,
        reason = "one row builder handles every row kind the list can show"
    )]
    pub(super) fn row_element(
        &mut self,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((definition, _)) = self.rows.get(index).cloned() else {
            return div().into_any_element();
        };
        // A disclosure's members wait, empty, until its flight clips them.
        let disclosure = self.row_disclosure(&definition);
        let group_deferred =
            disclosure.fraction.is_none() && disclosure.hidden && disclosure.clip.is_none();
        if self.behind_reveal(index, &definition) {
            // Waits, hidden, for the content before it to reveal; the last
            // row still keeps the end space under the transcript.
            let end = if index + 1 == self.rows.len() {
                self.row_context.end_space
            } else {
                0.0
            };
            return div().h(px(end)).into_any_element();
        }
        if group_deferred {
            return div().into_any_element();
        }
        if self.row_views.get(index).is_some_and(Option::is_none) {
            self.row_views[index] = Some(self.new_row_view(index, cx));
            self.touch_row_cache(index, true);
        } else {
            self.touch_row_cache(index, false);
        }
        let Some(row) = self.row_views.get(index).and_then(Clone::clone) else {
            return div().into_any_element();
        };
        let evidence_row = row.downgrade();
        // Drawn from cache at its measured height; fresh only while its
        // content changed or its motion runs.
        let content = match row.read(cx).cached_height() {
            Some(height) => row
                .cached(StyleRefinement::default().w_full().h(height))
                .into_any_element(),
            None => row.into_any_element(),
        };
        let observed = self
            .read_evidence(&definition, cx)
            .map(|seq| (self.open.clone(), definition.key(), seq));
        let source_row = matches!(definition.semantic(), Row::Text { .. }).then_some(definition);
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
                        if clip.intersects(&bounds)
                            && bounds.size.height > px(0.0)
                            && revealed
                            && let Some(source_row) = source_row
                        {
                            let owner = this.clone();
                            cx.defer(move |cx| {
                                owner
                                    .update(cx, |this, cx| {
                                        // Native ListState holds its mutable layout borrow while
                                        // building rows. Read its scroll state only after paint.
                                        if let Some((key, start)) =
                                            this.native_window_prefetch(&source_row)
                                        {
                                            let place = this.reader_place();
                                            if let Some(state) = this.bodies.native.get_mut(&key) {
                                                state.frontier =
                                                    Some((place, start > state.window.start));
                                            }
                                            this.native_window_edge(&key, start, cx);
                                        } else if let Some((id, slot, revision, offset)) =
                                            this.source_window_prefetch(&source_row)
                                        {
                                            let source = &this.bodies.stored[&(id.clone(), slot)];
                                            let (reference, newer) = (
                                                source.source_ref().clone(),
                                                offset > source.loaded_range().start,
                                            );
                                            this.bodies.frontiers.insert(
                                                (id.clone(), slot),
                                                (reference, this.reader_place(), newer),
                                            );
                                            this.source_window_edge(
                                                &id, slot, revision, offset, true, cx,
                                            );
                                        }
                                    })
                                    .ok();
                            });
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
                                this.update(cx, |this, cx| {
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
                                    if valid
                                        && let (Some(host), Some((_, thread))) =
                                            (this.host(cx), target)
                                    {
                                        host.update(cx, |host, cx| {
                                            host.acknowledge_read(thread, seq, cx);
                                        });
                                    }
                                })
                                .ok();
                            });
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }

    /// How far the reader has scrolled, and the row on top.
    pub(crate) fn reader_place(&self) -> crate::body::ReaderPlace {
        (
            self.reader_moves,
            self.rows
                .get(self.list.logical_scroll_top().item_ix)
                .map(|(row, _)| row.key()),
        )
    }

    /// Read evidence belongs to complete current leaf content, independently of controls.
    fn native_window_prefetch(&self, row: &Row) -> Option<(crate::fold::RowKey, usize)> {
        let visual = row.visual()?;
        let key = row.semantic().key();
        let state = self.bodies.native.get(&key)?;
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
        if reverses_in_place(
            state.frontier.as_ref(),
            &self.reader_place(),
            self.reader_toward_end,
            start > state.window.start,
        ) {
            return None;
        }
        Some((key, start))
    }

    fn source_window_prefetch(&self, row: &Row) -> Option<(artisan_api::ItemId, Slot, u64, usize)> {
        if let Some(visual) = row.visual() {
            let state = self.bodies.native.get(&row.semantic().key())?;
            if !state.painted.contains(&visual) || (visual != 0 && visual + 1 != state.count) {
                return None;
            }
        }
        let native_key = row.key();
        let row = row.semantic();
        let Row::Text {
            id, slot, block, ..
        } = row
        else {
            return None;
        };
        let (slot, block) = (*slot, *block);
        let source = self.bodies.stored.get(&(id.clone(), slot))?;
        let loaded = source.loaded_range();
        if !loaded.contains(&block) {
            return None;
        }
        let body_ready = if source.format() == artisan_api::SourceIndexFormat::Literal {
            self.bodies
                .literal
                .get(&(id.clone(), slot, block))
                .is_some_and(|entry| entry.leaf.source_ref == *source.source_ref())
        } else {
            let key = format!("{id}:{slot}:leaf:{block}").into();
            self.bodies
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
        if let Some((reference, place, newer)) = self.bodies.frontiers.get(&(id.clone(), slot))
            && reference == source.source_ref()
            && reverses_in_place(
                Some(&(place.clone(), *newer)),
                &self.reader_place(),
                self.reader_toward_end,
                offset > loaded.start,
            )
        {
            return None;
        }
        Some((id.clone(), slot, source.revision(), offset))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per row kind decides what reading it proves"
    )]
    pub(crate) fn read_evidence(&self, row: &Row, cx: &gpui::App) -> Option<artisan_api::Seq> {
        let visual = row.visual();
        let Row::Text {
            id, slot, block, ..
        } = row.semantic()
        else {
            return None;
        };
        let (slot, block) = (*slot, *block);
        let view = self.view.as_ref()?;
        let item = view.item(id)?;
        let text = match slot {
            Slot::Prose => match &item.kind {
                ItemKind::AssistantMessage { text, .. } => Some(text),
                ItemKind::UserMessage { .. }
                | ItemKind::Tool { .. }
                | ItemKind::Agent { .. }
                | ItemKind::Question { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => None,
            },
            slot @ (Slot::Plain(_)
            | Slot::Text(_)
            | Slot::Output
            | Slot::Answer
            | Slot::Preview) => slot.text(item),
        };
        let target = artisan_api::LiveTarget::Text {
            item_id: id.clone(),
        };
        let live = slot == Slot::Prose
            && item.status == artisan_api::Status::Running
            && view.live.contains_key(&target);
        let markdown = &self.bodies.markdown;
        let complete = match text {
            Some(artisan_api::TextContent::Stored(reference)) if !live => {
                let index = self.bodies.stored.get(&(id.clone(), slot))?;
                let key: gpui::SharedString = format!("{id}:{slot}:leaf:{block}").into();
                let literal = self.bodies.literal.get(&(id.clone(), slot, block));
                let projected = literal.is_some_and(|entry| entry.visible.len() > 16 * 1024);
                let projection_complete = if projected {
                    self.bodies
                        .native
                        .get(&row.semantic().key())
                        .is_some_and(|state| {
                            let ordinal = visual.unwrap_or(0);
                            state.reference == *reference
                                && state.error.is_none()
                                && ordinal + 1 == state.count
                                && state.painted.contains(&ordinal)
                        })
                } else {
                    visual.is_none()
                };
                reference == index.source_ref()
                    && projection_complete
                    && block + 1 == index.block_count()
                    && if slot.literal() {
                        index.format() == artisan_api::SourceIndexFormat::Literal
                            && literal.is_some_and(|entry| {
                                &entry.leaf.source_ref == reference
                                    && crate::body::same_leaf(index, block, &entry.leaf)
                            })
                    } else {
                        index.format() == artisan_api::SourceIndexFormat::Markdown
                            && markdown.is_stored_leaf_prepared(&key, index, block)
                            && markdown
                                .prepared_source(&key)
                                .is_some_and(|prepared| !prepared.is_preview())
                    }
            }
            _ if slot == Slot::Prose => {
                let key: gpui::SharedString = id.to_string().into();
                let current = if live {
                    view.live.get(&target).is_some_and(|source| {
                        let covered = self.host(cx).is_some_and(|host| {
                            host.read(cx)
                                .live_snapshot(&view.thread, &target)
                                .is_some_and(|snapshot| {
                                    std::sync::Arc::ptr_eq(&snapshot.text, source)
                                        && snapshot.coverage.is_complete(source.len_bytes())
                                })
                        });
                        covered && markdown.is_rope_source_prepared(&key, source)
                    })
                } else {
                    self.bodies.inline_prepared(id, slot, item.seq)
                };
                current
                    && markdown.prepared_source(&key).is_some_and(|prepared| {
                        !prepared.is_preview() && block + 1 == prepared.block_count()
                    })
            }
            _ if slot == Slot::Plain(crate::fold::PlainSection::AgentTask) => false,
            _ if slot.literal() => self
                .bodies
                .plain
                .get(&(id.clone(), slot))
                .is_some_and(|body| body.seq == item.seq && block + 1 == body.chunks.len()),
            _ => {
                let key: gpui::SharedString = format!("{id}:{slot}").into();
                self.bodies.inline_prepared(id, slot, item.seq)
                    && markdown.prepared_source(&key).is_some_and(|prepared| {
                        !prepared.is_preview() && block + 1 == prepared.block_count()
                    })
            }
        };
        complete.then(|| {
            self.row_context
                .read_boundary
                .as_ref()
                .filter(|(boundary, _)| boundary == id)
                .map_or(item.seq, |(_, seq)| *seq)
        })
    }

    /// The round glass chip with a chevron, above the composer.
    fn jump_button(&self, cx: &Context<Self>) -> AnyElement {
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
                this.update(cx, Self::jump_to_latest).ok();
            }),
        )
        .into_any_element()
    }

    /// The user's messages in the whole thread, in order, as Forge lists
    /// them: the navigator is a map of the thread, not of what is loaded.
    pub(crate) fn markers(&mut self, cx: &gpui::App) -> Arc<[Marker]> {
        let Some(summary) = self.host(cx).and_then(|host| {
            let thread = &self.view.as_ref()?.thread;
            host.read(cx).thread_summary(thread)?.value.clone()
        }) else {
            return Arc::from([]);
        };
        let source = Arc::as_ptr(&summary) as usize;
        if let Some((cached, markers)) = &self.navigator_markers
            && *cached == source
        {
            return Arc::clone(markers);
        }
        let markers: Arc<[Marker]> = summary
            .outline
            .iter()
            .map(|entry| Marker {
                position: entry.position,
                id: entry.id.clone(),
                label: entry.label.clone().into(),
            })
            .collect();
        self.navigator_markers = Some((source, Arc::clone(&markers)));
        markers
    }

    /// Where the reader is: the position of the item the top row shows, or
    /// of the nearest row above it that shows one.
    fn reading_position(&self) -> Option<artisan_api::Seq> {
        let view = self.view.as_ref()?;
        let top = self.list.logical_scroll_top().item_ix;
        self.rows
            .iter()
            .take(top + 1)
            .rev()
            .find_map(|(row, _)| view.item(row.anchor()?))
            .map(|item| item.position)
    }

    /// The transcript's scroll edges: one under the header, shown as the
    /// transcript scrolls away from its start, and one on the window's
    /// bottom edge, shown while later content runs past it.
    fn scroll_edges(&self) -> [AnyElement; 2] {
        use editor_ui::scroll_edge::{Edge, hidden_above, hidden_below, scroll_edge};
        let color = self.theme.colors.background;
        let (above, below) = (self.list.clone(), self.list.clone());
        [
            scroll_edge(Edge::Top, color, &self.edges[0], move || {
                hidden_above(&above)
            })
            .top_0()
            .into_any_element(),
            scroll_edge(Edge::Bottom, color, &self.edges[1], move || {
                hidden_below(&below)
            })
            .bottom_0()
            .into_any_element(),
        ]
    }

    /// The turn navigator (`render_turn_navigator`): one tick per user
    /// message on the right edge, the one in view drawn long and bright;
    /// hovering opens the glass menu of their first lines.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a thread's turn count is far inside f32's exact integer range"
    )]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the tick slot count saturates: a rail shorter than its padding fits none"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder keeps the rail, its ticks and their tick motion in visual order"
    )]
    fn navigator(
        &mut self,
        composer_bottom: f32,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let markers = self.markers(cx);
        if markers.len() < 2 {
            self.navigator_hovered = false;
            return None;
        }
        let theme = self.theme;
        let reading = self.reading_position();
        let active = markers
            .iter()
            .rev()
            .find(|marker| reading.is_some_and(|reading| marker.position <= reading))
            .or_else(|| markers.first())
            .map(|marker| marker.position);
        let viewport_height = f32::from(self.list.viewport_bounds().size.height)
            .max(f32::from(window.viewport_size().height) * 0.5);
        // Centred in the stretch above the floating composer card, never
        // reaching under it.
        let above = if self.composer.is_some() && self.agent.is_none() {
            self.composer_height + composer_bottom
        } else {
            0.0
        } + NAVIGATOR_EDGE_PX;
        let room = (viewport_height - above - NAVIGATOR_EDGE_PX).max(NAVIGATOR_MENU_ROW_PX);
        let max_height = room.min(viewport_height * NAVIGATOR_HEIGHT_CAP);
        let count = markers.len() as f32;
        let capped = |content: f32| content.min(max_height);
        let rail_height = capped(count.mul_add(NAVIGATOR_TICK_PITCH_PX, NAVIGATOR_PAD_PX * 2.0));
        let reduce_motion = cx.reduce_motion();
        // A tick entering or leaving the active range eases between the two
        // looks; one seen for the first time starts at rest.
        let animate_ticks = !reduce_motion && self.navigator_had_active;
        self.navigator_had_active = active.is_some();
        let now = Instant::now();
        let active_index = active.and_then(|position| {
            markers
                .iter()
                .position(|marker| marker.position == position)
        });
        let tick_indices = navigator_tick_indices(
            markers.len(),
            (NAVIGATOR_PAD_PX.mul_add(-2.0, rail_height) / NAVIGATOR_TICK_PITCH_PX).floor()
                as usize,
            active_index,
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
            let current = active == Some(marker.position);
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
            // At rest the tick is the old muted foreground at half alpha
            // over the transcript: gray 7.
            let paint = theme
                .colors
                .gray
                .step(7)
                .animate_to(theme.colors.text, presence)
                .to_paint();
            let tick_width = 8.0f32.mul_add(presence, 16.0);
            let position = marker.position;
            let this = cx.entity().downgrade();
            ticks = ticks.child(
                div()
                    .id(SharedString::from(format!("navigator-tick-{}", marker.id)))
                    .cursor_pointer()
                    .w_full()
                    .h(px(NAVIGATOR_TICK_PITCH_PX))
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| this.navigate(position, cx)).ok();
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
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if this.navigator_hovered != *hovered {
                    this.navigator_hovered = *hovered;
                    if *hovered {
                        // A fresh handle: the menu opens on the turn in view.
                        this.navigator_scroll = gpui::UniformListScrollHandle::new();
                        if let Some(index) = active_index {
                            this.navigator_scroll
                                .scroll_to_item(index, gpui::ScrollStrategy::Center);
                        }
                    } else {
                        this.navigator_hover.borrow_mut().hide();
                    }
                    cx.notify();
                }
            }));
        if self.navigator_hovered {
            region = region.child(
                div()
                    .pr(px(NAVIGATOR_MENU_GAP_PX))
                    .child(self.navigator_menu(markers, active, max_height, reduce_motion, cx)),
            );
        }
        region = region.child(ticks);
        Some(
            div()
                .absolute()
                .top(px(NAVIGATOR_EDGE_PX))
                .bottom(px(above))
                .right(px(NAVIGATOR_RAIL_INSET_PX))
                .flex()
                .flex_col()
                .justify_center()
                .child(region)
                .into_any_element(),
        )
    }

    /// Only visible native menu rows are constructed; the original shared
    /// hover pill flies behind their labels. Taller than `max_height`, the
    /// rows scroll inside the card.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder keeps the menu, its rows and the hover pill in visual order"
    )]
    fn navigator_menu(
        &self,
        markers: Arc<[Marker]>,
        active: Option<artisan_api::Seq>,
        max_height: f32,
        reduce_motion: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let pitch = NAVIGATOR_MENU_ROW_PX + 4.0;
        #[expect(
            clippy::cast_precision_loss,
            reason = "a thread's turn count is far inside f32's exact integer range"
        )]
        let content_height = (markers.len() as f32).mul_add(pitch, -4.0);
        let height = content_height.min(
            NAVIGATOR_PAD_PX
                .mul_add(-2.0, max_height)
                .max(NAVIGATOR_MENU_ROW_PX),
        );
        let this = cx.entity().downgrade();
        let hover = Rc::clone(&self.navigator_hover);
        let surface = self.navigator_hover_surface.clone();
        let rows = gpui::uniform_list("turn-navigator-list", markers.len(), move |range, _, _| {
            range
                .map(|position| {
                    let marker = &markers[position];
                    let index = marker.position;
                    let this = this.clone();
                    let move_hover = Rc::clone(&hover);
                    let move_id = marker.id.to_string();
                    let probe_hover = Rc::clone(&hover);
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
                                this.update(cx, |this, cx| this.navigate(index, cx)).ok();
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_size(theme.typography.control_text)
                                    .text_color(theme.colors.text.to_paint())
                                    .when(active == Some(index), |label| {
                                        label.font_weight(FontWeight::MEDIUM)
                                    })
                                    .child(marker.label.clone()),
                            )
                            .child(
                                canvas(
                                    |_, _, _| {},
                                    move |bounds, (), window, cx| {
                                        if let Some(surface) = probe_surface.get()
                                            && probe_hover.borrow_mut().measure(
                                                &probe_id,
                                                HoverRect::within(bounds, surface),
                                            )
                                        {
                                            window.defer(cx, |window, _| window.refresh());
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
        .track_scroll(&self.navigator_scroll)
        .h(px(height))
        .w_full();
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
            .child(self.navigator_hover_surface.record());
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

    /// Scrolls the transcript to a user message picked on the navigator,
    /// loading the stretch around it when it isn't.
    pub(crate) fn navigate(&mut self, position: artisan_api::Seq, cx: &mut Context<Self>) {
        self.jump_to_position(position, cx);
    }
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
    if let Some(active) = active.filter(|active| *active < count)
        && !indices.contains(&active)
        && let Some((nearest, _)) = indices
            .iter()
            .enumerate()
            .min_by_key(|(_, index)| index.abs_diff(active))
    {
        indices[nearest] = active;
        indices.sort_unstable();
    }
    indices
}

/// A page splice is not a reader movement. A page change is undone only
/// once the reader has scrolled on to another row, the way it undoes.
fn reverses_in_place(
    previous: Option<&(crate::body::ReaderPlace, bool)>,
    place: &crate::body::ReaderPlace,
    toward_end: Option<bool>,
    newer: bool,
) -> bool {
    previous.is_some_and(|((moves, row), old_newer)| {
        *old_newer != newer && (*moves == place.0 || *row == place.1 || toward_end != Some(newer))
    })
}

#[cfg(test)]
mod navigator_tests {
    use super::navigator_tick_indices;
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
}
