//! Composer submission admission, message flight, retry, send receipts, and failure handling for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-2 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    pub(super) fn message_submission_is_admissible(&self, cx: &App) -> bool {
        matches!(self.route(), NativeRoute::Thread { project, thread }
            if self.selected_project.as_ref() == Some(project)
                && self.selected_thread.as_ref() == Some(thread))
            && self.message_composer_visible(cx)
    }

    pub(super) fn project_picker_action_is_admissible(&self) -> bool {
        !self.shutdown_prepared
            && !self.service_stopped
            && self.intake_stage.is_none()
            && self.thread_switch_flight.is_none()
            && self.ordinary_unsubscribe_thread.is_none()
    }

    pub(super) fn command_submission_is_available(&self) -> bool {
        #[cfg(test)]
        if self.test_command_sink.is_some() {
            return true;
        }
        self.service
            .as_ref()
            .is_some_and(|service| !service.is_finished())
    }

    pub(super) fn add_project_action_is_admissible(&self) -> bool {
        self.project_picker_action_is_admissible() && self.command_submission_is_available()
    }

    pub(super) fn message_composer_visible(&self, cx: &App) -> bool {
        let Some(selected_thread) = self.selected_thread.as_ref() else {
            return false;
        };
        self.conversation_host.as_ref().is_some_and(|host| {
            host.read(cx).controller_view().delivery.thread_id == *selected_thread
        }) && matches!(&self.state, NativeViewState::Ready)
            && self.intake_stage.is_none()
            && self.thread_switch_flight.is_none()
            && self.ordinary_unsubscribe_thread.is_none()
            && self.command_submission_is_available()
            && !self.service_stopped
    }

    pub(super) fn sync_composer_controls(&mut self, cx: &mut Context<Self>) {
        let mut snapshot = self.composer_controls.read(cx).snapshot().clone();
        snapshot.send_ready =
            self.message_submission_is_admissible(cx) && self.composer.read(cx).send_ready();
        snapshot.disabled = self.service_stopped;
        self.project_run_controls(&mut snapshot);
        crate::native_composer_queue::project_controls_snapshot(
            &self.composer_queue.state,
            &mut snapshot,
            !self.service_stopped
                && self.command_submission_is_available()
                && self.composer.read(cx).capture_recall_target().is_some(),
        );
        // Reference behavior (thread-composer.svelte:381-382, audit D4/C5):
        // Forge-held rows surface ONLY as pending-steering lip rows
        // (projected above) and refusals surface as the Dismiss failure
        // banner below. No count/status copy is ever shown, so no
        // `queue_status`/`queue_retry` companion is populated here.
        snapshot.new_thread_ready =
            snapshot.run_active && snapshot.send_ready && self.add_project_action_is_admissible();
        // A refusal names the attempt that produced it (reference
        // `action-failure.svelte`): the starting-run guard carries its exact
        // copy and offers Dismiss only, while transport failures keep the
        // generic copy with the draft-matched retry.
        let failure_note = self.message_failure_note.clone();
        let failure_retryable = failure_note.is_none()
            && self
                .message_retry
                .as_ref()
                .is_some_and(|retry| retry.draft_matches)
            && self.command_submission_is_available();
        snapshot.failure = self.message_failure.map(|notice| {
            crate::native_composer_controls::NativeComposerFailure::new(
                notice.id,
                "Could not send message",
                failure_note.clone().unwrap_or_else(|| {
                    "Your draft is preserved. Check the connection and try again.".to_owned()
                }),
                failure_retryable,
            )
        });
        if let Some(message) = self.composer_model_run_error.clone() {
            snapshot.failure = Some(crate::native_composer_controls::NativeComposerFailure::new(
                0,
                "Could not start with this model",
                message,
                false,
            ));
        }
        self.composer_controls
            .update(cx, |controls, cx| controls.set_snapshot(snapshot, cx));
    }

    pub(super) fn sync_composer_availability(&mut self, cx: &mut Context<Self>) {
        let image_thread = self
            .conversation_host
            .as_ref()
            .map(|host| host.read(cx).controller_view().delivery.thread_id);
        if self
            .message_images
            .update(cx, |images, cx| {
                images.set_current_thread(image_thread.as_ref(), cx)
            })
            .is_err()
        {
            self.state = NativeViewState::Failure(invalid_service_failure());
        }
        let disabled = !self.message_submission_is_admissible(cx);
        // The composer label uses the same full-format composer as the picker
        // trigger: `<name> <context> <effort> <speed>`, never a raw model id
        // when the durable selection can be projected back onto the catalog.
        let model_label = {
            let catalog = self.model_selector.read(cx).state().snapshot();
            self.engine_settings
                .authoritative_config()
                .and_then(|config| {
                    crate::composer_model_config::policy_for_selection(catalog, config)
                        .ok()
                        .map(|policy| {
                            crate::native_model_selector::model_display_label(catalog, &policy)
                                .plain_text()
                        })
                        .or_else(|| {
                            config
                                .selection()
                                .model_id()
                                .map(|model| model.as_str().to_owned())
                        })
                })
                .unwrap_or_else(|| "Select model".into())
        };
        self.composer.update(cx, |composer, composer_cx| {
            composer.set_attachment_delivery_enabled(true, composer_cx);
            composer.set_surface(disabled, model_label, composer_cx);
        });
        self.sync_composer_controls(cx);
        self.schedule_run_observation(cx);
        self.schedule_composer_queue(false, cx);
    }

    /// Resolves the displayed model policy to its durable engine
    /// configuration for a first send: either the explicit choice for this
    /// thread or the selector's current policy.
    ///
    /// Native choices without an explicit profile persist under the supported
    /// default profile, and admission runs against the readiness-overlaid
    /// catalog so a probed ambient account needs no managed registry.
    /// Returns the static blocking message when no policy is displayed or
    /// the displayed policy cannot become a run configuration.
    pub(super) fn first_send_config(
        &self,
        cx: &App,
    ) -> Result<artisan_domain::EngineRunConfig, String> {
        let displayed = match &self.composer_model_choice {
            Some((thread, policy)) if thread == &self.selected_thread => Some(policy.clone()),
            _ => self.model_selector.read(cx).state().policy().cloned(),
        };
        let Some(raw_policy) = displayed else {
            return Err("Select a model before sending. Your draft is preserved.".to_owned());
        };
        // The harness must be runnable before anything is persisted: an
        // unrunnable engine would only requeue after the save lands. The
        // reason names the probed account state instead of a catch-all.
        let policy = crate::composer_model_config::with_default_native_profile(&raw_policy);
        let catalog = self.effective_catalog_snapshot(cx);
        catalog
            .admit_policy(&policy)
            .map_err(|_| self.readiness_block_reason(&policy.engine_id))?;
        crate::composer_model_config::config_for_policy(
            &catalog,
            &policy,
            self.engine_settings.authoritative_config(),
        )
        .map_err(std::borrow::ToOwned::to_owned)
    }

    /// Admits a first send on a thread without a persisted engine
    /// configuration. Persistence is owned by selection time (the
    /// proactive typed save in [`Self::handle_composer_model_event`]);
    /// this gate never holds a send visibly for a save. The backend accept
    /// transaction snapshots durable settings and refuses unconfigured
    /// sends typed â€” never a silent queue â€” so the send proceeds whenever
    /// a runnable configuration can be computed, and is refused with its
    /// reason (draft preserved) only when none can be.
    pub(super) fn admit_first_send(&mut self, cx: &mut Context<Self>) -> FirstSendAdmission {
        if self.engine_settings.authoritative_config().is_some() {
            self.composer_model_run_error = None;
            return FirstSendAdmission::Proceed;
        }
        // Admission rests on the backend-probed account verdict: request a
        // fresh read for the displayed engine before evaluating it, so the
        // first send at startup, on selection, and from Settings observes
        // true readiness instead of an empty row.
        let displayed_engine = match &self.composer_model_choice {
            Some((thread, policy)) if thread == &self.selected_thread => {
                Some(policy.engine_id.clone())
            }
            _ => self
                .model_selector
                .read(cx)
                .state()
                .policy()
                .map(|policy| policy.engine_id.clone()),
        };
        if let Some(engine_id) = displayed_engine.as_deref() {
            self.ensure_profile_usage(false, Some(engine_id), cx);
        }
        if let Err(message) = self.first_send_config(cx) {
            self.composer_model_run_error = Some(message);
            self.sync_composer_controls(cx);
            cx.notify();
            return FirstSendAdmission::Held;
        }
        self.composer_model_run_error = None;
        FirstSendAdmission::Proceed
    }

    /// Names the observed live run for this send when the selected engine
    /// matches the running engine, generation-fenced on the selected
    /// thread. A cross-engine selection, an idle thread, or a still-starting
    /// (`Queued`) run stays unnamed so the backend takes the fresh-send
    /// path. The frontend names only; it never validates liveness.
    pub(super) fn observed_steer_target(&self) -> Option<artisan_domain::SteerTarget> {
        let (run_id, engine) = self
            .run_controls
            .steer_candidate(self.selected_thread.as_ref())?;
        let selected = self
            .engine_settings
            .authoritative_config()
            .map(|config| config.selection().engine_id())?;
        if engine != selected {
            return None;
        }
        Some(artisan_domain::SteerTarget::new(run_id))
    }

    /// Captures the validated routed engine display label at send time.
    ///
    /// Resolved from the *authoritative* config only â€” never from the
    /// picker/choice, which may change after the send. `None` (unconfigured
    /// thread) renders the generic Waiting fallback. Provider-owned roster
    /// names with id fallback, matching the existing user-facing copy.
    pub(super) fn send_engine_label(&self) -> Option<String> {
        let engine = self
            .engine_settings
            .authoritative_config()?
            .selection()
            .engine_id();
        Some(profile_usage_display_name(engine.as_str()).to_owned())
    }

    pub(super) fn begin_message_submission(&mut self, cx: &mut Context<Self>) {
        if !self.message_submission_is_admissible(cx) || self.message_flight.is_some() {
            return;
        }
        // Reference (`commands.ts:158-165`): a send never enters a starting
        // run's queue from this UI. The refusal keeps the draft and names
        // the attempt; the user presses Send again once the run is live.
        if self
            .run_controls
            .starting_guard_active(self.selected_thread.as_ref())
        {
            self.message_failure = Some(NativeMessageFailure::new(ServiceFailure {
                stage: ServiceFailureStage::Request,
                category: ServiceFailureCategory::InvalidConfiguration,
            }));
            self.message_failure_note = Some(
                "The current run is still starting. Wait before sending another message."
                    .to_owned(),
            );
            self.sync_composer_availability(cx);
            cx.notify();
            return;
        }
        // The choice-versus-saved check is only meaningful once a thread
        // carries a persisted configuration. On an unconfigured thread it
        // would always fail and strand explicit selections; admission below
        // owns unconfigured sends.
        if self.engine_settings.authoritative_config().is_some()
            && let Some((thread, policy)) = &self.composer_model_choice
            && thread == &self.selected_thread
        {
            self.composer_model_run_error = crate::composer_model_config::validate_run_choice(
                &self.effective_catalog_snapshot(cx),
                policy,
                self.engine_settings.authoritative_config(),
            )
            .err()
            .map(std::borrow::ToOwned::to_owned);
            if self.composer_model_run_error.is_some() {
                self.sync_composer_controls(cx);
                cx.notify();
                return;
            }
        }
        // First-send admission never holds for a save: the selection-time
        // proactive save owns persistence, and the backend accept
        // transaction snapshots durable settings (refusing unconfigured
        // sends typed). Only an uncomputable configuration refuses here.
        if !matches!(self.admit_first_send(cx), FirstSendAdmission::Proceed) {
            return;
        }
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        self.clear_message_retry();
        let submission = self
            .composer
            .update(cx, |composer, _| composer.begin_payload_submission());
        let (body, token) = match submission {
            Ok(submission) => submission,
            Err(blocked) => {
                if let Some(failure) = submission_blocked_failure(blocked) {
                    self.message_failure = Some(NativeMessageFailure::new(failure));
                    self.message_failure_note = None;
                }
                cx.notify();
                return;
            }
        };
        self.message_receipt = None;
        self.message_failure = None;
        self.message_failure_note = None;
        let request_id = match create_message_request_id() {
            Ok(request_id) => request_id,
            Err(failure) => {
                self.reject_message_submission(token, failure, cx);
                return;
            }
        };
        let steer_target = self.observed_steer_target();
        let engine_label = self.send_engine_label();
        let mut queued =
            artisan_domain::QueueMessage::new(request_id.clone(), thread_id.clone(), body.clone());
        if let Some(target) = steer_target.clone() {
            queued = queued.with_steer_target(target);
        }
        let command = NativeTransportCommand::QueueMessage(Box::new(queued));
        match self.submit_command(command) {
            Ok(()) => {
                self.message_flight = Some(NativeMessageFlight {
                    thread_id,
                    request_id,
                    payload: body,
                    steer_target,
                    engine_label,
                    token,
                });
            }
            Err(error) => {
                self.reject_message_submission(token, command_failure(error), cx);
            }
        }
        self.sync_composer_availability(cx);
        cx.notify();
    }

    pub(super) fn message_retry_context_is_admissible(&self, cx: &App) -> bool {
        let Some(retry) = self.message_retry.as_ref() else {
            return false;
        };
        self.selected_thread.as_ref() == Some(&retry.thread_id)
            && self.message_flight.is_none()
            && self.message_submission_is_admissible(cx)
            && !self.composer.read(cx).is_submitting()
    }

    #[cfg(test)]
    pub(super) fn message_retry_is_admissible(&self, cx: &App) -> bool {
        self.message_retry_context_is_admissible(cx)
            && self
                .message_retry
                .as_ref()
                .is_some_and(|retry| retry.draft_matches)
    }

    pub(super) fn observe_composer_change(
        &mut self,
        composer: &Entity<NativeComposer>,
        cx: &mut Context<Self>,
    ) {
        self.sync_composer_controls(cx);
        let Some(retry) = self.message_retry.as_mut() else {
            return;
        };
        if self.message_flight.is_some() {
            return;
        }
        let matches = composer.read(cx).draft_matches_payload(&retry.payload);
        retry.draft_matches = matches;
        cx.notify();
    }

    pub(super) fn clear_message_retry(&mut self) {
        self.message_retry = None;
        self.message_retry_focus_handle = self.message_retry_focus_handle.clone().tab_stop(false);
    }

    pub(super) fn activate_message_retry(&mut self, cx: &mut Context<Self>) {
        if self.service_stopped || !self.command_submission_is_available() {
            self.service_stopped = true;
            self.set_picker_disabled(true, cx);
            self.set_thread_picker_disabled(true, cx);
            self.set_failure(command_failure(CommandSendError::Stopped), cx);
            return;
        }
        if !self.message_retry_context_is_admissible(cx) {
            return;
        }
        let Some(retry) = self.message_retry.as_ref() else {
            return;
        };
        let thread_id = retry.thread_id.clone();
        let request_id = retry.request_id.clone();
        let retry_body = retry.payload.clone();
        // Root freeze: a retry replays the WHOLE original command,
        // including its steer target. It never re-resolves the current
        // live run â€” a stale explicit target fails typed server-side with
        // the payload preserved, never a silent fresh run.
        let retry_target = retry.steer_target.clone();
        let retry_label = retry.engine_label.clone();
        let submission = self
            .composer
            .update(cx, |composer, _| composer.begin_payload_submission());
        let (body, token) = match submission {
            Ok(submission) => submission,
            Err(blocked) => {
                if let Some(retry) = self.message_retry.as_mut() {
                    retry.draft_matches = false;
                }
                if let Some(failure) = submission_blocked_failure(blocked) {
                    self.message_failure = Some(NativeMessageFailure::new(failure));
                }
                cx.notify();
                return;
            }
        };
        if body != retry_body {
            if let Some(retry) = self.message_retry.as_mut() {
                retry.draft_matches = false;
            }
            self.finish_composer_submission(token, DraftDisposition::Retained, cx);
            self.sync_composer_availability(cx);
            cx.notify();
            return;
        }

        self.message_receipt = None;
        self.message_failure = None;
        self.message_failure_note = None;
        let mut queued = artisan_domain::QueueMessage::new(
            request_id.clone(),
            thread_id.clone(),
            retry_body.clone(),
        );
        if let Some(target) = retry_target.clone() {
            queued = queued.with_steer_target(target);
        }
        let command = NativeTransportCommand::QueueMessage(Box::new(queued));
        match self.submit_command(command) {
            Ok(()) => {
                self.clear_message_retry();
                self.message_flight = Some(NativeMessageFlight {
                    thread_id,
                    request_id,
                    payload: retry_body,
                    steer_target: retry_target,
                    engine_label: retry_label,
                    token,
                });
            }
            Err(CommandSendError::Busy) => {
                self.finish_composer_submission(token, DraftDisposition::Retained, cx);
                self.message_failure = Some(NativeMessageFailure::new(command_failure(
                    CommandSendError::Busy,
                )));
            }
            Err(CommandSendError::Stopped) => {
                self.finish_composer_submission(token, DraftDisposition::Retained, cx);
                self.clear_message_retry();
                self.service_stopped = true;
                self.set_picker_disabled(true, cx);
                self.set_thread_picker_disabled(true, cx);
                self.set_failure(command_failure(CommandSendError::Stopped), cx);
                return;
            }
        }
        self.sync_composer_availability(cx);
        cx.notify();
    }

    pub(super) fn finish_composer_submission(
        &mut self,
        token: SubmissionToken,
        disposition: DraftDisposition,
        cx: &mut Context<Self>,
    ) {
        self.composer.update(cx, |composer, composer_cx| {
            composer.finish_submission(token, disposition, composer_cx);
        });
    }

    pub(super) fn reject_message_submission(
        &mut self,
        token: SubmissionToken,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        self.finish_composer_submission(token, DraftDisposition::Retained, cx);
        self.message_failure = Some(NativeMessageFailure::new(failure));
        self.message_failure_note = None;
        self.sync_composer_availability(cx);
        cx.notify();
    }

    pub(super) fn retain_message_flight(&mut self, cx: &mut Context<Self>) {
        if let Some(flight) = self.message_flight.take() {
            self.finish_composer_submission(flight.token, DraftDisposition::Retained, cx);
        }
        self.clear_message_retry();
        self.sync_composer_availability(cx);
    }

    pub(super) fn clear_message_presentation(&mut self) {
        self.clear_message_retry();
        self.message_receipt = None;
        self.message_failure = None;
        self.message_failure_note = None;
    }

    /// Clears transient service-owned state for a terminal transport failure.
    ///
    /// In-flight queue/failed/usage reads and observed run activity die with
    /// the service: the lip leaves a stuck Refreshing for a truthful
    /// `TransportFailed` and the stop control stops claiming an unobservable
    /// run. Drafts, restore candidates, failure notices, and the mounted
    /// transcript are preserved. No retry is scheduled here.
    pub(super) fn clear_transient_service_state(&mut self) {
        self.drop_transient_service_reads();
        self.run_controls.clear_transient_observation();
    }

    pub(super) fn handle_message_receipt(
        &mut self,
        receipt: QueueMessageReceipt,
        cx: &mut Context<Self>,
    ) {
        let Some(flight) = self.message_flight.as_ref() else {
            return;
        };
        if self.selected_thread.as_ref() != Some(&flight.thread_id)
            || receipt.thread_id != flight.thread_id
            || receipt.request_id != flight.request_id
            || !matches!(
                receipt.disposition,
                artisan_domain::ReceiptDisposition::Accepted
                    | artisan_domain::ReceiptDisposition::Duplicate
            )
        {
            return;
        }
        let flight = self
            .message_flight
            .take()
            .expect("flight was checked above");
        let message_id = receipt.message_id.clone();
        let steer_run_id = flight
            .steer_target
            .as_ref()
            .map(|target| target.run_id().clone());
        let engine_label = flight.engine_label;
        self.finish_composer_submission(flight.token, DraftDisposition::Accepted, cx);
        self.message_receipt = Some(receipt);
        // Stage the echo watch now that the Forge message id is known: the
        // lip retires and the engine label dispatches when the canonical
        // user item carrying this source id projects (frozen correlation
        // contract).
        self.composer_queue
            .state
            .stage_echo_watch(message_id.clone(), steer_run_id, engine_label);
        // The echo can already be projected (patch stream versus receipt
        // race): scan the current canonical snapshot so an already-present
        // echo retires immediately instead of waiting for the next batch.
        if let Some(host) = self.conversation_host.clone()
            && host.read(cx).controller_view().delivery.thread_id == flight.thread_id
            && let Some(snapshot) = host.read(cx).canonical_snapshot()
        {
            for item in snapshot.items() {
                let (source_id, turn_id) = match item {
                    ConversationItem::UserMessage(message) => {
                        (message.source_message_id.as_ref(), message.turn_id.clone())
                    }
                    ConversationItem::MultimodalUserMessage(message) => {
                        (message.source_message_id.as_ref(), message.turn_id.clone())
                    }
                    // Assistant messages are the only other item kind and
                    // never echo a send.
                    ConversationItem::AssistantMessage(_) => continue,
                };
                if source_id == Some(&message_id) {
                    self.retire_echo_matched(&flight.thread_id, &message_id, turn_id, &host, cx);
                }
            }
        }
        self.message_failure = None;
        self.message_failure_note = None;
        self.sync_composer_availability(cx);
        cx.notify();
    }

    pub(super) fn handle_message_failure(
        &mut self,
        thread_id: &ThreadId,
        request_id: &RequestId,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        let matches_active = self.message_flight.as_ref().is_some_and(|flight| {
            &flight.thread_id == thread_id
                && &flight.request_id == request_id
                && self.selected_thread.as_ref() == Some(thread_id)
        });
        if !matches_active {
            return;
        }
        let flight = self
            .message_flight
            .take()
            .expect("flight was checked above");
        self.finish_composer_submission(flight.token, DraftDisposition::Retained, cx);
        self.message_retry = Some(NativeMessageRetry {
            thread_id: flight.thread_id,
            request_id: flight.request_id,
            payload: flight.payload,
            steer_target: flight.steer_target,
            engine_label: flight.engine_label,
            draft_matches: false,
        });
        // A failure matches only an active flight, which never staged a
        // watch (watches stage at receipt, which ends the flight). A retry
        // stages its own watch at its own receipt with the preserved
        // original label.
        self.message_receipt = None;
        self.message_failure = Some(NativeMessageFailure::new(failure));
        self.message_failure_note = None;
        self.sync_composer_availability(cx);
        cx.notify();
    }

    /// Retains any admitted message before the application starts service
    /// shutdown. This runs on the GPUI application thread.
    pub(super) fn prepare_shutdown(&mut self, cx: &mut Context<Self>) {
        self.shutdown_prepared = true;
        self.thread_switch_flight = None;
        self.ordinary_unsubscribe_thread = None;
        self.pending_thread = None;
        self.pending_failed_recovery = None;
        self.set_picker_disabled(true, cx);
        self.set_thread_picker_disabled(true, cx);
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.composer.update(cx, |composer, composer_cx| {
            composer.set_disabled(true, composer_cx);
        });
        cx.notify();
    }

    /// Drains one queued answer batch into live transport, once per tick.
    ///
    /// Runs at the head of the controller tick beside the sibling drains, so
    /// a slow or failing transport cannot stall unrelated per-tick work.
    /// Admission follows the established submit path: without it the outbox
    /// is left untouched. Each taken dispatch submits once with its
    /// already-minted request id; `Busy`/`Stopped` keep rows pending with the
    /// existing retry/diagnostic texts, and single-flight holds until
    /// receipts pair through the existing settle-in-place pairing. Draining
    /// first also keeps a same-tick host retirement from dropping gestures.
    pub(super) fn drain_answer_dispatches(&mut self, cx: &mut Context<Self>) {
        if !self.command_submission_is_available() {
            return;
        }
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        let surface = host.read(cx).surface().clone();
        let this = &*self;
        surface.update(cx, |surface, _| {
            surface.drain_pending_answer_dispatches(&mut |command| this.submit_command(command));
        });
    }
}
