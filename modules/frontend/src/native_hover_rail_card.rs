//! Native GPUI thread hover card for the rail (grouped history list).
//!
//! Native counterpart of the settled-history half of
//! `routes/components/thread-hover-rail.svelte`: the `All threads` card that
//! lists settled threads under the exact `ThreadRailTimeGroups` labels
//! (`Yesterday`, `Last 3 days`, `Last 7 days`, `Past month`; the `Today`
//! group carries no label). The working/pinned card, proximity tracking,
//! hover-card travel, prefetch, and acknowledgement effects stay with the
//! orchestrator; this surface owns row hover, group labels, and click
//! emission.
//!
//! Grouping reuses [`crate::shell_rail_model::rail_list_model`] over borrowed
//! [`crate::shell_rail_model::RailThread`] rows, recency captions reuse
//! [`crate::thread_navigation_core::format_recent_thread_time`], and
//! attention facts reuse
//! [`crate::shell_rail_model::RailThread::needs_attention`], so labels,
//! order, and status semantics cannot drift from the audited policies.
//!
//! Fidelity mapping (legacy element → this module, Tailwind → Styled notes):
//!
//! - `section > h2` group labels (`px-2 pb-1 text-xs font-medium
//!   text-muted-foreground`) → same geometry/typography via theme label
//!   tokens; the `group_index === 0` top-inset rule maps onto per-section
//!   top padding for the first section only.
//! - Recent rows (`relative flex min-w-0 items-center gap-2 rounded-lg px-2
//!   py-2 text-sm font-medium`, `text-foreground` when active else
//!   `text-muted-foreground`, trailing `text-xs` time) → shared
//!   [`artisan_ui::list_row`] rail recipe with [`FontWeight::MEDIUM`] titles,
//!   [`ListRowTone`] by active state, and the formatted time as
//!   `trailing_caption`; the recipe's `--radius-lg` corners match
//!   `rounded-lg` exactly.
//! - Row hover (the `DropdownHoverSurface` sliding pill) → accent fill on the
//!   hovered row; the sliding-pill motion itself is an orchestrator concern
//!   (see [`crate::hover_pill_group_policy`]) and a documented gap.
//! - Pinned working card (`aria-label="Working"`, two-line rows with project
//!   subtitle and state dot) → the same rail recipe in its two-line shape
//!   with a state-dot trailing mark; the ten-row measured scroll cap
//!   (`VisibleWorkingRows`) is intentionally a bounded `max_h` scroll here
//!   because GPUI cannot measure painted row heights before layout.
//! - Row activation (thread links) → [`HoverRailAction::OpenThread`],
//!   drained once through
//!   [`NativeHoverRailCardState::take_pending_action`]; routing stays with
//!   the orchestrator.
//!
//! Deliberately absent: [`artisan_ui::button`] and [`artisan_ui::badge`]
//! have no counterpart in the legacy rail rows (state reads through the
//! state dot, not a badge); engine marks need asset-glyph plumbing owned
//! elsewhere, so rows keep their title/time structure without them.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::collections::HashMap;

use artisan_ui::{
    list_row::{
        ListRowContent, ListRowGeometry, ListRowSlots, ListRowStyle, ListRowTone, list_row,
    },
    theme::{ArtisanTheme, ThemeMode},
};
use gpui::{
    ClickEvent, Context, Div, FocusHandle, FontWeight, InteractiveElement as _, ParentElement as _,
    Render, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::{FluentBuilder as _, IntoElement},
    px,
};

use crate::shell_rail_model::{RailThread, rail_list_model};
use crate::thread_navigation_core::format_recent_thread_time;

/// Stable debug selector for the hover-rail card root.
pub const HOVER_RAIL_CARD_SELECTOR: &str = "artisan-native-hover-rail-card";
/// Prefix for the stable selectors painted on hover-rail rows.
pub const HOVER_RAIL_ROW_SELECTOR_PREFIX: &str = "artisan-native-hover-rail-row";
/// Prefix for the stable selectors painted on hover-rail groups.
pub const HOVER_RAIL_GROUP_SELECTOR_PREFIX: &str = "artisan-native-hover-rail-group";
/// Exact legacy working-card accessible name (`aria-label="Working"`).
pub const WORKING_GROUP_LABEL: &str = "Working";
/// Exact legacy history-card accessible name (`aria-label="All threads"`).
pub const HISTORY_GROUP_LABEL: &str = "All threads";
/// Bounded history-list height so long catalogs scroll (`VisibleWorkingRows`
/// measures this from painted rows in the legacy; see module docs).
const HISTORY_MAX_HEIGHT_PX: f32 = 480.0;
/// Bounded pinned-list height (ten rail rows at the recipe's line rhythm).
const PINNED_MAX_HEIGHT_PX: f32 = 400.0;

/// One thread row as the hover card paints it.
///
/// Mirrors [`RailThread`] with owned strings so the card can retain the
/// snapshot the orchestrator pushes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverRailThread {
    /// Durable thread identity used for hover and activation.
    pub thread_id: String,
    /// Already-resolved display title.
    pub title: String,
    /// Primary project's display name, when the thread has one.
    pub project_name: Option<String>,
    /// Exact live status string.
    pub live_status: String,
    /// Whether the reader has acknowledged the latest activity.
    pub settled: bool,
    /// Whether the thread reports the exact awaiting-answer status.
    pub awaiting_answer: bool,
    /// Resolved recency stamp, signed Unix epoch milliseconds.
    pub activity_ms: i64,
}

impl HoverRailThread {
    /// Builds one hover-card row from already-resolved adapter values.
    #[must_use]
    pub fn new(
        thread_id: impl Into<String>,
        title: impl Into<String>,
        project_name: Option<String>,
        live_status: impl Into<String>,
        settled: bool,
        awaiting_answer: bool,
        activity_ms: i64,
    ) -> Self {
        Self {
            thread_id: thread_id.into(),
            title: title.into(),
            project_name,
            live_status: live_status.into(),
            settled,
            awaiting_answer,
            activity_ms,
        }
    }
}

/// One action emitted after a hover-rail row has been activated.
///
/// The orchestrator consumes this action and owns all routing and transport
/// choreography. The card never routes a thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HoverRailAction {
    /// Request that the orchestrator open the listed thread.
    OpenThread {
        /// Durable thread identity of the activated row.
        thread_id: String,
    },
}

/// One non-empty settled time group as catalog indexes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverRailGroup {
    /// Exact legacy group label, if the group renders one (`Today` has
    /// none).
    pub label: Option<&'static str>,
    /// Catalog indexes of the group's rows, newest first.
    pub threads: Vec<usize>,
}

/// The hover-card split into its pinned card and settled time groups.
///
/// Indexes address [`NativeHoverRailCardState::threads`]; both collections
/// are newest-first, matching [`rail_list_model`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HoverRailSnapshot {
    /// Active and attention-needing catalog indexes, newest first.
    pub pinned: Vec<usize>,
    /// Non-empty settled groups in chronological partition order.
    pub groups: Vec<HoverRailGroup>,
}

impl HoverRailSnapshot {
    /// Returns whether the snapshot carries any row at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pinned.is_empty() && self.groups.is_empty()
    }

    /// Returns the number of rows across the pinned card and all groups.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pinned.len()
            + self
                .groups
                .iter()
                .map(|group| group.threads.len())
                .sum::<usize>()
    }
}

/// Explicit presentation state for [`NativeHoverRailCard`].
///
/// Pure and deterministic: the clock enters only through [`Self::set_now_ms`],
/// and effects leave only through [`Self::take_pending_action`], so the
/// whole contract is assertable without a window.
#[derive(Debug)]
pub struct NativeHoverRailCardState {
    threads: Vec<HoverRailThread>,
    now_ms: i64,
    active_thread: Option<String>,
    hovered: Option<String>,
    suppressed: bool,
    pending: Option<HoverRailAction>,
}

impl NativeHoverRailCardState {
    /// Builds card state over one thread snapshot and clock sample.
    #[must_use]
    pub fn new(threads: Vec<HoverRailThread>, now_ms: i64) -> Self {
        Self {
            threads,
            now_ms,
            active_thread: None,
            hovered: None,
            suppressed: false,
            pending: None,
        }
    }

    /// Replaces the thread snapshot. Hover survives only while its row still
    /// exists; a pending activation survives only while its thread does.
    pub fn replace_threads(&mut self, threads: Vec<HoverRailThread>) {
        self.threads = threads;
        if let Some(hovered) = self.hovered.take()
            && self.contains_thread(&hovered)
        {
            self.hovered = Some(hovered);
        }
        if let Some(HoverRailAction::OpenThread { thread_id }) = self.pending.take()
            && self.contains_thread(&thread_id)
        {
            self.pending = Some(HoverRailAction::OpenThread { thread_id });
        }
    }

    /// Returns the retained snapshot in catalog order.
    #[must_use]
    pub fn threads(&self) -> &[HoverRailThread] {
        &self.threads
    }

    /// Returns the retained clock sample.
    #[must_use]
    pub const fn now_ms(&self) -> i64 {
        self.now_ms
    }

    /// Publishes a fresh clock sample (controller push path).
    pub fn set_now_ms(&mut self, now_ms: i64) {
        self.now_ms = now_ms;
    }

    /// Points the card at the thread on screen, if the reader is on one.
    pub fn set_active_thread(&mut self, thread_id: Option<String>) {
        self.active_thread = thread_id;
    }

    /// Returns the active thread identity, if one is set.
    #[must_use]
    pub fn active_thread(&self) -> Option<&str> {
        self.active_thread.as_deref()
    }

    /// Sets the suppression flag owned by overlapping menus or viewers.
    /// Suppressing clears hover; it never fabricates an action.
    pub fn set_suppressed(&mut self, suppressed: bool) {
        self.suppressed = suppressed;
        if suppressed {
            self.hovered = None;
        }
    }

    /// Returns whether the card currently stands down.
    #[must_use]
    pub const fn is_suppressed(&self) -> bool {
        self.suppressed
    }

    /// Returns the hovered thread identity, if a row owns hover.
    #[must_use]
    pub fn hovered(&self) -> Option<&str> {
        self.hovered.as_deref()
    }

    /// Moves hover onto one row. Unknown rows and suppression are ignored;
    /// returns whether hover moved.
    pub fn hover_row(&mut self, thread_id: &str) -> bool {
        if self.suppressed || !self.contains_thread(thread_id) {
            return false;
        }
        let moved = self.hovered.as_deref() != Some(thread_id);
        self.hovered = Some(thread_id.to_owned());
        moved
    }

    /// Releases row hover without emitting.
    pub fn leave(&mut self) {
        self.hovered = None;
    }

    /// Activates one row from a pointer press or keyboard commit.
    ///
    /// At most one activation waits unconsumed (single emission): further
    /// activations return `None` until [`Self::take_pending_action`] drains
    /// it. Suppression and unknown rows emit nothing.
    pub fn activate_row(&mut self, thread_id: &str) -> Option<HoverRailAction> {
        if self.suppressed || self.pending.is_some() || !self.contains_thread(thread_id) {
            return None;
        }
        let action = HoverRailAction::OpenThread {
            thread_id: thread_id.to_owned(),
        };
        self.pending = Some(action.clone());
        Some(action)
    }

    /// Activates the hovered row, if one owns hover.
    pub fn activate_hovered(&mut self) -> Option<HoverRailAction> {
        let hovered = self.hovered.clone()?;
        self.activate_row(&hovered)
    }

    /// Returns and clears the one action waiting for orchestrator
    /// observation.
    pub fn take_pending_action(&mut self) -> Option<HoverRailAction> {
        self.pending.take()
    }

    /// Returns the pending action without consuming it.
    #[must_use]
    pub fn pending_action(&self) -> Option<&HoverRailAction> {
        self.pending.as_ref()
    }

    /// Returns the number of retained rows.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.threads.len()
    }

    /// Returns whether any row is retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }

    /// Projects the retained snapshot through [`rail_list_model`].
    ///
    /// Thread identities are domain-unique, so each grouped row maps back to
    /// exactly one catalog index.
    #[must_use]
    pub fn snapshot(&self) -> HoverRailSnapshot {
        let rails: Vec<RailThread> = self
            .threads
            .iter()
            .map(|thread| {
                RailThread::new(
                    thread.thread_id.as_str(),
                    thread.title.as_str(),
                    thread.project_name.as_deref(),
                    thread.live_status.as_str(),
                    thread.settled,
                    thread.awaiting_answer,
                    thread.activity_ms,
                )
            })
            .collect();
        let model = rail_list_model(&rails, self.now_ms);
        let index_of: HashMap<&str, usize> = self
            .threads
            .iter()
            .enumerate()
            .map(|(index, thread)| (thread.thread_id.as_str(), index))
            .collect();
        let resolve = |thread: &RailThread<'_>| index_of.get(thread.thread_id).copied();
        HoverRailSnapshot {
            pinned: model
                .pinned
                .iter()
                .filter_map(|thread| resolve(thread))
                .collect(),
            groups: model
                .groups
                .iter()
                .map(|group| HoverRailGroup {
                    label: group.label(),
                    threads: group
                        .threads
                        .iter()
                        .filter_map(|thread| resolve(thread))
                        .collect(),
                })
                .collect(),
        }
    }

    /// Formats one row's recency caption through the shared policy.
    #[must_use]
    pub fn time_caption(&self, thread: &HoverRailThread) -> String {
        format_recent_thread_time(thread.activity_ms, self.now_ms)
    }

    fn contains_thread(&self, thread_id: &str) -> bool {
        self.threads
            .iter()
            .any(|thread| thread.thread_id == thread_id)
    }
}

/// A real GPUI hover card over [`NativeHoverRailCardState`].
pub struct NativeHoverRailCard {
    state: NativeHoverRailCardState,
    theme: ArtisanTheme,
    focus: FocusHandle,
}

impl NativeHoverRailCard {
    /// Builds the card over one thread snapshot and clock sample.
    pub fn new(
        threads: Vec<HoverRailThread>,
        now_ms: i64,
        mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            state: NativeHoverRailCardState::new(threads, now_ms),
            theme: ArtisanTheme::for_mode(mode),
            focus: cx.focus_handle(),
        }
    }

    /// Read-only access to the presentation state.
    #[must_use]
    pub fn state(&self) -> &NativeHoverRailCardState {
        &self.state
    }

    /// The card's focus handle for orchestrator focus management.
    #[must_use]
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Replaces the retained thread snapshot.
    pub fn replace_threads(&mut self, threads: Vec<HoverRailThread>, cx: &mut Context<Self>) {
        self.state.replace_threads(threads);
        cx.notify();
    }

    /// Publishes a fresh clock sample.
    pub fn set_now_ms(&mut self, now_ms: i64, cx: &mut Context<Self>) {
        self.state.set_now_ms(now_ms);
        cx.notify();
    }

    /// Points the card at the thread on screen.
    pub fn set_active_thread(&mut self, thread_id: Option<String>, cx: &mut Context<Self>) {
        self.state.set_active_thread(thread_id);
        cx.notify();
    }

    /// Sets the suppression flag owned by overlapping surfaces.
    pub fn set_suppressed(&mut self, suppressed: bool, cx: &mut Context<Self>) {
        self.state.set_suppressed(suppressed);
        cx.notify();
    }

    /// Consumes the one pending open-thread action.
    pub fn take_pending_action(&mut self) -> Option<HoverRailAction> {
        self.state.take_pending_action()
    }

    fn hover_row(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        if self.state.hover_row(thread_id) {
            cx.notify();
        }
    }

    fn choose_row(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        self.state.activate_row(thread_id);
        cx.notify();
    }

    fn render_row(&self, index: usize, working: bool, cx: &Context<Self>) -> Stateful<Div> {
        let thread = &self.state.threads()[index];
        let active = self
            .state
            .active_thread()
            .is_some_and(|active| active == thread.thread_id);
        let hovered = self
            .state
            .hovered()
            .is_some_and(|hovered| hovered == thread.thread_id);
        let tone = if working || active {
            ListRowTone::Foreground
        } else {
            ListRowTone::Muted
        };
        let weight = if working {
            FontWeight::NORMAL
        } else {
            FontWeight::MEDIUM
        };
        let style = ListRowStyle::resolve(self.theme, ListRowGeometry::Rail, tone, weight);
        let caption = SharedString::from(self.state.time_caption(thread));
        let content = if working {
            ListRowContent::two_line(
                SharedString::from(thread.title.clone()),
                SharedString::from(
                    thread
                        .project_name
                        .clone()
                        .unwrap_or_else(|| String::from("No project")),
                ),
            )
        } else {
            ListRowContent::one_line(SharedString::from(thread.title.clone()))
        };
        let mut slots = ListRowSlots::new().trailing_caption(caption);
        if working {
            let dot = self.state_dot(thread);
            slots = slots.trailing(dot);
        }
        let selector = format!("{HOVER_RAIL_ROW_SELECTOR_PREFIX}-{index}");
        let thread_id = thread.thread_id.clone();
        let hover_id = thread_id.clone();
        list_row(style, content, slots)
            .id(SharedString::from(format!("hover-rail-row-{index}")))
            .debug_selector(move || selector.clone())
            .on_hover(cx.listener(move |view: &mut Self, hovered: &bool, _, cx| {
                if *hovered {
                    view.hover_row(&hover_id, cx);
                }
            }))
            .on_click(cx.listener(move |view: &mut Self, _: &ClickEvent, _, cx| {
                view.choose_row(&thread_id, cx);
            }))
            .when(hovered, |row| row.bg(self.theme.colors.accent.to_paint()))
    }

    fn state_dot(&self, thread: &HoverRailThread) -> Div {
        let rails = RailThread::new(
            thread.thread_id.as_str(),
            thread.title.as_str(),
            thread.project_name.as_deref(),
            thread.live_status.as_str(),
            thread.settled,
            thread.awaiting_answer,
            thread.activity_ms,
        );
        let tone: Option<gpui::Hsla> = if thread.awaiting_answer {
            Some(self.theme.colors.primary.to_paint())
        } else if rails.needs_attention() {
            Some(self.theme.colors.destructive.to_paint())
        } else {
            None
        };
        let mut dot = div().size(px(6.0)).flex_shrink_0().rounded_full();
        if let Some(tone) = tone {
            dot = dot.bg(tone);
        }
        dot
    }

    fn render_group_label(label: &str, first: bool, theme: &ArtisanTheme) -> Div {
        div()
            .px(px(8.0))
            .pb(px(4.0))
            .when(first, |heading| heading.pt(px(8.0)))
            .text_size(theme.typography.label_text)
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(label.to_owned())
    }
}

impl Render for NativeHoverRailCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let snapshot = self.state.snapshot();
        let popover = theme.colors.popover.to_paint();
        let border = theme.colors.border.to_paint();
        let mut card = div()
            .id("native-hover-rail-card")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .p(px(4.0))
            .rounded(px(12.0))
            .bg(popover)
            .border_1()
            .border_color(border)
            .debug_selector(|| HOVER_RAIL_CARD_SELECTOR.to_owned());

        if !snapshot.pinned.is_empty() {
            let mut pinned = div()
                .id("native-hover-rail-card-pinned")
                .flex()
                .flex_col()
                .w_full()
                .min_h(px(0.0))
                .max_h(px(PINNED_MAX_HEIGHT_PX))
                .overflow_y_scroll()
                .debug_selector(|| format!("{HOVER_RAIL_CARD_SELECTOR}-pinned"));
            pinned = pinned.child(
                div()
                    .px(px(8.0))
                    .pt(px(8.0))
                    .pb(px(4.0))
                    .text_size(theme.typography.label_text)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(WORKING_GROUP_LABEL),
            );
            for index in &snapshot.pinned {
                pinned = pinned.child(self.render_row(*index, true, cx));
            }
            card = card.child(pinned);
        }

        let mut history = div()
            .id("native-hover-rail-card-history")
            .flex()
            .flex_col()
            .w_full()
            .min_h(px(0.0))
            .max_h(px(HISTORY_MAX_HEIGHT_PX))
            .overflow_y_scroll()
            .debug_selector(|| format!("{HOVER_RAIL_CARD_SELECTOR}-history"));
        if snapshot.groups.is_empty() && snapshot.pinned.is_empty() {
            history = history.child(
                div()
                    .px(px(8.0))
                    .py(px(8.0))
                    .text_size(theme.typography.control_text)
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child("No threads yet."),
            );
        } else {
            for (group_index, group) in snapshot.groups.iter().enumerate() {
                let section_selector = format!("{HOVER_RAIL_GROUP_SELECTOR_PREFIX}-{group_index}");
                let mut section = div()
                    .flex()
                    .flex_col()
                    .debug_selector(move || section_selector.clone());
                if let Some(label) = group.label {
                    section =
                        section.child(Self::render_group_label(label, group_index == 0, &theme));
                }
                for index in &group.threads {
                    section = section.child(self.render_row(*index, false, cx));
                }
                history = history.child(section);
            }
        }
        card = card.child(history);

        card.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_rail_model::RailTimeGroupId;

    const HOUR_MS: i64 = 3_600_000;
    const DAY_MS: i64 = 24 * HOUR_MS;

    fn row(id: &str, status: &str, settled: bool, activity_ms: i64) -> HoverRailThread {
        HoverRailThread::new(id, id, None, status, settled, false, activity_ms)
    }

    #[test]
    fn snapshot_reuses_pinned_and_time_group_partition() {
        let now_ms = 10 * DAY_MS;
        let threads = vec![
            row("running", "Running", true, now_ms - HOUR_MS),
            row("today", "Idle", true, now_ms - HOUR_MS),
            row("yesterday", "Idle", true, now_ms - DAY_MS),
            row("month", "Idle", true, now_ms - 30 * DAY_MS),
        ];
        let state = NativeHoverRailCardState::new(threads, now_ms);
        let snapshot = state.snapshot();
        assert_eq!(snapshot.pinned.len(), 1);
        assert_eq!(state.threads()[snapshot.pinned[0]].thread_id, "running");
        let labels: Vec<Option<&str>> = snapshot.groups.iter().map(|group| group.label).collect();
        assert_eq!(labels, vec![None, Some("Yesterday"), Some("Past month")]);
        assert_eq!(snapshot.len(), 4);
        assert!(!snapshot.is_empty());
    }

    #[test]
    fn row_hover_tracks_known_rows_and_clears_on_leave() {
        let mut state = NativeHoverRailCardState::new(vec![row("a", "Idle", true, 0)], 0);
        assert!(state.hover_row("a"));
        assert_eq!(state.hovered(), Some("a"));
        assert!(!state.hover_row("a"));
        assert!(!state.hover_row("missing"));
        state.leave();
        assert!(state.hovered().is_none());
    }

    #[test]
    fn suppression_clears_hover_and_refuses_activation() {
        let mut state = NativeHoverRailCardState::new(vec![row("a", "Idle", true, 0)], 0);
        assert!(state.hover_row("a"));
        state.set_suppressed(true);
        assert!(state.is_suppressed());
        assert!(state.hovered().is_none());
        assert!(!state.hover_row("a"));
        assert!(state.activate_row("a").is_none());
        state.set_suppressed(false);
        assert!(state.activate_row("a").is_some());
    }

    #[test]
    fn click_emits_open_thread_exactly_once_with_typed_identity() {
        let mut state = NativeHoverRailCardState::new(vec![row("a", "Idle", true, 0)], 0);
        assert_eq!(
            state.activate_row("a"),
            Some(HoverRailAction::OpenThread {
                thread_id: String::from("a")
            })
        );
        assert!(state.activate_row("a").is_none());
        assert!(state.activate_hovered().is_none());
        assert_eq!(
            state.pending_action(),
            Some(&HoverRailAction::OpenThread {
                thread_id: String::from("a")
            })
        );
        assert_eq!(
            state.take_pending_action(),
            Some(HoverRailAction::OpenThread {
                thread_id: String::from("a")
            })
        );
        assert!(state.take_pending_action().is_none());
        assert!(state.activate_row("missing").is_none());
    }

    #[test]
    fn hovered_activation_opens_the_hovered_row() {
        let mut state = NativeHoverRailCardState::new(
            vec![row("a", "Idle", true, 0), row("b", "Idle", true, 0)],
            0,
        );
        assert!(state.hover_row("b"));
        assert_eq!(
            state.activate_hovered(),
            Some(HoverRailAction::OpenThread {
                thread_id: String::from("b")
            })
        );
    }

    #[test]
    fn replace_threads_retires_hover_and_pending_for_gone_rows() {
        let mut state = NativeHoverRailCardState::new(vec![row("a", "Idle", true, 0)], 0);
        assert!(state.hover_row("a"));
        assert!(state.activate_row("a").is_some());
        state.replace_threads(vec![row("b", "Idle", true, 0)]);
        assert!(state.hovered().is_none());
        assert!(state.pending_action().is_none());
        assert_eq!(state.row_count(), 1);
        assert!(!state.is_empty());
        state.replace_threads(Vec::new());
        assert!(state.snapshot().is_empty());
    }

    #[test]
    fn group_labels_match_legacy_strings() {
        assert_eq!(RailTimeGroupId::Yesterday.label(), Some("Yesterday"));
        assert_eq!(WORKING_GROUP_LABEL, "Working");
        assert_eq!(HISTORY_GROUP_LABEL, "All threads");
    }
}
