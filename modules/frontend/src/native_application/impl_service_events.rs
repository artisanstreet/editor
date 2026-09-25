//! Transport service event pump, subscription lifecycle, thread-switch machinery, and project intake handling for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-2 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    pub(super) fn poll_service(&mut self, cx: &mut Context<Self>) -> bool {
        self.drain_answer_dispatches(cx);
        let Some(service) = self.service.clone() else {
            return false;
        };
        let mut events = Vec::with_capacity(64);
        let mut channel_closed = false;
        loop {
            match service.try_recv() {
                Ok(Some(event)) => events.push(event),
                Ok(None) => break,
                Err(EventReceiveError::Stopped) => {
                    channel_closed = true;
                    break;
                }
            }
        }
        for event in events {
            self.handle_service_event(event, cx);
        }
        // Drain the final Failed/Stopped events before treating sender closure
        // as a generic bridge error; otherwise the real failure is discarded.
        if channel_closed && !self.service_stopped {
            if !matches!(self.state, NativeViewState::Failure(_)) {
                self.set_failure(
                    ServiceFailure {
                        stage: ServiceFailureStage::EventBridge,
                        category: ServiceFailureCategory::ChannelClosed,
                    },
                    cx,
                );
            }
            self.handle_service_stopped(ServiceStopStatus::Failed, cx);
        }
        self.refresh_sidebar_threads();
        self.retry_thread_switch_if_admitted(cx);
        self.try_mount_pending_thread(cx);
        self.sync_composer_availability(cx);
        !self.service_stopped
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive dispatch keeps every route visible in a single reviewable match"
    )]
    pub(super) fn handle_service_event(
        &mut self,
        event: NativeTransportEvent,
        cx: &mut Context<Self>,
    ) {
        if self.shutdown_prepared || self.service_stopped {
            return;
        }
        match event {
            NativeTransportEvent::ComposerState(event) => {
                self.handle_composer_state_event(event, cx);
            }
            NativeTransportEvent::ComposerDraft(event) => self.receive_draft_event(event, cx),
            NativeTransportEvent::ActiveRun {
                thread_id,
                generation,
                result,
            } => {
                self.receive_active_run(&thread_id, generation, Ok(result), cx);
            }
            NativeTransportEvent::ActiveRunFailed {
                thread_id,
                generation,
                failure,
            } => {
                self.receive_active_run(&thread_id, generation, Err(failure), cx);
            }
            NativeTransportEvent::RunStopped(receipt) => self.receive_run_stop(Ok(receipt), cx),
            NativeTransportEvent::StopRunFailed { command, failure } => {
                self.receive_run_stop_failure(&command, failure, cx);
            }
            NativeTransportEvent::MessageImageLoaded { reference, image } => {
                self.message_images.update(cx, |images, cx| {
                    images.accept_image(&reference, &image, cx);
                });
            }
            NativeTransportEvent::MessageImageFailed { reference, failure } => {
                self.message_images.update(cx, |images, cx| {
                    images.fail_image(reference, failure, cx);
                });
            }
            NativeTransportEvent::Starting => {
                self.state = NativeViewState::Loading;
                self.reset_profile_usage_for_connection();
                // Re-inspect repository facts for the new connection so a
                // retained decoration cannot outlive its session.
                self.titlebar_repository_project = None;
                self.sync_composer_availability(cx);
                cx.notify();
            }
            NativeTransportEvent::Projects(listing) => self.handle_projects(&listing, cx),
            NativeTransportEvent::Threads {
                project_id,
                listing,
            } => self.handle_threads(&project_id, &listing, cx),
            NativeTransportEvent::SidebarThreads {
                project_id,
                generation,
                result,
            } => {
                self.receive_sidebar_threads(&project_id, generation, result, cx);
            }
            NativeTransportEvent::Snapshot(snapshot) => self.handle_snapshot(snapshot, cx),
            NativeTransportEvent::ProjectIntakeProgress(stage) => {
                self.handle_intake_progress(stage, cx);
            }
            NativeTransportEvent::ProjectIntakeCancelled => self.handle_intake_cancelled(cx),
            NativeTransportEvent::ProjectIntakeReady {
                projects,
                project_id,
                threads,
                thread_id,
            } => self.handle_intake_ready(&projects, project_id, &threads, thread_id, cx),
            NativeTransportEvent::ProjectIntakeFailed {
                operation,
                failure,
                retryable,
            } => self.handle_intake_failed(operation, failure, retryable, cx),
            NativeTransportEvent::EmptyProjects => self.handle_empty_projects(cx),
            NativeTransportEvent::EmptyThreads { project_id } => {
                self.handle_empty_threads(&project_id, cx);
            }
            NativeTransportEvent::Failed(failure) => {
                self.retain_message_flight(cx);
                self.clear_transient_service_state();
                self.thread_switch_flight = None;
                self.ordinary_unsubscribe_thread = None;
                self.pending_thread = None;
                self.set_picker_disabled(true, cx);
                self.set_thread_picker_disabled(true, cx);
                self.reset_profile_usage_for_connection();
                self.set_failure(failure, cx);
            }
            NativeTransportEvent::ThreadEngineSettings { generation, result } => {
                self.handle_engine_settings(generation, result, cx);
            }
            NativeTransportEvent::RegisteredProfiles(result) => {
                self.handle_registered_profiles(result, cx);
            }
            NativeTransportEvent::RegisteredProfilesFailed(failure) => {
                self.handle_registered_profiles_failed(failure, cx);
            }
            NativeTransportEvent::AccountUsage {
                engine_id,
                generation,
                request_seq,
                entry,
            } => self.handle_account_usage(&engine_id, generation, request_seq, entry, cx),
            NativeTransportEvent::AccountUsageFailed {
                engine_id,
                generation,
                request_seq,
                failure,
            } => self.handle_account_usage_failed(&engine_id, generation, request_seq, failure, cx),
            NativeTransportEvent::ComposerCatalog {
                thread_id,
                profile_id,
                generation,
                result,
            } => self.handle_composer_catalog(thread_id, profile_id, generation, &result, cx),
            NativeTransportEvent::ComposerCatalogFailed {
                thread_id,
                profile_id,
                generation,
                failure,
            } => {
                self.handle_composer_catalog_failed(thread_id, profile_id, generation, failure, cx);
            }
            NativeTransportEvent::ModelFavorites {
                thread_id,
                profile_id,
                generation,
                result,
            } => self.handle_model_favorites(thread_id, profile_id, generation, &result, cx),
            NativeTransportEvent::ModelFavoritesFailed {
                thread_id,
                profile_id,
                generation,
                failure,
            } => self.handle_model_favorites_failed(thread_id, profile_id, generation, failure, cx),
            NativeTransportEvent::ModelFavoriteSet {
                thread_id,
                profile_id,
                request_id,
                receipt,
            } => self.handle_model_favorite_set(thread_id, profile_id, request_id, &receipt, cx),
            NativeTransportEvent::ModelFavoriteFailed {
                thread_id,
                profile_id,
                request_id,
                failure,
            } => self.handle_model_favorite_failed(thread_id, profile_id, request_id, failure, cx),
            NativeTransportEvent::RichLinkResolved {
                favicon,
                requested_url,
                page_name,
                expires_at_ms,
            } => self.handle_rich_link_resolved(
                &requested_url,
                &page_name,
                expires_at_ms,
                &favicon,
                cx,
            ),
            NativeTransportEvent::RichLinkFailed { requested_url, .. } => {
                self.handle_rich_link_failed(&requested_url, cx);
            }
            NativeTransportEvent::ProjectRepository {
                project_id,
                repository,
            } => self.handle_project_repository(&project_id, repository.as_ref(), cx),
            NativeTransportEvent::ProjectRepositoryFailed { project_id, .. } => {
                self.handle_project_repository_failure(&project_id, cx);
            }
            NativeTransportEvent::ThreadEngineConfigSet(result, retained) => {
                self.handle_engine_config_set(&result, *retained, cx);
            }
            NativeTransportEvent::ThreadEngineConfigConflict {
                thread_id,
                request_id,
            } => {
                self.handle_engine_conflict(thread_id, &request_id, cx);
            }
            NativeTransportEvent::ThreadEngineConfigFailed {
                thread_id,
                request_id,
                failure,
            } => {
                self.handle_engine_config_failed(&thread_id, &request_id, failure, cx);
            }
            NativeTransportEvent::ThreadEngineSettingsFailed {
                thread_id,
                generation,
                failure,
            } => {
                self.handle_engine_settings_failed(thread_id, generation, failure, cx);
            }
            // The shipping composer uses QueueMessage. Legacy first-message
            // results cannot settle a flight from the newer command family.
            NativeTransportEvent::FirstMessageQueued(_)
            | NativeTransportEvent::FirstMessageFailed { .. } => {}
            // Answer receipts and failures settle their row gates through the
            // existing transport pairing policy.
            NativeTransportEvent::ApprovalAnswered { .. }
            | NativeTransportEvent::ApprovalFailed { .. }
            | NativeTransportEvent::QuestionAnswered { .. }
            | NativeTransportEvent::QuestionFailed { .. } => self.handle_answer_event(event, cx),
            NativeTransportEvent::MessageQueued(receipt) => {
                self.handle_message_receipt(receipt, cx);
                self.schedule_composer_queue(true, cx);
            }
            NativeTransportEvent::MessageFailed {
                thread_id,
                request_id,
                failure,
            } => {
                self.handle_message_failure(&thread_id, &request_id, failure, cx);
            }
            NativeTransportEvent::ConversationSubscriptionStarted {
                thread_id,
                request_id,
                started,
            } => self.handle_subscription_started(&thread_id, &request_id, started, cx),
            NativeTransportEvent::ConversationSubscriptionStopped {
                thread_id,
                request_id,
                stopped,
            } => self.handle_subscription_stopped(&thread_id, &request_id, &stopped, cx),
            NativeTransportEvent::PatchBatch(batch) => self.handle_patch_batch(&batch, cx),
            NativeTransportEvent::EngineObservation(observation) => {
                self.handle_engine_observation(&observation, cx);
            }
            NativeTransportEvent::MessageOutbox(_) => {}
            NativeTransportEvent::DeliveryLost(failure) => self.handle_delivery_lost(failure, cx),
            NativeTransportEvent::Stopped(status) => self.handle_service_stopped(status, cx),
        }
    }

    pub(super) fn handle_empty_projects(&mut self, cx: &mut Context<Self>) {
        self.pending_thread = None;
        self.pending_snapshot = None;
        self.thread_listing = None;
        if self.thread_switch_flight.is_none() {
            self.retained_switch_listings.clear();
        }
        self.install_thread_picker(empty_thread_listing(), None, cx);
        if self.thread_switch_flight.is_some() {
            self.handle_removed_thread_during_switch(cx);
        } else {
            self.retire_host(cx);
        }
        self.state = NativeViewState::EmptyProjects;
        cx.notify();
    }

    pub(super) fn handle_empty_threads(&mut self, project_id: &ProjectId, cx: &mut Context<Self>) {
        if self.selected_project.as_ref() != Some(project_id) {
            return;
        }
        self.clear_message_retry();
        self.pending_thread = None;
        self.pending_snapshot = None;
        let listing = empty_thread_listing();
        if self.selected_thread.is_some() {
            self.remember_switch_listing();
        }
        self.thread_listing = Some(listing.clone());
        self.update_thread_picker(listing, None, cx);
        if self.thread_switch_flight.is_some() {
            self.handle_removed_thread_during_switch(cx);
        } else if self.conversation_host.is_some() && self.selected_thread.is_some() {
            self.begin_thread_retirement(cx);
        } else {
            self.selected_thread = None;
            self.composer.update(cx, |composer, cx| {
                composer.switch_thread(&format!("project:{}", project_id.as_str()), false, cx);
            });
            self.state = NativeViewState::EmptyThreads;
            self.sync_thread_picker_selected(cx);
            self.engine_settings.select_thread(None);
            self.reset_composer_catalog(cx);
            self.sync_composer_availability(cx);
        }
        cx.notify();
    }

    pub(super) fn handle_service_stopped(
        &mut self,
        status: ServiceStopStatus,
        cx: &mut Context<Self>,
    ) {
        self.retain_message_flight(cx);
        self.service_stopped = true;
        self.clear_transient_service_state();
        self.reset_composer_catalog(cx);
        self.reset_profile_usage_for_connection();
        self.thread_switch_flight = None;
        self.ordinary_unsubscribe_thread = None;
        self.pending_thread = None;
        self.set_picker_disabled(true, cx);
        self.set_thread_picker_disabled(true, cx);
        if matches!(status, ServiceStopStatus::Failed)
            && !matches!(&self.state, NativeViewState::Failure(_))
        {
            self.set_failure(
                ServiceFailure {
                    stage: ServiceFailureStage::Cleanup,
                    category: ServiceFailureCategory::Cleanup,
                },
                cx,
            );
        } else {
            self.sync_composer_availability(cx);
            cx.notify();
        }
    }

    pub(super) fn handle_subscription_started(
        &mut self,
        thread_id: &ThreadId,
        request_id: &RequestId,
        started: ConversationSubscriptionStarted,
        cx: &mut Context<Self>,
    ) {
        if self
            .retained_switch_request_ids
            .iter()
            .any(|id| id == request_id)
            || self.active_subscription_request_id.as_ref() == Some(request_id)
        {
            return;
        }

        if self.thread_switch_flight.is_some() {
            self.handle_thread_switch_subscription_started(thread_id, request_id, started, cx);
            return;
        }

        if self.ordinary_unsubscribe_thread.as_ref() == Some(thread_id) {
            self.remember_switch_request_id(request_id.clone());
            return;
        }

        self.handle_standalone_subscription_started(thread_id, request_id, started, cx);
    }

    pub(super) fn handle_thread_switch_subscription_started(
        &mut self,
        thread_id: &ThreadId,
        request_id: &RequestId,
        started: ConversationSubscriptionStarted,
        cx: &mut Context<Self>,
    ) {
        let Some((target_thread, generation, request_matches)) = self
            .thread_switch_flight
            .as_ref()
            .and_then(|flight| match &flight.phase {
                ThreadSwitchPhase::AwaitingSubscriptionStart {
                    request_id: receipt,
                } => Some((
                    flight.target_thread.clone(),
                    flight.generation,
                    receipt
                        .as_ref()
                        .is_none_or(|expected| expected == request_id),
                )),
                _ => None,
            })
        else {
            self.remember_switch_request_id(request_id.clone());
            return;
        };
        let Some(target_thread) = target_thread else {
            self.remember_switch_request_id(request_id.clone());
            return;
        };
        if self.shutdown_prepared
            || !request_matches
            || thread_id != &target_thread
            || !self.thread_is_listed(&target_thread)
            || self.selected_thread.as_ref() != Some(&target_thread)
        {
            self.remember_switch_request_id(request_id.clone());
            return;
        }

        let snapshot = match started {
            ConversationSubscriptionStarted::Fresh(start) => start.snapshot().clone(),
            // A switch always submits a fresh subscription with no cursor.
            // A resumed response cannot advance this flight.
            ConversationSubscriptionStarted::Resumed { .. } => {
                self.remember_switch_request_id(request_id.clone());
                return;
            }
        };
        if snapshot.thread_id() != &target_thread {
            self.remember_switch_request_id(request_id.clone());
            return;
        }
        self.advance_thread_switch_with_snapshot(
            &target_thread,
            generation,
            request_id,
            snapshot,
            cx,
        );
    }

    pub(super) fn advance_thread_switch_with_snapshot(
        &mut self,
        target_thread: &ThreadId,
        generation: u64,
        request_id: &RequestId,
        snapshot: ConversationSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.forget_switch_snapshot_thread(target_thread);
        self.standalone_snapshot_thread = Some(target_thread.clone());
        if let Some(flight) = self.thread_switch_flight.as_mut()
            && flight.generation == generation
            && let ThreadSwitchPhase::AwaitingSubscriptionStart {
                request_id: receipt,
            } = &mut flight.phase
        {
            *receipt = Some(request_id.clone());
        }
        self.remember_switch_request_id(request_id.clone());
        let Some(host) = self.conversation_host.clone() else {
            self.fail_thread_switch(invalid_service_failure(), false, cx);
            return;
        };
        if host.read(cx).controller_view().delivery.thread_id != *target_thread {
            self.fail_thread_switch(invalid_service_failure(), false, cx);
            return;
        }
        self.dispatch_snapshot(&host, snapshot, cx);
        if !self
            .conversation_host
            .as_ref()
            .is_some_and(|mounted| mounted.read(cx).controller_view().delivery.has_snapshot)
        {
            self.fail_thread_switch(invalid_service_failure(), false, cx);
            return;
        }
        let complete = self.thread_switch_flight.as_ref().is_some_and(|flight| {
            flight.generation == generation
                && matches!(
                    &flight.phase,
                    ThreadSwitchPhase::AwaitingSubscriptionStart {
                        request_id: Some(receipt)
                    } if receipt == request_id
                )
                && self.selected_thread.as_ref() == Some(target_thread)
                && self
                    .conversation_host
                    .as_ref()
                    .is_some_and(|mounted| mounted.read(cx).controller_view().delivery.has_snapshot)
        });
        if complete {
            self.active_subscription_request_id = Some(request_id.clone());
            self.thread_switch_flight = None;
            self.pending_thread = None;
            self.sync_thread_picker_selected(cx);
            self.sync_thread_picker_disabled(cx);
            self.sync_composer_availability(cx);
            cx.notify();
        }
    }

    pub(super) fn handle_standalone_subscription_started(
        &mut self,
        thread_id: &ThreadId,
        request_id: &RequestId,
        started: ConversationSubscriptionStarted,
        cx: &mut Context<Self>,
    ) {
        match started {
            ConversationSubscriptionStarted::Fresh(start) => {
                let snapshot = start.snapshot().clone();
                if thread_id != snapshot.thread_id() {
                    return;
                }
                if self.selected_thread.as_ref() != Some(thread_id) {
                    return;
                }
                self.forget_switch_snapshot_thread(thread_id);
                self.remember_active_subscription_request();
                self.active_subscription_request_id = Some(request_id.clone());
                self.handle_snapshot(snapshot, cx);
                self.standalone_snapshot_thread = Some(thread_id.clone());
            }
            ConversationSubscriptionStarted::Resumed {
                thread_id: resumed_thread,
                cursor,
            } => {
                if self.selected_thread.as_ref() != Some(&resumed_thread)
                    || &resumed_thread != thread_id
                {
                    return;
                }
                self.forget_switch_snapshot_thread(thread_id);
                self.remember_active_subscription_request();
                self.active_subscription_request_id = Some(request_id.clone());
                self.standalone_snapshot_thread = Some(thread_id.clone());
                let Some(host) = self.conversation_host.clone() else {
                    return;
                };
                let dispatch = host.update(cx, |host, host_cx| {
                    host.dispatch(
                        ConversationStateEvent::Delivery(
                            ConversationDeliveryEvent::SubscriptionResumed {
                                thread_id: resumed_thread.clone(),
                                cursor,
                            },
                        ),
                        host_cx,
                    )
                });
                if dispatch.is_err() {
                    self.set_failure(invalid_service_failure(), cx);
                } else {
                    self.acknowledge_host_cursor(&host, cx);
                    self.pump_host_boundary(&host, cx);
                    cx.notify();
                }
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive dispatch keeps every route visible in a single reviewable match"
    )]
    pub(super) fn handle_subscription_stopped(
        &mut self,
        thread_id: &ThreadId,
        request_id: &RequestId,
        stopped: &artisan_protocol::ConversationSubscriptionStopped,
        cx: &mut Context<Self>,
    ) {
        // A stale ack is ignored: unknown requests return here without
        // submitting Unsubscribe, and no host retirement runs on this path.
        let known_request = self
            .retained_switch_request_ids
            .iter()
            .any(|id| id == request_id);
        if known_request {
            return;
        }
        if &stopped.thread_id != thread_id {
            if self.thread_switch_flight.is_some() {
                self.remember_switch_request_id(request_id.clone());
            }
            return;
        }

        if self.thread_switch_flight.is_some()
            && !self.thread_switch_flight.as_ref().is_some_and(|flight| {
                matches!(
                    &flight.phase,
                    ThreadSwitchPhase::AwaitingUnsubscribeStop { .. }
                )
            })
        {
            self.remember_switch_request_id(request_id.clone());
            return;
        }

        if let Some((source_thread, generation, request_matches)) = self
            .thread_switch_flight
            .as_ref()
            .and_then(|flight| match &flight.phase {
                ThreadSwitchPhase::AwaitingUnsubscribeStop {
                    request_id: receipt,
                } => Some((
                    flight.source_thread.clone(),
                    flight.generation,
                    receipt
                        .as_ref()
                        .is_none_or(|expected| expected == request_id),
                )),
                _ => None,
            })
        {
            if self.shutdown_prepared || !request_matches || &source_thread != thread_id {
                self.remember_switch_request_id(request_id.clone());
                return;
            }
            if let Some(flight) = self.thread_switch_flight.as_mut()
                && flight.generation == generation
                && let ThreadSwitchPhase::AwaitingUnsubscribeStop {
                    request_id: receipt,
                } = &mut flight.phase
            {
                *receipt = Some(request_id.clone());
            }
            self.remember_switch_request_id(request_id.clone());
            if let Some(flight) = self.thread_switch_flight.as_mut()
                && flight.generation == generation
            {
                flight.phase = ThreadSwitchPhase::HostRetirement {
                    request_id: request_id.clone(),
                };
            }
            self.finish_thread_switch_after_stop(generation, cx);
            return;
        }

        if self.ordinary_unsubscribe_thread.as_ref() != Some(thread_id)
            || self.selected_thread.as_ref() != Some(thread_id)
        {
            return;
        }
        self.ordinary_unsubscribe_thread = None;
        self.remember_switch_request_id(request_id.clone());
        self.remember_active_subscription_request();
        // Stop ack must never create an unsubscribe loop. If this thread is not the active
        // selected thread, it is a stale ack and is ignored. If it is active, finish the
        // already-started local retirement without sending another Unsubscribe.
        // Finish retirement without sending Unsubscribe again
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        if let Some(host) = self.conversation_host.clone() {
            self.pump_host_boundary(&host, cx);
            if self.conversation_effects.is_empty()
                && host.read(cx).total_pending_effect_count() == 0
            {
                Self::release_transient_scroll_custody(&host, cx);
                self.conversation_host = None;
                drop(self.conversation_host_subscription.take());
                self.selected_thread = None;
                self.standalone_snapshot_thread = None;
                self.sync_thread_picker_selected(cx);
                self.sync_thread_picker_disabled(cx);
                self.engine_settings.select_thread(None);
                self.reset_composer_catalog(cx);
                self.sync_composer_availability(cx);
                cx.notify();
                return;
            }
        } else {
            self.selected_thread = None;
            self.standalone_snapshot_thread = None;
            self.sync_thread_picker_selected(cx);
            self.sync_thread_picker_disabled(cx);
            self.engine_settings.select_thread(None);
            self.reset_composer_catalog(cx);
            self.sync_composer_availability(cx);
            cx.notify();
        }
        // If host still has pending effects, keep selected_thread until drained; do not loop
        cx.notify();
    }

    /// Resolves the Waiting-narration engine label at echo time.
    ///
    /// The send-time captured label stands UNLESS exact correlation proves
    /// the observed run is ours: the watch named this run as its steer
    /// target, or the canonical snapshot already holds an assistant item of
    /// the echoed turn produced by the observed run. A merely same-thread
    /// observed run proves nothing â€” an old poll can describe the previous
    /// engine while a new cross-engine send just launched â€” so without
    /// proof the captured label stands, and without any label the generic
    /// fallback renders. The picker is never consulted after the send.
    pub(super) fn resolve_dispatch_engine_label(
        &self,
        thread_id: &ThreadId,
        echo_turn_id: &TurnId,
        send_label: Option<String>,
        steer_run_id: Option<&artisan_domain::RunId>,
        host: &Entity<ConversationHost>,
        cx: &App,
    ) -> Option<String> {
        if let Some((observed_id, observed_engine)) =
            self.run_controls.observed_run(Some(thread_id))
        {
            let exact = steer_run_id == Some(&observed_id)
                || host.read(cx).canonical_snapshot().is_some_and(|snapshot| {
                    snapshot.items().iter().any(|item| match item {
                        ConversationItem::AssistantMessage(message) => {
                            message.turn_id == *echo_turn_id && message.run_id == observed_id
                        }
                        _ => false,
                    })
                });
            if exact {
                return Some(profile_usage_display_name(observed_engine.as_str()).to_owned());
            }
        }
        send_label
    }

    /// Retires the staged echo watch for one pre-matched source id.
    ///
    /// Take-up marks immediately (the echo was observed, so the lip retires
    /// even while still listed), but the watch is kept until the label
    /// dispatch succeeds: backpressure must not permanently lose the label.
    /// A later echo re-attempts the dispatch; the lip marking and the
    /// watch take stay idempotent.
    pub(super) fn retire_echo_matched(
        &mut self,
        thread_id: &ThreadId,
        message_id: &MessageId,
        turn_id: TurnId,
        host: &Entity<ConversationHost>,
        cx: &mut Context<Self>,
    ) {
        if self.selected_thread.as_ref() != Some(thread_id) {
            return;
        }
        let (send_label, steer_run_id) = match self.composer_queue.state.echo_watch_for(message_id)
        {
            Some(watch) => (
                watch.engine_label().map(str::to_owned),
                watch.steer_run_id().cloned(),
            ),
            None => return,
        };
        self.composer_queue.state.mark_taken_up(message_id);
        let engine_label = self.resolve_dispatch_engine_label(
            thread_id,
            &turn_id,
            send_label,
            steer_run_id.as_ref(),
            host,
            cx,
        );
        let dispatch = host.update(cx, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::SetTurnEngineLabel {
                    turn_id,
                    engine_label,
                },
                host_cx,
            )
        });
        if dispatch.is_ok() {
            self.composer_queue.state.clear_echo_watch_for(message_id);
            // Drain the invalidation the label dispatch raised (plus any
            // sibling boundary work) so one effect does not accumulate per
            // send.
            self.pump_host_boundary(host, cx);
        }
        self.sync_composer_controls(cx);
    }

    /// Retires the staged echo watch when the canonical user item projects.
    ///
    /// Frozen native correlation contract: matches ONLY
    /// `item.source_message_id == receipt.message_id`, never item-id
    /// equality. An absent legacy source id matches nothing â€” no guessed
    /// take-up or label; the forced queue refresh stays the fallback.
    /// Take-up marks on first match so the lip cannot duplicate; the watch
    /// clears only once the label dispatch succeeds, so a later duplicate
    /// re-attempts a lost label idempotently.
    pub(super) fn retire_echo_for_item(
        &mut self,
        thread_id: &ThreadId,
        item: &ConversationItem,
        host: &Entity<ConversationHost>,
        cx: &mut Context<Self>,
    ) {
        if self.selected_thread.as_ref() != Some(thread_id) {
            return;
        }
        let (source_id, turn_id) = match item {
            ConversationItem::UserMessage(message) => {
                (message.source_message_id.as_ref(), message.turn_id.clone())
            }
            ConversationItem::MultimodalUserMessage(message) => {
                (message.source_message_id.as_ref(), message.turn_id.clone())
            }
            // Assistant messages are the only other item kind and
            // never echo a send.
            ConversationItem::AssistantMessage(_) => return,
        };
        let Some(source_id) = source_id else {
            return;
        };
        if self
            .composer_queue
            .state
            .echo_watch_for(source_id)
            .is_none()
        {
            return;
        }
        self.retire_echo_matched(thread_id, source_id, turn_id, host, cx);
    }

    /// Re-scans the canonical snapshot for staged echo watches.
    ///
    /// A watch retained across a failed label dispatch (backpressure) must
    /// not wait for another duplicate `ItemUpsert` that may never arrive:
    /// every authoritative refresh re-resolves retained watches against
    /// what is already projected. No-ops when no watch is staged.
    pub(super) fn rescan_retained_echo_watches(&mut self, cx: &mut Context<Self>) {
        if self.composer_queue.state.echo_watch_count() == 0 {
            return;
        }
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        if host.read(cx).controller_view().delivery.thread_id != thread_id {
            return;
        }
        let Some(snapshot) = host.read(cx).canonical_snapshot() else {
            return;
        };
        let mut matches = Vec::new();
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
            if let Some(source_id) = source_id
                && self
                    .composer_queue
                    .state
                    .echo_watch_for(source_id)
                    .is_some()
            {
                matches.push((source_id.clone(), turn_id));
            }
        }
        for (message_id, turn_id) in matches {
            self.retire_echo_matched(&thread_id, &message_id, turn_id, &host, cx);
        }
    }

    pub(super) fn handle_patch_batch(&mut self, batch: &PatchBatch, cx: &mut Context<Self>) {
        if self.thread_switch_flight.is_some() {
            self.remember_patch_ids(batch);
            return;
        }
        if self.retained_switch_patch_ids.iter().any(|patch_id| {
            batch
                .patches()
                .iter()
                .any(|patch| patch.patch_id() == patch_id)
        }) {
            return;
        }
        if self.selected_thread.as_ref() != Some(batch.thread_id()) {
            return;
        }
        self.remember_patch_ids(batch);
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        if host.read(cx).controller_view().delivery.thread_id != *batch.thread_id() {
            return;
        }
        let dispatch = host.update(cx, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Delivery(ConversationDeliveryEvent::BatchReceived(
                    batch.clone(),
                )),
                host_cx,
            )
        });
        if dispatch.is_err() {
            self.set_failure(invalid_service_failure(), cx);
        } else {
            self.acknowledge_host_cursor(&host, cx);
            self.pump_host_boundary(&host, cx);
            for patch in batch.patches() {
                let artisan_domain::ConversationPatch::ItemUpsert { item, .. } = patch else {
                    continue;
                };
                self.retire_echo_for_item(batch.thread_id(), item, &host, cx);
            }
            cx.notify();
        }
    }

    /// Pairs one uni-stream engine observation into presentation state.
    ///
    /// Only the selected thread's rows are retained; events for any other
    /// thread are ignored. Dedup is owned by [`EngineObservationState`],
    /// which is independent of host mounting, so unlike patch batches this
    /// path does not wait for a thread-switch flight to settle. The event
    /// carries its own Forge attribution for real DB deliveries; legacy
    /// deliveries pair without activity identity. This path never issues
    /// commands: approvals and questions render with their request ids for
    /// the later answer packet, but no answer is dispatched here.
    pub(super) fn handle_engine_observation(
        &mut self,
        observation: &ServerEvent,
        cx: &mut Context<Self>,
    ) {
        let artisan_domain::Event::EngineObservation(paired) = &observation.event else {
            return;
        };
        if self.selected_thread.as_ref() != Some(&paired.thread_id) {
            return;
        }
        let same_thread = self
            .engine_observations
            .as_ref()
            .is_some_and(|retained| retained.thread_id() == &paired.thread_id);
        if !same_thread {
            self.engine_observations = Some(EngineObservationState::new(paired.thread_id.clone()));
        }
        if let Some(state) = self.engine_observations.as_mut() {
            let outcome = state.apply(observation.cursor.get(), paired);
            if matches!(outcome, ApplyOutcome::Applied { .. }) {
                cx.notify();
            }
        }
        self.replay_observation_activity(cx);
    }

    /// Replays retained attributed observations into the mounted host scene.
    ///
    /// Projection runs only with a mounted host that belongs to the selected
    /// thread and already holds its canonical snapshot. Events arriving
    /// before mount or before the canonical turn exists stay retained in
    /// [`EngineObservationState`] and project on a later replay (after
    /// snapshot dispatch or thread mount). Each projected fact dispatches
    /// atomically through `SceneFactCommand::Upsert`: identical facts are a
    /// no-op, changed facts update in place, and no remove-then-register
    /// sequence ever runs. Unknown turns and scene conflicts skip without
    /// failure so retained rows project once the canonical turn exists;
    /// backpressure stops the replay so retained rows project later.
    pub(super) fn replay_observation_activity(&mut self, cx: &mut Context<Self>) {
        let Some(selected) = self.selected_thread.clone() else {
            return;
        };
        let Some(state) = self.engine_observations.as_ref() else {
            return;
        };
        if state.thread_id() != &selected {
            return;
        }
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        if host.read(cx).controller_view().delivery.thread_id != selected {
            return;
        }
        let Some(snapshot) = host.read(cx).canonical_snapshot() else {
            return;
        };
        let projection =
            crate::conversation_observation_projection::project_activities(state, &snapshot);
        if projection.facts.is_empty() {
            return;
        }
        let mut invalidated = false;
        for fact in projection.facts {
            let upsert = crate::conversation_state_machine::SceneFactCommand::Upsert(fact);
            match host.update(cx, |host, host_cx| {
                host.dispatch(ConversationStateEvent::Fact(upsert), host_cx)
            }) {
                Ok(()) => invalidated = true,
                Err(crate::conversation_host::ConversationHostError::Controller(
                    crate::conversation_state_machine::ConversationStateError::UnknownTurn {
                        ..
                    }
                    | crate::conversation_state_machine::ConversationStateError::SceneConflict {
                        ..
                    }
                    | crate::conversation_state_machine::ConversationStateError::FactTurnMismatch {
                        ..
                    }
                    | crate::conversation_state_machine::ConversationStateError::Scene { .. },
                )) => {}
                Err(_) => break,
            }
        }
        if invalidated {
            self.pump_host_boundary(&host, cx);
            cx.notify();
        }
    }

    /// Applies a listing removal to an in-flight switch. A target that has
    /// not been mounted is simply tombstoned; a target whose subscription
    /// admission already reached the service becomes the new mounted source
    /// of one fenced retirement. This keeps the old host until its matching
    /// stop receipt and never lets a removed target start replace it.
    pub(super) fn handle_removed_thread_during_switch(&mut self, cx: &mut Context<Self>) {
        let Some(flight) = self.thread_switch_flight.as_ref() else {
            return;
        };
        let Some(target_thread) = flight.target_thread.clone() else {
            return;
        };
        let target_subscribe_is_only_retained_retry = matches!(
            &flight.phase,
            ThreadSwitchPhase::SubscribeAdmission {
                retry_pending: true,
                ..
            }
        );
        let retire_mounted_target = matches!(
            &flight.phase,
            ThreadSwitchPhase::SubscribeAdmission { .. }
                | ThreadSwitchPhase::AwaitingSubscriptionStart { .. }
        ) && self.conversation_host.as_ref().is_some_and(|host| {
            self.selected_thread.as_ref() == Some(&target_thread)
                && host.read(cx).controller_view().delivery.thread_id == target_thread
        });

        if retire_mounted_target {
            if target_subscribe_is_only_retained_retry {
                self.thread_switch_flight = None;
                self.pending_thread = None;
                self.retire_host_after_switch_stop(cx);
                self.state = if self.thread_listing.is_none() || self.project_options.is_empty() {
                    NativeViewState::EmptyProjects
                } else {
                    NativeViewState::EmptyThreads
                };
                self.sync_thread_picker_selected(cx);
                self.sync_thread_picker_disabled(cx);
                self.sync_composer_availability(cx);
                cx.notify();
                return;
            }
            self.thread_switch_flight = None;
            self.pending_thread = None;
            self.begin_thread_transition(None, target_thread, false, cx);
            return;
        }

        let subscribe_generation = self.thread_switch_flight.as_ref().and_then(|flight| {
            matches!(&flight.phase, ThreadSwitchPhase::SubscribeAdmission { .. })
                .then_some(flight.generation)
        });
        if let Some(flight) = self.thread_switch_flight.as_mut() {
            flight.target_thread = None;
        }
        if let Some(generation) = subscribe_generation {
            self.pending_thread = None;
            self.complete_thread_retirement(generation, cx);
        }
    }

    pub(super) fn thread_is_listed(&self, thread_id: &ThreadId) -> bool {
        self.thread_listing.as_ref().is_some_and(|listing| {
            listing.threads().iter().any(|thread| {
                &thread.thread_id == thread_id
                    && self.selected_project.as_ref() == Some(&thread.project_id)
            })
        })
    }

    pub(super) fn remember_switch_request_id(&mut self, request_id: RequestId) {
        if self
            .retained_switch_request_ids
            .iter()
            .any(|retained| retained == &request_id)
        {
            return;
        }
        if self.retained_switch_request_ids.len() >= MAX_RETAINED_SWITCH_REQUEST_IDS {
            self.retained_switch_request_ids.remove(0);
        }
        self.retained_switch_request_ids.push(request_id);
    }

    pub(super) fn remember_active_subscription_request(&mut self) {
        if let Some(request_id) = self.active_subscription_request_id.take() {
            self.remember_switch_request_id(request_id);
        }
    }

    pub(super) fn remember_patch_ids(&mut self, batch: &PatchBatch) {
        for patch in batch.patches() {
            if self
                .retained_switch_patch_ids
                .iter()
                .any(|retained| retained == patch.patch_id())
            {
                continue;
            }
            if self.retained_switch_patch_ids.len() >= MAX_RETAINED_SWITCH_PATCH_IDS {
                self.retained_switch_patch_ids.remove(0);
            }
            self.retained_switch_patch_ids
                .push(patch.patch_id().clone());
        }
    }

    pub(super) fn remember_switch_listing(&mut self) {
        let Some(listing) = self.thread_listing.clone() else {
            return;
        };
        if self
            .retained_switch_listings
            .iter()
            .any(|retained| retained == &listing)
        {
            return;
        }
        if self.retained_switch_listings.len() >= MAX_RETAINED_SWITCH_LISTINGS {
            self.retained_switch_listings.remove(0);
        }
        self.retained_switch_listings.push(listing);
    }

    pub(super) fn remember_switch_snapshot_thread(&mut self, thread_id: ThreadId) {
        if self
            .retained_switch_snapshot_threads
            .iter()
            .any(|retained| retained == &thread_id)
        {
            return;
        }
        if self.retained_switch_snapshot_threads.len() >= MAX_RETAINED_SWITCH_REQUEST_IDS {
            self.retained_switch_snapshot_threads.remove(0);
        }
        self.retained_switch_snapshot_threads.push(thread_id);
    }

    pub(super) fn forget_switch_snapshot_thread(&mut self, thread_id: &ThreadId) {
        self.retained_switch_snapshot_threads
            .retain(|retained| retained != thread_id);
    }

    pub(super) fn sync_thread_picker_selected(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.thread_picker.clone() else {
            return;
        };
        let selected = self.selected_thread.clone();
        picker.update(cx, |picker, picker_cx| {
            picker.set_selected_thread(selected, picker_cx);
        });
    }

    pub(super) fn begin_thread_switch(&mut self, target_thread: ThreadId, cx: &mut Context<Self>) {
        if self.shutdown_prepared
            || self.service_stopped
            || self.intake_stage.is_some()
            || self.thread_switch_flight.is_some()
            || self.ordinary_unsubscribe_thread.is_some()
        {
            return;
        }
        let Some(source_thread) = self.selected_thread.clone() else {
            return;
        };
        if source_thread == target_thread {
            return;
        }
        if !self.thread_is_listed(&target_thread) {
            self.set_failure(invalid_service_failure(), cx);
            return;
        }
        self.begin_thread_transition(Some(target_thread), source_thread, false, cx);
    }

    pub(super) fn begin_thread_retirement(&mut self, cx: &mut Context<Self>) {
        if self.shutdown_prepared
            || self.service_stopped
            || self.intake_stage.is_some()
            || self.thread_switch_flight.is_some()
            || self.ordinary_unsubscribe_thread.is_some()
        {
            return;
        }
        let Some(source_thread) = self.selected_thread.clone() else {
            return;
        };
        self.begin_thread_transition(None, source_thread, false, cx);
    }

    pub(super) fn begin_thread_transition(
        &mut self,
        target_thread: Option<ThreadId>,
        source_thread: ThreadId,
        carry_draft: bool,
        cx: &mut Context<Self>,
    ) {
        // Only the exact newly created recovery thread may keep its recall.
        if !carry_draft
            || self
                .pending_failed_recovery
                .as_ref()
                .is_some_and(|recovery| {
                    recovery.new_thread.is_none()
                        || recovery.new_thread != target_thread
                        || recovery.old_thread != source_thread
                })
        {
            self.pending_failed_recovery = None;
        }
        if self
            .conversation_host
            .as_ref()
            .is_none_or(|host| host.read(cx).controller_view().delivery.thread_id != source_thread)
        {
            self.set_failure(invalid_service_failure(), cx);
            return;
        }
        let Some(generation) = self.next_thread_switch_generation.checked_add(1) else {
            self.set_failure(invalid_service_failure(), cx);
            return;
        };
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.next_thread_switch_generation = generation;
        self.remember_switch_listing();
        self.remember_switch_snapshot_thread(source_thread.clone());
        if let Some(target_thread) = target_thread.as_ref() {
            self.remember_switch_snapshot_thread(target_thread.clone());
        }
        self.remember_active_subscription_request();
        self.pending_thread = None;
        self.thread_switch_flight = Some(ThreadSwitchFlight {
            source_thread,
            target_thread,
            generation,
            carry_draft,
            phase: ThreadSwitchPhase::UnsubscribeAdmission {
                retry_pending: false,
                retry_used: false,
            },
        });
        self.sync_thread_picker_disabled(cx);
        self.sync_composer_availability(cx);
        self.submit_thread_switch_unsubscribe(cx);
    }

    pub(super) fn submit_thread_switch_unsubscribe(&mut self, cx: &mut Context<Self>) {
        let Some((source_thread, generation, retry_used)) = self
            .thread_switch_flight
            .as_ref()
            .and_then(|flight| match &flight.phase {
                ThreadSwitchPhase::UnsubscribeAdmission {
                    retry_pending: _,
                    retry_used,
                } => Some((flight.source_thread.clone(), flight.generation, *retry_used)),
                _ => None,
            })
        else {
            return;
        };
        match self.submit_command(NativeTransportCommand::Unsubscribe {
            thread_id: source_thread,
        }) {
            Ok(()) => {
                if let Some(flight) = self.thread_switch_flight.as_mut()
                    && flight.generation == generation
                {
                    flight.phase = ThreadSwitchPhase::AwaitingUnsubscribeStop { request_id: None };
                }
                cx.notify();
            }
            Err(CommandSendError::Busy) if !retry_used => {
                if let Some(flight) = self.thread_switch_flight.as_mut()
                    && flight.generation == generation
                {
                    flight.phase = ThreadSwitchPhase::UnsubscribeAdmission {
                        retry_pending: true,
                        retry_used: true,
                    };
                }
                cx.notify();
            }
            Err(CommandSendError::Busy) => self.fail_thread_switch(
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::Backpressure,
                },
                false,
                cx,
            ),
            Err(CommandSendError::Stopped) => self.fail_thread_switch(
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::ChannelClosed,
                },
                true,
                cx,
            ),
        }
    }

    pub(super) fn submit_thread_switch_subscribe(&mut self, cx: &mut Context<Self>) {
        let Some((target_thread, generation, retry_used)) = self
            .thread_switch_flight
            .as_ref()
            .and_then(|flight| match &flight.phase {
                ThreadSwitchPhase::SubscribeAdmission {
                    retry_pending: _,
                    retry_used,
                } => Some((flight.target_thread.clone(), flight.generation, *retry_used)),
                _ => None,
            })
        else {
            return;
        };
        let Some(target_thread) = target_thread else {
            self.complete_thread_retirement(generation, cx);
            return;
        };
        if self.shutdown_prepared || !self.thread_is_listed(&target_thread) {
            self.complete_thread_retirement(generation, cx);
            return;
        }
        match self.submit_command(NativeTransportCommand::Subscribe {
            thread_id: target_thread,
            after: None,
        }) {
            Ok(()) => {
                if let Some(flight) = self.thread_switch_flight.as_mut()
                    && flight.generation == generation
                {
                    flight.phase =
                        ThreadSwitchPhase::AwaitingSubscriptionStart { request_id: None };
                }
                cx.notify();
            }
            Err(CommandSendError::Busy) if !retry_used => {
                if let Some(flight) = self.thread_switch_flight.as_mut()
                    && flight.generation == generation
                {
                    flight.phase = ThreadSwitchPhase::SubscribeAdmission {
                        retry_pending: true,
                        retry_used: true,
                    };
                }
                cx.notify();
            }
            Err(CommandSendError::Busy) => self.fail_thread_switch(
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::Backpressure,
                },
                false,
                cx,
            ),
            Err(CommandSendError::Stopped) => self.fail_thread_switch(
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::ChannelClosed,
                },
                true,
                cx,
            ),
        }
    }

    pub(super) fn retry_thread_switch_if_admitted(&mut self, cx: &mut Context<Self>) {
        if self.shutdown_prepared || self.service_stopped {
            return;
        }
        let retry = self.thread_switch_flight.as_ref().is_some_and(|flight| {
            matches!(
                &flight.phase,
                ThreadSwitchPhase::UnsubscribeAdmission {
                    retry_pending: true,
                    ..
                } | ThreadSwitchPhase::SubscribeAdmission {
                    retry_pending: true,
                    ..
                }
            )
        });
        if !retry {
            return;
        }
        match self
            .thread_switch_flight
            .as_ref()
            .map(|flight| &flight.phase)
        {
            Some(ThreadSwitchPhase::UnsubscribeAdmission { .. }) => {
                self.submit_thread_switch_unsubscribe(cx);
            }
            Some(ThreadSwitchPhase::SubscribeAdmission { .. }) => {
                self.submit_thread_switch_subscribe(cx);
            }
            _ => {}
        }
    }

    pub(super) fn finish_thread_switch_after_stop(
        &mut self,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let Some((target_thread, stop_request_id)) =
            self.thread_switch_flight.as_ref().and_then(|flight| {
                if flight.generation != generation {
                    return None;
                }
                match &flight.phase {
                    ThreadSwitchPhase::HostRetirement { request_id } => {
                        Some((flight.target_thread.clone(), request_id.clone()))
                    }
                    _ => None,
                }
            })
        else {
            return;
        };
        self.remember_switch_request_id(stop_request_id);
        self.retire_host_after_switch_stop(cx);
        if self.shutdown_prepared {
            self.thread_switch_flight = None;
            return;
        }
        let target_thread = target_thread.filter(|thread_id| self.thread_is_listed(thread_id));
        if let Some(flight) = self.thread_switch_flight.as_mut()
            && flight.generation == generation
        {
            flight.phase = ThreadSwitchPhase::SubscribeAdmission {
                retry_pending: false,
                retry_used: false,
            };
        }
        self.pending_thread = target_thread;
        if self.pending_thread.is_some() {
            self.state = NativeViewState::Loading;
            self.sync_thread_picker_selected(cx);
            self.try_mount_pending_thread(cx);
        } else {
            self.complete_thread_retirement(generation, cx);
        }
    }

    pub(super) fn complete_thread_retirement(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self
            .thread_switch_flight
            .as_ref()
            .is_none_or(|flight| flight.generation != generation)
        {
            return;
        }
        self.thread_switch_flight = None;
        self.pending_thread = None;
        self.state = if self.project_navigation.awaiting_threads {
            NativeViewState::LoadingThreads
        } else if self.thread_listing.is_none() || self.project_options.is_empty() {
            NativeViewState::EmptyProjects
        } else {
            NativeViewState::EmptyThreads
        };
        self.sync_thread_picker_selected(cx);
        self.sync_thread_picker_disabled(cx);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    pub(super) fn retire_host_after_switch_stop(&mut self, cx: &mut Context<Self>) {
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.pending_snapshot = None;
        self.conversation_effects.clear();
        if let Some(host) = self.conversation_host.clone() {
            Self::release_transient_scroll_custody(&host, cx);
        }
        self.conversation_host = None;
        drop(self.conversation_host_subscription.take());
        self.selected_thread = None;
        self.standalone_snapshot_thread = None;
        self.engine_settings.select_thread(None);
        self.reset_composer_catalog(cx);
        self.sync_thread_picker_selected(cx);
        self.sync_composer_availability(cx);
    }

    pub(super) fn fail_thread_switch(
        &mut self,
        failure: ServiceFailure,
        terminal: bool,
        cx: &mut Context<Self>,
    ) {
        let target_host_mounted = self.thread_switch_flight.as_ref().is_some_and(|flight| {
            matches!(
                &flight.phase,
                ThreadSwitchPhase::SubscribeAdmission { .. }
                    | ThreadSwitchPhase::AwaitingSubscriptionStart { .. }
            ) && flight
                .target_thread
                .as_ref()
                .is_some_and(|target| self.selected_thread.as_ref() == Some(target))
        });
        if target_host_mounted {
            self.retire_host_after_switch_stop(cx);
        }
        self.thread_switch_flight = None;
        self.pending_thread = None;
        if terminal {
            self.service_stopped = true;
        }
        self.set_failure(failure, cx);
        self.sync_thread_picker_disabled(cx);
        cx.notify();
    }

    pub(super) fn handle_delivery_lost(&mut self, failure: ServiceFailure, cx: &mut Context<Self>) {
        // Use mounted host's last-good cursor and existing recovery policy to resubscribe
        if let Some(thread_id) = self.selected_thread.clone()
            && let Some(host) = self.conversation_host.clone()
        {
            let cursor = host.read(cx).controller_view().delivery.cursor;
            if let Some(service) = self.service.clone() {
                // Explicit retry via Subscribe with last-good cursor; Busy/Stopped remain explicit
                let result = service.submit(NativeTransportCommand::Subscribe {
                    thread_id: thread_id.clone(),
                    after: cursor,
                });
                match result {
                    Ok(()) => {
                        // keep current view, await Started/Patch; do not fabricate snapshot
                        cx.notify();
                        return;
                    }
                    Err(CommandSendError::Busy) => {
                        self.set_failure(
                            ServiceFailure {
                                stage: ServiceFailureStage::EventBridge,
                                category: ServiceFailureCategory::Backpressure,
                            },
                            cx,
                        );
                        return;
                    }
                    Err(CommandSendError::Stopped) => {
                        self.set_failure(
                            ServiceFailure {
                                stage: ServiceFailureStage::EventBridge,
                                category: ServiceFailureCategory::ChannelClosed,
                            },
                            cx,
                        );
                        return;
                    }
                }
            }
        }
        self.set_failure(failure, cx);
    }

    pub(super) fn handle_intake_progress(
        &mut self,
        stage: NativeProjectIntakeStage,
        cx: &mut Context<Self>,
    ) {
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        if self.intake_restore_state.is_none() {
            self.intake_restore_state = Some(self.state.clone());
        }
        self.intake_stage = Some(stage);
        self.intake_failure_operation = None;
        self.intake_retry_available = false;
        self.set_picker_disabled(true, cx);
        self.set_thread_picker_disabled(true, cx);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    pub(super) fn handle_intake_cancelled(&mut self, cx: &mut Context<Self>) {
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.intake_stage = None;
        self.intake_failure_operation = None;
        self.intake_retry_available = false;
        if let Some(state) = self.intake_restore_state.take() {
            self.state = state;
        }
        let options = self.project_options.clone();
        self.install_picker(options.clone(), self.selected_project.clone(), cx);
        self.install_home_picker(options, self.selected_project.clone(), cx);
        self.install_thread_picker(
            self.thread_listing
                .clone()
                .unwrap_or_else(empty_thread_listing),
            self.selected_thread.clone(),
            cx,
        );
        self.sync_composer_availability(cx);
        cx.notify();
    }

    pub(super) fn handle_intake_failed(
        &mut self,
        operation: NativeProjectIntakeOperation,
        failure: ServiceFailure,
        retryable: bool,
        cx: &mut Context<Self>,
    ) {
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.pending_failed_recovery = None;
        self.intake_stage = None;
        self.intake_failure_operation = Some(operation);
        self.intake_retry_available = retryable;
        self.state = NativeViewState::Failure(failure);
        self.last_picker_action = None;
        if !retryable {
            self.intake_restore_state = None;
        }
        // Recreate the public picker so the previous NewProject action
        // cannot be observed as a second retry before the user acts.
        let options = self.project_options.clone();
        self.install_picker(options.clone(), self.selected_project.clone(), cx);
        self.install_home_picker(options, self.selected_project.clone(), cx);
        self.install_thread_picker(
            self.thread_listing
                .clone()
                .unwrap_or_else(empty_thread_listing),
            self.selected_thread.clone(),
            cx,
        );
        self.sync_composer_availability(cx);
        cx.notify();
    }

    pub(super) fn handle_intake_ready(
        &mut self,
        projects: &ProjectListing,
        project_id: ProjectId,
        threads: &artisan_domain::ThreadListing,
        thread_id: ThreadId,
        cx: &mut Context<Self>,
    ) {
        if self.thread_switch_flight.is_some() {
            return;
        }
        self.arm_failed_recovery(&thread_id);
        if !ready_membership_is_valid(projects, &project_id, threads, &thread_id) {
            self.handle_intake_failed(
                NativeProjectIntakeOperation::RefreshThreads,
                invalid_service_failure(),
                false,
                cx,
            );
            return;
        }
        let options = self.ordered_project_options(projects);
        let keep_mounted_thread = self.selected_project.as_ref() == Some(&project_id)
            && self.selected_thread.as_ref() == Some(&thread_id)
            && self.conversation_host.as_ref().is_some_and(|host| {
                host.read(cx).controller_view().delivery.thread_id == thread_id
            });
        if self.selected_project.as_ref() != Some(&project_id) {
            if let (Some(project), Some(thread)) = (&self.selected_project, &self.selected_thread) {
                self.project_navigation
                    .last_threads
                    .insert(project.clone(), thread.clone());
            }
            self.retained_switch_listings.clear();
        }
        let source_thread = (!keep_mounted_thread)
            .then(|| {
                self.conversation_host
                    .as_ref()
                    .map(|host| host.read(cx).controller_view().delivery.thread_id)
            })
            .flatten();
        let project_changed = self.selected_project.as_ref() != Some(&project_id);
        self.project_options.clone_from(&options);
        if project_changed {
            self.promote_project(&project_id);
        }
        let options = self.project_options.clone();
        self.selected_project = Some(project_id.clone());
        self.thread_listing = Some(threads.clone());
        if keep_mounted_thread || source_thread.is_some() {
            self.pending_thread = None;
        } else {
            self.selected_thread = None;
            self.pending_thread = Some(thread_id.clone());
        }
        self.pending_snapshot = None;
        self.intake_stage = None;
        self.intake_failure_operation = None;
        self.intake_retry_available = false;
        self.intake_restore_state = None;
        self.state = if keep_mounted_thread
            && self
                .conversation_host
                .as_ref()
                .is_some_and(|host| host.read(cx).controller_view().delivery.has_snapshot)
        {
            NativeViewState::Ready
        } else {
            NativeViewState::Loading
        };
        self.install_picker(options.clone(), Some(project_id.clone()), cx);
        self.install_home_picker(options, Some(project_id), cx);
        self.install_thread_picker(threads.clone(), self.selected_thread.clone(), cx);
        if let Some(source_thread) = source_thread {
            // Retire on the stop receipt even if the old, hidden surface has
            // scroll effects left to paint. The new thread receives the draft.
            self.begin_thread_transition(Some(thread_id), source_thread, true, cx);
        } else {
            self.try_mount_pending_thread(cx);
        }
        self.sync_command_menu_groups(cx);
        self.sync_composer_availability(cx);
        self.request_project_repository(cx);
        cx.notify();
    }
}
