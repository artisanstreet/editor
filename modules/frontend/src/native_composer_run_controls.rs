//! Application-side exact-run observation and cancellation controls.
use super::*;
use artisan_domain::{RunId, StopRun};
use artisan_protocol::{ActiveRunResult, RunLiveStatus, StopRunDisposition, StopRunReceipt};

#[derive(Default)]
pub(super) struct RunControlsState {
    thread: Option<ThreadId>,
    active: Option<RunId>,
    status: Option<RunLiveStatus>,
    engine: Option<artisan_domain::EngineId>,
    available: bool,
    generation: u64,
    pending: Option<u64>,
    stop: Option<StopRun>,
    poll: Option<Task<()>>,
}

impl RunControlsState {
    /// Clears transient run observation for a terminal transport failure.
    ///
    /// An observed active run, pending read, poll task, stop request, and
    /// availability all resolve against the dead service; dropping them
    /// stops the controls from claiming an unobservable run. The mounted
    /// thread scope is preserved so a later service still knows its owner.
    pub(super) fn clear_transient_observation(&mut self) {
        self.active = None;
        self.status = None;
        self.engine = None;
        self.available = false;
        self.pending = None;
        self.stop = None;
        self.poll = None;
    }

    /// Returns the observed live run eligible for steer naming on the
    /// selected thread: a `Running`/`Waiting` run (never a `Queued`
    /// starting run) with its observed engine. The caller additionally
    /// requires the selected engine to match. Scope-fenced on the observed
    /// thread; never validates liveness beyond the last read.
    pub(super) fn steer_candidate(
        &self,
        selected_thread: Option<&ThreadId>,
    ) -> Option<(RunId, artisan_domain::EngineId)> {
        if self.thread.as_ref() != selected_thread {
            return None;
        }
        if !matches!(
            self.status,
            Some(RunLiveStatus::Running | RunLiveStatus::Waiting)
        ) {
            return None;
        }
        Some((self.active.clone()?, self.engine?))
    }

    /// Whether the observed run on the selected thread is still starting.
    ///
    /// Reference (`commands.ts:158-165`): sends never enter a starting
    /// run's queue from this UI. Scope-fenced like [`Self::steer_candidate`].
    pub(super) fn starting_guard_active(&self, selected_thread: Option<&ThreadId>) -> bool {
        self.thread.as_ref() == selected_thread
            && self.active.is_some()
            && matches!(self.status, Some(RunLiveStatus::Queued))
    }

    /// Returns the observed live run and engine on the selected thread, if
    /// any. Scope-fenced like [`Self::steer_candidate`] but status-agnostic.
    pub(super) fn observed_run(
        &self,
        selected_thread: Option<&ThreadId>,
    ) -> Option<(RunId, artisan_domain::EngineId)> {
        if self.thread.as_ref() != selected_thread {
            return None;
        }
        Some((self.active.clone()?, self.engine?))
    }
}

impl NativeApplication {
    pub(super) fn project_run_controls(&self, snapshot: &mut NativeComposerControlsSnapshot) {
        let current = self.run_controls.thread == self.selected_thread;
        snapshot.run_id = current
            .then(|| {
                self.run_controls
                    .active
                    .as_ref()
                    .map(|id| id.as_str().to_owned())
            })
            .flatten();
        snapshot.run_active = snapshot.run_id.is_some();
        snapshot.cancelling = current && self.run_controls.stop.is_some();
        snapshot.abort_available = current && self.run_controls.available && self.service.is_some();
    }

    pub(super) fn schedule_run_observation(&mut self, cx: &mut Context<Self>) {
        if self.run_controls.thread != self.selected_thread {
            self.run_controls.thread = self.selected_thread.clone();
            self.run_controls.active = None;
            self.run_controls.status = None;
            self.run_controls.engine = None;
            self.run_controls.available = false;
            self.run_controls.pending = None;
            self.run_controls.stop = None;
            self.run_controls.poll = None;
            self.sync_composer_controls(cx);
        }
        if self.service.is_none()
            || self.service_stopped
            || self.shutdown_prepared
            || self.run_controls.pending.is_some()
            || self.run_controls.poll.is_some()
            || !self.message_composer_visible(cx)
        {
            return;
        }
        self.run_controls.poll = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(750))
                .await;
            let _ = this.update(cx, |application, cx| {
                application.run_controls.poll = None;
                application.query_composer_run(cx);
            });
        }));
    }

    fn query_composer_run(&mut self, cx: &mut Context<Self>) {
        if self.service_stopped || self.shutdown_prepared || !self.message_composer_visible(cx) {
            return;
        }
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        let Some(generation) = self.run_controls.generation.checked_add(1) else {
            return;
        };
        self.run_controls.generation = generation;
        if let Ok(()) = self.submit_command(NativeTransportCommand::ReadActiveRun {
            thread_id,
            generation,
        }) {
            self.run_controls.pending = Some(generation);
        } else {
            self.run_controls.available = false;
            self.schedule_run_observation(cx);
        }
        self.sync_composer_controls(cx);
    }

    pub(super) fn receive_active_run(
        &mut self,
        thread_id: &ThreadId,
        generation: u64,
        result: Result<ActiveRunResult, ServiceFailure>,
        cx: &mut Context<Self>,
    ) {
        if self.run_controls.pending != Some(generation)
            || self.run_controls.thread.as_ref() != Some(thread_id)
            || self.selected_thread.as_ref() != Some(thread_id)
        {
            return;
        }
        let was_active = self.run_controls.active.is_some();
        self.run_controls.pending = None;
        match result {
            Ok(ActiveRunResult::Active {
                thread_id: owner,
                run_id,
                status,
                engine_id,
            }) if owner == *thread_id => {
                if self.run_controls.active.as_ref() != Some(&run_id) {
                    self.run_controls.stop = None;
                }
                self.run_controls.active = Some(run_id);
                self.run_controls.status = Some(status);
                self.run_controls.engine = Some(engine_id);
                self.run_controls.available = true;
            }
            Ok(ActiveRunResult::NoActive { thread_id: owner }) if owner == *thread_id => {
                self.run_controls.active = None;
                self.run_controls.status = None;
                self.run_controls.engine = None;
                self.run_controls.stop = None;
                self.run_controls.available = true;
            }
            _ => self.run_controls.available = false,
        }
        if let (Some(host), Some(run_id), Some(engine)) = (
            self.conversation_host.clone(),
            self.run_controls.active.clone(),
            self.run_controls.engine,
        ) {
            let turns: Vec<_> = host
                .read(cx)
                .canonical_snapshot()
                .map(|snapshot| {
                    snapshot
                        .items()
                        .iter()
                        .filter_map(|item| {
                            if let ConversationItem::AssistantMessage(message) = item {
                                (message.run_id == run_id).then(|| message.turn_id.clone())
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            for turn_id in turns {
                let _ = host.update(cx, |host, cx| {
                    host.dispatch(
                        ConversationStateEvent::SetTurnEngineLabel {
                            turn_id,
                            engine_label: Some(
                                profile_usage_display_name(engine.as_str()).to_owned(),
                            ),
                        },
                        cx,
                    )
                });
            }
            self.pump_host_boundary(&host, cx);
        }
        self.sync_composer_controls(cx);
        self.schedule_composer_queue(false, cx);
        if was_active && self.run_controls.active.is_none() {
            self.request_composer_usage(cx);
        }
        self.schedule_run_observation(cx);
        cx.notify();
    }

    pub(super) fn stop_composer_run(&mut self, run_id: &str, cx: &mut Context<Self>) {
        if self.run_controls.stop.is_some()
            || !self.run_controls.available
            || self.run_controls.thread != self.selected_thread
            || self.service_stopped
        {
            return;
        }
        let Some(active) = self
            .run_controls
            .active
            .clone()
            .filter(|id| id.as_str() == run_id)
        else {
            return;
        };
        let Some(thread) = self.selected_thread.clone() else {
            return;
        };
        let Ok(request_id) = create_message_request_id() else {
            return;
        };
        let command = StopRun::new(request_id, thread, active);
        match self.submit_command(NativeTransportCommand::StopRun(command.clone())) {
            Ok(()) => self.run_controls.stop = Some(command),
            Err(error) => {
                self.message_failure = Some(NativeMessageFailure::new(command_failure(error)));
            }
        }
        self.sync_composer_controls(cx);
        cx.notify();
    }

    pub(super) fn receive_run_stop(
        &mut self,
        result: Result<StopRunReceipt, ServiceFailure>,
        cx: &mut Context<Self>,
    ) {
        let Ok(receipt) = result else {
            return;
        };
        let Some(command) = self.run_controls.stop.as_ref() else {
            return;
        };
        if command.request_id != receipt.request_id
            || command.thread_id != receipt.thread_id
            || command.run_id != receipt.run_id
        {
            return;
        }
        // A requested receipt is not terminal completion. The live registry and
        // durable conversation projection remain authoritative until settlement.
        if receipt.disposition == StopRunDisposition::NotActive {
            self.run_controls.stop = None;
            self.run_controls.available = false;
        }
        self.sync_composer_controls(cx);
        self.schedule_run_observation(cx);
    }

    pub(super) fn receive_run_stop_failure(
        &mut self,
        command: &StopRun,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        if self.run_controls.stop.as_ref() != Some(command) {
            return;
        }
        self.run_controls.stop = None;
        self.message_failure = Some(NativeMessageFailure::new(failure));
        self.sync_composer_controls(cx);
        self.schedule_run_observation(cx);
        cx.notify();
    }
}

#[cfg(test)]
impl super::NativeApplication {
    /// Seeds one observed active run for terminal-failure presentation tests.
    ///
    /// Test-only direct state seeding: production observes runs exclusively
    /// through the generation-fenced read path.
    pub(super) fn seed_active_run_for_tests(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        status: RunLiveStatus,
        engine_id: artisan_domain::EngineId,
    ) {
        self.run_controls.thread = Some(thread_id);
        self.run_controls.active = Some(run_id);
        self.run_controls.status = Some(status);
        self.run_controls.engine = Some(engine_id);
        self.run_controls.available = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn stale_run_reads_and_stop_receipts_cannot_settle_a_replacement(cx: &mut TestAppContext) {
        let thread = ThreadId::parse("thread-a").unwrap();
        let run = RunId::parse("run-a").unwrap();
        let stop = StopRun::new(
            RequestId::parse("stop-a").unwrap(),
            thread.clone(),
            run.clone(),
        );
        let (view, _) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        cx.update(|app| {
            view.update(app, |application, cx| {
                application.selected_thread = Some(thread.clone());
                application.run_controls.thread = Some(thread.clone());
                application.run_controls.active = Some(run.clone());
                application.run_controls.available = true;
                application.run_controls.pending = Some(2);
                application.run_controls.stop = Some(stop.clone());
                application.receive_active_run(
                    &thread,
                    1,
                    Ok(ActiveRunResult::NoActive {
                        thread_id: thread.clone(),
                    }),
                    cx,
                );
                assert_eq!(application.run_controls.active.as_ref(), Some(&run));
                application.receive_run_stop(
                    Ok(StopRunReceipt {
                        request_id: stop.request_id.clone(),
                        thread_id: thread.clone(),
                        run_id: RunId::parse("other-run").unwrap(),
                        disposition: StopRunDisposition::NotActive,
                    }),
                    cx,
                );
                assert_eq!(application.run_controls.stop.as_ref(), Some(&stop));
                application.receive_run_stop(
                    Ok(StopRunReceipt {
                        request_id: stop.request_id.clone(),
                        thread_id: thread.clone(),
                        run_id: run.clone(),
                        disposition: StopRunDisposition::Requested,
                    }),
                    cx,
                );
                assert!(application.composer_controls.read(cx).snapshot().cancelling);
                assert_eq!(application.run_controls.active.as_ref(), Some(&run));
                application.receive_active_run(
                    &thread,
                    2,
                    Ok(ActiveRunResult::NoActive {
                        thread_id: thread.clone(),
                    }),
                    cx,
                );
                assert!(application.run_controls.active.is_none());
                assert!(application.run_controls.stop.is_none());
                assert!(!application.composer_controls.read(cx).snapshot().run_active);
            });
        });
    }
}
