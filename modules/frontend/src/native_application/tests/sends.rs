use super::*;

#[gpui::test]
fn tick_drains_queued_answer_into_submit_with_preserved_ids(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    let expected = cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_answer_surface(application, cx, sink);
            let expected = queue_approval_answer(application, cx);
            application.poll_service(cx);
            expected
        })
    });
    let recorded = commands.borrow();
    let submitted = recorded_approval(&recorded);
    assert_eq!(submitted.request_id(), &expected);
    assert_eq!(submitted.thread_id(), &answer_thread());
    assert_eq!(submitted.run_id(), &answer_run());
    assert_eq!(submitted.approval_id().as_str(), "approval-1");
    assert!(submitted.approved);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let host = application.conversation_host.clone().expect("answer host");
            assert!(
                host.read(cx)
                    .surface()
                    .read(cx)
                    .pending_answer_dispatches()
                    .is_empty()
            );
        });
    });
}

#[gpui::test]
fn tick_busy_keeps_row_pending_with_retry_state(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([Err(super::CommandSendError::Busy)]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_answer_surface(application, cx, sink);
            let expected = queue_approval_answer(application, cx);
            application.poll_service(cx);
            assert_eq!(commands.borrow().len(), 1);
            let host = application.conversation_host.clone().expect("answer host");
            let surface = host.read(cx).surface().clone();
            assert_eq!(surface.read(cx).pending_answer_dispatches().len(), 1);
            assert_eq!(
                surface.read(cx).pending_answer_dispatches()[0].request_id,
                expected,
                "nothing is silently dropped"
            );
            assert!(
                !surface.update(cx, |surface, surface_cx| {
                    surface.submit_approval_gesture(
                        "approval-1",
                        &answer_approval(),
                        true,
                        surface_cx,
                    )
                }),
                "single-flight holds across ticks until receipt pairing"
            );
        });
    });
}

#[gpui::test]
fn tick_stopped_reports_diagnostic_without_drop(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([Err(super::CommandSendError::Stopped)]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_answer_surface(application, cx, sink);
            queue_approval_answer(application, cx);
            application.poll_service(cx);
            assert_eq!(commands.borrow().len(), 1);
            let host = application.conversation_host.clone().expect("answer host");
            assert_eq!(
                host.read(cx)
                    .surface()
                    .read(cx)
                    .pending_answer_dispatches()
                    .len(),
                1,
                "a stopped service degrades without drop"
            );
        });
    });
}

#[gpui::test]
fn tick_empty_outbox_leaves_transport_untouched(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([Err(super::CommandSendError::Busy)]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_answer_surface(application, cx, sink);
            application.poll_service(cx);
            assert!(commands.borrow().is_empty());
        });
    });
}

#[gpui::test]
fn second_tick_does_not_resend_before_pairing(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_answer_surface(application, cx, sink);
            queue_approval_answer(application, cx);
            application.poll_service(cx);
            application.poll_service(cx);
            assert_eq!(commands.borrow().len(), 1);
        });
    });
}

#[gpui::test]
fn picker_offline_choice_survives_sync_and_rejects_send_without_losing_draft(
    cx: &mut TestAppContext,
) {
    let (view, cx) = cx.add_window_view(|window, cx| signed_out_test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                ThreadId::parse("picker-task").unwrap(),
                "keep my draft",
                sink,
            );
            let policy = crate::composer_model_config::with_default_native_profile(
                &application
                    .model_selector
                    .read(cx)
                    .state()
                    .snapshot()
                    .selection_policy_for_model("codex-sol")
                    .unwrap(),
            );
            application.handle_composer_model_event(
                &crate::native_model_selector::NativeModelSelectorEvent::SelectPolicy(
                    policy.clone(),
                ),
                cx,
            );
            application.sync_composer_model_policy(cx);
            // A pending offline save may leave the picker showing its own
            // default policy; the durable choice is what has to survive.
            assert_eq!(
                application
                    .composer_model_choice
                    .as_ref()
                    .map(|(_, choice)| choice),
                Some(&policy)
            );
            assert!(
                application
                    .model_selector
                    .read(cx)
                    .state()
                    .status()
                    .error
                    .is_none()
            );
            assert!(application.composer_model_run_error.is_none());
            application.begin_message_submission(cx);
            // The offline choice never reaches the transport: no save
            // command can be built and no message may queue. Readiness
            // probes may be recorded on the shared boundary; the
            // meaningful assertion is that nothing queues.
            assert!(
                commands.borrow().iter().all(|command| !matches!(
                    command,
                    NativeTransportCommand::SubmitComposerDraft(_)
                )),
                "rejected offline send must never queue its message"
            );
            assert_eq!(application.composer.read(cx).draft(), "keep my draft");
            assert!(!application.composer.read(cx).is_submitting());
            assert!(application.composer_model_run_error.is_some());
            application.selected_thread = None;
            application.sync_composer_model_policy(cx);
            assert!(application.composer_model_choice.is_none());
            assert!(application.composer_model_run_error.is_none());
        });
    });
}

#[gpui::test]
fn unconfigured_first_send_without_an_account_verdict_is_refused_not_held(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                ThreadId::parse("first-send-task").unwrap(),
                "keep my draft",
                sink,
            );
            assert!(application.engine_settings.authoritative_config().is_none());
            // The picker always displays its default policy, exactly the
            // production first-send shape: a visible model with no
            // persisted thread configuration and no explicit choice.
            assert!(application.composer_model_choice.is_none());
            assert!(
                application
                    .model_selector
                    .read(cx)
                    .state()
                    .policy()
                    .is_some()
            );
            application.begin_message_submission(cx);
            // The Editor never holds a send for an account check: without a
            // runnable verdict the send is refused with its reason and the
            // draft stays. Readiness probes may be recorded on the shared
            // boundary; the blocked send never queues.
            assert!(
                commands.borrow().iter().all(|command| !matches!(
                    command,
                    NativeTransportCommand::SubmitComposerDraft(_)
                )),
                "blocked first send must never queue its message"
            );
            assert!(application.message_flight.is_none());
            assert!(!application.composer.read(cx).is_submitting());
            assert_eq!(application.composer.read(cx).draft(), "keep my draft");
            assert!(application.composer_model_run_error.is_some());
            admit_probed_codex_usage(application, cx);
            application.begin_message_submission(cx);
            assert!(
                application.message_flight.is_some(),
                "the next Send is admitted once the verdict is runnable"
            );
        });
    });
}

#[gpui::test]
fn explicit_policy_selection_saves_proactively_and_sends_without_hold(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("explicit-first-send-task").expect("thread");
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                thread_id.clone(),
                "explicit pick draft",
                sink,
            );
            // Readiness arrives as a probed usage reply through the real
            // handler â€” never as a manually seated runnable flag â€” so
            // the explicit choice below travels the production path.
            admit_probed_codex_usage(application, cx);
            // The real selection event with the picker's own policy
            // shape: no validate-run gate may strand this explicit
            // choice before the send, and no manual profile is needed
            // for the native default.
            let policy = application
                .model_selector
                .read(cx)
                .state()
                .snapshot()
                .selection_policy_for_model("codex-sol")
                .expect("codex policy");
            application.handle_composer_model_event(
                &crate::native_model_selector::NativeModelSelectorEvent::SelectPolicy(policy),
                cx,
            );
            assert!(application.engine_settings.authoritative_config().is_none());
            // The selection auto-saved through the shared direct typed
            // save; the send queues immediately without holding for its
            // acknowledgment (unnamed: no authoritative engine yet).
            let save_request = admitted_save_request(application);
            let retained = application
                .engine_settings
                .pending_save()
                .map(|(_, config)| config.clone())
                .expect("selection save tracked");
            application.begin_message_submission(cx);
            let flight = application
                .message_flight
                .as_ref()
                .expect("unheld first send");
            assert_eq!(flight.thread_id, thread_id);
            assert!(application.composer_model_run_error.is_none());
            // The authoritative acknowledgment only seats the durable
            // configuration; it neither creates nor continues the send.
            application.handle_engine_config_set(
                &artisan_protocol::SetThreadEngineConfigResult {
                    request_id: save_request,
                    thread_id: thread_id.clone(),
                    revision: artisan_domain::EngineConfigRevision::new(1).expect("revision"),
                    disposition: artisan_domain::ReceiptDisposition::Accepted,
                },
                retained,
                cx,
            );
            assert!(application.engine_settings.authoritative_config().is_some());
            assert_eq!(
                application
                    .message_flight
                    .as_ref()
                    .expect("send unaffected by ack")
                    .thread_id,
                thread_id
            );
        });
    });
    let commands = commands.borrow();
    // The production-shaped command sequence: the probed account read,
    // the proactive typed selection save with its `Unconfigured`
    // precondition, then the immediate unheld queue. Nothing here seats
    // readiness or holds the save by hand.
    let save = commands
        .iter()
        .find_map(|command| match command {
            NativeTransportCommand::SetThreadEngineConfig(command) => Some(command),
            _ => None,
        })
        .expect("typed selection save");
    assert_eq!(save.thread_id(), &thread_id);
    assert_eq!(
        save.precondition(),
        artisan_domain::EngineConfigUpdatePrecondition::Unconfigured
    );
    let queued = commands
        .iter()
        .find_map(|command| match command {
            NativeTransportCommand::SubmitComposerDraft(command) => Some(command),
            _ => None,
        })
        .expect("unheld explicit send must queue its message");
    assert_eq!(queued.thread_id, thread_id);
    assert!(
        queued.draft_revision.get() > 0,
        "the send names its stored draft"
    );
    assert!(queued.steer_target.is_none());
}

#[gpui::test]
fn queued_rows_surface_in_timeline_with_dispatch_diagnostics(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("queued-unconfigured-task").expect("thread");
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, _) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(application, cx, thread_id.clone(), "draft", sink);
            assert!(application.engine_settings.authoritative_config().is_none());
            // The Forge pushes one accepted row it is holding, with the
            // dispatcher's reason as its status.
            let mut row = queued_summary(
                &thread_id,
                "message-a",
                artisan_domain::QueuedMessageState::Queued,
            );
            row.last_error = Some(
                artisan_domain::DispatchError::parse("engine unconfigured".to_owned())
                    .expect("dispatcher diagnostic"),
            );
            application.handle_service_event(
                NativeTransportEvent::MessageOutbox(message_outbox(
                    &thread_id,
                    vec![row],
                    Vec::new(),
                )),
                cx,
            );
            let snapshot = application.composer_controls.read(cx).snapshot().clone();
            assert!(!snapshot.run_active);
            // Reference C5: rows surface in the transcript tail only; no
            // count/status banner copy exists.
            assert!(snapshot.pending_steering.is_empty());
            let host = application.conversation_host.clone().unwrap();
            let surface = host.read(cx).surface().read(cx);
            let rows = surface.pending_message_rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].message_id, "message-a");
            assert_eq!(rows[0].status, "Waiting: engine unconfigured");
            assert_eq!(
                application.composer_queue.state.entries()[0].dispatch_error(),
                Some("engine unconfigured")
            );
            assert!(snapshot.failure.is_none());
        });
    });
}

#[gpui::test]
fn save_ack_seats_config_without_touching_the_unheld_send(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("first-send-task").expect("thread");
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([Ok(())]);
    let (retained, save_request) = cx.update(|_, app| {
        view.update(app, |application, cx| {
            let retained =
                install_admitted_first_send(application, cx, &thread_id, "keep my draft", sink);
            let save_request = admitted_save_request(application);
            // The send queued immediately, before any acknowledgment.
            let flight = application.message_flight.as_ref().expect("unheld flight");
            assert_eq!(flight.thread_id, thread_id);
            (retained, save_request)
        })
    });
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_engine_config_set(
                &artisan_protocol::SetThreadEngineConfigResult {
                    request_id: save_request,
                    thread_id: thread_id.clone(),
                    revision: artisan_domain::EngineConfigRevision::new(1).expect("revision"),
                    disposition: artisan_domain::ReceiptDisposition::Accepted,
                },
                retained,
                cx,
            );
            // The acknowledgment only seats the durable configuration;
            // the already-queued send is untouched.
            let flight = application
                .message_flight
                .as_ref()
                .expect("send unaffected by ack");
            assert_eq!(flight.thread_id, thread_id);
            assert!(application.engine_settings.authoritative_config().is_some());
            assert!(application.composer_model_run_error.is_none());
            assert_eq!(application.composer.read(cx).draft(), "");
        });
    });
    let commands = commands.borrow();
    // The production-shaped command sequence: the probed account read,
    // the proactive typed save with its `Unconfigured` precondition,
    // and the immediate unheld queue. Nothing waits for the ack.
    let save = commands
        .iter()
        .find_map(|command| match command {
            NativeTransportCommand::SetThreadEngineConfig(command) => Some(command),
            _ => None,
        })
        .expect("typed first-send save");
    assert_eq!(save.thread_id(), &thread_id);
    assert_eq!(
        save.precondition(),
        artisan_domain::EngineConfigUpdatePrecondition::Unconfigured
    );
    let queued = commands
        .iter()
        .find_map(|command| match command {
            NativeTransportCommand::SubmitComposerDraft(command) => Some(command),
            _ => None,
        })
        .expect("unheld first send must queue its message");
    assert_eq!(queued.thread_id, thread_id);
    assert!(
        queued.draft_revision.get() > 0,
        "the send names its stored draft"
    );
}

#[gpui::test]
fn save_failure_does_not_hold_the_first_send(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("first-send-failed-task").expect("thread");
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_admitted_first_send(application, cx, &thread_id, "keep my draft", sink);
            // The send queued immediately while the save was in flight;
            // the save failure cannot recall it. The backend accept
            // transaction refuses the unconfigured send typed; the
            // failure banner for that refusal arrives separately.
            let save_request = admitted_save_request(application);
            application.handle_engine_config_failed(
                &thread_id,
                &save_request,
                message_failure(),
                cx,
            );
            let flight = application
                .message_flight
                .as_ref()
                .expect("send not held by save failure");
            assert_eq!(flight.thread_id, thread_id);
            assert!(application.composer.read(cx).is_submitting());
            assert_eq!(application.composer.read(cx).draft(), "");
        });
    });
    // The unheld send queued despite the failed save. The admitted
    // typed save and readiness probes stay recorded.
    assert!(
        commands
            .borrow()
            .iter()
            .any(|command| matches!(command, NativeTransportCommand::SubmitComposerDraft(_))),
        "unheld first send must queue despite the save failure"
    );
}

#[gpui::test]
fn same_engine_live_run_names_the_send_as_a_steer(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("steer-task").expect("thread");
    let run_id = RunId::parse("run-live").expect("run");
    let (view, cx) = cx.add_window_view(|window, cx| signed_in_test_application(window, cx));
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                thread_id.clone(),
                "steer this turn",
                sink,
            );
            install_configured_engine_settings(application, cx);
            application.seed_active_run_for_tests(
                thread_id.clone(),
                run_id.clone(),
                artisan_protocol::RunLiveStatus::Running,
                artisan_domain::EngineId::Codex,
            );
            application.begin_message_submission(cx);
            assert!(application.message_flight.is_some(), "named flight");
        });
    });
    let queued = commands
        .borrow()
        .iter()
        .find_map(|command| match command {
            NativeTransportCommand::SubmitComposerDraft(command) => Some(command.clone()),
            _ => None,
        })
        .expect("named send must queue");
    assert_eq!(
        queued
            .steer_target
            .as_ref()
            .expect("wire command carries the named run")
            .run_id()
            .as_str(),
        "run-live"
    );
}

#[gpui::test]
fn cross_engine_selection_sends_unnamed(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("cross-engine-task").expect("thread");
    let (view, cx) = cx.add_window_view(|window, cx| signed_in_test_application(window, cx));
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                thread_id.clone(),
                "next engine turn",
                sink,
            );
            install_configured_engine_settings(application, cx);
            // The live run belongs to another engine: naming it would
            // pin the command back to its engine, so the send stays
            // unnamed and the backend takes the fresh-run path.
            application.seed_active_run_for_tests(
                thread_id.clone(),
                RunId::parse("run-other-engine").expect("run"),
                artisan_protocol::RunLiveStatus::Running,
                artisan_domain::EngineId::Claude,
            );
            application.begin_message_submission(cx);
            assert!(application.message_flight.is_some(), "unnamed flight");
        });
    });
    let queued = commands
        .borrow()
        .iter()
        .find_map(|command| match command {
            NativeTransportCommand::SubmitComposerDraft(command) => Some(command.clone()),
            _ => None,
        })
        .expect("unnamed send must queue");
    assert!(queued.steer_target.is_none());
}

#[gpui::test]
fn starting_run_refuses_the_send_with_its_reason(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("starting-task").expect("thread");
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                thread_id.clone(),
                "too early draft",
                sink,
            );
            install_configured_engine_settings(application, cx);
            application.seed_active_run_for_tests(
                thread_id.clone(),
                RunId::parse("run-starting").expect("run"),
                artisan_protocol::RunLiveStatus::Queued,
                artisan_domain::EngineId::Codex,
            );
            application.begin_message_submission(cx);
            // Refused, never queued: no flight, draft preserved, and
            // the banner names the attempt with Dismiss only.
            assert!(application.message_flight.is_none());
            assert!(!application.composer.read(cx).is_submitting());
            assert_eq!(application.composer.read(cx).draft(), "too early draft");
            let note = application
                .message_failure_note
                .clone()
                .expect("starting refusal names its reason");
            assert!(
                note.contains("still starting"),
                "unexpected refusal: {note}"
            );
            let snapshot = application.composer_controls.read(cx).snapshot().clone();
            let failure = snapshot.failure.expect("refusal banner");
            assert!(failure.failure.description.contains("still starting"));
            assert!(!failure.retryable);
        });
    });
    assert!(
        commands
            .borrow()
            .iter()
            .all(|command| !matches!(command, NativeTransportCommand::SubmitComposerDraft(_))),
        "starting-guard refusal must never queue"
    );
}

#[gpui::test]
fn delivered_message_turn_is_labelled_from_its_outbox_row(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("echo-task").expect("thread");
    let (view, cx) = cx.add_window_view(|window, cx| signed_in_test_application(window, cx));
    let (sink, _) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                thread_id.clone(),
                "visible text here",
                sink,
            );
            install_configured_engine_settings(application, cx);
            application.begin_message_submission(cx);
            let receipt = send_receipt_for_flight(application, &thread_id, "message-echo");
            application.handle_service_event(NativeTransportEvent::MessageQueued(receipt), cx);
            assert!(application.message_flight.is_none());
            assert_eq!(application.composer.read(cx).draft(), "");
            // The Forge's row names the engine its accepted configuration
            // routes to; it differs from the picker on purpose.
            let mut row = queued_summary(
                &thread_id,
                "message-echo",
                artisan_domain::QueuedMessageState::Dispatching,
            );
            row.engine = Some(artisan_domain::EngineId::Claude);
            application.handle_service_event(
                NativeTransportEvent::MessageOutbox(message_outbox(
                    &thread_id,
                    vec![row],
                    Vec::new(),
                )),
                cx,
            );
            let host = application.conversation_host.clone().expect("mounted host");
            assert!(host.read(cx).surface().read(cx).has_pending_messages());
            // The Forge delivers the transcript item before the outbox that
            // no longer lists it.
            application.handle_service_event(
                NativeTransportEvent::Snapshot(snapshot_for(&thread_id, 1)),
                cx,
            );
            application.handle_service_event(
                NativeTransportEvent::PatchBatch(echo_batch(
                    &thread_id,
                    1,
                    "item-echo",
                    Some("message-echo"),
                    "turn-echo",
                    0,
                    1,
                    "visible text here",
                )),
                cx,
            );
            application.handle_service_event(
                NativeTransportEvent::MessageOutbox(message_outbox(
                    &thread_id,
                    Vec::new(),
                    Vec::new(),
                )),
                cx,
            );
            assert!(!host.read(cx).surface().read(cx).has_pending_messages());
            let scene = staged_turn_scene(application, cx, "turn-echo");
            assert_eq!(staged_user_bodies(&scene), ["visible text here"]);
            assert_eq!(
                staged_status(&scene)
                    .and_then(|(_, label)| label)
                    .as_deref(),
                Some("Claude")
            );
            assert!(application.message_failure.is_none());
        });
    });
}

#[gpui::test]
fn legacy_echo_without_source_id_takes_no_label(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("legacy-echo-task").expect("thread");
    let (view, cx) = cx.add_window_view(|window, cx| signed_in_test_application(window, cx));
    let (sink, _) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(application, cx, thread_id.clone(), "legacy text", sink);
            install_configured_engine_settings(application, cx);
            application.begin_message_submission(cx);
            let receipt = send_receipt_for_flight(application, &thread_id, "message-legacy");
            application.handle_service_event(NativeTransportEvent::MessageQueued(receipt), cx);
            application.handle_service_event(
                NativeTransportEvent::MessageOutbox(message_outbox(
                    &thread_id,
                    vec![queued_summary(
                        &thread_id,
                        "message-legacy",
                        artisan_domain::QueuedMessageState::Dispatching,
                    )],
                    Vec::new(),
                )),
                cx,
            );
            application.handle_service_event(
                NativeTransportEvent::Snapshot(snapshot_for(&thread_id, 1)),
                cx,
            );
            // The item id equals the message id here, but without a source
            // id that proves nothing: the turn takes no label.
            application.handle_service_event(
                NativeTransportEvent::PatchBatch(echo_batch(
                    &thread_id,
                    1,
                    "message-legacy",
                    None,
                    "turn-legacy",
                    0,
                    1,
                    "legacy text",
                )),
                cx,
            );
            let scene = staged_turn_scene(application, cx, "turn-legacy");
            assert_eq!(staged_status(&scene).and_then(|(_, label)| label), None);
            assert!(application.message_failure.is_none());
        });
    });
}

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end scenario drives the full event chain; splitting it would hide the causal ordering the test asserts"
)]
fn mounted_send_streams_waiting_thinking_reply_terminal(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("staged-task").expect("thread");
    // Flow test, not animation test: motion holds would obscure paint.
    cx.update(|app| {
        app.set_reduce_motion(true);
    });
    let (view, cx) = cx.add_window_view(|window, cx| signed_in_test_application(window, cx));
    let (sink, _) = command_sink([Ok(())]);
    // Stage 1: submit + receipt, then the Forge's outbox row naming the
    // engine the accepted configuration routes to.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                thread_id.clone(),
                "staged prompt",
                sink,
            );
            install_configured_engine_settings(application, cx);
            application.begin_message_submission(cx);
            assert!(application.message_flight.is_some(), "staged flight");
            let receipt = send_receipt_for_flight(application, &thread_id, "message-staged");
            application.handle_service_event(NativeTransportEvent::MessageQueued(receipt), cx);
            assert!(application.message_flight.is_none());
            assert_eq!(application.composer.read(cx).draft(), "");
            application.handle_service_event(
                NativeTransportEvent::MessageOutbox(message_outbox(
                    &thread_id,
                    vec![queued_summary(
                        &thread_id,
                        "message-staged",
                        artisan_domain::QueuedMessageState::Dispatching,
                    )],
                    Vec::new(),
                )),
                cx,
            );
        });
    });
    cx.run_until_parked();
    // Stage 2: canonical snapshot, the delivered item, then the outbox that
    // no longer lists it. The painted user body appears exactly once with
    // the provider wait labelled from the outbox row.
    let user_selector: String = cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                NativeTransportEvent::Snapshot(snapshot_for(&thread_id, 1)),
                cx,
            );
            application.handle_service_event(
                NativeTransportEvent::PatchBatch(echo_batch(
                    &thread_id,
                    1,
                    "item-staged",
                    Some("message-staged"),
                    "turn-staged",
                    0,
                    1,
                    "staged prompt",
                )),
                cx,
            );
            application.handle_service_event(
                NativeTransportEvent::MessageOutbox(message_outbox(
                    &thread_id,
                    Vec::new(),
                    Vec::new(),
                )),
                cx,
            );
            assert!(
                application
                    .composer_controls
                    .read(cx)
                    .snapshot()
                    .pending_steering
                    .is_empty()
            );
            assert!(application.message_failure.is_none());
            let scene = staged_turn_scene(application, cx, "turn-staged");
            assert_eq!(staged_user_bodies(&scene), ["staged prompt"]);
            assert_eq!(
                staged_status(&scene),
                Some((TurnNarration::ProviderWait, Some("Codex".to_owned())))
            );
            let user_block = scene
                .blocks()
                .iter()
                .find_map(|block| match block {
                    TurnBlock::UserMessage(message) => Some(message.clone()),
                    _ => None,
                })
                .expect("painted user block");
            crate::conversation_surface::block_selector(
                &TurnId::parse("turn-staged").expect("turn"),
                &TurnBlock::UserMessage(user_block),
            )
        })
    });
    cx.run_until_parked();
    let staged_turn = TurnId::parse("turn-staged").expect("turn");
    let staged_turn_selector: &'static str =
        Box::leak(crate::conversation_surface::turn_selector(&staged_turn).into_boxed_str());
    let staged_status_selector: &'static str =
        Box::leak(crate::conversation_surface::status_selector(&staged_turn).into_boxed_str());
    assert!(
        cx.debug_bounds(staged_turn_selector).is_some(),
        "echoed turn paints"
    );
    assert!(
        cx.debug_bounds(Box::leak(user_selector.into_boxed_str()))
            .is_some(),
        "echoed user body paints"
    );
    assert!(
        cx.debug_bounds(staged_status_selector).is_some(),
        "provider wait row paints"
    );
    // Stage 3: genuine attributed reasoning before any assistant text.
    // The finished sentence is the trace: header owns Thinking, the
    // status row renders the summary, the user body stays exact-once.
    let header_selector: String = cx.update(|_, app| {
        view.update(app, |application, cx| {
            let delta = artisan_domain::ReasoningSummaryDeltaObservation::new(
                artisan_domain::ObservationId::parse("obs-1").expect("observation"),
                artisan_domain::ObservationSequence::new(0).expect("sequence"),
                artisan_domain::ObservationId::parse("obs-item-1").expect("observation item"),
                0,
                "Considering options.".to_owned(),
                None,
                artisan_domain::ObservationId::parse("obs-turn-1").expect("observation turn"),
            )
            .expect("reasoning delta");
            let observation = artisan_domain::EngineObservationEvent {
                thread_id: thread_id.clone(),
                observation: artisan_domain::Observation::ReasoningSummaryDelta(delta),
                attribution: Some(artisan_domain::EngineObservationAttribution {
                    run_id: RunId::parse("run-staged").expect("run"),
                    turn_id: TurnId::parse("turn-staged").expect("turn"),
                    committed_at: UnixMillis::from_millis(11),
                    delivery_sequence: 1,
                }),
            };
            application.handle_service_event(
                NativeTransportEvent::EngineObservation(artisan_protocol::ServerEvent {
                    cursor: artisan_protocol::EventCursor::new(1).expect("cursor"),
                    event: artisan_domain::Event::EngineObservation(observation),
                }),
                cx,
            );
            let scene = staged_turn_scene(application, cx, "turn-staged");
            assert_eq!(
                staged_status(&scene).map(|(narration, _)| narration),
                Some(TurnNarration::Thinking)
            );
            assert_eq!(staged_user_bodies(&scene), ["staged prompt"]);
            assert!(application.message_failure.is_none());
            // The finished sentence is the trace: the owning work-group
            // header carries it and the status row renders it through
            // the same summary policy (no duplicate Thinking line).
            let group = scene
                .blocks()
                .iter()
                .find_map(|block| match block {
                    TurnBlock::WorkGroup(group) => Some(group.clone()),
                    _ => None,
                })
                .expect("thinking work group");
            assert_eq!(
                group.reasoning_summary.as_deref(),
                Some("Considering options.")
            );
            let status = scene
                .blocks()
                .iter()
                .find_map(|block| match block {
                    TurnBlock::TurnStatus(status) => Some(status.clone()),
                    _ => None,
                })
                .expect("thinking status row");
            assert_eq!(
                status.reasoning_summary.as_deref(),
                Some("Considering options.")
            );
            assert_eq!(
                crate::conversation_surface::turn_status_copy_text(
                    status.narration,
                    status.active_started_at_ms,
                    None,
                    status.reasoning_summary.as_deref(),
                    status.engine_label.as_deref(),
                )
                .as_deref(),
                Some("Considering options.")
            );
            format!(
                "{}-header",
                crate::conversation_surface::block_selector(
                    &TurnId::parse("turn-staged").expect("turn"),
                    &TurnBlock::WorkGroup(group),
                )
            )
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds(Box::leak(header_selector.into_boxed_str()))
            .is_some(),
        "thinking header paints"
    );
    assert!(
        cx.debug_bounds(staged_status_selector).is_some(),
        "thinking trace paints"
    );
    // Stage 4: first streamed body on the SAME echoed turn.
    let first_reply_selector: String = cx.update(|_, app| {
        view.update(app, |application, cx| {
            let assistant = ConversationItem::AssistantMessage(AssistantMessageItem {
                item_id: ItemId::parse("item-reply").expect("item"),
                turn_id: TurnId::parse("turn-staged").expect("turn"),
                run_id: RunId::parse("run-staged").expect("run"),
                ordinal: ItemOrdinal::new(2),
                revision: Revision::new(0),
                lifecycle: ConversationLifecycle::Streaming,
                body: AssistantBody::parse("Hel").expect("assistant body"),
                phase: AssistantMessagePhase::Final,
                created_at: UnixMillis::EPOCH,
                updated_at: UnixMillis::from_millis(12),
            });
            application.handle_service_event(
                NativeTransportEvent::PatchBatch(
                    PatchBatch::new(
                        thread_id.clone(),
                        ConversationCursor::new(3),
                        ConversationCursor::new(4),
                        vec![ConversationPatch::ItemUpsert {
                            patch_id: PatchId::parse("patch-reply-item").expect("patch"),
                            sequence: PatchSequence::new(4).expect("sequence"),
                            item: assistant,
                        }],
                    )
                    .expect("reply batch"),
                ),
                cx,
            );
            let scene = staged_turn_scene(application, cx, "turn-staged");
            let reply: Vec<_> = scene
                .blocks()
                .iter()
                .filter_map(|block| match block {
                    TurnBlock::AssistantMessage(message) => Some(message.body.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(reply, ["Hel"]);
            assert_eq!(staged_user_bodies(&scene), ["staged prompt"]);
            scene
                .blocks()
                .iter()
                .find_map(|block| match block {
                    TurnBlock::AssistantMessage(message) => {
                        Some(crate::conversation_surface::block_selector(
                            &TurnId::parse("turn-staged").expect("turn"),
                            &TurnBlock::AssistantMessage(message.clone()),
                        ))
                    }
                    _ => None,
                })
                .expect("painted first reply block")
        })
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds(Box::leak(first_reply_selector.into_boxed_str()))
            .is_some(),
        "first streamed body paints"
    );
    // Stage 5: two streamed bodies, one update each. "Hel" + "lo " +
    // "world". Each fragment is asserted painted before the next lands.
    let mut append_reply = |patch_id: &str,
                            fragment: &str,
                            from: u64,
                            sequence: u64,
                            revision: u64,
                            stamp: i64,
                            expected: &str| {
        let assistant_selector: String = cx.update(|_, app| {
            view.update(app, |application, cx| {
                application.handle_service_event(
                    NativeTransportEvent::PatchBatch(
                        PatchBatch::new(
                            thread_id.clone(),
                            ConversationCursor::new(from),
                            ConversationCursor::new(from + 1),
                            vec![ConversationPatch::ItemAppend {
                                patch_id: PatchId::parse(patch_id).expect("patch"),
                                sequence: PatchSequence::new(sequence).expect("sequence"),
                                item_id: ItemId::parse("item-reply").expect("item"),
                                revision: Revision::new(revision),
                                text: IncrementalText::parse(fragment).expect("fragment"),
                                updated_at: UnixMillis::from_millis(stamp),
                            }],
                        )
                        .expect("append batch"),
                    ),
                    cx,
                );
                let scene = staged_turn_scene(application, cx, "turn-staged");
                let reply: Vec<_> = scene
                    .blocks()
                    .iter()
                    .filter_map(|block| match block {
                        TurnBlock::AssistantMessage(message) => Some(message.body.clone()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(reply.len(), 1);
                assert_eq!(reply, [expected]);
                assert_eq!(staged_user_bodies(&scene), ["staged prompt"]);
                let assistant = scene
                    .blocks()
                    .iter()
                    .find_map(|block| match block {
                        TurnBlock::AssistantMessage(message) => Some(message.clone()),
                        _ => None,
                    })
                    .expect("painted assistant block");
                crate::conversation_surface::block_selector(
                    &TurnId::parse("turn-staged").expect("turn"),
                    &TurnBlock::AssistantMessage(assistant),
                )
            })
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(Box::leak(assistant_selector.into_boxed_str()))
                .is_some(),
            "streamed reply paints after its own update"
        );
    };
    append_reply("patch-append-one", "lo ", 4, 5, 1, 13, "Hello ");
    append_reply("patch-append-two", "world", 5, 6, 2, 14, "Hello world");
    // Stage 6: terminal. Exactly one user body, settled reply, no
    // failures, no flights, no Forge rows.
    let terminal_reply: Vec<String> = cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                NativeTransportEvent::PatchBatch(
                    PatchBatch::new(
                        thread_id.clone(),
                        ConversationCursor::new(6),
                        ConversationCursor::new(8),
                        vec![
                            ConversationPatch::TurnLifecycle {
                                patch_id: PatchId::parse("patch-terminal-turn").expect("patch"),
                                sequence: PatchSequence::new(7).expect("sequence"),
                                turn_id: TurnId::parse("turn-staged").expect("turn"),
                                revision: Revision::new(1),
                                lifecycle: ConversationLifecycle::Completed,
                                updated_at: UnixMillis::from_millis(15),
                            },
                            ConversationPatch::ItemLifecycle {
                                patch_id: PatchId::parse("patch-terminal-item").expect("patch"),
                                sequence: PatchSequence::new(8).expect("sequence"),
                                item_id: ItemId::parse("item-reply").expect("item"),
                                revision: Revision::new(3),
                                lifecycle: ConversationLifecycle::Completed,
                                updated_at: UnixMillis::from_millis(15),
                            },
                        ],
                    )
                    .expect("terminal batch"),
                ),
                cx,
            );
            let scene = staged_turn_scene(application, cx, "turn-staged");
            assert_eq!(staged_user_bodies(&scene), ["staged prompt"]);
            assert!(application.message_failure.is_none());
            assert!(application.message_flight.is_none());
            assert!(application.composer_queue.state.entries().is_empty());
            assert!(
                application
                    .composer_controls
                    .read(cx)
                    .snapshot()
                    .pending_steering
                    .is_empty()
            );
            scene
                .blocks()
                .iter()
                .filter_map(|block| match block {
                    TurnBlock::AssistantMessage(message) => Some(message.body.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
    });
    cx.run_until_parked();
    assert_eq!(terminal_reply, ["Hello world"]);
    assert!(
        cx.debug_bounds(staged_turn_selector).is_some(),
        "settled turn still paints"
    );
}

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end scenario drives the full event chain; splitting it would hide the causal ordering the test asserts"
)]
fn two_sends_leave_the_outbox_independently(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("two-send-task").expect("thread");
    let (view, cx) = cx.add_window_view(|window, cx| signed_in_test_application(window, cx));
    let (sink, _) = command_sink([Ok(()), Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(application, cx, thread_id.clone(), "first send", sink);
            install_configured_engine_settings(application, cx);
            application.begin_message_submission(cx);
            let receipt_one = send_receipt_for_flight(application, &thread_id, "message-one");
            application.handle_service_event(NativeTransportEvent::MessageQueued(receipt_one), cx);
            // The accepted flight clears the composer, so the second
            // send re-drafts before submitting.
            application.composer.update(cx, |composer, composer_cx| {
                composer.set_draft("second send".to_owned());
                composer_cx.notify();
            });
            application.begin_message_submission(cx);
            let receipt_two = send_receipt_for_flight(application, &thread_id, "message-two");
            application.handle_service_event(NativeTransportEvent::MessageQueued(receipt_two), cx);
            // Receipts end flights; the Forge's outbox lists both rows.
            application.handle_service_event(
                NativeTransportEvent::MessageOutbox(message_outbox(
                    &thread_id,
                    vec![
                        queued_summary(
                            &thread_id,
                            "message-one",
                            artisan_domain::QueuedMessageState::Dispatching,
                        ),
                        queued_summary(
                            &thread_id,
                            "message-two",
                            artisan_domain::QueuedMessageState::Queued,
                        ),
                    ],
                    Vec::new(),
                )),
                cx,
            );
            assert_eq!(application.composer_queue.state.entries().len(), 2);
            application.handle_service_event(
                NativeTransportEvent::Snapshot(snapshot_for(&thread_id, 1)),
                cx,
            );
            let batch = PatchBatch::new(
                thread_id.clone(),
                ConversationCursor::new(1),
                ConversationCursor::new(5),
                vec![
                    ConversationPatch::TurnUpsert {
                        patch_id: PatchId::parse("patch-turn-one").expect("patch"),
                        sequence: PatchSequence::new(2).expect("sequence"),
                        turn: ConversationTurn {
                            turn_id: TurnId::parse("turn-one").expect("turn"),
                            ordinal: TurnOrdinal::new(0),
                            revision: Revision::new(0),
                            lifecycle: ConversationLifecycle::Pending,
                            created_at: UnixMillis::EPOCH,
                            updated_at: UnixMillis::from_millis(10),
                        },
                    },
                    ConversationPatch::ItemUpsert {
                        patch_id: PatchId::parse("patch-item-one").expect("patch"),
                        sequence: PatchSequence::new(3).expect("sequence"),
                        item: ConversationItem::UserMessage(UserMessageItem {
                            item_id: ItemId::parse("item-one").expect("item"),
                            turn_id: TurnId::parse("turn-one").expect("turn"),
                            ordinal: ItemOrdinal::new(1),
                            revision: Revision::new(0),
                            lifecycle: ConversationLifecycle::Pending,
                            body: MessageBody::parse("first send".to_owned()).expect("user body"),
                            source_message_id: Some(
                                artisan_domain::MessageId::parse("message-one")
                                    .expect("source message"),
                            ),
                            created_at: UnixMillis::EPOCH,
                            updated_at: UnixMillis::from_millis(10),
                        }),
                    },
                    ConversationPatch::TurnUpsert {
                        patch_id: PatchId::parse("patch-turn-two").expect("patch"),
                        sequence: PatchSequence::new(4).expect("sequence"),
                        turn: ConversationTurn {
                            turn_id: TurnId::parse("turn-two").expect("turn"),
                            ordinal: TurnOrdinal::new(2),
                            revision: Revision::new(0),
                            lifecycle: ConversationLifecycle::Pending,
                            created_at: UnixMillis::EPOCH,
                            updated_at: UnixMillis::from_millis(10),
                        },
                    },
                    ConversationPatch::ItemUpsert {
                        patch_id: PatchId::parse("patch-item-two").expect("patch"),
                        sequence: PatchSequence::new(5).expect("sequence"),
                        item: ConversationItem::UserMessage(UserMessageItem {
                            item_id: ItemId::parse("item-two").expect("item"),
                            turn_id: TurnId::parse("turn-two").expect("turn"),
                            ordinal: ItemOrdinal::new(3),
                            revision: Revision::new(0),
                            lifecycle: ConversationLifecycle::Pending,
                            body: MessageBody::parse("second send".to_owned()).expect("user body"),
                            source_message_id: Some(
                                artisan_domain::MessageId::parse("message-two")
                                    .expect("source message"),
                            ),
                            created_at: UnixMillis::EPOCH,
                            updated_at: UnixMillis::from_millis(10),
                        }),
                    },
                ],
            )
            .expect("twin echo batch");
            application.handle_service_event(NativeTransportEvent::PatchBatch(batch), cx);
            application.handle_service_event(
                NativeTransportEvent::MessageOutbox(message_outbox(
                    &thread_id,
                    Vec::new(),
                    Vec::new(),
                )),
                cx,
            );
            // Both rows left with the outbox; both bodies exact-once.
            assert!(application.composer_queue.state.entries().is_empty());
            assert!(application.message_failure.is_none());
            let canonical = application
                .conversation_host
                .clone()
                .expect("mounted host")
                .read(cx)
                .canonical_snapshot()
                .expect("canonical snapshot");
            let bodies: Vec<_> = canonical
                .items()
                .iter()
                .filter_map(|item| match item {
                    ConversationItem::UserMessage(message) => {
                        Some(message.body.as_str().to_owned())
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(bodies, ["first send", "second send"]);
        });
    });
}

#[gpui::test]
fn desktop_new_task_preserves_draft_and_blocks_repeat_creation(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let old = ThreadId::parse("existing-task").expect("thread");
            install_ready_message_surface(application, cx, old.clone(), "keep this draft", sink);
            let project = application.selected_project.clone().expect("project");
            application.begin_new_task(cx);
            assert_eq!(
                *commands.borrow(),
                vec![NativeTransportCommand::CreateTask(project)]
            );
            assert_eq!(application.selected_thread.as_ref(), Some(&old));
            assert_eq!(application.composer.read(cx).draft(), "keep this draft");
            assert!(!application.message_submission_is_admissible(cx));
            application.begin_new_task(cx);
            application.begin_message_submission(cx);
            assert_eq!(commands.borrow().len(), 1);
            application.handle_intake_failed(
                NativeProjectIntakeOperation::CreateThread,
                message_failure(),
                false,
                cx,
            );
            assert_eq!(application.composer.read(cx).draft(), "keep this draft");
            assert!(!application.message_submission_is_admissible(cx));
        });
    });
}

#[gpui::test]
fn desktop_route_mismatch_cannot_send_to_previous_task(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                ThreadId::parse("old-task").expect("thread"),
                "keep",
                sink,
            );
            assert!(application.message_submission_is_admissible(cx));
            application.navigate(
                NativeRoute::NewThread {
                    project: application.selected_project.clone(),
                },
                cx,
            );
            application.begin_message_submission(cx);
            assert!(commands.borrow().is_empty());
            assert_eq!(application.composer.read(cx).draft(), "keep");
            application.navigate(
                NativeRoute::Thread {
                    project: application.selected_project.clone().expect("project"),
                    thread: ThreadId::parse("different-task").expect("thread"),
                },
                cx,
            );
            application.begin_message_submission(cx);
            assert!(commands.borrow().is_empty());
        });
    });
}

#[gpui::test]
fn desktop_busy_sidebar_navigation_keeps_visible_task_and_draft(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                ThreadId::parse("old-task").expect("thread"),
                "keep",
                sink,
            );
            application.thread_listing = Some(
                ThreadListing::new(vec![thread("target-task", "message-project", "Target")])
                    .expect("listing"),
            );
            let old_route = application.route().clone();
            application.intake_stage = Some(NativeProjectIntakeStage::CreatingThread);
            application
                .open_thread_from_sidebar(ThreadId::parse("target-task").expect("thread"), cx);
            assert_eq!(application.route(), &old_route);
            assert_eq!(application.composer.read(cx).draft(), "keep");
            assert!(commands.borrow().is_empty());
        });
    });
}

#[test]
fn one_domain_body_parse_admits_one_single_flight_and_retains_raw_text() {
    let mut composer = ComposerState::new();
    let raw = "  exact\n\tðŸ˜€  ";
    composer.set_draft(raw);
    let (body, token) = composer.begin_submission().expect("valid body");
    assert_eq!(body.as_str(), raw);
    assert_eq!(
        composer.begin_submission(),
        Err(crate::composer::SubmissionBlocked::InFlight)
    );
    composer.finish_submission(token, DraftDisposition::Retained);
    assert_eq!(composer.draft(), raw);
    assert!(!composer.is_submitting());
}

#[test]
fn each_new_message_submission_mints_a_fresh_request_id() {
    let first = create_message_request_id().expect("first request");
    let second = create_message_request_id().expect("second request");
    assert_ne!(first, second);
    assert!(first.as_str().starts_with("native-message-"));
    assert!(second.as_str().starts_with("native-message-"));
    // Random UUIDv7, not a process counter: no pid or counter suffix that a
    // restarted Editor could repeat.
    assert_eq!(first.as_str().len(), "native-message-".len() + 36);
}

#[gpui::test]
fn first_send_persists_displayed_one_million_window_before_queueing(cx: &mut TestAppContext) {
    let thread = ThreadId::parse("default-extended-window").unwrap();
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(application, cx, thread.clone(), "Hello", sink);
            admit_probed_codex_usage(application, cx);
            let mut policy = application
                .model_selector
                .read(cx)
                .state()
                .snapshot()
                .selection_policy_for_model("codex-sol")
                .unwrap();
            policy.context_window = Some(crate::native_model_catalog::NativeContextSelection {
                id: "extended".to_owned(),
                native_suffix: "1m".to_owned(),
                native_config: Some(crate::native_model_catalog::NativeContextConfig {
                    model_context_window: 1_050_000,
                }),
            });
            application.composer_model_choice = Some((Some(thread), policy));
            application.begin_message_submission(cx);
            assert!(application.message_flight.is_some(), "send should be eager");
            assert!(application.engine_settings.authoritative_config().is_none());
        })
    });
    let commands = commands.borrow();
    let save = commands
        .iter()
        .position(|command| matches!(command, NativeTransportCommand::SetThreadEngineConfig(_)))
        .expect("default choice must be saved");
    let queue = commands
        .iter()
        .position(|command| matches!(command, NativeTransportCommand::SubmitComposerDraft(_)))
        .unwrap();
    assert!(save < queue);
    let NativeTransportCommand::SetThreadEngineConfig(command) = &commands[save] else {
        unreachable!()
    };
    let artisan_domain::EngineSelection::Codex(selection) = command.config().selection() else {
        unreachable!()
    };
    assert_eq!(selection.model_context_window().unwrap().get(), 1_050_000);
}

#[gpui::test]
fn context_change_during_save_is_persisted_after_ack(cx: &mut TestAppContext) {
    let thread = ThreadId::parse("coalesced-context").unwrap();
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(application, cx, thread.clone(), "Hello", sink);
            admit_probed_codex_usage(application, cx);
            let base = application
                .model_selector
                .read(cx)
                .state()
                .snapshot()
                .selection_policy_for_model("codex-sol")
                .unwrap();
            application.handle_composer_model_event(
                &crate::native_model_selector::NativeModelSelectorEvent::SelectPolicy(base.clone()),
                cx,
            );
            let first_request = admitted_save_request(application);
            let first_config = application
                .engine_settings
                .pending_save()
                .unwrap()
                .1
                .clone();
            let mut extended = base;
            extended.context_window = Some(crate::native_model_catalog::NativeContextSelection {
                id: "extended".to_owned(),
                native_suffix: "1m".to_owned(),
                native_config: Some(crate::native_model_catalog::NativeContextConfig {
                    model_context_window: 1_050_000,
                }),
            });
            application.handle_composer_model_event(
                &crate::native_model_selector::NativeModelSelectorEvent::SelectPolicy(extended),
                cx,
            );
            application.begin_message_submission(cx);
            assert!(
                application.message_flight.is_none(),
                "must not run the superseded base configuration"
            );
            assert_eq!(application.composer.read(cx).draft(), "Hello");
            application.handle_engine_config_set(
                &artisan_protocol::SetThreadEngineConfigResult {
                    request_id: first_request,
                    thread_id: thread.clone(),
                    revision: artisan_domain::EngineConfigRevision::new(1).unwrap(),
                    disposition: artisan_domain::ReceiptDisposition::Accepted,
                },
                first_config,
                cx,
            );
            let next_request = admitted_save_request(application);
            let next_config = application
                .engine_settings
                .pending_save()
                .expect("latest choice is saved after ack")
                .1
                .clone();
            let artisan_domain::EngineSelection::Codex(selection) = next_config.selection() else {
                unreachable!()
            };
            assert_eq!(selection.model_context_window().unwrap().get(), 1_050_000);
            application.handle_engine_config_set(
                &artisan_protocol::SetThreadEngineConfigResult {
                    request_id: next_request,
                    thread_id: thread.clone(),
                    revision: artisan_domain::EngineConfigRevision::new(2).unwrap(),
                    disposition: artisan_domain::ReceiptDisposition::Accepted,
                },
                next_config.clone(),
                cx,
            );
            assert!(
                application.engine_settings.pending_save().is_none(),
                "ack must not cause a save loop"
            );
            assert_eq!(
                application.engine_settings.authoritative_config(),
                Some(&next_config)
            );
        })
    });
    assert_eq!(
        commands
            .borrow()
            .iter()
            .filter(|command| matches!(command, NativeTransportCommand::SetThreadEngineConfig(_)))
            .count(),
        2
    );
}

#[gpui::test]
fn host_catalog_refresh_updates_the_send_choice_revision(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(test_application);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let catalog = application.served_catalog(cx);
            let policy = catalog.selection_policy_for_model("codex-luna").unwrap();
            application.composer_model_choice = Some((None, policy));
            let mut refreshed = catalog;
            refreshed.catalog_revision = "host-new-revision".to_owned();
            application
                .model_selector
                .update(cx, |selector, cx| selector.set_snapshot(refreshed, cx));
            application.sync_composer_model_policy(cx);
            let (_, choice) = application.composer_model_choice.as_ref().unwrap();
            assert_eq!(choice.catalog_revision, "host-new-revision");
            assert_eq!(choice.model_id, "codex-luna");
            application
                .served_catalog(cx)
                .validate_policy(choice)
                .unwrap();
        })
    });
}

#[gpui::test]
fn unconnected_application_never_offers_bundled_models(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            assert!(application.served_catalog(cx).manifest.models.is_empty());
            assert!(
                application
                    .model_selector
                    .read(cx)
                    .state()
                    .policy()
                    .is_none()
            );
        })
    });
}

#[gpui::test]
fn local_send_leaves_detached_viewport_before_receipt(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                ThreadId::parse("send-scroll").unwrap(),
                "New message",
                sink,
            );
            install_configured_engine_settings(application, cx);
            admit_probed_codex_usage(application, cx);
            let host = application.conversation_host.clone().unwrap();
            host.update(cx, |host, cx| {
                host.dispatch(
                    crate::conversation_state_machine::ConversationStateEvent::Viewport(
                        crate::conversation_view_machine::ViewportEvent::UserScrolled {
                            at_bottom: false,
                        },
                    ),
                    cx,
                )
            })
            .unwrap();
            assert!(host.read(cx).controller_view().viewport_state.is_detached());
            application.begin_message_submission(cx);
            assert!(application.message_flight.is_some());
            assert!(
                !host.read(cx).surface().read(cx).has_pending_messages(),
                "the row appears only when the Forge's outbox carries it"
            );
            assert!(!host.read(cx).controller_view().viewport_state.is_detached());
        })
    });
}
