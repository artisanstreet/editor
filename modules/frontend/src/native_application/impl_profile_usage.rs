//! Account usage for [`NativeApplication`]; each report carries the Forge's
//! readiness verdict for its engine. The Forge pushes every change; the
//! only reads the Editor makes are the user's explicit refreshes.
//!
//! Extracted verbatim from `native_application.rs` during the phase-4 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

// Include time waiting in the shared command queue, before the transport's
// own request deadline starts. A missing completion must not strand the row.
pub(super) const PROFILE_USAGE_REFRESH_TIMEOUT: Duration = Duration::from_secs(60);

impl NativeApplication {
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
        let previous = engine_readiness(&self.profile_usage, engine_id);
        let accepted = self.profile_usage.try_accept(entry, request_seq);
        // The Forge applies readiness to the catalogs it serves, so a new
        // verdict is picked up by reading the catalog again.
        if accepted
            && self
                .profile_usage
                .entry(engine_id)
                .is_some_and(|entry| entry.report.is_some())
            && engine_readiness(&self.profile_usage, engine_id) != previous
        {
            self.refresh_model_catalog(cx);
        }
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }

    /// Applies one engine's usage the Forge pushed. It replaces an older
    /// reading exactly like an explicit refresh's answer; a changed verdict
    /// is picked up by reading the catalog again, as the Forge applies
    /// readiness to the catalogs it serves.
    pub(super) fn apply_pushed_usage(
        &mut self,
        snapshot: &artisan_domain::EngineUsageSnapshot,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = crate::native_profile_usage::usage_entry(snapshot) else {
            return;
        };
        let engine_id = entry.engine_id.clone();
        let previous = engine_readiness(&self.profile_usage, &engine_id);
        self.profile_usage.accept(entry);
        if engine_readiness(&self.profile_usage, &engine_id) != previous {
            self.refresh_model_catalog(cx);
        }
        self.sync_profile_actions();
        self.refresh_settings_engine_snapshot(cx);
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
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }
}
