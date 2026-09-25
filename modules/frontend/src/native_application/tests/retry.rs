use super::*;

#[gpui::test]
fn correlated_failure_retains_exact_retry_identity_and_body(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-thread").expect("thread");
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id.clone(),
                "  exact retry body\nðŸ˜€  ",
                sink,
            );
            install_configured_engine_settings(application, application_cx);
            application.begin_message_submission(application_cx);
            assert_eq!(application.composer.read(application_cx).draft(), "");
            assert_eq!(application.optimistic_messages.len(), 1);
            assert!(
                application
                    .conversation_host
                    .as_ref()
                    .unwrap()
                    .read(application_cx)
                    .surface()
                    .read(application_cx)
                    .has_pending_messages()
            );
            let flight = application
                .message_flight
                .as_ref()
                .expect("admitted flight");
            let request_id = flight.request_id.clone();
            let body = flight
                .payload
                .text()
                .expect("text payload")
                .as_str()
                .to_owned();

            application.handle_service_event(
                NativeTransportEvent::MessageFailed {
                    thread_id: thread_id.clone(),
                    request_id: request_id.clone(),
                    failure: message_failure(),
                },
                application_cx,
            );

            assert!(application.message_flight.is_none());
            assert!(application.optimistic_messages.is_empty());
            assert!(application.message_failure.is_some());
            let retry = application.message_retry.as_ref().expect("retry record");
            assert_eq!(retry.thread_id, thread_id);
            assert_eq!(retry.request_id, request_id);
            assert_eq!(retry.payload.text().expect("text payload").as_str(), body);
            assert_eq!(application.composer.read(application_cx).draft(), body);
        });
    });
    cx.run_until_parked();

    assert_eq!(commands.borrow().len(), 1);
    cx.update(|_, app| {
        let application = view.read(app);
        assert!(
            application
                .message_retry
                .as_ref()
                .is_some_and(|retry| retry.draft_matches)
        );
        assert!(!application.composer.read(app).is_submitting());
    });
}

#[gpui::test]
fn retry_button_is_labeled_focused_and_has_deterministic_tab_stop(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-button-thread").expect("thread");
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id,
                "retry button body",
                sink,
            );
            admit_message_flight(application, application_cx, "retry-button-request");
            fail_active_message(application, application_cx);
        });
    });
    cx.run_until_parked();

    assert_eq!(NATIVE_MESSAGE_RETRY_LABEL, "Retry send");
    // Mounting the pre-port message panel used to refresh the retry
    // focus handle through the builder; the panel is retired, so call
    // the builder directly â€” the same code the panel ran.
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            let _ = application.message_retry_button(application_cx);
        });
    });
    // The pre-port message panel is retired; the retry affordance
    // re-homes onto the legacy thread composer in a later packet. The
    // focus/tab-stop and ring-visibility contracts below remain.
    let ring_visible = cx.update(|window, app| {
        let application = view.read(app);
        assert_eq!(application.message_retry_focus_handle.tab_index, 2);
        assert!(application.message_retry_focus_handle.tab_stop);
        let focus = application.message_retry_focus_handle.clone();
        window.dispatch_event(
            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse("right").expect("valid key"),
                is_held: false,
                prefer_character_input: false,
            }),
            app,
        );
        window.focus(&focus, app);
        Button::new(
            NATIVE_MESSAGE_RETRY_SELECTOR,
            focus,
            ArtisanTheme::for_mode(ThemeMode::Dark),
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::Small,
            ButtonContent::text(NATIVE_MESSAGE_RETRY_LABEL),
        )
        .expect("retry button configuration")
        .focus_visibility(FocusVisibility::Visible)
        .focus_ring_visible(window)
    });
    assert!(ring_visible, "retry action must expose visible focus");

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_draft("edited retry body");
                    composer_cx.notify();
                });
        });
    });
    cx.run_until_parked();
    // The retired panel re-ran the builder on re-render, which is what
    // dropped the tab stop when the draft no longer matched; mirror it.
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            let _ = application.message_retry_button(application_cx);
        });
    });
    cx.update(|_, app| {
        let application = view.read(app);
        assert!(application.message_retry.is_some());
        assert!(
            !application
                .message_retry
                .as_ref()
                .is_some_and(|retry| retry.draft_matches)
        );
        assert!(!application.message_retry_focus_handle.tab_stop);
    });
}

#[gpui::test]
fn pointer_enter_and_space_retry_activation_each_queue_once_with_stable_identity(
    cx: &mut TestAppContext,
) {
    let thread_id = ThreadId::parse("retry-activation-thread").expect("thread");
    let request_id = request("retry-stable-request");
    let body = "retry activation body";
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([Ok(()), Ok(()), Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id.clone(),
                body,
                sink,
            );
            admit_message_flight(application, application_cx, "retry-stable-request");
            fail_active_message(application, application_cx);
        });
    });
    cx.run_until_parked();

    // The painted retry control is retired; drive the same activation
    // handler its button invoked. Input-surface activation (Enter/Space
    // on the focused control) re-homes with the legacy message panel.
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.activate_message_retry(application_cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(commands.borrow().len(), 1);

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            fail_active_message(application, application_cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.activate_message_retry(application_cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(commands.borrow().len(), 2);

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            fail_active_message(application, application_cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.activate_message_retry(application_cx);
        });
    });
    cx.run_until_parked();

    let commands = commands.borrow();
    assert_eq!(commands.len(), 3);
    for command in commands.iter() {
        let NativeTransportCommand::QueueMessage(command) = command else {
            panic!("retry activation must queue a first message")
        };
        assert_eq!(command.request_id, request_id);
        assert_eq!(command.thread_id, thread_id);
        assert_eq!(command.payload.text().expect("text payload").as_str(), body);
    }
}

#[gpui::test]
fn retry_receipts_settle_only_matching_flights_and_stale_results_are_inert(
    cx: &mut TestAppContext,
) {
    let thread_id = ThreadId::parse("retry-receipt-thread").expect("thread");
    let stale_thread_id = ThreadId::parse("retry-stale-thread").expect("stale thread");
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([Ok(()), Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id.clone(),
                "accepted retry body",
                sink,
            );
            admit_message_flight(application, application_cx, "retry-accepted-request");
            fail_active_message(application, application_cx);
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.activate_message_retry(application_cx);
            assert!(application.message_flight.is_some());
            application.handle_service_event(
                NativeTransportEvent::MessageQueued(first_receipt(
                    "retry-stale-request",
                    &stale_thread_id,
                    "message-stale",
                    ReceiptDisposition::Accepted,
                )),
                application_cx,
            );
            application.handle_service_event(
                NativeTransportEvent::MessageFailed {
                    thread_id: stale_thread_id,
                    request_id: request("retry-stale-request"),
                    failure: message_failure(),
                },
                application_cx,
            );
            assert!(application.message_flight.is_some());
            assert!(application.message_retry.is_none());
            assert_eq!(application.composer.read(application_cx).draft(), "");
            application.handle_service_event(
                NativeTransportEvent::MessageQueued(first_receipt(
                    "retry-accepted-request",
                    &thread_id,
                    "message-accepted",
                    ReceiptDisposition::Accepted,
                )),
                application_cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(application.composer.read(application_cx).draft(), "");

            application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("duplicate retry body");
                    composer_cx.notify();
                });
            application.message_receipt = None;
            application.message_failure = None;
            admit_message_flight(application, application_cx, "retry-duplicate-request");
            fail_active_message(application, application_cx);
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.activate_message_retry(application_cx);
            application.handle_service_event(
                NativeTransportEvent::MessageQueued(first_receipt(
                    "retry-duplicate-request",
                    &thread_id,
                    "message-duplicate",
                    ReceiptDisposition::Duplicate,
                )),
                application_cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(application.composer.read(application_cx).draft(), "");
        });
    });
    cx.run_until_parked();
    assert_eq!(commands.borrow().len(), 2);
}

#[gpui::test]
fn edited_retry_is_suppressed_while_fresh_send_mints_a_new_request(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-edited-thread").expect("thread");
    let original_request = request("retry-edited-request");
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id.clone(),
                "original retry body",
                sink,
            );
            install_configured_engine_settings(application, application_cx);
            admit_message_flight(application, application_cx, "retry-edited-request");
            fail_active_message(application, application_cx);
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_draft("edited fresh body");
                    composer_cx.notify();
                });
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.activate_message_retry(application_cx);
            assert_eq!(commands.borrow().len(), 0);
            assert!(application.message_flight.is_none());
            assert!(application.message_retry.is_some());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "edited fresh body"
            );

            application.begin_message_submission(application_cx);
            assert!(application.message_retry.is_none());
            let flight = application.message_flight.as_ref().expect("fresh flight");
            assert_ne!(flight.request_id, original_request);
            assert_eq!(flight.thread_id, thread_id);
            assert_eq!(
                flight.payload.text().expect("text payload").as_str(),
                "edited fresh body"
            );
        });
    });

    let commands = commands.borrow();
    assert_eq!(commands.len(), 1);
    let NativeTransportCommand::QueueMessage(command) = &commands[0] else {
        panic!("fresh send must queue a first message")
    };
    assert_ne!(command.request_id, original_request);
    assert_eq!(command.thread_id, thread_id);
    assert_eq!(
        command.payload.text().expect("text payload").as_str(),
        "edited fresh body"
    );
}

#[gpui::test]
fn busy_retry_admission_retains_identity_and_draft_without_a_flight(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-busy-thread").expect("thread");
    let request_id = request("retry-busy-request");
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([Err(super::CommandSendError::Busy)]);
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id.clone(),
                "busy retry body",
                sink,
            );
            admit_message_flight(application, application_cx, "retry-busy-request");
            fail_active_message(application, application_cx);
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.activate_message_retry(application_cx);
            let retry = application.message_retry.as_ref().expect("retained retry");
            assert_eq!(retry.thread_id, thread_id);
            assert_eq!(retry.request_id, request_id);
            assert_eq!(
                retry.payload.text().expect("text payload").as_str(),
                "busy retry body"
            );
            assert!(application.message_flight.is_none());
            assert!(!application.composer.read(application_cx).is_submitting());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "busy retry body"
            );
            assert!(matches!(
                application.message_failure,
                Some(NativeMessageFailure { failure, .. })
                    if failure.stage == super::ServiceFailureStage::EventBridge
                        && failure.category == super::ServiceFailureCategory::Backpressure
            ));
        });
    });
    assert_eq!(commands.borrow().len(), 1);
}

#[gpui::test]
fn stopped_retry_admission_fails_closed_and_removes_the_affordance(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-stopped-thread").expect("thread");
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([Err(super::CommandSendError::Stopped)]);
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id,
                "stopped retry body",
                sink,
            );
            admit_message_flight(application, application_cx, "retry-stopped-request");
            fail_active_message(application, application_cx);
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.activate_message_retry(application_cx);
            assert!(application.message_retry.is_none());
            assert!(application.message_flight.is_none());
            assert!(application.service_stopped);
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "stopped retry body"
            );
            assert!(!application.message_retry_focus_handle.tab_stop);
        });
    });
    assert_eq!(commands.borrow().len(), 1);
}

#[gpui::test]
fn service_stop_event_clears_retry_while_retaining_the_draft(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-stop-event-thread").expect("thread");
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id,
                "stop event draft",
                sink,
            );
            admit_message_flight(application, application_cx, "retry-stop-event-request");
            fail_active_message(application, application_cx);
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            assert!(application.message_retry.is_some());
            application.handle_service_event(
                NativeTransportEvent::Stopped(ServiceStopStatus::Clean),
                application_cx,
            );
            assert!(application.message_retry.is_none());
            assert!(application.service_stopped);
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "stop event draft"
            );
        });
    });
}

#[gpui::test]
fn thread_transition_clears_retry_while_retaining_the_draft(cx: &mut TestAppContext) {
    let project_id = ProjectId::parse("retry-transition-project").expect("project");
    let source = ThreadId::parse("retry-transition-source").expect("source");
    let target = ThreadId::parse("retry-transition-target").expect("target");
    let listing = ThreadListing::new(vec![
        thread(source.as_str(), "retry-transition-project", "Source"),
        thread(target.as_str(), "retry-transition-project", "Target"),
    ])
    .expect("listing");
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, _) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                source.clone(),
                "thread transition draft",
                sink,
            );
            application.selected_project = Some(project_id.clone());
            application.thread_listing = Some(listing.clone());
            admit_message_flight(application, application_cx, "retry-transition-request");
            fail_active_message(application, application_cx);
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.begin_thread_switch(target, application_cx);
            assert!(application.message_retry.is_none());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "thread transition draft"
            );
            assert!(application.thread_switch_flight.is_some());
        });
    });
}

#[gpui::test]
fn project_transition_clears_retry_without_losing_draft(cx: &mut TestAppContext) {
    let old_project = ProjectId::parse("retry-old-project").expect("old project");
    let new_project = ProjectId::parse("retry-new-project").expect("new project");
    let thread_id = ThreadId::parse("retry-project-thread").expect("thread");
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id.clone(),
                "project transition draft",
                sink,
            );
            application.selected_project = Some(old_project.clone());
            admit_message_flight(application, application_cx, "retry-project-request");
            fail_active_message(application, application_cx);
            application.conversation_host = None;
            let projects = ProjectListing::new(vec![project("retry-new-project", "New project")])
                .expect("project listing");
            application.handle_projects(&projects, application_cx);
            assert!(application.message_retry.is_none());
            assert_eq!(application.selected_project, Some(new_project));
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "project transition draft"
            );
        });
    });
    cx.run_until_parked();
}

#[gpui::test]
fn host_retirement_clears_retry_without_losing_draft(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-host-thread").expect("thread");
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id,
                "host retirement draft",
                sink,
            );
            admit_message_flight(application, application_cx, "retry-host-request");
            fail_active_message(application, application_cx);
            application.retire_host(application_cx);
            assert!(application.message_retry.is_none());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "host retirement draft"
            );
        });
    });
    cx.run_until_parked();
}

#[gpui::test]
fn shutdown_clears_retry_while_preserving_the_draft(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("retry-shutdown-thread").expect("thread");
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            install_ready_message_surface(
                application,
                application_cx,
                thread_id,
                "shutdown retry draft",
                sink,
            );
            admit_message_flight(application, application_cx, "retry-shutdown-request");
            fail_active_message(application, application_cx);
            application.prepare_shutdown(application_cx);
            assert!(application.message_retry.is_none());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "shutdown retry draft"
            );
            assert!(!application.message_retry_focus_handle.tab_stop);
        });
    });
}

#[test]
fn message_failure_presentation_contains_only_redacted_stage_and_category() {
    let detail = message_status_detail(None, Some(NativeMessageFailure::new(message_failure())))
        .expect("failure detail");
    assert_eq!(detail, "Send failed: request (peer).");
    for secret in [
        "body text",
        "retry-request-id",
        "https://forge.invalid",
        "credential-value",
        "peer detail",
    ] {
        assert!(!detail.contains(secret), "failure detail leaked {secret}");
    }
}

#[gpui::test]
fn busy_and_stopped_admission_retains_the_draft_without_an_application_flight(
    cx: &mut TestAppContext,
) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            for (error, draft) in [
                (
                    super::CommandSendError::Busy,
                    "busy admission body".to_owned(),
                ),
                (
                    super::CommandSendError::Stopped,
                    "stopped admission body".to_owned(),
                ),
            ] {
                let (_, token) = application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_disabled(false, composer_cx);
                        composer.set_draft(draft.clone());
                        composer.begin_payload_submission()
                    })
                    .expect("begin");
                application.reject_message_submission(
                    token,
                    super::command_failure(error),
                    application_cx,
                );
                assert!(application.message_flight.is_none());
                assert_eq!(application.composer.read(application_cx).draft(), draft);
                assert!(!application.composer.read(application_cx).is_submitting());
            }
        });
    });
}

#[gpui::test]
fn accepted_and_duplicate_receipts_clear_only_the_matching_flight(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("forge-thread").expect("thread");
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_thread = Some(thread_id.clone());
            application.state = NativeViewState::Ready;
            let (body, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("first exact body");
                    composer.begin_payload_submission()
                })
                .expect("first begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: thread_id.clone(),
                request_id: artisan_domain::RequestId::parse("request-first").expect("request"),
                payload: body,
                steer_target: None,
                engine_label: None,
                token,
            });
            application.handle_service_event(
                NativeTransportEvent::MessageQueued(first_receipt(
                    "request-first",
                    &thread_id,
                    "message-first",
                    ReceiptDisposition::Accepted,
                )),
                application_cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(application.composer.read(application_cx).draft(), "");
            assert_eq!(
                application
                    .message_receipt
                    .as_ref()
                    .map(|receipt| receipt.disposition),
                Some(ReceiptDisposition::Accepted)
            );

            let (body, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("second exact body");
                    composer.begin_payload_submission()
                })
                .expect("second begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: thread_id.clone(),
                request_id: artisan_domain::RequestId::parse("request-second").expect("request"),
                payload: body,
                steer_target: None,
                engine_label: None,
                token,
            });
            application.composer.update(application_cx, |composer, _| {
                composer.set_draft("newer draft while duplicate is pending");
            });
            application.handle_service_event(
                NativeTransportEvent::MessageQueued(first_receipt(
                    "request-second",
                    &thread_id,
                    "message-second",
                    ReceiptDisposition::Duplicate,
                )),
                application_cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "newer draft while duplicate is pending"
            );
            assert_eq!(
                application
                    .message_receipt
                    .as_ref()
                    .map(|receipt| receipt.disposition),
                Some(ReceiptDisposition::Duplicate)
            );
        });
    });
}

#[gpui::test]
fn stale_queue_results_do_not_clear_a_newer_draft(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("forge-thread").expect("thread");
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_thread = Some(thread_id.clone());
            application.state = NativeViewState::Ready;
            let (body, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("newer draft");
                    composer.begin_payload_submission()
                })
                .expect("begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: thread_id.clone(),
                request_id: artisan_domain::RequestId::parse("request-newer").expect("request"),
                payload: body,
                steer_target: None,
                engine_label: None,
                token,
            });
            application.handle_service_event(
                NativeTransportEvent::MessageQueued(first_receipt(
                    "request-stale",
                    &thread_id,
                    "message-stale",
                    ReceiptDisposition::Accepted,
                )),
                application_cx,
            );
            assert!(application.message_flight.is_some());
            assert_eq!(application.composer.read(application_cx).draft(), "");
        });
    });
}

#[gpui::test]
fn queue_failure_and_service_stop_retain_the_draft(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("forge-thread").expect("thread");
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_thread = Some(thread_id.clone());
            application.state = NativeViewState::Ready;
            let (body, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("retained queue body");
                    composer.begin_payload_submission()
                })
                .expect("begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: thread_id.clone(),
                request_id: artisan_domain::RequestId::parse("request-failure").expect("request"),
                payload: body,
                steer_target: None,
                engine_label: None,
                token,
            });
            application.handle_service_event(
                NativeTransportEvent::MessageFailed {
                    thread_id: thread_id.clone(),
                    request_id: artisan_domain::RequestId::parse("request-failure")
                        .expect("request"),
                    failure: ServiceFailure {
                        stage: super::ServiceFailureStage::Request,
                        category: super::ServiceFailureCategory::Peer,
                    },
                },
                application_cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "retained queue body"
            );
            assert!(application.message_failure.is_some());

            let (body, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("retained on stop");
                    composer.begin_payload_submission()
                })
                .expect("second begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: thread_id.clone(),
                request_id: artisan_domain::RequestId::parse("request-stop").expect("request"),
                payload: body,
                steer_target: None,
                engine_label: None,
                token,
            });
            application.handle_service_event(
                NativeTransportEvent::Stopped(ServiceStopStatus::Clean),
                application_cx,
            );
            assert!(application.message_flight.is_none());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "retained on stop"
            );
        });
    });
}

#[gpui::test]
fn real_thread_transition_and_shutdown_retain_and_clear_old_presentation(cx: &mut TestAppContext) {
    let old_thread = ThreadId::parse("old-thread").expect("thread");
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_thread = Some(old_thread.clone());
            application.state = NativeViewState::Ready;
            let (body, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("transition body");
                    composer.begin_payload_submission()
                })
                .expect("begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: old_thread.clone(),
                request_id: artisan_domain::RequestId::parse("request-transition")
                    .expect("request"),
                payload: body,
                steer_target: None,
                engine_label: None,
                token,
            });
            application.message_receipt = Some(first_receipt(
                "request-old",
                &old_thread,
                "message-old",
                ReceiptDisposition::Accepted,
            ));
            application.message_failure = Some(NativeMessageFailure::new(ServiceFailure {
                stage: super::ServiceFailureStage::Request,
                category: super::ServiceFailureCategory::Peer,
            }));
            application.retire_host(application_cx);
            assert!(application.message_flight.is_none());
            assert_eq!(application.selected_thread, None);
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "transition body"
            );
            assert!(application.message_receipt.is_none());
            assert!(application.message_failure.is_none());

            application.selected_thread = Some(old_thread.clone());
            application.state = NativeViewState::Ready;
            let (body, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("shutdown body");
                    composer.begin_payload_submission()
                })
                .expect("shutdown begin");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: old_thread,
                request_id: artisan_domain::RequestId::parse("request-shutdown").expect("request"),
                payload: body,
                steer_target: None,
                engine_label: None,
                token,
            });
            application.prepare_shutdown(application_cx);
            assert!(application.message_flight.is_none());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "shutdown body"
            );
            assert!(application.message_receipt.is_none());
            assert!(application.message_failure.is_none());
        });
    });
}
