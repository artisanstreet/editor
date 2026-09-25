//! Composer submission admission, message flight, send receipts, and failure
//! handling for [`NativeApplication`].
//!
//! A send is a request, not local state: the Forge accepts it and owns the
//! message from then on. The flight keeps only the request identity and the
//! connection hold until the receipt; the rows the transcript shows come
//! from the Forge message outbox (see `forge_outbox`). There is no local
//! retry copy: a failed request leaves the draft in the composer, and a
//! failed delivery is retried by the Forge from its stored payload.

use super::*;

impl NativeApplication {
    pub(super) fn message_submission_is_admissible(&self, cx: &App) -> bool {
        matches!(self.route(), NativeRoute::Thread { project, thread }
            if self.selected_project.as_ref() == Some(project)
                && self.selected_thread.as_ref() == Some(thread))
            && self.message_composer_visible(cx)
            && !self.host_switch_pending()
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
        self.settle_message_flight_hold(None);
        self.sync_pending_rows(cx);
        let mut snapshot = self.composer_controls.read(cx).snapshot().clone();
        snapshot.send_ready =
            self.message_submission_is_admissible(cx) && self.composer.read(cx).send_ready();
        snapshot.disabled = self.service_stopped;
        self.project_run_controls(&mut snapshot);
        // The Forge decides which failures are still offered (a later
        // delivered message supersedes one) and whether each is retryable;
        // the rows are projected unfiltered.
        crate::native_composer_queue::project_controls_snapshot(
            &self.composer_queue.state,
            &mut snapshot,
            !self.service_stopped
                && self.command_submission_is_available()
                && self.project_picker_action_is_admissible(),
        );
        snapshot.pending_steering.clear();
        // Reference behavior (thread-composer.svelte:381-382, audit D4/C5):
        // Forge-held rows surface ONLY as pending-steering lip rows
        // (projected above) and refusals surface as the Dismiss failure
        // banner below. No count/status copy is ever shown, so no
        // `queue_status`/`queue_retry` companion is populated here.
        snapshot.new_thread_ready =
            snapshot.run_active && snapshot.send_ready && self.add_project_action_is_admissible();
        // A refusal names the attempt that produced it (reference
        // `action-failure.svelte`): the starting-run guard carries its exact
        // copy. A failed request keeps the draft in the composer, so the
        // banner offers Dismiss only and Send submits the draft again.
        let failure_note = self.message_failure_note.clone();
        snapshot.failure = self.message_failure.map(|notice| {
            crate::native_composer_controls::NativeComposerFailure::new(
                notice.id,
                "Could not send message",
                failure_note.clone().unwrap_or_else(|| {
                    match notice.failure.category {
                        ServiceFailureCategory::Peer => "Forge could not accept this message. Your draft is preserved; review the model settings and try again.",
                        ServiceFailureCategory::InvalidConfiguration => "The selected settings could not be used. Your draft is preserved; review the model settings and try again.",
                        _ => "Your draft is preserved. Check the connection and try again.",
                    }.to_owned()
                }),
                false,
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
        self.schedule_composer_queue(cx);
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
    /// configuration. Selection-time saves are reused; an inherited default
    /// is saved here before submitting the message.
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
        let config = match self.first_send_config(cx) {
            Ok(config) => config,
            Err(message) => {
                self.composer_model_run_error = Some(message);
                self.sync_composer_controls(cx);
                cx.notify();
                return FirstSendAdmission::Held;
            }
        };
        if self
            .engine_settings
            .pending_save()
            .is_some_and(|(_, pending)| pending != &config)
        {
            self.composer_model_run_error = Some(
                "The selected model settings are still saving. Your draft is preserved; try again once saving finishes.".to_owned(),
            );
            self.sync_composer_controls(cx);
            cx.notify();
            return FirstSendAdmission::Held;
        }
        // A default/inherited picker choice emits no selection event. Save
        // it on the same ordered command stream before the first message.
        if self.engine_settings.pending_save_request_id().is_none()
            && let Some(thread) = self.selected_thread.clone()
            && !self.submit_direct_save(thread, config)
        {
            self.composer_model_run_error = Some(
                "Could not save the selected model settings. Your draft is preserved; try again."
                    .to_owned(),
            );
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
        // The Forge holds an accepted message until its engine can run it and
        // reports the waiting reason on the outbox row, so a send is never
        // held here for an account check. The choice-versus-saved check is only meaningful once a thread
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
        // The Forge sends the stored draft, so every image must be stored.
        if !self.composer.read(cx).unstored_attachments().is_empty() {
            self.message_failure = Some(NativeMessageFailure::new(ServiceFailure {
                stage: ServiceFailureStage::Request,
                category: ServiceFailureCategory::InvalidConfiguration,
            }));
            self.message_failure_note = Some(
                "Images are still uploading. Your draft is preserved; send again in a moment."
                    .to_owned(),
            );
            self.sync_composer_controls(cx);
            cx.notify();
            return;
        }
        // The body being sent, captured before the composer clears it.
        self.sync_composer_draft(cx);
        let body = self.composer.read(cx).draft_body();
        let submission = self
            .composer
            .update(cx, |composer, _| composer.begin_payload_submission());
        let (_, token) = match submission {
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
        let flight = NativeMessageFlight {
            thread_id: thread_id.clone(),
            request_id,
            token,
        };
        self.launch_message_flight(flight, cx);
        self.begin_draft_submission(&thread_id, body, cx);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    pub(super) fn observe_composer_change(&mut self, cx: &mut Context<Self>) {
        self.sync_composer_controls(cx);
        self.sync_composer_draft(cx);
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

    /// Starts an admitted message flight: it holds the connection until its
    /// reply arrives. The transcript row appears when the Forge's outbox
    /// carries the accepted message.
    pub(super) fn launch_message_flight(
        &mut self,
        flight: NativeMessageFlight,
        cx: &mut Context<Self>,
    ) {
        self.message_flight = Some(flight);
        self.message_flight_hold = self.connection_hold(HoldKind::Message);
        self.follow_transcript_tail(cx);
    }

    pub(super) fn retain_message_flight(&mut self, cx: &mut Context<Self>) {
        if let Some(flight) = self.message_flight.take() {
            let scope = artisan_domain::ComposerDraftScope::Thread(flight.thread_id.clone());
            self.end_draft_submission(&scope);
            self.finish_composer_submission(flight.token, DraftDisposition::Retained, cx);
        }
        self.sync_composer_availability(cx);
    }

    pub(super) fn clear_message_presentation(&mut self) {
        self.message_receipt = None;
        self.message_failure = None;
        self.message_failure_note = None;
    }

    /// Clears transient service-owned state for a terminal transport failure.
    ///
    /// An in-flight usage read and observed run activity die with the
    /// service: the queue reports a truthful `TransportFailed` and the stop
    /// control stops claiming an unobservable run. Drafts, the Forge's last
    /// outbox rows, failure notices, and the mounted transcript are
    /// preserved. No retry is scheduled here.
    pub(super) fn clear_transient_service_state(&mut self) {
        self.drop_transient_service_reads();
        self.run_controls.clear_transient_observation();
        self.release_composer_drafts();
    }

    pub(super) fn handle_message_receipt(
        &mut self,
        receipt: QueueMessageReceipt,
        cx: &mut Context<Self>,
    ) {
        self.settle_message_flight_hold(Some(&receipt.request_id));
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
        // The Forge owns the message now: the composer clears, and the row
        // the transcript shows is the one its outbox carries.
        self.finish_composer_submission(flight.token, DraftDisposition::Accepted, cx);
        self.message_receipt = Some(receipt);
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
        self.settle_message_flight_hold(Some(request_id));
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
        // The draft stays in the composer; sending again is a new request.
        self.finish_composer_submission(flight.token, DraftDisposition::Retained, cx);
        self.message_receipt = None;
        self.message_failure = Some(NativeMessageFailure::new(failure));
        self.message_failure_note = None;
        self.sync_composer_availability(cx);
        cx.notify();
    }
}
