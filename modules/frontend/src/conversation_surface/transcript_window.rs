//! Transcript row windowing for [`ConversationSurface`].
//!
//! The transcript's rows are turns with genuinely variable heights (user
//! bubbles, Markdown prose, cards, work groups), so GPUI's `uniform_list` is
//! not applicable, and swapping the surface's `ScrollArea`/`ScrollAnchor`
//! machinery for GPUI's sum-tree `List` would rewrite selection, scroll
//! targets, navigator geometry, and end-space measurement at once. Instead
//! this module windows the existing children in place, using what the surface
//! already has:
//!
//! - Every turn keeps exactly one transcript child. In-window turns build
//!   their real element subtree; off-window turns build a placeholder box
//!   carrying their remembered height. Because the child count and order are
//!   unchanged, the existing prepaint listeners keep their one-child-per-turn
//!   indexing for end space, navigator geometry, and scroll-target handoff.
//! - Heights are recorded from the same prepaint listener that measures the
//!   children and are remembered per stable turn id, so a row is measured
//!   once, not per frame. Rows never seen yet use a bounded running estimate;
//!   the mismatch only shifts estimated content height until the row enters
//!   the window and is measured.
//! - The build window comes from
//!   [`plan_transcript_window`](super::render_budget::plan_transcript_window)
//!   (viewport + bounded overscan + hard row cap), with one addition: the
//!   FIFO head of the pending scroll-target queue is force-built when it lies
//!   outside the window, so a navigator or host scroll to a far turn still
//!   resolves instead of being dropped as an unknown anchor. At most one row
//!   is force-built per frame, matching the one-target-per-frame drain.
//! - Markdown shaping is budgeted per row through
//!   [`Self::render_budgeted_markdown`]; only built rows reach the shaper.
//!
//! Scroll offsets, scroll targets, selection, navigator markers, and the send
//! flow remain owned by the existing paths; this module never writes the
//! scroll handle and never mutates the scene.

use std::collections::HashSet;
use std::ops::Range;

use super::render_budget::{
    TRANSCRIPT_MAX_ROW_HEIGHT_PX, TRANSCRIPT_MIN_ROW_HEIGHT_PX, TRANSCRIPT_TURN_GAP_PX,
    TRANSCRIPT_UNMEASURED_ROW_HEIGHT_PX, plan_transcript_window, transcript_markdown_within_budget,
};
use super::*;

/// Stable debug-selector suffix for a placeholder standing in for one turn.
///
/// Tests use this to prove an off-window turn was not built as a real row.
#[must_use]
pub(super) fn turn_placeholder_selector(turn_id: &TurnId) -> String {
    format!("{}-placeholder", turn_selector(turn_id))
}

/// One frame's built turn rows.
///
/// `primary` is the contiguous viewport-derived window; `forced` is at most
/// one extra out-of-window row built so the FIFO head of the pending
/// scroll-target queue has a painted anchor to resolve against. Together they
/// keep the per-frame built set bounded by the render budget.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TranscriptBuildWindow {
    /// Contiguous window derived from the viewport and the row budget.
    pub(super) primary: Range<usize>,
    /// One additional turn built outside `primary` for the scroll-target head.
    pub(super) forced: Option<usize>,
    /// Whether the row budget trimmed the requested primary window.
    pub(super) capped: bool,
}

impl TranscriptBuildWindow {
    /// Whether one turn index builds its real element subtree this frame.
    #[must_use]
    pub(super) fn contains(&self, index: usize) -> bool {
        self.primary.contains(&index) || self.forced == Some(index)
    }

    /// Every turn index built this frame, in scene order.
    pub(super) fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.primary
            .clone()
            .chain(self.forced.filter(|index| !self.primary.contains(index)))
    }
}

/// Render-local diagnostics from the latest transcript build pass.
///
/// Overwritten by each render pass. Tests assert windowing and shaping
/// budgets from this report instead of timing frames.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TranscriptWindowReport {
    /// Turn rows in the accepted scene.
    pub total_rows: usize,
    /// Start of the primary build window (inclusive).
    pub window_start: usize,
    /// End of the primary build window (exclusive).
    pub window_end: usize,
    /// One out-of-window row built for the pending scroll-target head.
    pub forced_row: Option<usize>,
    /// Whether the primary window hit the row cap.
    pub capped: bool,
    /// Indices whose real element subtrees were built, in scene order.
    pub built_rows: Vec<usize>,
    /// Markdown rows shaped this frame.
    pub shaped_rows: usize,
    /// Markdown source bytes shaped this frame.
    pub shaped_bytes: usize,
    /// Rows that exceeded the per-row Markdown shaping budget.
    pub over_budget_rows: usize,
}

/// Per-frame Markdown shaping ledger, reset by each render pass.
///
/// Exposed separately from [`TranscriptWindowReport`] so a test can exercise
/// the budgeted shaper directly without pumping a frame in between.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TranscriptShapeLedger {
    /// Markdown rows shaped since the frame started.
    pub rows: usize,
    /// Markdown source bytes shaped since the frame started.
    pub bytes: usize,
    /// Rows that exceeded the per-row budget and took the plain fallback.
    pub over_budget_rows: usize,
}

/// Measured turn heights plus cached prefix offsets for window planning.
///
/// Heights are remembered by stable turn identity so a scene replacement
/// (streaming appends, disclosure rebuilds) never re-measures rows the reader
/// already saw. Offsets are maintained incrementally: a measurement change
/// recomputes only the suffix behind it, and every frame plans from a binary
/// search instead of walking all turns.
#[derive(Default)]
pub(super) struct TranscriptHeightTable {
    /// Turn ids in the scene order the offsets were built for.
    order: Vec<String>,
    /// `offsets[i]` is the top of turn `i`; `offsets[rows]` the total height.
    offsets: Vec<f64>,
    /// Last measured height per turn identity, retained across replacements.
    measured: HashMap<String, f64>,
    /// Running sum/count of measured heights for unmeasured estimates.
    measured_sum: f64,
    measured_count: u32,
}

impl TranscriptHeightTable {
    /// Rebuilds the ordered offsets from one accepted scene.
    pub(super) fn scene_replaced(&mut self, scene: &ConversationScene) {
        let turns = scene.turn_scenes();
        self.order.clear();
        self.order
            .extend(turns.iter().map(|turn| turn.turn_id.as_str().to_owned()));
        let live: HashSet<&str> = turns.iter().map(|turn| turn.turn_id.as_str()).collect();
        self.measured.retain(|id, _| live.contains(id.as_str()));
        self.rebuild_offsets();
    }

    /// Rebuilds only when the scene order is not loaded yet.
    ///
    /// Scene replacements rebuild eagerly through [`Self::scene_replaced`],
    /// so a length mismatch here can only be the first frame after
    /// construction.
    fn ensure_current(&mut self, scene: &ConversationScene) {
        if self.order.len() != scene.turn_scenes().len() {
            self.scene_replaced(scene);
        }
    }

    /// Records one measured height for the turn at `index`.
    ///
    /// A measurement only rewrites offsets when it actually changed, so a
    /// steady frame does no offset work at all. Heights are clamped before
    /// they enter the running estimate.
    pub(super) fn record(&mut self, index: usize, height: f32) {
        if !height.is_finite() {
            return;
        }
        let Some(id) = self.order.get(index).cloned() else {
            return;
        };
        let clamped =
            f64::from(height).clamp(TRANSCRIPT_MIN_ROW_HEIGHT_PX, TRANSCRIPT_MAX_ROW_HEIGHT_PX);
        let previous = self.measured.get(&id).copied();
        if previous == Some(clamped) {
            return;
        }
        if let Some(previous) = previous {
            self.measured_sum += clamped - previous;
        } else {
            self.measured_sum += clamped;
            self.measured_count = self.measured_count.saturating_add(1);
        }
        self.measured.insert(id, clamped);
        self.recompute_from(index);
    }

    /// The planned height in px for one turn, measured or estimated.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "planner heights are stored as f64 but originate from GPUI's f32 pixel bounds and are clamped well below f32 precision limits"
    )]
    pub(super) fn height_px(&self, index: usize) -> f32 {
        let Some(id) = self.order.get(index) else {
            return TRANSCRIPT_UNMEASURED_ROW_HEIGHT_PX as f32;
        };
        self.height_for(id) as f32
    }

    /// The cached offsets: `offsets[i]` is the top of turn `i`.
    pub(super) fn offsets(&self) -> &[f64] {
        &self.offsets
    }

    /// The height for one turn identity, measured or estimated.
    fn height_for(&self, id: &str) -> f64 {
        self.measured
            .get(id)
            .copied()
            .unwrap_or_else(|| self.estimate())
    }

    /// The bounded running estimate for turns without a measurement.
    fn estimate(&self) -> f64 {
        if self.measured_count == 0 {
            return TRANSCRIPT_UNMEASURED_ROW_HEIGHT_PX;
        }
        let average = self.measured_sum / f64::from(self.measured_count);
        average.clamp(TRANSCRIPT_MIN_ROW_HEIGHT_PX, TRANSCRIPT_MAX_ROW_HEIGHT_PX)
    }

    /// Rebuilds every offset from the current order and measurements.
    fn rebuild_offsets(&mut self) {
        let rows = self.order.len();
        let mut offsets = Vec::with_capacity(rows + 1);
        offsets.push(0.0_f64);
        for position in 0..rows {
            let height = self.height_for(&self.order[position]);
            let gap = if position + 1 < rows {
                TRANSCRIPT_TURN_GAP_PX
            } else {
                0.0
            };
            let top = offsets[position];
            offsets.push(top + height + gap);
        }
        self.offsets = offsets;
    }

    /// Recomputes offsets behind one changed measurement.
    fn recompute_from(&mut self, index: usize) {
        if self.offsets.len() != self.order.len() + 1 {
            return;
        }
        for position in index..self.order.len() {
            let height = self.height_for(&self.order[position]);
            let gap = if position + 1 < self.order.len() {
                TRANSCRIPT_TURN_GAP_PX
            } else {
                0.0
            };
            let top = self.offsets[position];
            self.offsets[position + 1] = top + height + gap;
        }
    }
}

/// Surface-owned windowing state: remembered heights and frame diagnostics.
#[derive(Default)]
pub(super) struct TranscriptWindowState {
    table: TranscriptHeightTable,
    report: TranscriptWindowReport,
    shaped_rows: usize,
    shaped_bytes: usize,
    over_budget_rows: usize,
}

impl TranscriptWindowState {
    /// Rebuilds the height model for one replacement scene.
    pub(super) fn scene_replaced(&mut self, scene: &ConversationScene) {
        self.table.scene_replaced(scene);
    }

    /// Starts one render pass: loads heights if needed, resets the ledger.
    fn begin_frame(&mut self, scene: &ConversationScene) {
        self.table.ensure_current(scene);
        self.shaped_rows = 0;
        self.shaped_bytes = 0;
        self.over_budget_rows = 0;
    }

    /// Notes one Markdown row shaped (or skipped) this frame.
    fn record_shaping(&mut self, bytes: usize, within_budget: bool) {
        self.shaped_rows = self.shaped_rows.saturating_add(1);
        self.shaped_bytes = self.shaped_bytes.saturating_add(bytes);
        if !within_budget {
            self.over_budget_rows = self.over_budget_rows.saturating_add(1);
        }
    }

    /// Publishes the frame's window and shaping diagnostics.
    fn finish_frame(&mut self, built: &TranscriptBuildWindow, total_rows: usize) {
        self.report = TranscriptWindowReport {
            total_rows,
            window_start: built.primary.start,
            window_end: built.primary.end,
            forced_row: built.forced,
            capped: built.capped,
            built_rows: built.indices().collect(),
            shaped_rows: self.shaped_rows,
            shaped_bytes: self.shaped_bytes,
            over_budget_rows: self.over_budget_rows,
        };
    }

    /// The live shaping ledger, before the next frame resets it.
    fn shape_ledger(&self) -> TranscriptShapeLedger {
        TranscriptShapeLedger {
            rows: self.shaped_rows,
            bytes: self.shaped_bytes,
            over_budget_rows: self.over_budget_rows,
        }
    }
}

impl ConversationSurface {
    /// Returns the latest transcript windowing/shaping diagnostics.
    ///
    /// This is a test and review seam: the report is overwritten by every
    /// render pass and is never read by product code.
    #[must_use]
    pub fn transcript_window_report(&self) -> TranscriptWindowReport {
        self.transcript_window.borrow().report.clone()
    }

    /// Returns the live Markdown shaping ledger for the current frame.
    ///
    /// Unlike [`Self::transcript_window_report`] this is not reset until the
    /// next render, so a test can call the budgeted shaper directly and then
    /// read exactly what it shaped.
    #[must_use]
    pub fn transcript_shape_ledger(&self) -> TranscriptShapeLedger {
        self.transcript_window.borrow().shape_ledger()
    }

    /// Returns the Markdown parse-cache counters for this surface's renderer.
    ///
    /// Test and review seam: repeated renders of the same body must report one
    /// parse and a growing hit count, while a changed body reports a fresh
    /// parse. The cache stays inside its entry and byte bounds.
    #[must_use]
    pub fn markdown_parse_report(&self) -> MarkdownParseReport {
        self.markdown_renderer.parse_report()
    }

    /// Plans this frame's built rows from remembered heights and scroll state.
    ///
    /// The window is the viewport plus bounded overscan, capped by the row
    /// budget; the FIFO head of the pending scroll-target queue is forced in
    /// when it falls outside, so a scroll to a far turn still finds an anchor.
    pub(super) fn plan_transcript_build(&self) -> TranscriptBuildWindow {
        let mut state = self.transcript_window.borrow_mut();
        state.begin_frame(&self.scene);
        let offset_y = f64::from(self.scroll_handle.offset().y);
        let viewport_top = -offset_y;
        let viewport_height = f64::from(self.scroll_handle.bounds().size.height);
        let max_offset = f64::from(self.scroll_handle.max_offset().y).max(0.0);
        let at_bottom = max_offset > 0.0 && (max_offset + offset_y).abs() <= 1.0;
        let plan = plan_transcript_window(
            state.table.offsets(),
            viewport_top,
            viewport_height,
            at_bottom,
        );
        let forced = self
            .pending_scroll_targets
            .first()
            .and_then(|target| self.turn_index_for_target(target))
            .filter(|index| !plan.built.contains(index));
        TranscriptBuildWindow {
            primary: plan.built,
            forced,
            capped: plan.capped,
        }
    }

    /// Finds the turn owning one pending scroll target.
    ///
    /// Runs only while a scroll target waits, never per frame: it may walk
    /// block identities to resolve work-group and item aliases exactly like
    /// the painted-anchor handoff does.
    fn turn_index_for_target(&self, target: &ConversationSurfaceTarget) -> Option<usize> {
        self.scene.turn_scenes().iter().position(|turn| {
            let turn_identity = (SceneId::parse(turn.turn_id.as_str()).ok(), None::<ItemId>);
            scroll_target_matches_identity(target, &turn_identity)
                || turn.blocks().iter().any(|block| {
                    scroll_target_matches_identity(
                        target,
                        &block_scroll_identity(&turn.turn_id, block),
                    )
                })
        })
    }

    /// Builds the height placeholder standing in for one off-window turn.
    ///
    /// The placeholder keeps the turn's slot in the transcript flow so the
    /// content height (and therefore the scroll handle) stays consistent;
    /// its selector is the stable seam tests use to prove the real row was
    /// not built.
    pub(super) fn render_turn_placeholder(&self, index: usize) -> AnyElement {
        let height = self.transcript_window.borrow().table.height_px(index);
        let selector = self
            .scene
            .turn_scenes()
            .get(index)
            .map_or_else(String::new, |turn| turn_placeholder_selector(&turn.turn_id));
        div()
            .w_full()
            .h(px(height))
            .debug_selector(move || selector.clone())
            .into_any_element()
    }

    /// Publishes this frame's window diagnostics after the row loop.
    pub(super) fn finish_transcript_window(&self, built: &TranscriptBuildWindow) {
        self.transcript_window
            .borrow_mut()
            .finish_frame(built, self.scene.turn_scenes().len());
    }

    /// Shapes one Markdown body under the per-row byte budget.
    ///
    /// Within budget the shared renderer parses and shapes normally. Over
    /// budget the exact full text renders through the plain selectable leaf
    /// instead, so no frame parses an unbounded document; accepted scenes
    /// never reach this path because their body ceiling equals the budget.
    pub(super) fn render_budgeted_markdown(
        &self,
        body: &str,
        theme: &ArtisanTheme,
        selector: String,
        tone: MarkdownBodyTone,
    ) -> AnyElement {
        let within_budget = transcript_markdown_within_budget(body.len());
        self.transcript_window
            .borrow_mut()
            .record_shaping(body.len(), within_budget);
        if !within_budget {
            let plain_id = SharedString::from(format!("{selector}-markdown-plain"));
            let root_selector = format!("{selector}-markdown");
            return div()
                .w_full()
                .debug_selector(move || root_selector)
                .child(SelectableText::retained(
                    plain_id,
                    body.to_owned(),
                    *theme,
                    Vec::new(),
                ))
                .into_any_element();
        }
        let rich_link_titles = self.rich_link_probe(crate::conversation_host::host_now_millis());
        self.markdown_renderer.render_source_with_tone_and_titles(
            body,
            *theme,
            selector,
            tone,
            &rich_link_titles,
        )
    }

    /// Prunes scene-keyed footer state and rebuilds the height table.
    ///
    /// Called only from [`Self::replace_scene`], so a render pass never has to
    /// walk every turn to find departed footer keys.
    pub(super) fn scene_replaced(&mut self) {
        let live: HashSet<String> = self
            .scene
            .turn_scenes()
            .iter()
            .map(|turn| footer_key(&turn.turn_id))
            .collect();
        self.footer_mirrors.retain(|key, _| live.contains(key));
        self.footer_focus.retain(|key, _| live.contains(key));
        self.transcript_window
            .borrow_mut()
            .scene_replaced(&self.scene);
    }

    /// Ensures per-turn footer focus handles for the rows about to be built.
    ///
    /// Handles persist across window changes on purpose (a focused copy
    /// control keeps its focus while its turn scrolls out and back), and
    /// handles for turns that left the scene are pruned by
    /// [`Self::scene_replaced`], so this never scans the whole transcript.
    pub(super) fn sync_footer_focus(
        &mut self,
        built: &TranscriptBuildWindow,
        cx: &mut Context<Self>,
    ) {
        for index in built.indices() {
            let Some(turn) = self.scene.turn_scenes().get(index) else {
                continue;
            };
            self.footer_focus
                .entry(footer_key(&turn.turn_id))
                .or_insert_with(|| cx.focus_handle().tab_stop(true));
        }
    }

    /// Records measured heights for the rows this frame actually built.
    ///
    /// Placeholder boxes report their own planned height back, so only rows
    /// in the build window contribute measurements; the running estimate for
    /// never-seen rows stays unpolluted.
    fn record_measured_turn_heights(&self, children_bounds: &[gpui::Bounds<gpui::Pixels>]) {
        let built = self.transcript_window.borrow().report.built_rows.clone();
        let mut state = self.transcript_window.borrow_mut();
        for index in built {
            let Some(bounds) = children_bounds.get(index) else {
                continue;
            };
            state.table.record(index, f32::from(bounds.size.height));
        }
    }

    /// Runs one transcript prepaint observation after the children painted.
    ///
    /// Extracted verbatim from the render closure during the windowing split.
    /// The paint token still gates custody, and the geometry observations
    /// still write only window-local state with change-guarded notifications.
    /// Two additions serve windowing: measured heights are recorded first, so
    /// the next frame's plan reflects what actually painted, and a retained
    /// scroll target now defers an explicit refresh alongside its
    /// notification, so a freshly painted anchor reliably gets the frame that
    /// executes it.
    pub(super) fn handle_transcript_prepaint(
        &mut self,
        children_bounds: &[gpui::Bounds<gpui::Pixels>],
        paint_token: &Rc<()>,
        end_space_state: &Entity<f32>,
        navigator_active_state: &Entity<Option<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .scroll_anchor_paint_token
            .as_ref()
            .is_some_and(|current| Rc::ptr_eq(current, paint_token));
        if !current {
            return;
        }

        self.record_measured_turn_heights(children_bounds);

        // End-space measurement is window-local state, written only by the
        // current frame: two windows sharing one surface converge
        // independently, and an unchanged value never notifies, so neither
        // window can loop the other.
        let viewport_height = f64::from(self.scroll_handle.bounds().size.height);
        let offset = f64::from(window.element_offset().y);
        if let Some(measured) =
            ConversationSurface::measured_end_space_height(children_bounds, viewport_height, offset)
        {
            let changed = end_space_state.update(cx, |value, _| {
                #[expect(
                    clippy::float_cmp,
                    reason = "the loop guard only notifies when the measurement changes at all; an epsilon would skip sub-pixel re-layout and could strand the end space"
                )]
                let changed = *value != measured;
                *value = measured;
                changed
            });
            if changed {
                cx.notify();
                window.defer(cx, |window, _| window.refresh());
            }
        }

        // Reader-position tracking rides the same prepaint boundary into the
        // same window-local state discipline: per-window geometry in,
        // change-guarded notify out. Markers carry their turn index, so this
        // touches only marker-bearing turns instead of walking the scene.
        let active = ConversationSurface::navigator_active_for_geometry(
            &self.navigator_markers,
            &self.scroll_handle,
            children_bounds,
            window,
        );
        let active_changed = navigator_active_state.update(cx, |value, _| {
            let changed = *value != active;
            *value = active;
            changed
        });
        if active_changed {
            cx.notify();
            // A prepaint notification alone can be consumed by this frame.
            // Paint the new marker even when following stays inside its leeway
            // and emits no separate viewport action to trigger another draw.
            window.defer(cx, |window, _| window.refresh());
        }

        let mut newly_painted = false;
        for anchor in &mut self.scroll_anchors {
            if !anchor.painted {
                anchor.painted = true;
                newly_painted = true;
            }
        }
        if newly_painted && !self.pending_scroll_targets.is_empty() {
            // The retained target needs one more frame before its freshly
            // painted anchor can execute. A notification raised inside
            // prepaint does not itself request a frame on every platform
            // path (the navigator metrics listener hits the same edge), so
            // defer an explicit refresh alongside it.
            cx.notify();
            window.defer(cx, |window, _| window.refresh());
        }
        // Turn roots are the transcript's direct children in scene order, so
        // executed turn targets resolve here exactly like block and item
        // targets resolve one level down.
        if !self.executed_scroll_targets.is_empty() {
            let turn_identities = self.turn_scroll_identities();
            self.apply_executed_scroll_targets(&turn_identities, children_bounds, window);
        }
    }
}
