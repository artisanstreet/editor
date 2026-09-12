//! Mounts the durable queue and immutable usage state into the composer.
use super::*;
use crate::composer_queue_state::{
    ComposerQueueState, QueueRefreshToken, RecallRestoreCandidate, UsageReadToken,
};
use crate::native_composer_queue::{self as queue_view, QueueControlIntent};
use crate::native_transport_service::{
    ComposerStateCommand as Command, ComposerStateEvent as Event,
};
use artisan_domain::{QueuedMessageListOrder, RunId};

pub(super) struct QueueApplicationState {
    pub(super) state: ComposerQueueState,
    generation: u64,
    refresh: Option<QueueRefreshToken>,
    failed_refresh: Option<QueueRefreshToken>,
    usage: Option<UsageReadToken>,
    poll: Option<Task<()>>,
}
impl QueueApplicationState {
    pub(super) fn new(_cx: &mut Context<NativeApplication>) -> Self {
        Self {
            state: ComposerQueueState::new(),
            generation: 0,
            refresh: None,
            failed_refresh: None,
            usage: None,
            poll: None,
        }
    }
}
impl NativeApplication {
    pub(super) fn schedule_composer_queue(&mut self, force: bool, cx: &mut Context<Self>) {
        let changed = self.composer_queue.state.current_thread() != self.selected_thread.as_ref();
        if changed {
            let Some(next) = self.composer_queue.generation.checked_add(1) else {
                return;
            };
            self.composer_queue.generation = next;
            self.composer_queue
                .state
                .set_scope(self.selected_thread.clone(), next);
            self.composer_queue.refresh = None;
            self.composer_queue.failed_refresh = None;
            self.composer_queue.usage = None;
            self.composer_queue.poll = None;
        }
        if !self.message_composer_visible(cx)
            || self.service.is_none()
            || self.service_stopped
            || self.shutdown_prepared
        {
            self.composer_queue.poll = None;
            return;
        }
        if changed {
            self.sync_composer_controls(cx);
        }
        if changed || force {
            self.refresh_composer_queue(true, cx);
        }
        self.observe_composer_usage_scope(cx);
        let run_active = self.composer_controls.read(cx).snapshot().run_active;
        if self.composer_queue.poll.is_some()
            || !self.composer_queue.state.slow_poll_is_eligible(run_active)
        {
            return;
        }
        self.composer_queue.poll = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(
                    crate::composer_queue_state::COMPOSER_QUEUE_REFRESH_INTERVAL_MS,
                ))
                .await;
            let _ = this.update(cx, |app, cx| {
                app.composer_queue.poll = None;
                app.refresh_composer_queue(false, cx);
                app.request_composer_usage(cx);
                app.schedule_composer_queue(false, cx);
            });
        }));
    }

    fn refresh_composer_queue(&mut self, force: bool, cx: &mut Context<Self>) {
        let visible = self.message_composer_visible(cx);
        let ready = self.command_submission_is_available();
        let active = self.composer_controls.read(cx).snapshot().run_active;
        let stopped = self.service_stopped || self.shutdown_prepared;
        let Some(token) = self
            .composer_queue
            .state
            .begin_queue_refresh(visible, ready, stopped, active, force)
        else {
            return;
        };
        let command = Command::ListQueuedMessages {
            thread_id: token.thread_id().clone(),
            generation: token.generation(),
            order: QueuedMessageListOrder::OldestFirst,
            limit: crate::composer_queue_state::COMPOSER_QUEUE_PAGE_LIMIT,
        };
        if self
            .submit_command(NativeTransportCommand::ComposerState(command))
            .is_ok()
        {
            self.composer_queue.refresh = Some(token);
        } else {
            self.composer_queue.state.finish_queue_refresh(&token);
            self.composer_queue.state.mark_transport_failure();
        }
        self.refresh_failed_dispatches(cx);
    }

    /// Fetches one bounded failed-dispatch page beside the queued listing.
    ///
    /// The failed read is independent: it has its own fence token and never
    /// disturbs the queued refresh, withdrawal, recall, or usage flows. A
    /// failed submit only clears its own fence; the queued page stays
    /// authoritative.
    fn refresh_failed_dispatches(&mut self, cx: &mut Context<Self>) {
        let visible = self.message_composer_visible(cx);
        let ready = self.command_submission_is_available();
        let stopped = self.service_stopped || self.shutdown_prepared;
        let Some(token) = self
            .composer_queue
            .state
            .begin_failed_refresh(visible, ready, stopped, true)
        else {
            return;
        };
        let command = Command::ListFailedMessages {
            thread_id: token.thread_id().clone(),
            generation: token.generation(),
            limit: crate::composer_queue_state::COMPOSER_QUEUE_PAGE_LIMIT,
        };
        if self
            .submit_command(NativeTransportCommand::ComposerState(command))
            .is_ok()
        {
            self.composer_queue.failed_refresh = Some(token);
        } else {
            self.composer_queue.state.finish_failed_refresh(&token);
        }
    }

    fn observe_composer_usage_scope(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self.selected_thread.clone() else {
            return;
        };
        let active = self.composer_controls.read(cx).snapshot().run_id.clone();
        let Some(run) = active.and_then(|run| RunId::parse(run).ok()) else {
            return;
        };
        if self
            .composer_queue
            .state
            .usage_scope()
            .is_some_and(|(_, current, _)| current == &run)
        {
            return;
        }
        let Some(config) = self.engine_settings.authoritative_config() else {
            return;
        };
        // Usage scope is OpenCode2-shaped; another engine starts no usage
        // scope instead of attributing usage as OpenCode2.
        let artisan_domain::EngineSelection::OpenCode2(selection) = config.selection() else {
            return;
        };
        self.composer_queue.state.begin_usage_scope(
            thread,
            self.composer_queue.generation,
            run,
            selection.model_id().clone(),
            selection.route_id().clone(),
            selection.variant_id().cloned(),
        );
        self.composer_queue.usage = None;
        self.request_composer_usage(cx);
    }

    pub(super) fn request_composer_usage(&mut self, cx: &mut Context<Self>) {
        if !self.message_composer_visible(cx) || !self.command_submission_is_available() {
            return;
        }
        let Some((token, query)) = self.composer_queue.state.begin_usage_read() else {
            return;
        };
        let command = Command::ReadRunUsage {
            generation: token.generation(),
            sequence: token.sequence(),
            query,
        };
        if self
            .submit_command(NativeTransportCommand::ComposerState(command))
            .is_ok()
        {
            self.composer_queue.usage = Some(token);
        } else {
            self.composer_queue.state.mark_usage_read_failed(&token);
        }
    }

    pub(super) fn handle_queue_control(
        &mut self,
        event: &NativeComposerControlsEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if matches!(event, NativeComposerControlsEvent::RetryQueue) {
            self.retry_composer_queue(cx);
            return true;
        }
        let Some(intent) = queue_view::resolve_controls_event(&self.composer_queue.state, event)
        else {
            return false;
        };
        // The new-chat recovery owns thread creation and draft restore in
        // the application subscription; resolving it here must not mint a
        // withdrawal request id or consume the event.
        if matches!(intent, QueueControlIntent::NewThread(_)) {
            return false;
        }
        let Ok(request_id) = create_save_request_id() else {
            return true;
        };
        let command = match intent {
            QueueControlIntent::Edit(identity) => {
                let Some(target) = self.composer.read(cx).capture_recall_target() else {
                    return true;
                };
                self.composer_queue
                    .state
                    .begin_edit(&identity, request_id, target)
            }
            QueueControlIntent::Discard(identity) => self
                .composer_queue
                .state
                .begin_discard(&identity, request_id),
            QueueControlIntent::NewThread(_) => return false,
        };
        if let Ok(command) = command {
            self.send_queue_withdrawal(command);
        }
        self.sync_composer_controls(cx);
        cx.notify();
        true
    }

    fn send_queue_withdrawal(&mut self, command: artisan_domain::WithdrawQueuedMessageCommand) {
        let command = Command::WithdrawQueuedMessage {
            generation: self.composer_queue.generation,
            command: Box::new(command),
        };
        if self
            .submit_command(NativeTransportCommand::ComposerState(command))
            .is_err()
        {
            self.composer_queue.state.mark_withdrawal_failed();
        }
    }

    fn request_recalled_payload(&mut self) {
        if let Some(query) = self.composer_queue.state.next_recalled_message_read() {
            let command = Command::ReadRecalledMessage {
                generation: self.composer_queue.generation,
                query: query.clone(),
            };
            if self
                .submit_command(NativeTransportCommand::ComposerState(command))
                .is_err()
            {
                self.composer_queue.state.mark_recalled_read_failed(&query);
            }
        }
    }

    /// Drops a pending new-chat recovery whose recall read failed.
    ///
    /// Returns whether `query` belonged to the pending recovery's old scope.
    /// Old history and the failed row stay untouched; the notice names the
    /// transport failure while the failed card keeps the exact dispatcher
    /// reason.
    fn abandon_failed_recovery_for_query(
        &mut self,
        query: &artisan_domain::ReadRecalledMessage,
        failure: crate::native_transport_service::ServiceFailure,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(recovery) = self.pending_failed_recovery.as_ref() else {
            return false;
        };
        if query.thread_id != recovery.old_thread
            || query.message_id != recovery.message_id
            || query.original_request_id != recovery.original_request_id
        {
            return false;
        }
        self.pending_failed_recovery = None;
        self.message_failure = Some(NativeMessageFailure::new(failure));
        self.sync_composer_availability(cx);
        cx.notify();
        true
    }

    fn restore_queue_candidate(
        &mut self,
        candidate: RecallRestoreCandidate,
        cx: &mut Context<Self>,
    ) {
        if self.selected_thread.as_ref() != Some(&candidate.thread_id) {
            let _ = self
                .composer_queue
                .state
                .retain_restore_candidate(candidate);
            return;
        }
        let RecallRestoreCandidate {
            identity,
            thread_id,
            message_id,
            original_request_id,
            target,
            payload,
        } = candidate;
        match self.composer.update(cx, |composer, cx| {
            composer.restore_recalled_payload(&target, payload, cx)
        }) {
            Ok(()) => {
                self.composer_queue.state.mark_restore_succeeded(&identity);
            }
            Err(payload) => {
                let candidate = RecallRestoreCandidate {
                    identity,
                    thread_id,
                    message_id,
                    original_request_id,
                    target,
                    payload,
                };
                let _ = self
                    .composer_queue
                    .state
                    .retain_restore_candidate(candidate);
            }
        }
    }

    pub(super) fn retry_composer_queue(&mut self, cx: &mut Context<Self>) {
        if let Some(command) = self.composer_queue.state.retry_pending_withdrawal() {
            self.send_queue_withdrawal(command);
        } else if self.composer_queue.state.can_retry_restore() {
            if let Some(target) = self.composer.read(cx).capture_recall_target()
                && let Some(candidate) = self
                    .composer_queue
                    .state
                    .take_restore_candidate_with_target(target)
            {
                self.restore_queue_candidate(candidate, cx);
            }
        } else {
            self.request_recalled_payload();
        }
        self.schedule_composer_queue(true, cx);
        self.sync_composer_controls(cx);
        cx.notify();
    }

    /// Drops transient read tokens for a terminal service failure.
    ///
    /// Outstanding queue, failed-listing, and usage reads will never
    /// resolve against a dead service; dropping their tokens returns the
    /// lip from a stuck Refreshing without touching entries, restore
    /// candidates, or drafts. No retry is scheduled here.
    pub(super) fn drop_transient_service_reads(&mut self) {
        self.composer_queue.refresh = None;
        self.composer_queue.failed_refresh = None;
        self.composer_queue.usage = None;
        self.composer_queue.state.mark_service_failed();
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive event dispatch keeps every composer-state projection and its refresh fencing visible together"
    )]
    pub(super) fn handle_composer_state_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::QueuedMessages {
                thread_id,
                generation,
                listing,
            } => {
                if let Some(token) = self.composer_queue.refresh.take_if(|token| {
                    token.thread_id() == &thread_id && token.generation() == generation
                }) && self
                    .composer_queue
                    .state
                    .apply_queue_listing(&token, &listing)
                    .is_ok()
                {
                    // Re-resolve watches retained across a failed label
                    // dispatch against the fresh authoritative page.
                    self.rescan_retained_echo_watches(cx);
                }
            }
            Event::QueuedMessagesFailed {
                thread_id,
                generation,
                ..
            } => {
                if let Some(token) = self.composer_queue.refresh.take_if(|token| {
                    token.thread_id() == &thread_id && token.generation() == generation
                }) {
                    self.composer_queue.state.finish_queue_refresh(&token);
                    self.composer_queue.state.mark_transport_failure();
                }
            }
            Event::FailedMessages {
                thread_id,
                generation,
                listing,
            } => {
                if let Some(token) = self.composer_queue.failed_refresh.take_if(|token| {
                    token.thread_id() == &thread_id && token.generation() == generation
                }) {
                    let _ = self
                        .composer_queue
                        .state
                        .apply_failed_listing(&token, &listing);
                    self.sync_composer_controls(cx);
                }
            }
            Event::FailedMessagesFailed {
                thread_id,
                generation,
                ..
            } => {
                if let Some(token) = self.composer_queue.failed_refresh.take_if(|token| {
                    token.thread_id() == &thread_id && token.generation() == generation
                }) {
                    self.composer_queue.state.finish_failed_refresh(&token);
                }
            }
            Event::MessageWithdrawn { result, .. } => {
                if self
                    .composer_queue
                    .state
                    .accept_withdrawal_result(result)
                    .is_ok()
                {
                    self.request_recalled_payload();
                    self.refresh_composer_queue(true, cx);
                }
            }
            Event::MessageWithdrawalFailed { command, .. } => {
                if self.composer_queue.state.pending_withdrawal() == Some(command.as_ref()) {
                    self.composer_queue.state.mark_withdrawal_failed();
                }
            }
            Event::RecalledMessage { result, .. } => {
                if !self.accept_failed_recovery_result(&result, cx)
                    && self
                        .composer_queue
                        .state
                        .accept_recalled_message(*result)
                        .is_ok()
                    && let Some(candidate) = self.composer_queue.state.take_restore_candidate()
                {
                    self.restore_queue_candidate(candidate, cx);
                }
            }
            Event::RecalledMessageFailed { query, failure, .. } => {
                if !self.abandon_failed_recovery_for_query(&query, failure, cx) {
                    self.composer_queue.state.mark_recalled_read_failed(&query);
                }
            }
            Event::RunUsage {
                generation,
                sequence,
                query,
                result,
            } => {
                if let Some(token) = self.composer_queue.usage.take_if(|token| {
                    token.generation() == generation
                        && token.sequence() == sequence
                        && token.thread_id() == &query.thread_id
                        && token.run_id() == &query.run_id
                }) {
                    let name = result
                        .report
                        .as_ref()
                        .map(|report| report.model_id().as_str().to_owned())
                        .unwrap_or_default();
                    let _ = self
                        .composer_queue
                        .state
                        .accept_usage_result(result, &token, name);
                }
            }
            Event::RunUsageFailed {
                generation,
                sequence,
                query,
                ..
            } => {
                if let Some(token) = self.composer_queue.usage.take_if(|token| {
                    token.generation() == generation
                        && token.sequence() == sequence
                        && token.thread_id() == &query.thread_id
                        && token.run_id() == &query.run_id
                }) {
                    self.composer_queue.state.mark_usage_read_failed(&token);
                }
            }
        }
        self.sync_composer_controls(cx);
        self.schedule_composer_queue(false, cx);
        cx.notify();
    }
}
