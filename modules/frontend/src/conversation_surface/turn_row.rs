//! Transcript turn rows as cached child views of [`ConversationSurface`].
//!
//! Every animated leaf in a turn (the live status shimmer, the header
//! entrance, the send rise, copy feedback, disclosure flights) asks GPUI for
//! the next frame through its *current view*. While turns rendered inline in
//! the surface, that view was the surface, so one shimmering status line
//! rebuilt every built row of the transcript at the display rate. Each turn
//! now renders from its own [`TurnRowView`] entity:
//!
//! - An animation notifies its row. GPUI dirties that row and its ancestors,
//!   so the surface re-renders its cheap shell and the row re-renders, while
//!   every sibling row that is clean is laid out from its remembered height
//!   and replays its previous prepaint and paint.
//! - A row renders cached only when nothing it paints can have changed: its
//!   surface-owned inputs compared equal this frame, it has not been notified
//!   since its last render, the height table holds its painted height, and
//!   no scroll target is waiting on anchors. Anything else renders it
//!   uncached, which measures its real height exactly as before.
//! - Every surface-owned value a row reads is an explicit input, synced by
//!   the surface's render loop and compared before it is set. A row never
//!   reads the surface during its own render, so a cached row cannot paint a
//!   stale surface value. Interactions reach the surface through a weak
//!   handle, exactly like the inline rows did.
//!
//! The transcript still holds exactly one child per turn, so the surface's
//! one-child-per-turn prepaint indexing (end space, navigator geometry,
//! scroll-target handoff, height measurement) is unchanged.

use gpui::{AppContext as _, StyleRefinement, Subscription, WeakEntity};

use super::transcript_window::{TranscriptShapeLedger, TranscriptShaper};
use super::*;

/// One approval row's gate snapshot, the only answer state a row paints.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ApprovalRowState {
    /// The engine's approval identity; the gate key.
    pub(super) key: String,
    pub(super) in_flight: bool,
    pub(super) pending_decision: Option<bool>,
    pub(super) answered_decision: Option<bool>,
    pub(super) failure: Option<String>,
}

/// The clock-dependent copy one turn paints.
///
/// A new host clock sample only re-renders a row when this changes, so a
/// ticking live clock touches the live turn and leaves settled history
/// cached.
#[derive(Clone, Debug, Default, PartialEq)]
struct ClockDigest {
    owner_header: Option<String>,
    section_title: String,
    statuses: Vec<(Option<String>, bool)>,
}

impl ClockDigest {
    fn of(turn: &TurnScene, now_ms: Option<i64>) -> Self {
        let has_work_group = turn
            .blocks()
            .iter()
            .any(|block| matches!(block, TurnBlock::WorkGroup(_)));
        Self {
            owner_header: turn_owner_header(turn, now_ms),
            section_title: turn_section_title(turn, now_ms),
            statuses: turn
                .blocks()
                .iter()
                .filter_map(|block| match block {
                    TurnBlock::TurnStatus(status) => Some((
                        turn_status_block_copy(status, now_ms),
                        turn_status_block_paints(Some(turn), status, has_work_group, now_ms),
                    )),
                    _ => None,
                })
                .collect(),
        }
    }
}

/// Borrowed surface state one row is synced from, once per surface render.
pub(super) struct TurnRowSource<'a> {
    pub(super) turn: &'a TurnScene,
    /// Bumped by every scene replacement, so an unchanged scene skips the
    /// turn comparison entirely.
    pub(super) scene_generation: u64,
    pub(super) theme_mode: ThemeMode,
    pub(super) status_motion: MotionPolicy,
    pub(super) reduce_motion: bool,
    pub(super) active_now_ms: Option<i64>,
    pub(super) footer_mirror: Option<&'a TurnFooterMirror>,
    pub(super) footer_focus: Option<&'a FocusHandle>,
    pub(super) footer_revealed: bool,
    pub(super) approval_gates: &'a HashMap<String, ApprovalAnswerGate>,
    pub(super) answer_ready: bool,
    pub(super) send_entrance: Option<&'a SendEntrance>,
    pub(super) message_images:
        Option<&'a Entity<crate::native_message_images::NativeMessageImages>>,
    pub(super) rich_link_generation: u64,
}

/// Row bookkeeping shared with the surface without reading the entity.
#[derive(Default)]
pub(super) struct TurnRowStats {
    /// Renders since the row was created: the reuse test seam.
    renders: Cell<u64>,
    /// Set when the row is notified outside its own render (an animation
    /// frame, a toggle, retained element state); cleared by its render.
    dirty: Cell<bool>,
    /// Markdown shaped by the row's latest render.
    ledger: Cell<TranscriptShapeLedger>,
}

impl TurnRowStats {
    pub(super) fn renders(&self) -> u64 {
        self.renders.get()
    }

    pub(super) fn ledger(&self) -> TranscriptShapeLedger {
        self.ledger.get()
    }
}

/// The surface's handle to one retained row.
pub(super) struct TurnRowHandle {
    pub(super) view: Entity<TurnRowView>,
    pub(super) anchors: Rc<RefCell<Vec<RenderedScrollAnchor>>>,
    pub(super) stats: Rc<TurnRowStats>,
}

impl TurnRowHandle {
    /// Creates one row for `turn`, synced on its first surface render.
    pub(super) fn new(
        turn: &TurnScene,
        services: &TurnRowServices,
        cx: &mut Context<ConversationSurface>,
    ) -> Self {
        let anchors = Rc::new(RefCell::new(Vec::new()));
        let stats = Rc::new(TurnRowStats::default());
        let view = cx.new(|row_cx| {
            let observed = stats.clone();
            // Any notification that is not the surface's own sync (animation
            // frames, toggles, retained state) can change what the row
            // paints, so the next frame renders it uncached and measures it.
            let dirty = row_cx.observe_self(move |_, _| observed.dirty.set(true));
            TurnRowView {
                surface: services.surface.clone(),
                shaper: services.shaper.clone(),
                trace_groups_open: services.trace_groups_open.clone(),
                scroll_handle: services.scroll_handle.clone(),
                disclosure_focus: services.disclosure_focus.clone(),
                answer_focus: services.answer_focus.clone(),
                turn: turn.clone(),
                scene_generation: None,
                theme_mode: ThemeMode::Dark,
                status_motion: MotionPolicy::Full,
                reduce_motion: false,
                active_now_ms: None,
                clock: ClockDigest::of(turn, None),
                footer_mirror: None,
                footer_focus: None,
                footer_revealed: false,
                approvals: Vec::new(),
                answer_ready: false,
                send_entrance: None,
                message_images: None,
                rich_link_generation: 0,
                anchors: anchors.clone(),
                stats: stats.clone(),
                _self_observation: dirty,
                images_observation: None,
            }
        });
        Self {
            view,
            anchors,
            stats,
        }
    }

    /// Syncs the row and returns the transcript child for this frame.
    ///
    /// `measured` is the height table's painted height for the turn; without
    /// it the row must render uncached to be measured. `force_render` holds
    /// every row uncached while a scroll target waits: the handoff runs in
    /// the rows' own prepaint listeners, which a replayed row never runs.
    pub(super) fn child(
        &self,
        source: TurnRowSource<'_>,
        measured: Option<f32>,
        force_render: bool,
        cx: &mut Context<ConversationSurface>,
    ) -> AnyElement {
        let changed = self
            .view
            .update(cx, |row, row_cx| row.sync(&source, row_cx));
        let stale = changed || self.stats.dirty.get() || self.stats.renders.get() == 0;
        match measured {
            Some(height) if !stale && !force_render => {
                // The row root's own box (see `render_turn`) at its painted
                // height, so a cached row lays out exactly where it painted.
                let style = StyleRefinement::default()
                    .w_full()
                    .max_w(px(TRANSCRIPT_PROSE_WIDTH_PX))
                    .mx_auto()
                    .h(px(height));
                self.view.clone().cached(style).into_any_element()
            }
            _ => self.view.clone().into_any_element(),
        }
    }
}

/// Surface services every row holds for its whole life.
pub(super) struct TurnRowServices {
    pub(super) surface: WeakEntity<ConversationSurface>,
    pub(super) shaper: Rc<TranscriptShaper>,
    pub(super) trace_groups_open: Rc<RefCell<HashMap<String, bool>>>,
    pub(super) scroll_handle: ScrollHandle,
    pub(super) disclosure_focus: FocusHandle,
    pub(super) answer_focus: FocusHandle,
}

/// One transcript turn rendered as its own view.
pub(super) struct TurnRowView {
    /// Interactions route through the surface exactly like the inline rows.
    pub(super) surface: WeakEntity<ConversationSurface>,
    pub(super) shaper: Rc<TranscriptShaper>,
    /// Activity-chain disclosure overrides, shared with the surface so an
    /// explicit navigation can pin a chain open.
    pub(super) trace_groups_open: Rc<RefCell<HashMap<String, bool>>>,
    pub(super) scroll_handle: ScrollHandle,
    pub(super) disclosure_focus: FocusHandle,
    pub(super) answer_focus: FocusHandle,
    pub(super) turn: TurnScene,
    scene_generation: Option<u64>,
    pub(super) theme_mode: ThemeMode,
    pub(super) status_motion: MotionPolicy,
    pub(super) reduce_motion: bool,
    pub(super) active_now_ms: Option<i64>,
    clock: ClockDigest,
    pub(super) footer_mirror: Option<TurnFooterMirror>,
    pub(super) footer_focus: Option<FocusHandle>,
    pub(super) footer_revealed: bool,
    pub(super) approvals: Vec<ApprovalRowState>,
    pub(super) answer_ready: bool,
    /// The running send rise, when it targets a message in this turn.
    pub(super) send_entrance: Option<(Instant, SceneId)>,
    pub(super) message_images: Option<Entity<crate::native_message_images::NativeMessageImages>>,
    rich_link_generation: u64,
    anchors: Rc<RefCell<Vec<RenderedScrollAnchor>>>,
    pub(super) stats: Rc<TurnRowStats>,
    _self_observation: Subscription,
    images_observation: Option<Subscription>,
}

impl TurnRowView {
    /// Copies every changed input and reports whether anything changed.
    ///
    /// Runs inside the surface's render, where a notification could not
    /// reach this frame's cache check; the surface renders a changed row
    /// uncached instead, which re-renders it unconditionally.
    fn sync(&mut self, source: &TurnRowSource<'_>, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        if self.scene_generation != Some(source.scene_generation) {
            self.scene_generation = Some(source.scene_generation);
            if self.turn != *source.turn {
                self.turn = source.turn.clone();
                self.clock = ClockDigest::of(&self.turn, source.active_now_ms);
                self.active_now_ms = source.active_now_ms;
                changed = true;
            }
        }
        if self.active_now_ms != source.active_now_ms {
            // The latest sample is kept either way; only copy that reads
            // differently at the new sample repaints the row.
            self.active_now_ms = source.active_now_ms;
            let clock = ClockDigest::of(&self.turn, source.active_now_ms);
            if clock != self.clock {
                self.clock = clock;
                changed = true;
            }
        }
        changed |= replace_if_changed(&mut self.theme_mode, source.theme_mode);
        changed |= replace_if_changed(&mut self.status_motion, source.status_motion);
        changed |= replace_if_changed(&mut self.reduce_motion, source.reduce_motion);
        changed |= replace_if_changed(&mut self.footer_revealed, source.footer_revealed);
        changed |= replace_if_changed(&mut self.answer_ready, source.answer_ready);
        changed |= replace_if_changed(&mut self.rich_link_generation, source.rich_link_generation);
        if self.footer_mirror.as_ref() != source.footer_mirror {
            self.footer_mirror = source.footer_mirror.cloned();
            changed = true;
        }
        if self.footer_focus.as_ref() != source.footer_focus {
            self.footer_focus = source.footer_focus.cloned();
            changed = true;
        }
        changed |= self.sync_approvals(source.approval_gates);
        changed |= self.sync_send_entrance(source.send_entrance);
        if self.message_images.as_ref() != source.message_images {
            self.message_images = source.message_images.cloned();
            // Thumbnails decode after the row painted; a row showing
            // attachments repaints (and re-measures) when they land.
            self.images_observation = self.message_images.as_ref().map(|images| {
                cx.observe(images, |row, _, cx| {
                    if row.has_attachments() {
                        cx.notify();
                    }
                })
            });
            changed = true;
        }
        changed
    }

    /// Mirrors the approval gates of this turn's approval blocks.
    fn sync_approvals(&mut self, gates: &HashMap<String, ApprovalAnswerGate>) -> bool {
        let mut index = 0;
        let mut changed = false;
        for block in self.turn.blocks() {
            let TurnBlock::Approval(approval) = block else {
                continue;
            };
            let key = approval.approval_id.as_str();
            let gate = gates.get(key);
            let in_flight = gate.is_some_and(ApprovalAnswerGate::is_in_flight);
            let pending_decision = gate.and_then(ApprovalAnswerGate::pending_decision);
            let answered_decision = gate.and_then(ApprovalAnswerGate::answered_decision);
            let failure = gate.and_then(ApprovalAnswerGate::failure_message);
            let same = self.approvals.get(index).is_some_and(|state| {
                state.key == key
                    && state.in_flight == in_flight
                    && state.pending_decision == pending_decision
                    && state.answered_decision == answered_decision
                    && state.failure.as_deref() == failure
            });
            if !same {
                let state = ApprovalRowState {
                    key: key.to_owned(),
                    in_flight,
                    pending_decision,
                    answered_decision,
                    failure: failure.map(str::to_owned),
                };
                if index < self.approvals.len() {
                    self.approvals[index] = state;
                } else {
                    self.approvals.push(state);
                }
                changed = true;
            }
            index += 1;
        }
        if self.approvals.len() != index {
            self.approvals.truncate(index);
            changed = true;
        }
        changed
    }

    /// Keeps the send rise only when it targets a message in this turn.
    fn sync_send_entrance(&mut self, entrance: Option<&SendEntrance>) -> bool {
        let next = entrance.and_then(|entrance| {
            let target = entrance.target.as_ref()?;
            self.turn
                .blocks()
                .iter()
                .any(|block| matches!(block, TurnBlock::UserMessage(message) if &message.id == target))
                .then(|| (entrance.started, target.clone()))
        });
        replace_if_changed(&mut self.send_entrance, next)
    }

    fn has_attachments(&self) -> bool {
        self.turn.blocks().iter().any(|block| {
            matches!(block, TurnBlock::UserMessage(message) if !message.attachments.is_empty())
        })
    }

    /// The approval gate snapshot for one block key.
    pub(super) fn approval_state(&self, key: &str) -> Option<&ApprovalRowState> {
        self.approvals.iter().find(|state| state.key == key)
    }

    /// Shapes one Markdown body under the per-row budget, recording it in
    /// both the frame ledger and this row's own ledger.
    pub(super) fn render_budgeted_markdown(
        &self,
        body: &str,
        theme: &ArtisanTheme,
        selector: String,
        tone: MarkdownBodyTone,
    ) -> AnyElement {
        self.shaper
            .render(body, theme, selector, tone, Some(&self.stats.ledger))
    }

    /// Hands every unresolved rich-link destination to the surface.
    ///
    /// The row renders after the surface's render tail has flushed, so the
    /// flush runs deferred, outside the draw, where its notification
    /// schedules the host's next pass normally.
    fn schedule_rich_link_flush(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shaper.has_missing_links() {
            let surface = self.surface.clone();
            window.defer(cx, move |_, app| {
                let _ = surface.update(app, |surface, cx| surface.flush_rich_link_requests(cx));
            });
        }
    }
}

impl Render for TurnRowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.stats.dirty.set(false);
        self.stats
            .renders
            .set(self.stats.renders.get().saturating_add(1));
        self.stats.ledger.set(TranscriptShapeLedger::default());
        let theme = ArtisanTheme::for_mode(self.theme_mode);
        // Anchors are retained per identity across this row's renders, so a
        // painted anchor stays painted while its block stays in the turn.
        let previous = std::mem::take(&mut *self.anchors.borrow_mut());
        let mut rendered = Vec::with_capacity(previous.len());
        let element = {
            let mut anchors = ScrollAnchorRegistry {
                handle: &self.scroll_handle,
                previous: &previous,
                next_element_id: 0,
                rendered: &mut rendered,
            };
            self.render_turn(
                &self.turn,
                &theme,
                &mut anchors,
                window,
                self.status_motion,
                cx,
            )
        };
        *self.anchors.borrow_mut() = rendered;
        self.schedule_rich_link_flush(window, cx);
        element
    }
}

/// Replaces `slot` with `next` when they differ; reports whether it did.
fn replace_if_changed<T: PartialEq>(slot: &mut T, next: T) -> bool {
    if *slot == next {
        false
    } else {
        *slot = next;
        true
    }
}
