//! Account-usage readiness and refresh flights for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-4 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

// Include time waiting in the shared command queue, before the transport's
// own request deadline starts. A missing completion must not strand the row
// or a composer send waiting for account readiness.
pub(super) const PROFILE_USAGE_REFRESH_TIMEOUT: Duration = Duration::from_secs(60);

/// A send waiting for its engine's account readiness before admission.
///
/// Nothing is admitted to the Forge yet, so it takes no connection hold; a
/// host switch or quit cancels it and the draft stays in the composer.
pub(super) struct PendingAccountSend {
    pub(super) thread: Option<ThreadId>,
    pub(super) connection: ProfileUsageGeneration,
    pub(super) draft: (u64, u64),
    pub(super) policy: crate::native_model_selector::SelectPolicy,
}

impl NativeApplication {
    /// The model policy a send would use: the thread's explicit choice or the
    /// selector's current policy.
    pub(super) fn displayed_send_policy(
        &self,
        cx: &App,
    ) -> Option<crate::native_model_selector::SelectPolicy> {
        match &self.composer_model_choice {
            Some((thread, policy)) if thread == &self.selected_thread => Some(policy.clone()),
            _ => self.model_selector.read(cx).state().policy().cloned(),
        }
    }

    pub(super) fn resume_account_send(&mut self, engine: &str, cx: &mut Context<Self>) {
        if self
            .pending_account_send
            .as_ref()
            .is_none_or(|send| send.policy.engine_id != engine)
        {
            return;
        }
        let send = self
            .pending_account_send
            .take()
            .expect("matched pending send");
        if send.thread != self.selected_thread
            || send.connection != self.profile_usage_generation
            || send.draft != self.composer.read(cx).send_draft_identity()
            || self.displayed_send_policy(cx).as_ref() != Some(&send.policy)
        {
            self.sync_composer_controls(cx);
            return;
        }
        if engine_readiness(&self.profile_usage, engine, profile_usage_now_ms())
            == EngineReadiness::Ready
        {
            self.begin_message_submission(cx);
        } else {
            self.composer_model_run_error = Some(self.readiness_block_reason(engine));
            self.sync_composer_controls(cx);
            cx.notify();
        }
    }

    pub(super) fn sync_profile_actions(&mut self) {
        let show_usage = self.profile_usage_visible();
        let count = if show_usage { 2 } else { 1 };
        if self.profile_menu.entries().len() != count {
            let mut entries = vec![DropdownMenuEntry::item(DropdownMenuItem::new(
                "settings", "Settings",
            ))];
            if show_usage {
                entries.push(DropdownMenuEntry::item(DropdownMenuItem::new(
                    "usage", "Usage",
                )));
            }
            self.profile_menu.set_entries(entries);
            self.clear_profile_hover();
            self.cancel_profile_usage_scroll();
        }
    }

    pub(super) fn profile_usage_visible(&self) -> bool {
        self.profile_usage_connected() && !self.profile_usage.visible_usage_entries().is_empty()
    }

    /// Returns whether the Forge connection can admit an account-usage read.
    pub(super) fn profile_usage_connected(&self) -> bool {
        #[cfg(test)]
        if self.test_command_sink.is_some() {
            return !self.service_stopped && !self.shutdown_prepared;
        }
        self.service
            .as_ref()
            .is_some_and(|service| !service.is_finished())
            && !self.service_stopped
            && !self.shutdown_prepared
            && !matches!(
                self.state,
                NativeViewState::Loading | NativeViewState::Failure(_)
            )
    }

    /// Advances the connection scope and drops incompatible cache/pending.
    pub(super) fn reset_profile_usage_for_connection(&mut self) {
        let next = self
            .profile_usage_generation
            .checked_next()
            .unwrap_or(ProfileUsageGeneration::first());
        self.profile_usage_generation = next;
        self.profile_usage.clear_for_connection();
        self.pending_account_send = None;
    }

    /// Ensures per-engine usage for an opened menu.
    ///
    /// Missing and stale (180s) rows are dispatched independently with named
    /// pending rows; fresh rows are retained. `force` bypasses freshness and
    /// keeps the menu open. Each dispatch carries the current connection
    /// generation plus a per-engine request sequence so an older same-engine
    /// reply arriving after a forced refresh cannot settle or replace the
    /// newer request.
    pub(super) fn ensure_profile_usage(
        &mut self,
        force: bool,
        only_engine_id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if !self.profile_usage_connected() {
            return;
        }
        let now_ms = profile_usage_now_ms();
        let wanted = plan_profile_usage_loads(&self.profile_usage, now_ms, force, only_engine_id);
        if wanted.is_empty() {
            return;
        }
        let generation = self.profile_usage_generation;
        for engine_id in wanted {
            let Some(request_seq) = self.profile_usage_next_seq.checked_add(1) else {
                let display_name = profile_usage_display_name(&engine_id).to_owned();
                self.profile_usage.accept_failure(
                    &engine_id,
                    &display_name,
                    invalid_service_failure().to_string(),
                    None,
                );
                continue;
            };
            self.profile_usage_next_seq = request_seq;
            let display_name = profile_usage_display_name(&engine_id).to_owned();
            if self.profile_usage.entry(&engine_id).is_none() {
                self.profile_usage.entries.push(NativeUsageEntry::pending(
                    engine_id.clone(),
                    display_name.clone(),
                ));
            }
            self.profile_usage
                .begin_refresh_seq(&engine_id, request_seq);
            let command = NativeTransportCommand::ReadAccountUsage {
                engine_id: engine_id.clone(),
                generation,
                request_seq,
                force,
            };
            if let Err(error) = self.submit_command(command) {
                let failure = command_failure(error);
                let _ = self
                    .profile_usage
                    .finish_refresh_seq(&engine_id, request_seq);
                self.profile_usage.accept_failure(
                    &engine_id,
                    &display_name,
                    failure.to_string(),
                    None,
                );
            } else {
                cx.spawn(async move |view, cx| {
                    cx.background_executor()
                        .timer(PROFILE_USAGE_REFRESH_TIMEOUT)
                        .await;
                    let _ = view.update(cx, |application, cx| {
                        application.settle_account_usage_failure(
                            &engine_id,
                            generation,
                            request_seq,
                            "Usage refresh timed out. Retry to check again.".to_owned(),
                            cx,
                        );
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Refreshes one provider row from its explicit refresh control.
    pub(super) fn refresh_single_profile_engine(
        &mut self,
        engine_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.ensure_profile_usage(true, Some(engine_id), cx);
    }

    pub(super) fn handle_account_usage(
        &mut self,
        engine_id: &str,
        generation: ProfileUsageGeneration,
        request_seq: u64,
        entry: NativeUsageEntry,
        cx: &mut Context<Self>,
    ) {
        if !account_usage_response_current(
            &self.profile_usage,
            generation,
            self.profile_usage_generation,
            engine_id,
            request_seq,
        ) || entry.engine_id.as_str() != engine_id
        {
            return;
        }
        self.profile_usage.try_accept(entry, request_seq);
        self.resume_account_send(engine_id, cx);
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }

    pub(super) fn handle_account_usage_failed(
        &mut self,
        engine_id: &str,
        generation: ProfileUsageGeneration,
        request_seq: u64,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        self.settle_account_usage_failure(
            engine_id,
            generation,
            request_seq,
            failure.to_string(),
            cx,
        );
    }

    fn settle_account_usage_failure(
        &mut self,
        engine_id: &str,
        generation: ProfileUsageGeneration,
        request_seq: u64,
        failure: String,
        cx: &mut Context<Self>,
    ) {
        if !account_usage_response_current(
            &self.profile_usage,
            generation,
            self.profile_usage_generation,
            engine_id,
            request_seq,
        ) {
            return;
        }
        let display_name = self.profile_usage.entry(engine_id).map_or_else(
            || profile_usage_display_name(engine_id).to_owned(),
            |entry| {
                entry
                    .report
                    .as_ref()
                    .map_or(entry.display_name.clone(), |report| {
                        report.display_name.clone()
                    })
            },
        );
        self.profile_usage
            .try_accept_failure(engine_id, &display_name, failure, None, request_seq);
        self.resume_account_send(engine_id, cx);
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }
}
