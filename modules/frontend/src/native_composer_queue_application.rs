//! Mounts the Forge message outbox, queue withdrawals, failed-message retry,
//! and run usage into the composer.
//!
//! The outbox is pushed by the Forge over the thread subscription (see
//! `forge_outbox`); nothing here polls it. Every action names a Forge row by
//! identity: an edit asks the Forge to move the queued payload into the
//! thread's draft, a retry asks it to dispatch its stored payload again, and
//! a new-chat recovery asks it to move the prompt into a new thread.
use super::*;
use crate::composer_queue_state::{
    ComposerQueueIdentity, ComposerQueueState, UsageReadToken, WithdrawalReceiptDisposition,
};
use crate::native_composer_queue::{self as queue_view, QueueControlIntent};
use crate::native_transport_service::{
    ComposerStateCommand as Command, ComposerStateEvent as Event,
};
use artisan_domain::{FailedMessageRetryOutcome, RetryFailedMessage, RunId};

pub(super) struct QueueApplicationState {
    pub(super) state: ComposerQueueState,
    generation: u64,
    usage: Option<UsageReadToken>,
}
impl QueueApplicationState {
    pub(super) fn new(_cx: &mut Context<NativeApplication>) -> Self {
        Self {
            state: ComposerQueueState::new(),
            generation: 0,
            usage: None,
        }
    }
}
impl NativeApplication {
    /// Keeps the outbox and run-usage scope on the selected thread. The
    /// outbox and the live run's usage arrive pushed with the subscription;
    /// a new run's scope reads its usage once.
    pub(super) fn schedule_composer_queue(&mut self, cx: &mut Context<Self>) {
        let changed = self.composer_queue.state.current_thread() != self.selected_thread.as_ref();
        if changed {
            let Some(next) = self.composer_queue.generation.checked_add(1) else {
                return;
            };
            self.composer_queue.generation = next;
            self.composer_queue
                .state
                .set_scope(self.selected_thread.clone(), next);
            self.composer_queue.usage = None;
        }
        if !self.message_composer_visible(cx)
            || self.service.is_none()
            || self.service_stopped
            || self.shutdown_prepared
        {
            return;
        }
        if changed {
            self.sync_composer_controls(cx);
        }
        self.observe_composer_usage_scope(cx);
    }

    /// Applies the live run usage the Forge pushed for a subscribed thread:
    /// to the composer's usage scope when it names the same run, and to a
    /// footer that asked for that run.
    pub(super) fn apply_pushed_run_usage(
        &mut self,
        usage: artisan_domain::RunUsageResult,
        cx: &mut Context<Self>,
    ) {
        if let Some(host) = self.conversation_host.clone() {
            let query =
                artisan_domain::ReadRunUsage::new(usage.thread_id.clone(), usage.run_id.clone());
            let footer = usage.clone();
            host.update(cx, |host, cx| {
                host.accept_footer_usage(&query, Some(footer), cx);
            });
        }
        let name = usage
            .report
            .as_ref()
            .map(|report| report.model_id().as_str().to_owned())
            .unwrap_or_default();
        let _ = self.composer_queue.state.accept_pushed_usage(usage, name);
        self.sync_composer_availability(cx);
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

    /// Applies one queue or failure-card control. Returns whether the event
    /// belonged to the outbox rows.
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
        match intent {
            QueueControlIntent::NewThread(identity) => {
                self.begin_failed_prompt_recovery(&identity, cx);
            }
            QueueControlIntent::Retry(identity) => self.retry_failed_dispatch(&identity, cx),
            QueueControlIntent::Edit(identity) => self.begin_queue_edit(&identity, cx),
            QueueControlIntent::Discard(identity) => {
                if let Ok(request_id) = mint_request_id("native-withdraw")
                    && let Ok(command) = self
                        .composer_queue
                        .state
                        .begin_discard(&identity, request_id)
                {
                    self.send_queue_withdrawal(command);
                }
            }
        }
        self.sync_composer_controls(cx);
        cx.notify();
        true
    }

    /// Asks the Forge to move one queued message back into the thread's
    /// draft. The composer must be empty; it stays locked until the Forge
    /// answers, so the recalled draft cannot race local typing.
    fn begin_queue_edit(&mut self, identity: &ComposerQueueIdentity, cx: &mut Context<Self>) {
        if self.composer.read(cx).capture_recall_target().is_none() {
            return;
        }
        let Ok(request_id) = mint_request_id("native-withdraw") else {
            return;
        };
        let Ok(command) = self.composer_queue.state.begin_edit(identity, request_id) else {
            return;
        };
        self.composer.update(cx, |composer, composer_cx| {
            composer.set_disabled(true, composer_cx);
        });
        self.send_queue_withdrawal(command);
    }

    /// Asks the Forge to dispatch one failed message again from its stored
    /// payload. Only the identity crosses the wire; the new row state comes
    /// back with the pushed outbox.
    fn retry_failed_dispatch(&mut self, identity: &ComposerQueueIdentity, cx: &mut Context<Self>) {
        let Some(target) = self
            .composer_queue
            .state
            .failed_entry_for_identity(identity)
            .map(crate::composer_queue_state::FailedQueueEntry::target)
        else {
            return;
        };
        let Ok(request_id) = mint_request_id("native-retry") else {
            return;
        };
        let command = Command::RetryFailedMessage {
            generation: self.composer_queue.generation,
            command: Box::new(RetryFailedMessage { request_id, target }),
        };
        if let Err(error) = self.submit_command(NativeTransportCommand::ComposerState(command)) {
            self.message_failure = Some(NativeMessageFailure::new(command_failure(error)));
            self.message_failure_note = None;
            self.sync_composer_availability(cx);
        }
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

    /// Releases the edit lock once no recall is waiting for the Forge.
    fn release_queue_edit_lock(&mut self, cx: &mut Context<Self>) {
        if self.composer_queue.state.edit_pending() {
            return;
        }
        let locked = self.host_switch_pending() || self.shutdown_prepared;
        self.composer.update(cx, |composer, composer_cx| {
            composer.set_disabled(locked, composer_cx);
        });
    }

    pub(super) fn retry_composer_queue(&mut self, cx: &mut Context<Self>) {
        if let Some(command) = self.composer_queue.state.retry_pending_withdrawal() {
            self.send_queue_withdrawal(command);
        }
        self.sync_composer_controls(cx);
        cx.notify();
    }

    /// Drops the in-flight usage read for a terminal service failure. The
    /// outbox rows stay as the Forge last pushed them.
    pub(super) fn drop_transient_service_reads(&mut self) {
        self.composer_queue.usage = None;
        self.composer_queue.state.mark_service_failed();
    }

    pub(super) fn handle_composer_state_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::FooterUsage { query, result } => {
                if let Some(host) = self.conversation_host.clone() {
                    host.update(cx, |host, cx| host.accept_footer_usage(&query, result, cx));
                }
            }
            Event::MessageWithdrawn { result, .. } => {
                if let Ok(WithdrawalReceiptDisposition::Recalled) =
                    self.composer_queue.state.accept_withdrawal_result(result)
                {
                    self.reload_forge_draft(cx);
                }
                self.release_queue_edit_lock(cx);
            }
            Event::MessageWithdrawalFailed { command, .. } => {
                if self.composer_queue.state.pending_withdrawal() == Some(command.as_ref()) {
                    self.composer_queue.state.mark_withdrawal_failed();
                }
            }
            Event::FailedMessageRetried { result, .. } => {
                if result.outcome == FailedMessageRetryOutcome::NotRetryable {
                    self.message_failure =
                        Some(NativeMessageFailure::new(invalid_service_failure()));
                    self.message_failure_note = Some(
                        "The Forge can no longer retry this message. Start a new chat with it instead."
                            .to_owned(),
                    );
                }
            }
            Event::FailedMessageRetryFailed { failure, .. } => {
                self.message_failure = Some(NativeMessageFailure::new(failure));
                self.message_failure_note = None;
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
        self.sync_composer_availability(cx);
        cx.notify();
    }
}
