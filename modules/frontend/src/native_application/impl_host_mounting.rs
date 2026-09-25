//! Host mounting, retirement, and effect pumping for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-4 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    pub(super) fn handle_snapshot(
        &mut self,
        snapshot: ConversationSnapshot,
        cx: &mut Context<Self>,
    ) {
        let thread_id = snapshot.thread_id().clone();
        // A standalone snapshot cannot identify the subscription generation.
        // During a switch only the matching fresh-start payload may advance
        // the target host; every other snapshot is stale until the flight has
        // completed.
        if self.thread_switch_flight.is_some() {
            return;
        }
        if self.standalone_snapshot_thread.as_ref() == Some(&thread_id) {
            return;
        }
        if self
            .retained_switch_snapshot_threads
            .iter()
            .any(|retained| retained == &thread_id)
        {
            return;
        }
        if self.selected_thread.as_ref() != Some(&thread_id) {
            self.pending_snapshot = Some(snapshot);
            if self.pending_thread.as_ref() != Some(&thread_id) {
                self.set_failure(invalid_service_failure(), cx);
            }
            return;
        }
        let Some(host) = self.conversation_host.clone() else {
            self.pending_snapshot = Some(snapshot);
            return;
        };
        self.dispatch_snapshot(&host, snapshot, cx);
    }

    pub(super) fn dispatch_snapshot(
        &mut self,
        host: &Entity<ConversationHost>,
        snapshot: ConversationSnapshot,
        cx: &mut Context<Self>,
    ) {
        // Collect echo candidates before the snapshot moves into the host:
        // matching needs only watched source ids, so a small owned
        // (message, turn) list suffices and no transcript data is cloned.
        let dispatch = host.update(cx, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Delivery(ConversationDeliveryEvent::SnapshotReceived(
                    snapshot,
                )),
                host_cx,
            )
        });
        if dispatch.is_err() {
            self.set_failure(invalid_service_failure(), cx);
        } else {
            self.pending_snapshot = None;
            self.state = NativeViewState::Ready;
            self.acknowledge_host_cursor(host, cx);
            self.pump_host_boundary(host, cx);
            self.sync_composer_availability(cx);
            // The canonical turn may have arrived after retained observations;
            // replay them now that the snapshot exists.
            self.replay_observation_activity(cx);
            cx.notify();
        }
    }

    pub(super) fn acknowledge_host_cursor(
        &mut self,
        host: &Entity<ConversationHost>,
        cx: &mut Context<Self>,
    ) {
        let delivery = host.read(cx).controller_view().delivery;
        let Some(cursor) = delivery.cursor else {
            self.set_failure(invalid_service_failure(), cx);
            return;
        };
        let Some(service) = self.service.clone() else {
            // Host-only tests deliberately omit the service and therefore have
            // no custody owner to acknowledge.
            return;
        };
        match service.submit(NativeTransportCommand::AcknowledgePatch {
            thread_id: delivery.thread_id,
            cursor,
        }) {
            Ok(()) => {}
            Err(CommandSendError::Busy) => self.set_failure(
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::Backpressure,
                },
                cx,
            ),
            Err(CommandSendError::Stopped) => self.set_failure(
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::ChannelClosed,
                },
                cx,
            ),
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one mount transaction keeps the pending-thread checks and state handoff together"
    )]
    pub(super) fn try_mount_pending_thread(&mut self, cx: &mut Context<Self>) {
        if self.shutdown_prepared {
            self.pending_thread = None;
            return;
        }
        let Some(pending_thread) = self.pending_thread.clone() else {
            return;
        };
        let switch_generation = self.thread_switch_flight.as_ref().and_then(|flight| {
            (matches!(&flight.phase, ThreadSwitchPhase::SubscribeAdmission { .. })
                && flight.target_thread.as_ref() == Some(&pending_thread))
            .then_some(flight.generation)
        });
        let same_mounted_thread = self.conversation_host.as_ref().is_some_and(|host| {
            self.selected_thread.as_ref() == Some(&pending_thread)
                && host.read(cx).controller_view().delivery.thread_id == pending_thread
        });
        if same_mounted_thread {
            self.pending_thread = None;
            if switch_generation.is_some() {
                self.submit_thread_switch_subscribe(cx);
                return;
            }
            let has_matching_snapshot = self
                .pending_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.thread_id() == &pending_thread);
            if has_matching_snapshot {
                if let Some(snapshot) = self.pending_snapshot.take()
                    && let Some(host) = self.conversation_host.clone()
                {
                    self.dispatch_snapshot(&host, snapshot, cx);
                }
            } else if self
                .conversation_host
                .as_ref()
                .is_some_and(|host| host.read(cx).controller_view().delivery.has_snapshot)
            {
                self.state = NativeViewState::Ready;
                self.sync_composer_availability(cx);
            }
            return;
        }
        if switch_generation.is_none() {
            self.retire_host(cx);
            if self.conversation_host.is_some() {
                return;
            }
        } else if self.conversation_host.is_some() {
            return;
        }
        let Some(thread_id) = self.pending_thread.take() else {
            return;
        };
        let carry_draft = self
            .thread_switch_flight
            .as_ref()
            .map_or(!self.project_navigation.restore_draft, |flight| {
                flight.carry_draft
            });
        self.project_navigation.restore_draft = false;
        self.composer.update(cx, |composer, cx| {
            composer.switch_thread(thread_id.as_str(), carry_draft, cx);
        });
        self.selected_thread = Some(thread_id.clone());
        if let Some(project) = self.selected_project.clone() {
            self.report_navigation(project, Some(thread_id.clone()));
        }
        if matches!(
            self.route(),
            NativeRoute::NewThread { .. } | NativeRoute::Thread { .. }
        ) && let Some(project) = self.selected_project.clone()
        {
            self.navigate(
                NativeRoute::Thread {
                    project,
                    thread: thread_id.clone(),
                },
                cx,
            );
        }
        if switch_generation.is_none() {
            self.standalone_snapshot_thread = None;
        }
        self.sync_thread_picker_selected(cx);
        self.engine_settings.select_thread(Some(&thread_id));
        self.reset_composer_catalog(cx);
        self.request_engine_settings_for_selected(cx);
        let Ok(host) = ConversationHost::mount(thread_id.clone(), ThemeMode::Dark, &mut *cx) else {
            self.set_failure(invalid_service_failure(), cx);
            return;
        };
        let subscription = cx.observe(&host, |application, host, cx| {
            application.collect_host_effects(&host, cx);
            application.pump_host_boundary(&host, cx);
        });
        self.conversation_host = Some(host.clone());
        let images = self.message_images.clone();
        host.read(cx)
            .surface()
            .clone()
            .update(cx, |surface, cx| surface.set_message_images(images, cx));
        drop(self.conversation_host_subscription.replace(subscription));
        suppress_conversation_tab_stops(&host, cx);
        self.collect_host_effects(&host, cx);
        if switch_generation.is_some() {
            // A fresh subscription start owns the authoritative target
            // snapshot. Drop only this newly mounted host's initial snapshot
            // request; no separate RequestSnapshot is admitted for a switch.
            self.discard_initial_snapshot_request(&thread_id);
            self.submit_thread_switch_subscribe(cx);
        } else {
            self.pump_host_boundary(&host, cx);
            // Subscribe for durable PatchBatch delivery using current cursor when available.
            let after = host.read(cx).controller_view().delivery.cursor;
            if let Some(service) = self.service.clone() {
                match service.submit(NativeTransportCommand::Subscribe {
                    thread_id: thread_id.clone(),
                    after,
                }) {
                    Ok(()) => {}
                    Err(error) => self.set_failure(command_failure(error), cx),
                }
            }
        }
        if switch_generation.is_none()
            && self
                .pending_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.thread_id() == &thread_id)
            && let Some(snapshot) = self.pending_snapshot.take()
        {
            self.dispatch_snapshot(&host, snapshot, cx);
        }
    }

    pub(super) fn discard_initial_snapshot_request(&mut self, thread_id: &ThreadId) {
        self.conversation_effects.retain(|effect| {
            !matches!(
                effect,
                ConversationHostEffect::Controller(ConversationStateEffect::Delivery(
                    ConversationDeliveryEffect::RequestSnapshot { thread_id: requested, .. }
                )) if requested == thread_id
            )
        });
    }

    pub(super) fn retire_host(&mut self, cx: &mut Context<Self>) {
        self.remember_switch_listing();
        if let Some(thread_id) = self.selected_thread.clone() {
            self.remember_switch_snapshot_thread(thread_id.clone());
            if let Some(service) = self.service.clone()
                && self.ordinary_unsubscribe_thread.as_ref() != Some(&thread_id)
            {
                match service.submit(NativeTransportCommand::Unsubscribe {
                    thread_id: thread_id.clone(),
                }) {
                    Ok(()) => self.ordinary_unsubscribe_thread = Some(thread_id),
                    Err(error) => self.set_failure(command_failure(error), cx),
                }
            }
        }
        self.sync_thread_picker_disabled(cx);
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        let Some(host) = self.conversation_host.clone() else {
            self.selected_thread = None;
            self.standalone_snapshot_thread = None;
            self.engine_settings.select_thread(None);
            self.reset_composer_catalog(cx);
            self.sync_thread_picker_selected(cx);
            self.sync_composer_availability(cx);
            cx.notify();
            return;
        };
        self.pump_host_boundary(&host, cx);
        if self.conversation_effects.is_empty() && host.read(cx).total_pending_effect_count() == 0 {
            Self::release_transient_scroll_custody(&host, cx);
            self.conversation_host = None;
            drop(self.conversation_host_subscription.take());
            self.selected_thread = None;
            self.ordinary_unsubscribe_thread = None;
            self.standalone_snapshot_thread = None;
            self.engine_settings.select_thread(None);
            self.reset_composer_catalog(cx);
            self.sync_thread_picker_selected(cx);
            self.sync_thread_picker_disabled(cx);
            self.sync_composer_availability(cx);
            cx.notify();
        }
    }

    pub(super) fn release_transient_scroll_custody(
        host: &Entity<ConversationHost>,
        cx: &mut Context<Self>,
    ) {
        let surface = host.read(cx).surface().clone();
        surface.update(cx, |surface, _| {
            surface.release_transient_scroll_custody();
        });
    }

    pub(super) fn collect_host_effects(
        &mut self,
        host: &Entity<ConversationHost>,
        cx: &mut Context<Self>,
    ) {
        for _ in 0..=CONVERSATION_HOST_MAX_EFFECTS {
            let (pending, total_pending) = {
                let host_ref = host.read(cx);
                (
                    host_ref.pending_effect_count(),
                    host_ref.total_pending_effect_count(),
                )
            };
            let available =
                CONVERSATION_HOST_MAX_EFFECTS.saturating_sub(self.conversation_effects.len());
            if pending > available || total_pending == 0 {
                break;
            }
            let effects = host.update(cx, |host, _| host.drain_effects());
            if effects.is_empty() {
                if host.read(cx).pending_effect_count() == 0 {
                    break;
                }
                continue;
            }
            self.conversation_effects.extend(effects);
        }
    }

    pub(super) fn pump_host_boundary(
        &mut self,
        host: &Entity<ConversationHost>,
        cx: &mut Context<Self>,
    ) {
        let mut retried_surface = false;
        for _ in 0..=CONVERSATION_HOST_MAX_EFFECTS {
            self.collect_host_effects(host, cx);
            while let Some(effect) = self.conversation_effects.first().cloned() {
                match effect {
                    ConversationHostEffect::Controller(ConversationStateEffect::Delivery(
                        ConversationDeliveryEffect::RequestSnapshot { thread_id, .. },
                    )) => {
                        match self
                            .submit_command(NativeTransportCommand::RequestSnapshot(thread_id))
                        {
                            Ok(()) => {
                                self.conversation_effects.remove(0);
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
                    ConversationHostEffect::Controller(
                        ConversationStateEffect::SceneInvalidated
                        | ConversationStateEffect::Delivery(ConversationDeliveryEffect::Invalidate),
                    ) => {
                        self.conversation_effects.remove(0);
                    }
                    ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                        effect,
                    )) => {
                        if !self.apply_viewport_effect(host, &effect, cx) {
                            return;
                        }
                        self.conversation_effects.remove(0);
                    }
                    ConversationHostEffect::ScrollIntent { target } => {
                        let surface = host.read(cx).surface().clone();
                        let accepted = surface.update(cx, |surface, surface_cx| {
                            surface.schedule_scroll_target(target, surface_cx)
                        });
                        if !accepted {
                            return;
                        }
                        self.conversation_effects.remove(0);
                    }
                    ConversationHostEffect::ReadFooterUsage { query } => {
                        let command = crate::native_transport_service::ComposerStateCommand::ReadFooterUsage { query };
                        if self
                            .submit_command(NativeTransportCommand::ComposerState(command))
                            .is_err()
                        {
                            return;
                        }
                        self.conversation_effects.remove(0);
                    }
                    ConversationHostEffect::RichLinkRequests { urls } => {
                        if !self.submit_rich_link_requests(urls, cx) {
                            return;
                        }
                        self.conversation_effects.remove(0);
                    }
                    _ => {
                        self.set_failure(invalid_service_failure(), cx);
                        return;
                    }
                }
            }
            if !self.conversation_effects.is_empty() {
                return;
            }
            if host.read(cx).total_pending_effect_count() != 0 || retried_surface {
                return;
            }
            retried_surface = true;
            host.update(cx, ConversationHost::process_pending_actions);
        }
    }

    pub(super) fn apply_viewport_effect(
        &mut self,
        host: &Entity<ConversationHost>,
        effect: &crate::conversation_view_machine::ViewportEffect,
        cx: &mut Context<Self>,
    ) -> bool {
        match effect {
            crate::conversation_view_machine::ViewportEffect::ShowJumpToLatest => {
                let surface = host.read(cx).surface().clone();
                surface.update(cx, |surface, surface_cx| {
                    surface.set_jump_to_latest_visible(true, surface_cx);
                });
            }
            crate::conversation_view_machine::ViewportEffect::HideJumpToLatest => {
                let surface = host.read(cx).surface().clone();
                surface.update(cx, |surface, surface_cx| {
                    surface.set_jump_to_latest_visible(false, surface_cx);
                });
            }
            crate::conversation_view_machine::ViewportEffect::RequestBottomScroll {
                generation,
            } => {
                let can_scroll = {
                    let view = host.read(cx).controller_view();
                    view.viewport_generation == *generation
                        && match &view.viewport_state {
                            ViewportState::Following => true,
                            ViewportState::Scrolling {
                                generation: active_generation,
                            } => *active_generation == *generation,
                            _ => false,
                        }
                };
                if can_scroll {
                    let surface = host.read(cx).surface().clone();
                    let smooth = matches!(
                        host.read(cx).controller_view().viewport_state,
                        ViewportState::Scrolling { .. }
                    );
                    surface.update(cx, |surface, surface_cx| {
                        if smooth {
                            surface.smooth_scroll_to_bottom(surface_cx);
                        } else {
                            surface.follow_to_bottom(surface_cx);
                        }
                    });
                }
            }
            crate::conversation_view_machine::ViewportEffect::None
            | crate::conversation_view_machine::ViewportEffect::InvalidateRender
            | crate::conversation_view_machine::ViewportEffect::CompletionRejected { .. } => {}
            crate::conversation_view_machine::ViewportEffect::GenerationExhausted => {
                self.set_failure(invalid_service_failure(), cx);
                return false;
            }
        }
        true
    }
}

fn suppress_conversation_tab_stops(
    host: &Entity<ConversationHost>,
    cx: &Context<NativeApplication>,
) {
    let (transcript_focus, disclosure_focus) = {
        let host_ref = host.read(cx);
        let surface = host_ref.surface().read(cx);
        (
            surface.transcript_focus_handle().clone(),
            surface.disclosure_focus_handle().clone(),
        )
    };
    transcript_focus.tab_stop(false);
    disclosure_focus.tab_stop(false);
}
