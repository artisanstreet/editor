//! Surface state, accessors, answer/question gestures, the action queue, and
//! transcript viewport/scroll tracking for [`ConversationSurface`].
//!
//! Extracted verbatim from `conversation_surface.rs` during the phase-2 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl ConversationSurface {
    pub(crate) fn set_composer_clearance(
        &mut self,
        window: gpui::WindowId,
        height: f32,
        cx: &mut Context<Self>,
    ) {
        if !height.is_finite() || height < 0.0 {
            return;
        }
        if self
            .composer_clearance
            .get(&window)
            .is_some_and(|old| (*old - height).abs() < 0.5)
        {
            return;
        }
        self.composer_clearance.insert(window, height);
        cx.notify();
    }

    pub(crate) fn has_pending_messages(&self) -> bool {
        !self.pending_messages.is_empty()
    }

    pub(crate) fn set_pending_messages(
        &mut self,
        rows: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pending_messages != rows {
            self.pending_messages = rows;
            cx.notify();
            return true;
        }
        false
    }

    /// Creates a surface with keyboard-focusable transcript and disclosure
    /// handles. The surface starts with the supplied scene and no actions.
    #[must_use]
    pub fn new(scene: ConversationScene, theme_mode: ThemeMode, cx: &mut Context<Self>) -> Self {
        let navigator_markers = Rc::new(loaded_turn_navigator_markers(&scene));
        let mut surface = Self {
            composer_clearance: HashMap::new(),
            pending_messages: Vec::new(),
            scene,
            navigator_markers,
            transcript_window: RefCell::new(TranscriptWindowState::default()),
            message_images: None,
            message_images_observation: None,
            theme_mode,
            markdown_renderer: MarkdownRenderer::new(),
            rich_link_titles: RichLinkTitleTable::new(),
            rich_link_missing: RefCell::new(Vec::new()),
            scroll_handle: ScrollHandle::new(),
            transcript_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            disclosure_focus: cx.focus_handle().tab_index(1).tab_stop(true),
            jump_to_latest_focus: cx.focus_handle().tab_index(2).tab_stop(true),
            answer_focus: cx.focus_handle().tab_index(3).tab_stop(true),
            jump_to_latest_visible: false,
            last_viewport_observation: Some(ViewportObservation {
                first_visible: None,
                last_visible: None,
                at_bottom: true,
            }),
            pending_viewport_observation: None,
            last_viewport_geometry: None,
            viewport_observation_scheduled: false,
            viewport_next_frame_scheduled: false,
            actions: Vec::new(),
            pending_scroll_targets: Vec::with_capacity(CONVERSATION_SURFACE_MAX_SCROLL_TARGETS),
            executed_scroll_targets: Vec::new(),
            scroll_anchors: Vec::new(),
            scroll_anchor_paint_token: None,
            navigator_focus: HashMap::new(),
            navigator_expanded: false,
            navigator_scroll: ScrollHandle::new(),
            navigator_hover: Rc::new(RefCell::new(SlidingHoverState::default())),
            navigator_hover_surface: Rc::new(RefCell::new(None)),
            navigator_focused_key: None,
            navigator_width_generation: 0,
            navigator_width_px: Rc::new(RefCell::new(40.0)),
            navigator_width_from: 40.0,
            transcript_scroll: PickerScrollState::default(),
            transcript_scroll_frame_scheduled: false,
            active_now_ms: None,
            status_motion: MotionPolicy::Full,
            trace_groups_open: RefCell::new(HashMap::new()),
            footer_mirrors: HashMap::new(),
            footer_focus: HashMap::new(),
            footer_revealed: None,
            question_focus: HashMap::new(),
            answer_thread: None,
            answer_run: None,
            approval_gates: HashMap::new(),
            question_gates: HashMap::new(),
            question_choices: HashMap::new(),
            pending_answer_dispatches: Vec::new(),
        };
        surface.sync_question_focus(cx);
        surface
    }

    /// Returns the currently accepted scene without cloning it.
    #[must_use]
    pub fn scene(&self) -> &ConversationScene {
        &self.scene
    }

    /// Returns the selected theme mode.
    #[must_use]
    pub const fn theme_mode(&self) -> ThemeMode {
        self.theme_mode
    }

    /// Returns the shared GPUI scroll handle owned by the surface.
    #[must_use]
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll_handle
    }

    /// Shares the application's bounded image cache with transcript cells.
    pub fn set_message_images(
        &mut self,
        images: Entity<crate::native_message_images::NativeMessageImages>,
        cx: &mut Context<Self>,
    ) {
        self.message_images_observation = Some(cx.observe(&images, |_, _, cx| cx.notify()));
        self.message_images = Some(images);
        cx.notify();
    }

    /// Returns the focus handle tracked by the transcript viewport.
    #[must_use]
    pub fn transcript_focus_handle(&self) -> &FocusHandle {
        &self.transcript_focus
    }

    /// Returns the focus handle used by controlled disclosure triggers.
    #[must_use]
    pub fn disclosure_focus_handle(&self) -> &FocusHandle {
        &self.disclosure_focus
    }

    /// Returns the focus handle retained for one navigator control, if its
    /// target is currently rendered.
    #[must_use]
    pub fn navigator_focus_handle(
        &self,
        target: &ConversationSurfaceTarget,
    ) -> Option<FocusHandle> {
        self.navigator_focus
            .get(&navigator_focus_key(target))
            .cloned()
    }

    /// Returns the focus handle retained for one free-form question row, if
    /// the row is currently rendered.
    #[must_use]
    pub fn question_focus_handle(&self, block_id: &str) -> Option<FocusHandle> {
        self.question_focus.get(block_id).cloned()
    }

    /// Rebuilds per-row question focus handles from the accepted scene.
    ///
    /// Generations survive for rows still present, so unrelated scene
    /// updates never steal focus; handles for removed rows drop with the
    /// rebuild, so drafts and focus never leak across questions.
    pub(super) fn sync_question_focus(&mut self, cx: &mut Context<Self>) {
        let mut next = HashMap::with_capacity(self.question_focus.len());
        for block_id in question_block_ids(&self.scene) {
            let handle = self
                .question_focus
                .remove(&block_id)
                .unwrap_or_else(|| cx.focus_handle().tab_index(4).tab_stop(true));
            next.insert(block_id, handle);
        }
        self.question_focus = next;
    }

    /// Mirrors the controller's detached-reader affordance into rendering.
    ///
    /// The viewport controller remains the sole authority for whether the
    /// button should be visible; this flag is only a paint-time mirror.
    pub fn set_jump_to_latest_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.jump_to_latest_visible != visible {
            self.jump_to_latest_visible = visible;
            cx.notify();
        }
    }

    /// Requests the existing GPUI scroll handle to move to the transcript end.
    ///
    /// Completion is intentionally not synthesized here. The next physical
    /// viewport observation is the controller's completion signal.
    pub fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.scroll_handle.scroll_to_bottom();
        cx.notify();
    }

    /// Replaces the accepted scene. Disclosure state is not changed locally;
    /// the next replacement scene remains authoritative.
    pub fn replace_scene(&mut self, scene: ConversationScene, cx: &mut Context<Self>) {
        self.navigator_markers = Rc::new(loaded_turn_navigator_markers(&scene));
        self.scene = scene;
        // Scene-keyed presentation state is pruned once here instead of on
        // every render, so a windowed render never scans the whole transcript.
        self.scene_replaced();
        self.sync_question_focus(cx);
        cx.notify();
    }

    /// Mirrors one host clock sample for live Thinking/Working elapsed paint.
    ///
    /// The value is display input only; the authoritative elapsed basis stays
    /// scene-owned and is never reset here.
    pub fn set_active_now_ms(&mut self, now_ms: Option<i64>, cx: &mut Context<Self>) {
        if self.active_now_ms != now_ms {
            self.active_now_ms = now_ms;
            cx.notify();
        }
    }

    /// Returns the motion policy applied to the live status shimmer and the
    /// work-session disclosure flights.
    #[must_use]
    pub const fn status_motion(&self) -> MotionPolicy {
        self.status_motion
    }

    /// Mirrors an explicit reduced-motion preference for the live status
    /// shimmer and the work-session disclosure flights.
    ///
    /// `Full` (the default) follows the window's reduced-motion signal at
    /// render time; `Reduced` forces immediate static presentation regardless
    /// of it. Settled status rows never animate regardless of this preference.
    pub fn set_status_motion(&mut self, motion: MotionPolicy, cx: &mut Context<Self>) {
        if self.status_motion != motion {
            self.status_motion = motion;
            cx.notify();
        }
    }

    /// Mirrors the adapter-formatted relative age for one settled footer.
    pub fn set_footer_relative_age(
        &mut self,
        turn_id: &TurnId,
        relative_age: String,
        cx: &mut Context<Self>,
    ) {
        let mirror = self.footer_mirrors.entry(footer_key(turn_id)).or_default();
        if mirror.relative_age != relative_age {
            mirror.relative_age = relative_age;
            cx.notify();
        }
    }

    /// Mirrors validated request throughput without manufacturing absent measurements.
    pub(crate) fn set_footer_speed(
        &mut self,
        turn: &TurnId,
        speed: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let mirror = self.footer_mirrors.entry(footer_key(turn)).or_default();
        if mirror.token_speed != speed {
            mirror.token_speed = speed;
            cx.notify();
        }
    }

    /// Mirrors the reader-facing copy status for one settled footer.
    ///
    /// An empty message clears a previous failure notice.
    pub fn set_footer_copy_message(
        &mut self,
        turn_id: &TurnId,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let mirror = self.footer_mirrors.entry(footer_key(turn_id)).or_default();
        if mirror.copy_message != message {
            mirror.copy_message = message;
            cx.notify();
        }
    }

    /// Starts visual confirmation only after the clipboard write has succeeded.
    pub fn confirm_footer_copy(&mut self, turn_id: &TurnId, cx: &mut Context<Self>) {
        let mirror = self.footer_mirrors.entry(footer_key(turn_id)).or_default();
        let now = std::time::Instant::now();
        let elapsed = mirror.copied_at.map(|at| now.saturating_duration_since(at));
        // Repeated copies extend the hold or reverse the return at its current
        // opacity; they never flash back to the copy icon.
        let entered = elapsed.map_or(Duration::ZERO, |elapsed| {
            if elapsed < Duration::from_millis(250) {
                elapsed
            } else if elapsed < Duration::from_millis(1500) {
                Duration::from_millis(250)
            } else {
                Duration::from_millis(1750).saturating_sub(elapsed)
            }
        });
        mirror.copied_at = now.checked_sub(entered);
        cx.notify();
    }

    /// Sets the shared theme mode and repaints the surface when it changes.
    pub fn set_theme_mode(&mut self, theme_mode: ThemeMode, cx: &mut Context<Self>) {
        if self.theme_mode != theme_mode {
            self.theme_mode = theme_mode;
            cx.notify();
        }
    }

    /// Mirrors one resolved rich-link title and repaints when it is new.
    ///
    /// The URL is the backend's echoed canonical URL; the table re-applies
    /// the shared canonical policy before retaining it.
    pub fn set_rich_link_title(
        &mut self,
        requested_url: &str,
        page_name: &SharedString,
        expires_at_ms: i64,
        cx: &mut Context<Self>,
    ) {
        let before = self.rich_link_titles.lookup(requested_url);
        self.rich_link_titles
            .resolve(requested_url, page_name.clone(), expires_at_ms);
        if before.as_ref() != Some(page_name) {
            cx.notify();
        }
    }

    /// Records one failed rich-link resolution; the authored label stays.
    pub fn set_rich_link_failure(&mut self, requested_url: &str, cx: &mut Context<Self>) {
        self.rich_link_titles.fail(requested_url);
        cx.notify();
    }

    /// Returns one render-scoped probe over the surface title table.
    pub(super) fn rich_link_probe(&self, now_ms: i64) -> SurfaceRichLinkTitles<'_> {
        SurfaceRichLinkTitles {
            titles: &self.rich_link_titles,
            missing: &self.rich_link_missing,
            now_ms,
        }
    }

    /// Queues every newly missing rich-link destination exactly once.
    ///
    /// Runs at the end of one render pass: the probe recorded misses while
    /// the renderer flattened links, and this drain marks them pending and
    /// hands the host one bounded resolver action. The extra notification is
    /// guarded by the table's pending state, so a second frame queues nothing
    /// and the loop ends.
    pub(super) fn flush_rich_link_requests(&mut self, cx: &mut Context<Self>) {
        let missing = std::mem::take(&mut *self.rich_link_missing.borrow_mut());
        if missing.is_empty() {
            return;
        }
        let now_ms = crate::conversation_host::host_now_millis();
        let mut queued = Vec::new();
        for destination in missing {
            if let Some(url) = self.rich_link_titles.queue(&destination, now_ms) {
                queued.push(url);
            }
        }
        if queued.is_empty() {
            return;
        }
        if self.enqueue_action(ConversationSurfaceAction::ResolveRichLinks { urls: queued }) {
            cx.notify();
        }
    }

    /// Sets the explicit live answer context for engine approval/question rows.
    ///
    /// Both identities join each submit at dispatch; nothing ambient is read
    /// elsewhere. Submit affordances stay disabled until the controller
    /// supplies the live owning thread and run.
    pub fn set_answer_context(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        cx: &mut Context<Self>,
    ) {
        self.answer_thread = Some(thread_id);
        self.answer_run = Some(run_id);
        cx.notify();
    }

    /// Clears the live answer context, disabling submit affordances.
    pub fn clear_answer_context(&mut self, cx: &mut Context<Self>) {
        self.answer_thread = None;
        self.answer_run = None;
        cx.notify();
    }

    /// Mirrors one engine row's offered choices for choice rendering.
    ///
    /// Rows without cached options render free-form entry. Only labels (plus
    /// optional descriptions) are mirrored; the authoritative option views
    /// remain in [`EngineObservationState`].
    pub fn set_question_choices(
        &mut self,
        block_id: String,
        multi_select: bool,
        options: Vec<(String, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        self.question_choices.insert(
            block_id,
            QuestionChoiceCache {
                multi_select,
                options,
            },
        );
        cx.notify();
    }

    /// Stages one row's free-form draft exactly as supplied.
    ///
    /// GPUI text-entry binding for this draft lands with the controller input
    /// packet; until then the controller (and tests) stage drafts through
    /// this setter and the Answer control submits them.
    pub fn set_question_draft(&mut self, block_id: String, draft: &str, cx: &mut Context<Self>) {
        self.question_gates
            .entry(block_id)
            .or_default()
            .set_draft(draft);
        cx.notify();
    }

    /// Returns built answer dispatches awaiting controller drain.
    #[must_use]
    pub fn pending_answer_dispatches(&self) -> &[AnswerDispatch] {
        &self.pending_answer_dispatches
    }

    /// Drains built answer dispatches in FIFO order.
    pub fn take_answer_dispatches(&mut self) -> Vec<AnswerDispatch> {
        std::mem::take(&mut self.pending_answer_dispatches)
    }

    /// Drains pending answer dispatches toward transport through the mirrored
    /// send path.
    ///
    /// Takes the outbox via [`Self::take_answer_dispatches`] (`mem::take`
    /// semantics already in the accessor: the queue is never re-taken or
    /// cloned) and hands each domain command with its already-minted request
    /// id to `send` at most once per call. `send` mirrors
    /// [`NativeTransportService::submit`](crate::native_transport_service::NativeTransportService::submit):
    /// it reports [`CommandSendError::Busy`] when its bounded queue is full
    /// and [`CommandSendError::Stopped`] after the service has stopped.
    ///
    /// Failed dispatches are re-queued in order with the existing retry
    /// message from [`pair_answer_failure`], so rows stay pending (their
    /// gates remain in flight until a receipt pairs through the existing
    /// settle-in-place pairing) with no silent drop and no same-call retry:
    /// one drain attempt per controller tick scope. Resolutions continue to
    /// pair through the existing receipt path, never through the outbox.
    ///
    /// `submit` is the transport submit entry point (live:
    /// [`NativeTransportService::submit`](crate::native_transport_service::NativeTransportService::submit)):
    /// each taken dispatch is mapped to its
    /// [`NativeTransportCommand`] through [`answer_transport_command`] with
    /// its already-minted request id and handed over by value exactly like
    /// the composer send path.
    pub fn drain_pending_answer_dispatches(
        &mut self,
        submit: &mut impl FnMut(NativeTransportCommand) -> Result<(), CommandSendError>,
    ) -> AnswerDrainReport {
        let queue = self.take_answer_dispatches();
        let (requeue, report) = drain_answer_queue(queue, submit);
        self.pending_answer_dispatches.extend(requeue);
        report
    }

    /// Attempts one approval gesture for a rendered row.
    ///
    /// Returns false when the row has no live context, no parsable approval
    /// identity, or an outstanding flight; in all three cases nothing is
    /// minted or dispatched. Every admitted attempt mints a fresh request
    /// identity.
    pub fn submit_approval_gesture(
        &mut self,
        block_key: &str,
        approval_id: &ObservationId,
        approved: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let (Some(thread_id), Some(run_id)) = (self.answer_thread.clone(), self.answer_run.clone())
        else {
            return false;
        };
        let admitted = self
            .approval_gates
            .entry(block_key.to_owned())
            .or_default()
            .begin(thread_id, run_id, approval_id.clone(), approved)
            .map(|attempt| AnswerDispatch {
                action: AnswerDispatchAction::Approval(attempt.action),
                command: attempt.command,
                request_id: attempt.request_id,
            });
        if let Some(dispatch) = admitted {
            self.pending_answer_dispatches.push(dispatch);
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Attempts one staged question submission for a rendered row.
    ///
    /// Choice rows submit the staged selection (single-select choices submit
    /// through [`Self::submit_question_option_gesture`] at click time);
    /// free-form rows submit the staged draft. Empty submissions are rejected
    /// client-side with the row staying pending.
    pub fn submit_question_gesture(&mut self, block_key: &str, cx: &mut Context<Self>) -> bool {
        let (answer_context, is_choice) = (
            self.answer_thread.clone().zip(self.answer_run.clone()),
            self.question_choices
                .get(block_key)
                .is_some_and(answer_state::QuestionChoiceCache::is_choice),
        );
        let (Some((thread_id, run_id)), Some(question_id)) =
            (answer_context, ObservationId::parse(block_key).ok())
        else {
            return false;
        };
        let gate = self.question_gates.entry(block_key.to_owned()).or_default();
        let admitted = if is_choice {
            gate.submit_selected(thread_id, run_id, question_id)
        } else {
            gate.submit_freeform(thread_id, run_id, question_id)
        }
        .map(|attempt| AnswerDispatch {
            action: AnswerDispatchAction::Question(attempt.action),
            command: attempt.command,
            request_id: attempt.request_id,
        });
        if let Some(dispatch) = admitted {
            self.pending_answer_dispatches.push(dispatch);
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Handles one keystroke for a focused free-form question row.
    ///
    /// Only pending free-form rows handle keys: choice rows keep their
    /// buttons, and rows with an outstanding flight ignore everything, so a
    /// second gesture can never mint a second identity. `enter` submits the
    /// staged draft through the existing [`Self::submit_question_gesture`]
    /// path (fresh request id, empty drafts rejected); `backspace` deletes
    /// one character; `escape` returns focus to the transcript without
    /// submitting; other unmodified single-character keys (plus `space`)
    /// append. Modified keys are ignored so shortcuts and focus navigation
    /// keep working.
    #[must_use]
    pub fn handle_question_key(
        &mut self,
        block_key: &str,
        key: &str,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) -> QuestionKeyOutcome {
        let freeform = !self
            .question_choices
            .get(block_key)
            .is_some_and(QuestionChoiceCache::is_choice);
        let pending = !self
            .question_gates
            .get(block_key)
            .is_some_and(QuestionAnswerGate::is_in_flight);
        if !(freeform && pending) {
            return QuestionKeyOutcome::Ignored;
        }
        if key == "escape" {
            return QuestionKeyOutcome::FocusTranscript;
        }
        if key == "enter" && !modifiers.modified() {
            return if self.submit_question_gesture(block_key, cx) {
                QuestionKeyOutcome::Submitted
            } else {
                QuestionKeyOutcome::Ignored
            };
        }
        if key == "backspace" && !modifiers.modified() {
            let gate = self.question_gates.entry(block_key.to_owned()).or_default();
            if gate.delete_backward() {
                cx.notify();
                return QuestionKeyOutcome::Edited;
            }
            return QuestionKeyOutcome::Ignored;
        }
        // The spacebar reports its key name, not a blank character; every
        // other named key (tab, arrows, function keys) is ignored so focus
        // navigation keeps working.
        let text = if key == "space" { " " } else { key };
        let mut chars = text.chars();
        let printable = match chars.next() {
            Some(first) => chars.next().is_none() && !first.is_control(),
            None => false,
        };
        let plain = !modifiers.control && !modifiers.alt && !modifiers.platform;
        if plain && printable {
            let gate = self.question_gates.entry(block_key.to_owned()).or_default();
            if gate.insert_text(text) {
                cx.notify();
                return QuestionKeyOutcome::Edited;
            }
        }
        QuestionKeyOutcome::Ignored
    }

    /// Attempts one single-select option choice the moment it is clicked.
    ///
    /// Multi-select choices only stage through the gate; they confirm through
    /// [`Self::submit_question_gesture`].
    pub fn submit_question_option_gesture(
        &mut self,
        block_key: &str,
        option: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let multi_select = self
            .question_choices
            .get(block_key)
            .is_some_and(|choices| choices.multi_select);
        if multi_select {
            self.question_gates
                .entry(block_key.to_owned())
                .or_default()
                .toggle_option(option, true);
            cx.notify();
            return true;
        }
        let (Some(thread_id), Some(run_id), Some(question_id)) = (
            self.answer_thread.clone(),
            self.answer_run.clone(),
            ObservationId::parse(block_key).ok(),
        ) else {
            return false;
        };
        let admitted = self
            .question_gates
            .entry(block_key.to_owned())
            .or_default()
            .submit_single(thread_id, run_id, question_id, option)
            .map(|attempt| AnswerDispatch {
                action: AnswerDispatchAction::Question(attempt.action),
                command: attempt.command,
                request_id: attempt.request_id,
            });
        if let Some(dispatch) = admitted {
            self.pending_answer_dispatches.push(dispatch);
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Returns pending typed actions in FIFO order without draining them.
    #[must_use]
    pub fn pending_actions(&self) -> &[ConversationSurfaceAction] {
        &self.actions
    }

    /// Borrows the oldest pending action without removing it.
    ///
    /// The host uses this head peek to make downstream capacity decisions
    /// before acknowledging an action. That keeps a refused action at the
    /// surface boundary instead of losing a drained tail.
    #[must_use]
    pub fn next_action(&self) -> Option<&ConversationSurfaceAction> {
        self.actions.first()
    }

    /// Removes and returns exactly the oldest pending action.
    ///
    /// This one-action operation is intentionally separate from
    /// [`Self::take_actions`]. Host routing can therefore acknowledge actions
    /// one at a time after the controller or outer effect outbox accepts them.
    pub fn take_next_action(&mut self) -> Option<ConversationSurfaceAction> {
        if self.actions.is_empty() {
            None
        } else {
            Some(self.actions.remove(0))
        }
    }

    /// Drains pending actions in FIFO order.
    pub fn take_actions(&mut self) -> Vec<ConversationSurfaceAction> {
        std::mem::take(&mut self.actions)
    }

    /// Queues a viewport observation when the bounded outbox has capacity.
    pub fn observe_viewport(
        &mut self,
        observation: ViewportObservation,
        cx: &mut Context<Self>,
    ) -> bool {
        self.retry_pending_viewport_observation(cx);

        if self.last_viewport_observation.as_ref() == Some(&observation) {
            return false;
        }
        if self.pending_viewport_observation.as_ref() == Some(&observation) {
            return false;
        }
        if self.pending_viewport_observation.is_some() {
            return false;
        }

        if self.enqueue_action(ConversationSurfaceAction::ViewportObserved(
            observation.clone(),
        )) {
            self.last_viewport_observation = Some(observation);
            cx.notify();
            true
        } else {
            self.pending_viewport_observation = Some(observation);
            false
        }
    }

    /// Queues a scroll intent without mutating the GPUI handle or scene.
    pub fn request_scroll(
        &mut self,
        target: ConversationSurfaceTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        self.retry_pending_viewport_observation(cx);
        if self.pending_viewport_observation.is_some() {
            return false;
        }
        let queued = self.enqueue_action(ConversationSurfaceAction::ScrollIntent { target });
        if queued {
            cx.notify();
        }
        queued
    }

    /// Queues one host-owned scroll target for the next render.
    ///
    /// The target queue is deliberately separate from surface actions: a
    /// host effect is acknowledged only when this bounded transient queue has
    /// room, and execution never dispatches a controller event.
    pub(crate) fn schedule_scroll_target(
        &mut self,
        target: ConversationSurfaceTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pending_scroll_targets.len() >= CONVERSATION_SURFACE_MAX_SCROLL_TARGETS {
            return false;
        }
        self.pending_scroll_targets.push(target);
        cx.notify();
        true
    }

    /// Handles one transcript wheel event with the model picker's smoothing.
    ///
    /// The content wrapper is the first bubble listener inside the scroll
    /// container, so stopping propagation here keeps the viewport's built-in
    /// immediate jump from applying on top. Precise trackpad deltas and
    /// reduced motion take the direct path; coarse wheel ticks accumulate
    /// into one bounded target and settle through interpolation frames.
    pub(super) fn handle_transcript_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if delta.abs() <= f32::EPSILON {
            return;
        }
        let offset = self.scroll_handle.offset();
        let current = f32::from(offset.y);
        let maximum = f32::from(self.scroll_handle.max_offset().y).max(0.0);
        if event.delta.precise() || cx.reduce_motion() {
            let next = (current + delta).clamp(-maximum, 0.0);
            self.scroll_handle.set_offset(point(offset.x, px(next)));
            self.transcript_scroll.cancel_to(next, maximum);
            cx.notify();
            return;
        }
        self.transcript_scroll.push(current, delta, maximum);
        self.schedule_transcript_scroll_frame(window, cx);
    }

    /// Schedules one transcript smoothing frame while a target is outstanding.
    pub(super) fn schedule_transcript_scroll_frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.transcript_scroll.active() || self.transcript_scroll_frame_scheduled {
            return;
        }
        self.transcript_scroll_frame_scheduled = true;
        cx.on_next_frame(window, |surface, window, cx| {
            surface.advance_transcript_scroll(window, cx);
        });
    }

    /// Applies one bounded smoothing step toward the wheel target.
    pub(super) fn advance_transcript_scroll(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.transcript_scroll_frame_scheduled = false;
        let offset = self.scroll_handle.offset();
        let maximum = f32::from(self.scroll_handle.max_offset().y).max(0.0);
        if let Some(next) = self.transcript_scroll.step(f32::from(offset.y), maximum) {
            self.scroll_handle.set_offset(point(offset.x, px(next)));
            cx.notify();
        }
        self.schedule_transcript_scroll_frame(window, cx);
    }

    /// Selects the reader's current marker from painted turn geometry.
    ///
    /// Pure over its inputs: markers carry their owning turn index from the
    /// scene-derived cache, so this touches only marker-bearing turns instead
    /// of walking every turn and block per frame. A turn counts as reached
    /// once its top passes the reference 96 px threshold below the viewport
    /// top; the last reached marker-bearing turn wins, else the first marker.
    /// Callers keep the result in window-local state with a change guard, so
    /// two windows sharing one surface converge independently and never
    /// ping-pong.
    ///
    /// Turn tops resolve to viewport coordinates through
    /// [`navigator_turn_top_viewport`]: content origin in window coordinates
    /// (`bounds.origin - element_offset`, the same quantity
    /// [`apply_painted_scroll_offset`](Self::apply_painted_scroll_offset)
    /// reproduces), re-based onto the viewport origin the host header
    /// offsets in the full app.
    pub(super) fn navigator_active_for_geometry(
        markers: &[NavigatorMarker],
        scroll_handle: &ScrollHandle,
        children_bounds: &[gpui::Bounds<gpui::Pixels>],
        window: &Window,
    ) -> Option<String> {
        if markers.is_empty() {
            return None;
        }
        let scroll_top = -f64::from(scroll_handle.offset().y);
        let element_offset = f64::from(window.element_offset().y);
        let viewport_top = f64::from(scroll_handle.bounds().origin.y);
        let mut reached: Vec<ConversationTurnOffset<'_>> = Vec::with_capacity(markers.len());
        // Markers arrive in ordinal order, so a turn's markers are adjacent;
        // the first marker per turn reproduces the previous per-turn walk.
        let mut last_turn: Option<usize> = None;
        for marker in markers {
            if last_turn == Some(marker.turn_index) {
                continue;
            }
            last_turn = Some(marker.turn_index);
            let Some(bounds) = children_bounds.get(marker.turn_index) else {
                continue;
            };
            let content_top = f64::from(bounds.origin.y) - element_offset;
            let top = Self::navigator_turn_top_viewport(content_top, scroll_top, viewport_top);
            reached.push(ConversationTurnOffset::new(
                navigator_target_slug(&marker.target),
                top,
            ));
        }
        active_conversation_turn(&reached).map(str::to_owned)
    }

    /// Rebases one content top onto the viewport origin.
    ///
    /// `content_top` is the turn origin in window coordinates, `scroll_top`
    /// the scrolled distance, and `viewport_top` the viewport origin the
    /// host header offsets in the full app (zero in standalone tests, which
    /// is why geometry tests must cover a nonzero origin explicitly).
    pub(super) fn navigator_turn_top_viewport(
        content_top: f64,
        scroll_top: f64,
        viewport_top: f64,
    ) -> f64 {
        content_top - scroll_top - viewport_top
    }

    /// Measures end-space height from live prepaint geometry, if measurable.
    ///
    /// Pure over its inputs: `children_bounds` holds every transcript child
    /// in order with the spacer last, so all but the last entry are turns
    /// and the last entry is the spacer itself. The last turn anchors the
    /// formula; an empty transcript yields `None` and the caller keeps
    /// whatever its window state holds. Origins cancel the element offset
    /// by subtraction, exactly like the painted scroll-offset path, so
    /// window and content spaces agree.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the end-space measurement is bounded f64 layout math narrowed to the f32 window state GPUI stores"
    )]
    pub(super) fn measured_end_space_height(
        children_bounds: &[gpui::Bounds<gpui::Pixels>],
        viewport_height: f64,
        element_offset_y: f64,
    ) -> Option<f32> {
        let (end_space, turns) = children_bounds.split_last()?;
        let first = turns.first()?;
        let last = turns.last()?;
        let content_height =
            f64::from(end_space.origin.y - first.origin.y) + f64::from(TRANSCRIPT_PAD_TOP_PX);
        if content_height <= viewport_height {
            return Some(0.0);
        }
        let item_top = f64::from(last.origin.y) - element_offset_y;
        let end_space_top = f64::from(end_space.origin.y) - element_offset_y;
        let measured = end_space_height(viewport_height, item_top, end_space_top);
        if !measured.is_finite() {
            return None;
        }
        Some(measured as f32)
    }

    /// Releases render-only scroll custody when the owning host retires.
    ///
    /// The queue contains only typed targets. The registry and its deferred
    /// paint token are also transient, so neither can outlive this surface's
    /// host ownership or revive a replacement surface.
    pub(crate) fn release_transient_scroll_custody(&mut self) {
        self.pending_scroll_targets.clear();
        self.executed_scroll_targets.clear();
        self.scroll_anchors.clear();
        self.scroll_anchor_paint_token = None;
    }

    /// Writes the exact anchor-equivalent offset for one executed target.
    ///
    /// GPUI records each anchor origin during Div prepaint as
    /// `bounds.origin - window.element_offset()` (gpui-0.2.2
    /// `src/elements/div.rs:1368-1369`) and `ScrollAnchor::scroll_to`
    /// applies `viewport_bounds.origin - last_origin` from an
    /// `on_next_frame` callback (`div.rs:3029-3037`). Only the `ScrollArea`
    /// viewport pushes a nonzero element offset on this path
    /// (`src/window.rs:2410-2412`), so subtracting the listener-observed
    /// offset from the measured child origin reproduces that private origin
    /// exactly. The write lands inside the test-harness dirty draw
    /// (`src/app.rs:1247`), where `on_next_frame` callbacks never run
    /// (`src/platform/test/window.rs:235`).
    pub(super) fn apply_painted_scroll_offset(
        &self,
        child_origin: gpui::Point<gpui::Pixels>,
        window: &Window,
    ) {
        let viewport_origin = self.scroll_handle.bounds().origin;
        let content_origin = child_origin - window.element_offset();
        let max_offset = self.scroll_handle.max_offset();
        let mut offset = viewport_origin - content_origin;
        offset.x = offset.x.clamp(-max_offset.x, px(0.0));
        offset.y = offset.y.clamp(-max_offset.y, px(0.0));
        self.scroll_handle.set_offset(offset);
    }

    /// Applies and retires the handoff against one listener's children.
    ///
    /// Targets apply in queue order so the last queued target wins, exactly
    /// like the equivalent chain of `ScrollAnchor::scroll_to` callbacks.
    /// Targets with no identity at this level stay queued for the remaining
    /// levels of the same frame; render clears any true orphans.
    pub(super) fn apply_executed_scroll_targets(
        &mut self,
        identities: &[(Option<SceneId>, Option<ItemId>)],
        children_bounds: &[gpui::Bounds<gpui::Pixels>],
        window: &Window,
    ) {
        if self.executed_scroll_targets.is_empty() {
            return;
        }
        let stashed = std::mem::take(&mut self.executed_scroll_targets);
        for target in stashed {
            let Some((_, bounds)) = identities
                .iter()
                .zip(children_bounds.iter())
                .find(|(identity, _)| scroll_target_matches_identity(&target, identity))
            else {
                self.executed_scroll_targets.push(target);
                continue;
            };
            self.apply_painted_scroll_offset(bounds.origin, window);
        }
    }

    /// Executes queued scroll targets against freshly rendered anchors.
    ///
    /// Painted matches run the GPUI anchor scroll and join the prepaint
    /// handoff; unpainted matches stay queued behind the paint gate and
    /// unknown targets drop as no-ops. Returns whether any scroll executed.
    pub(super) fn drain_painted_scroll_targets(
        &mut self,
        rendered_anchors: &[RenderedScrollAnchor],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let pending_targets = std::mem::take(&mut self.pending_scroll_targets);
        // The handoff is render-local: leftovers from a draw whose prepaint
        // never resolved them must not leak into a later frame.
        self.executed_scroll_targets.clear();
        let mut retained_targets = Vec::with_capacity(pending_targets.len());
        let mut waiting_for_paint = false;
        let mut scroll_executed = false;
        for target in pending_targets {
            if waiting_for_paint {
                retained_targets.push(target);
                continue;
            }

            match rendered_anchors
                .iter()
                .find(|rendered| rendered.matches(&target))
            {
                Some(rendered) if rendered.painted => {
                    rendered.anchor.scroll_to(window, cx);
                    self.executed_scroll_targets.push(target);
                    scroll_executed = true;
                }
                Some(_) => {
                    retained_targets.push(target);
                    waiting_for_paint = true;
                }
                None => {}
            }
        }
        self.pending_scroll_targets = retained_targets;
        scroll_executed
    }

    /// Returns the transcript-level identity pair for every turn root.
    ///
    /// Turn roots are the transcript's direct children in scene order.
    pub(super) fn turn_scroll_identities(&self) -> Vec<(Option<SceneId>, Option<ItemId>)> {
        self.scene
            .turn_scenes()
            .iter()
            .map(|turn| (SceneId::parse(turn.turn_id.as_str()).ok(), None))
            .collect()
    }

    /// Retries one geometry observation retained when the action queue was
    /// full. This is `pub(crate)` so the host can retry after draining its
    /// downstream effect queue without inventing another surface action.
    pub(crate) fn retry_pending_viewport_observation(&mut self, cx: &mut Context<Self>) {
        let Some(observation) = self.pending_viewport_observation.take() else {
            return;
        };

        if self.last_viewport_observation.as_ref() == Some(&observation) {
            return;
        }
        if self.enqueue_action(ConversationSurfaceAction::ViewportObserved(
            observation.clone(),
        )) {
            self.last_viewport_observation = Some(observation);
            cx.notify();
        } else {
            self.pending_viewport_observation = Some(observation);
        }
    }

    pub(super) fn enqueue_action(&mut self, action: ConversationSurfaceAction) -> bool {
        if self.actions.len() >= CONVERSATION_SURFACE_MAX_ACTIONS {
            return false;
        }
        self.actions.push(action);
        true
    }

    pub(super) fn observe_current_viewport(&mut self, cx: &mut Context<Self>) {
        self.retry_pending_viewport_observation(cx);

        let offset = self.scroll_handle.offset();
        let bounds = self.scroll_handle.bounds();
        let max_offset = self.scroll_handle.max_offset();
        let scroll_top = -f64::from(offset.y);
        let viewport_height = f64::from(bounds.size.height);
        let scroll_height = viewport_height + f64::from(max_offset.y);
        let geometry = ViewportGeometry {
            scroll_top,
            viewport_height,
            scroll_height,
        };
        if self.last_viewport_geometry == Some(geometry) {
            return;
        }

        self.last_viewport_geometry = Some(geometry);
        let observation = ViewportObservation {
            first_visible: None,
            last_visible: None,
            at_bottom: conversation_is_following(scroll_top, scroll_height, viewport_height),
        };
        let _ = self.observe_viewport(observation, cx);
    }

    pub(super) fn schedule_viewport_observation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.viewport_observation_scheduled {
            self.viewport_observation_scheduled = true;
            let entity = cx.entity().downgrade();
            // Read after GPUI has painted the current layout so the initial
            // geometry is observable even when an otherwise idle platform
            // does not deliver a later frame callback.
            window.defer(cx, move |_, app| {
                let _ = entity.update(app, |surface, cx| {
                    surface.viewport_observation_scheduled = false;
                    surface.observe_current_viewport(cx);
                });
            });
        }

        if self.viewport_next_frame_scheduled {
            return;
        }
        self.viewport_next_frame_scheduled = true;
        let entity = cx.entity().downgrade();
        window.on_next_frame(move |_, app| {
            let _ = entity.update(app, |surface, cx| {
                surface.viewport_next_frame_scheduled = false;
                surface.observe_current_viewport(cx);
            });
        });
    }
}
