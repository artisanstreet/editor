use super::*;

#[gpui::test]
fn ready_mounts_the_exact_returned_project_and_thread_and_requests_its_snapshot(
    cx: &mut TestAppContext,
) {
    let projects = ProjectListing::new(vec![
        project("forge-p1", "First"),
        project("forge-p2", "Second"),
    ])
    .expect("projects");
    let threads = ThreadListing::new(vec![
        thread("forge-t1", "forge-p2", "Existing"),
        thread("forge-t2", "forge-p2", "New thread"),
    ])
    .expect("threads");
    let project_id = ProjectId::parse("forge-p2").expect("project");
    let thread_id = ThreadId::parse("forge-t2").expect("thread");
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));

    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.handle_intake_ready(
                &projects,
                project_id.clone(),
                &threads,
                thread_id.clone(),
                application_cx,
            );
            assert_eq!(application.selected_project.as_ref(), Some(&project_id));
            assert_eq!(application.selected_thread.as_ref(), Some(&thread_id));
            assert_eq!(application.project_options[0].id.as_str(), "forge-p2");
            assert_eq!(application.project_options[1].id.as_str(), "forge-p1");
            let host = application.conversation_host.as_ref().expect("host");
            assert_eq!(
                host.read(application_cx)
                    .controller_view()
                    .delivery
                    .thread_id,
                thread_id
            );
            assert!(matches!(
                application.conversation_effects.as_slice(),
                [ConversationHostEffect::Controller(
                    ConversationStateEffect::Delivery(
                        ConversationDeliveryEffect::RequestSnapshot {
                            thread_id: requested,
                            ..
                        }
                    )
                )] if requested == &thread_id
            ));
        });
    });
}

#[gpui::test]
fn mismatched_ready_does_not_replace_the_real_host_or_add_rows(cx: &mut TestAppContext) {
    let old_project_id = ProjectId::parse("forge-p1").expect("project");
    let old_thread_id = ThreadId::parse("forge-t1").expect("thread");
    let options = vec![ProjectOption {
        id: old_project_id.clone(),
        name: "First".into(),
    }];
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.project_options = options.clone();
            application.selected_project = Some(old_project_id.clone());
            application.pending_thread = Some(old_thread_id.clone());
            application.try_mount_pending_thread(application_cx);
            let host_before = application.conversation_host.clone().expect("host");
            application.install_picker(
                options.clone(),
                Some(old_project_id.clone()),
                application_cx,
            );
            application.last_picker_action = Some(ProjectPickerAction::NewProject);
            let mismatched_projects =
                ProjectListing::new(vec![project("forge-p1", "First")]).expect("projects");
            let mismatched_threads =
                ThreadListing::new(vec![thread("forge-t2", "forge-p2", "New thread")])
                    .expect("threads");
            application.handle_intake_ready(
                &mismatched_projects,
                ProjectId::parse("forge-p2").expect("project"),
                &mismatched_threads,
                ThreadId::parse("forge-t2").expect("thread"),
                application_cx,
            );
            assert_eq!(application.project_options, options);
            assert_eq!(application.conversation_host.as_ref(), Some(&host_before));
            assert_eq!(
                application
                    .picker
                    .as_ref()
                    .expect("picker")
                    .read(application_cx)
                    .state()
                    .projects(),
                options.as_slice()
            );
            assert_eq!(
                application
                    .picker
                    .as_ref()
                    .expect("picker")
                    .read(application_cx)
                    .last_action(),
                None
            );
            assert!(matches!(&application.state, NativeViewState::Failure(_)));
        });
    });
}

#[gpui::test]
fn real_thread_host_mount_retains_exact_initial_snapshot_request(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("forge-thread").expect("thread");
    let (host, _) = cx.add_window_view(|_, host_cx| {
        ConversationHost::new(thread_id.clone(), ThemeMode::Dark, host_cx).expect("host")
    });
    let effects = cx.update(|app| host.update(app, |host, _| host.drain_effects()));
    assert!(matches!(
        effects.as_slice(),
        [ConversationHostEffect::Controller(
            ConversationStateEffect::Delivery(
                ConversationDeliveryEffect::RequestSnapshot {
                    thread_id: requested,
                    ..
                }
            )
        )] if requested == &thread_id
    ));
}

#[gpui::test]
fn viewport_effect_pumping_is_typed_and_rejects_stale_bottom_scroll(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let thread_id = ThreadId::parse("viewport-pump-thread").expect("thread");
    let host =
        cx.update(|_, app| ConversationHost::mount(thread_id, ThemeMode::Dark, app).expect("host"));
    let generation = cx.update(|_, app| host.read(app).controller_view().viewport_generation);
    let stale_generation = ViewportGeneration::new(generation.value().saturating_add(1));
    cx.update(|_, app| {
        host.update(app, |host, _| {
            let _ = host.drain_effects();
        });
    });

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.state = NativeViewState::Ready;
            application.conversation_host = Some(host.clone());
            application.conversation_effects = vec![
                ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                    ViewportEffect::ShowJumpToLatest,
                )),
                ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                    ViewportEffect::HideJumpToLatest,
                )),
                ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                    ViewportEffect::None,
                )),
                ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                    ViewportEffect::InvalidateRender,
                )),
                ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                    ViewportEffect::CompletionRejected {
                        generation,
                        reason: CompletionRejection::NoActiveScroll,
                    },
                )),
                ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                    ViewportEffect::RequestBottomScroll { generation },
                )),
                ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                    ViewportEffect::RequestBottomScroll {
                        generation: stale_generation,
                    },
                )),
            ];

            application.pump_host_boundary(&host, application_cx);

            assert!(application.conversation_effects.is_empty());
            assert!(matches!(&application.state, NativeViewState::Ready));
        });
    });
}

#[gpui::test]
fn scroll_intent_pumping_preserves_controller_view_without_completion(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let thread_id = ThreadId::parse("scroll-intent-thread").expect("thread");
    let host =
        cx.update(|_, app| ConversationHost::mount(thread_id, ThemeMode::Dark, app).expect("host"));
    let surface = cx.update(|_, app| host.read(app).surface().clone());
    cx.update(|_, app| {
        host.update(app, |host, _| {
            let _ = host.drain_effects();
        });
    });
    let before = cx.update(|_, app| host.read(app).controller_view());
    let effect = ConversationHostEffect::ScrollIntent {
        target: ConversationSurfaceTarget::Scene(
            SceneId::parse("scroll-target").expect("scene id"),
        ),
    };

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.state = NativeViewState::Ready;
            application.conversation_host = Some(host.clone());
            application.conversation_effects = vec![effect.clone()];
            application.pump_host_boundary(&host, application_cx);

            assert!(application.conversation_effects.is_empty());
            assert!(matches!(&application.state, NativeViewState::Ready));
            assert_eq!(host.read(application_cx).controller_view(), before);
            assert!(host.read(application_cx).pending_effects().is_empty());
            assert!(surface.read(application_cx).pending_actions().is_empty());
        });
    });
}

#[gpui::test]
fn scroll_intent_pumping_retains_fifo_head_when_surface_is_full(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let thread_id = ThreadId::parse("scroll-backpressure-thread").expect("thread");
    let host =
        cx.update(|_, app| ConversationHost::mount(thread_id, ThemeMode::Dark, app).expect("host"));
    let surface = cx.update(|_, app| host.read(app).surface().clone());
    cx.update(|_, app| {
        host.update(app, |host, _| {
            let _ = host.drain_effects();
        });
        surface.update(app, |surface, surface_cx| {
            for index in 0..CONVERSATION_SURFACE_MAX_SCROLL_TARGETS {
                assert!(surface.schedule_scroll_target(
                    ConversationSurfaceTarget::Scene(
                        SceneId::parse(format!("queued-{index}")).expect("scene id"),
                    ),
                    surface_cx,
                ));
            }
        });
    });
    let effect = ConversationHostEffect::ScrollIntent {
        target: ConversationSurfaceTarget::Scene(
            SceneId::parse("backpressure-head").expect("scene id"),
        ),
    };

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.state = NativeViewState::Ready;
            application.conversation_host = Some(host.clone());
            application.conversation_effects = vec![effect.clone()];
            application.pump_host_boundary(&host, application_cx);

            assert_eq!(application.conversation_effects.as_slice(), &[effect]);
            assert!(matches!(&application.state, NativeViewState::Ready));
        });
    });
}

#[gpui::test]
fn host_retirement_drops_pending_transient_scroll_target_with_surface(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let thread_id = ThreadId::parse("scroll-retirement-thread").expect("thread");
    let host =
        cx.update(|_, app| ConversationHost::mount(thread_id, ThemeMode::Dark, app).expect("host"));
    let surface = cx.update(|_, app| host.read(app).surface().clone());
    let weak_surface = surface.downgrade();
    cx.update(|_, app| {
        host.update(app, |host, _| {
            let _ = host.drain_effects();
        });
        surface.update(app, |surface, surface_cx| {
            assert!(surface.schedule_scroll_target(
                ConversationSurfaceTarget::Scene(
                    SceneId::parse("retiring-target").expect("scene id"),
                ),
                surface_cx,
            ));
        });
        view.update(app, |application, application_cx| {
            application.state = NativeViewState::Ready;
            application.conversation_host = Some(host.clone());
            application.retire_host(application_cx);
            assert!(application.conversation_host.is_none());
            assert!(application.conversation_effects.is_empty());
        });
    });
    drop(surface);
    drop(host);
    // Entity-data release runs at the end of the App update cycle:
    // dropping the last host handle queues host removal, and only the
    // cycle drops the host value that holds the final surface handle.
    // Queue cleanup alone cannot release real surface custody, so run an
    // update cycle before asserting it.
    cx.update(|_, _| {});
    assert!(weak_surface.upgrade().is_none());
}

#[gpui::test]
fn ordinary_mount_boundary_retains_ready_host_without_replacement(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("forge-thread").expect("thread");
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let snapshot = ConversationSnapshot::new(
        thread_id.clone(),
        ConversationCursor::new(0),
        Vec::new(),
        Vec::new(),
        UnixMillis::EPOCH,
    )
    .expect("empty snapshot");
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.pending_thread = Some(thread_id.clone());
            application.try_mount_pending_thread(application_cx);
            let host = application.conversation_host.clone().expect("mounted host");

            // The test has no service thread to accept the host's initial
            // request, so model that already-accepted command before
            // exercising the ordinary no-replacement boundary.
            application.conversation_effects.clear();
            application.dispatch_snapshot(&host, snapshot, application_cx);
            assert!(matches!(&application.state, NativeViewState::Ready));
            assert!(
                host.read(application_cx)
                    .controller_view()
                    .delivery
                    .has_snapshot
            );
            assert!(application.conversation_host_subscription.is_some());

            application.try_mount_pending_thread(application_cx);

            assert_eq!(application.conversation_host.as_ref(), Some(&host));
            assert_eq!(application.selected_thread.as_ref(), Some(&thread_id));
            assert!(
                host.read(application_cx)
                    .controller_view()
                    .delivery
                    .has_snapshot
            );
        });
    });
}

#[gpui::test]
fn application_root_renders_without_a_service_thread(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    cx.run_until_parked();
    cx.update(|app| {
        assert!(view.read(app).service.is_none());
        assert!(matches!(&view.read(app).state, NativeViewState::Failure(_)));
    });
}

#[gpui::test]
fn exact_snapshot_received_event_is_dispatched_to_the_real_host(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("forge-thread").expect("thread");
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let snapshot = ConversationSnapshot::new(
        thread_id.clone(),
        ConversationCursor::new(0),
        Vec::new(),
        Vec::new(),
        UnixMillis::EPOCH,
    )
    .expect("empty snapshot");
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.pending_thread = Some(thread_id.clone());
            application.try_mount_pending_thread(application_cx);
            let host = application.conversation_host.clone().expect("real host");
            application.dispatch_snapshot(&host, snapshot, application_cx);
            assert!(
                host.read(application_cx)
                    .controller_view()
                    .delivery
                    .has_snapshot
            );
        });
    });
}

#[gpui::test]
fn thread_switch_is_serial_and_rejects_old_generation_delivery(cx: &mut TestAppContext) {
    let project_id = ProjectId::parse("switch-project").expect("project");
    let source = ThreadId::parse("switch-thread-a").expect("source thread");
    let target = ThreadId::parse("switch-thread-b").expect("target thread");
    let listing = ThreadListing::new(vec![
        thread("switch-thread-a", "switch-project", "A"),
        thread("switch-thread-b", "switch-project", "B"),
    ])
    .expect("listing");
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));

    cx.update(|app| {
        view.update(app, |application, application_cx| {
            let commands = prepare_thread_switch_fixture(
                application,
                application_cx,
                &project_id,
                &source,
                &target,
                &listing,
            );
            let stop_request = complete_first_thread_switch(
                application,
                application_cx,
                &source,
                &target,
                &commands,
            );
            refresh_listing_and_reject_stale_snapshot(
                application,
                application_cx,
                &project_id,
                &source,
                &listing,
            );
            return_to_source_and_reject_old_generation(
                application,
                application_cx,
                &source,
                &target,
                &commands,
                stop_request,
            );
        });
    });
}

#[gpui::test]
fn thread_switch_busy_is_retried_once_without_duplicate_admission(cx: &mut TestAppContext) {
    let project_id = ProjectId::parse("busy-project").expect("project");
    let source = ThreadId::parse("busy-thread-a").expect("source thread");
    let target = ThreadId::parse("busy-thread-b").expect("target thread");
    let listing = ThreadListing::new(vec![
        thread("busy-thread-a", "busy-project", "A"),
        thread("busy-thread-b", "busy-project", "B"),
    ])
    .expect("listing");
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            let source_host =
                ConversationHost::mount(source.clone(), ThemeMode::Dark, &mut *application_cx)
                    .expect("source host");
            application.project_options = vec![ProjectOption {
                id: project_id.clone(),
                name: "Busy project".into(),
            }];
            application.selected_project = Some(project_id);
            application.thread_listing = Some(listing);
            application.selected_thread = Some(source);
            application.conversation_host = Some(source_host);
            application.state = NativeViewState::Ready;
            let (sink, commands) = command_sink([Err(super::CommandSendError::Busy), Ok(())]);
            application.test_command_sink = Some(sink);
            application.begin_thread_switch(target, application_cx);
            assert_eq!(commands.borrow().len(), 1);
            assert!(matches!(
                application
                    .thread_switch_flight
                    .as_ref()
                    .map(|flight| &flight.phase),
                Some(ThreadSwitchPhase::UnsubscribeAdmission {
                    retry_pending: true,
                    retry_used: true,
                })
            ));
            application.retry_thread_switch_if_admitted(application_cx);
            assert_eq!(commands.borrow().len(), 2);
            assert!(matches!(
                application
                    .thread_switch_flight
                    .as_ref()
                    .map(|flight| &flight.phase),
                Some(ThreadSwitchPhase::AwaitingUnsubscribeStop { request_id: None })
            ));
            application.retry_thread_switch_if_admitted(application_cx);
            assert_eq!(commands.borrow().len(), 2);
        });
    });
}

#[gpui::test]
fn terminal_switch_refusal_preserves_old_host_and_disables_picker(cx: &mut TestAppContext) {
    let project_id = ProjectId::parse("stopped-project").expect("project");
    let source = ThreadId::parse("stopped-thread-a").expect("source thread");
    let target = ThreadId::parse("stopped-thread-b").expect("target thread");
    let listing = ThreadListing::new(vec![
        thread("stopped-thread-a", "stopped-project", "A"),
        thread("stopped-thread-b", "stopped-project", "B"),
    ])
    .expect("listing");
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            let source_host =
                ConversationHost::mount(source.clone(), ThemeMode::Dark, &mut *application_cx)
                    .expect("source host");
            application.project_options = vec![ProjectOption {
                id: project_id.clone(),
                name: "Stopped project".into(),
            }];
            application.selected_project = Some(project_id);
            application.thread_listing = Some(listing.clone());
            application.selected_thread = Some(source.clone());
            application.conversation_host = Some(source_host.clone());
            application.state = NativeViewState::Ready;
            let (body, token) = application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.set_disabled(false, composer_cx);
                    composer.set_draft("refused switch draft");
                    composer.begin_payload_submission()
                })
                .expect("message flight");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: source.clone(),
                request_id: request("message-stopped"),
                payload: body,
                steer_target: None,
                engine_label: None,
                token,
            });
            let (sink, commands) = command_sink([Err(super::CommandSendError::Stopped)]);
            application.test_command_sink = Some(sink);
            application.install_thread_picker(listing, Some(source.clone()), application_cx);
            application.begin_thread_switch(target, application_cx);
            assert_eq!(commands.borrow().len(), 1);
            assert!(application.thread_switch_flight.is_none());
            assert!(application.service_stopped);
            assert_eq!(application.conversation_host.as_ref(), Some(&source_host));
            assert_eq!(application.selected_thread.as_ref(), Some(&source));
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "refused switch draft"
            );
            assert!(matches!(&application.state, NativeViewState::Failure(_)));
            assert!(
                application
                    .thread_picker
                    .as_ref()
                    .expect("thread picker")
                    .read(application_cx)
                    .state()
                    .is_disabled()
            );
        });
    });
}

#[gpui::test]
fn removed_switch_target_retires_without_subscribing_it(cx: &mut TestAppContext) {
    let project_id = ProjectId::parse("removed-project").expect("project");
    let source = ThreadId::parse("removed-thread-a").expect("source thread");
    let target = ThreadId::parse("removed-thread-b").expect("target thread");
    let listing = ThreadListing::new(vec![
        thread("removed-thread-a", "removed-project", "A"),
        thread("removed-thread-b", "removed-project", "B"),
    ])
    .expect("listing");
    let remaining = ThreadListing::new(vec![thread("removed-thread-a", "removed-project", "A")])
        .expect("remaining listing");
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            let source_host =
                ConversationHost::mount(source.clone(), ThemeMode::Dark, &mut *application_cx)
                    .expect("source host");
            application.project_options = vec![ProjectOption {
                id: project_id.clone(),
                name: "Removed project".into(),
            }];
            application.selected_project = Some(project_id.clone());
            application.thread_listing = Some(listing.clone());
            application.selected_thread = Some(source.clone());
            application.conversation_host = Some(source_host);
            application.state = NativeViewState::Ready;
            let (sink, commands) = command_sink([Ok(())]);
            application.test_command_sink = Some(sink);
            application.begin_thread_switch(target, application_cx);
            application.handle_service_event(
                NativeTransportEvent::Threads {
                    project_id,
                    listing: remaining,
                },
                application_cx,
            );
            assert!(matches!(
                application
                    .thread_switch_flight
                    .as_ref()
                    .map(|flight| &flight.target_thread),
                Some(None)
            ));
            application.handle_service_event(
                NativeTransportEvent::ConversationSubscriptionStopped {
                    thread_id: source.clone(),
                    request_id: request("removed-stop-a"),
                    stopped: ConversationSubscriptionStopped { thread_id: source },
                },
                application_cx,
            );
            assert_eq!(commands.borrow().len(), 1);
            assert!(application.thread_switch_flight.is_none());
            assert!(application.conversation_host.is_none());
            assert!(application.selected_thread.is_none());
            assert!(matches!(&application.state, NativeViewState::EmptyThreads));
        });
    });
}

#[test]
fn production_title_is_the_native_title() {
    assert_eq!(WINDOW_TITLE, "Artisan Editor");
    assert!(!WINDOW_TITLE.contains("phase"));
}

#[gpui::test]
fn terminal_service_failure_clears_transient_state_keeps_draft_and_transcript(
    cx: &mut TestAppContext,
) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([]);
    let old_thread = ThreadId::parse("forge-t1").expect("old thread");
    let run = RunId::parse("run-a").expect("run");
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_project = Some(ProjectId::parse("forge-p1").expect("project"));
            install_ready_message_surface(
                application,
                application_cx,
                old_thread.clone(),
                "who are you",
                sink,
            );
            application.seed_active_run_for_tests(
                old_thread.clone(),
                run.clone(),
                artisan_protocol::RunLiveStatus::Running,
                artisan_domain::EngineId::Codex,
            );
            seed_refresh_in_flight(application, &old_thread);
            application.sync_composer_controls(application_cx);
            assert!(
                application
                    .composer_controls
                    .read(application_cx)
                    .snapshot()
                    .run_active,
                "stop control owns the observed run before the failure"
            );
            assert!(application.composer_queue.state.queue_refresh_in_flight());

            application.handle_service_event(
                NativeTransportEvent::Failed(message_failure()),
                application_cx,
            );

            assert_eq!(
                application.composer_queue.state.status(),
                crate::composer_queue_state::QueueStatus::TransportFailed
            );
            assert!(!application.composer_queue.state.queue_refresh_in_flight());
            assert!(
                !application
                    .composer_controls
                    .read(application_cx)
                    .snapshot()
                    .run_active,
                "no stop control for an unobservable run"
            );
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "who are you"
            );
            assert!(
                application.conversation_host.is_some(),
                "transcript host survives service death"
            );
            assert!(matches!(application.state, NativeViewState::Failure(_)));
            assert!(commands.borrow().is_empty());
        });
    });
}

#[gpui::test]
fn service_stopped_clears_stale_run_and_refresh_keeps_draft(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([]);
    let old_thread = ThreadId::parse("forge-t1").expect("old thread");
    let run = RunId::parse("run-a").expect("run");
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_project = Some(ProjectId::parse("forge-p1").expect("project"));
            install_ready_message_surface(
                application,
                application_cx,
                old_thread.clone(),
                "who are you",
                sink,
            );
            application.seed_active_run_for_tests(
                old_thread.clone(),
                run.clone(),
                artisan_protocol::RunLiveStatus::Running,
                artisan_domain::EngineId::Codex,
            );
            seed_refresh_in_flight(application, &old_thread);
            application.sync_composer_controls(application_cx);
            assert!(
                application
                    .composer_controls
                    .read(application_cx)
                    .snapshot()
                    .run_active
            );

            application.handle_service_stopped(ServiceStopStatus::Failed, application_cx);

            assert!(application.service_stopped);
            assert_eq!(
                application.composer_queue.state.status(),
                crate::composer_queue_state::QueueStatus::TransportFailed
            );
            assert!(!application.composer_queue.state.queue_refresh_in_flight());
            assert!(
                !application
                    .composer_controls
                    .read(application_cx)
                    .snapshot()
                    .run_active
            );
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "who are you"
            );
            assert!(application.conversation_host.is_some());
            assert!(matches!(application.state, NativeViewState::Failure(_)));
            assert!(commands.borrow().is_empty());
        });
    });
}

#[gpui::test]
fn failed_recovery_creates_same_project_task_without_autosend(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([]);
    let project_id = ProjectId::parse("forge-p1").expect("project");
    let old_thread = ThreadId::parse("forge-t1").expect("old thread");
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_project = Some(project_id.clone());
            install_ready_message_surface(
                application,
                application_cx,
                old_thread.clone(),
                "",
                sink,
            );
            application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.switch_thread(old_thread.as_str(), true, composer_cx);
                });
            seed_failed_entry(application, &old_thread, 5);
            application.sync_composer_controls(application_cx);
            application.begin_failed_prompt_recovery("queue-1", 5, application_cx);

            let recorded = commands.borrow();
            assert_eq!(recorded.len(), 1);
            assert!(
                matches!(
                    recorded[0],
                    NativeTransportCommand::CreateTask(ref created) if created == &project_id
                ),
                "recovery creates the same project task and nothing else"
            );
            let pending = application
                .pending_failed_recovery
                .as_ref()
                .expect("pending recovery");
            assert_eq!(pending.old_thread, old_thread);
            assert_eq!(
                pending.message_id,
                artisan_domain::MessageId::parse("message-1").expect("message")
            );
            assert!(pending.new_thread.is_none());
            assert!(!pending.recalled);
        });
    });
}

#[gpui::test]
fn failed_recovery_full_chain_restores_prompt_model_and_project_unsent(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([]);
    let project_id = ProjectId::parse("forge-p1").expect("project");
    let old_thread = ThreadId::parse("forge-t1").expect("old thread");
    let new_thread = ThreadId::parse("forge-t2").expect("new thread");
    let policy = failed_policy();
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_project = Some(project_id.clone());
            install_ready_message_surface(
                application,
                application_cx,
                old_thread.clone(),
                "",
                sink,
            );
            application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.switch_thread(old_thread.as_str(), true, composer_cx);
                });
            application.composer_model_choice = Some((Some(old_thread.clone()), policy.clone()));
            seed_failed_entry(application, &old_thread, 5);
            application.sync_composer_controls(application_cx);
            application.begin_failed_prompt_recovery("queue-1", 5, application_cx);
            assert!(
                application.pending_failed_recovery.is_some(),
                "recovery arms before intake resolves"
            );

            let projects =
                ProjectListing::new(vec![project("forge-p1", "First")]).expect("projects");
            let threads = ThreadListing::new(vec![
                thread("forge-t1", "forge-p1", "Old"),
                thread("forge-t2", "forge-p1", "New"),
            ])
            .expect("threads");
            application.handle_intake_progress(
                NativeProjectIntakeStage::RefreshingThreads,
                application_cx,
            );
            application.handle_intake_ready(
                &projects,
                project_id.clone(),
                &threads,
                new_thread.clone(),
                application_cx,
            );
            let pending = application
                .pending_failed_recovery
                .as_ref()
                .expect("pending survives intake");
            assert_eq!(pending.new_thread.as_ref(), Some(&new_thread));
            assert!(!pending.recalled, "source stop must precede recall");
            assert_eq!(application.selected_thread.as_ref(), Some(&old_thread));
            super::projects::finish_project_transition(application, application_cx);
            let pending = application
                .pending_failed_recovery
                .as_ref()
                .expect("pending survives intake");
            assert_eq!(pending.new_thread.as_ref(), Some(&new_thread));
            assert!(pending.recalled, "mount recalls the exact failed payload");
            assert_eq!(
                application.composer_model_choice,
                Some((Some(new_thread.clone()), policy.clone())),
                "the old thread policy seeds the new thread"
            );
            assert!(
                matches!(application.state, NativeViewState::Ready),
                "the real snapshot event readies the new thread before restore"
            );

            let recorded = commands.borrow();
            assert!(
                recorded.iter().any(|command| matches!(
                    command,
                    NativeTransportCommand::ComposerState(
                        crate::native_transport_service::ComposerStateCommand::ReadRecalledMessage {
                            ..
                        }
                    )
                )),
                "recovery recalls the exact failed payload, never sends"
            );
            assert!(
                !recorded
                    .iter()
                    .any(|command| matches!(command, NativeTransportCommand::QueueMessage(_))),
                "nothing is autosent during recovery"
            );

            let payload = artisan_domain::QueueMessagePayload::text_only("hello").expect("payload");
            assert!(
                application
                    .accept_failed_recovery_result(&recovery_result(Some(payload)), application_cx),
                "the armed result is consumed"
            );
            assert!(application.pending_failed_recovery.is_none());
            assert_eq!(application.composer.read(application_cx).draft(), "hello");
            assert!(
                !commands
                    .borrow()
                    .iter()
                    .any(|command| matches!(command, NativeTransportCommand::QueueMessage(_))),
                "restore never sends"
            );
        });
    });
}

#[gpui::test]
fn failed_recovery_event_routes_through_controls_subscription(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([]);
    let project_id = ProjectId::parse("forge-p1").expect("project");
    let old_thread = ThreadId::parse("forge-t1").expect("old thread");
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_project = Some(project_id.clone());
            install_ready_message_surface(
                application,
                application_cx,
                old_thread.clone(),
                "",
                sink,
            );
            application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.switch_thread(old_thread.as_str(), true, composer_cx);
                });
            seed_failed_entry(application, &old_thread, 5);
            application.sync_composer_controls(application_cx);
            application
                .composer_controls
                .update(application_cx, |_, controls_cx| {
                    controls_cx.emit(
                        NativeComposerControlsEvent::StartNewThreadWithFailedPrompt {
                            command_id: "queue-1".to_owned(),
                            generation: 5,
                        },
                    );
                });
        });
    });
    cx.update(|app| {
        view.update(app, |application, _| {
            let recorded = commands.borrow();
            assert_eq!(recorded.len(), 1);
            assert!(
                matches!(
                    recorded[0],
                    NativeTransportCommand::CreateTask(ref created) if created == &project_id
                ),
                "the subscription routes the failed event into same-project creation"
            );
            assert!(
                application.pending_failed_recovery.is_some(),
                "the routed event arms the exact failed recovery"
            );
            assert!(
                !recorded
                    .iter()
                    .any(|command| matches!(command, NativeTransportCommand::QueueMessage(_))),
                "routing never sends"
            );
        });
    });
}

#[gpui::test]
fn failed_recovery_unknown_identity_is_a_noop(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([]);
    let old_thread = ThreadId::parse("forge-t1").expect("old thread");
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.test_command_sink = Some(sink);
            application.selected_project = Some(ProjectId::parse("forge-p1").expect("project"));
            application.selected_thread = Some(old_thread.clone());
            seed_failed_entry(application, &old_thread, 5);
            application.begin_failed_prompt_recovery("queue-unknown", 5, application_cx);
            application.begin_failed_prompt_recovery("queue-1", 6, application_cx);
            assert!(application.pending_failed_recovery.is_none());
            assert!(commands.borrow().is_empty());
        });
    });
}

#[gpui::test]
fn failed_recovery_busy_composer_refuses_with_notice(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([]);
    let old_thread = ThreadId::parse("forge-t1").expect("old thread");
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_project = Some(ProjectId::parse("forge-p1").expect("project"));
            install_ready_message_surface(
                application,
                application_cx,
                old_thread.clone(),
                "",
                sink,
            );
            application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.switch_thread(old_thread.as_str(), true, composer_cx);
                    composer.set_draft("already typing");
                });
            seed_failed_entry(application, &old_thread, 5);
            application.begin_failed_prompt_recovery("queue-1", 5, application_cx);
            assert!(application.pending_failed_recovery.is_none());
            assert!(application.message_failure.is_some());
            assert!(commands.borrow().is_empty());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "already typing"
            );
        });
    });
}

#[gpui::test]
fn failed_recovery_accept_into_typed_composer_keeps_text(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (_sink, commands) = command_sink([]);
    let old_thread = ThreadId::parse("forge-t1").expect("old thread");
    let new_thread = ThreadId::parse("forge-t2").expect("new thread");
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_thread = Some(new_thread.clone());
            application.pending_failed_recovery = Some(PendingFailedRecovery {
                old_thread: old_thread.clone(),
                message_id: artisan_domain::MessageId::parse("message-1").expect("message"),
                original_request_id: request("queue-1"),
                policy: None,
                new_thread: Some(new_thread.clone()),
                recalled: true,
            });
            application
                .composer
                .update(application_cx, |composer, composer_cx| {
                    composer.switch_thread(new_thread.as_str(), true, composer_cx);
                    composer.set_draft("typed during the race");
                });
            let payload = artisan_domain::QueueMessagePayload::text_only("hello").expect("payload");
            assert!(
                application
                    .accept_failed_recovery_result(&recovery_result(Some(payload)), application_cx),
                "the armed result is consumed"
            );
            assert!(application.pending_failed_recovery.is_none());
            assert_eq!(
                application.composer.read(application_cx).draft(),
                "typed during the race",
                "already-typed input is never overwritten"
            );
            assert!(application.message_failure.is_some());
            assert!(
                !commands
                    .borrow()
                    .iter()
                    .any(|command| matches!(command, NativeTransportCommand::QueueMessage(_))),
                "refusal never sends"
            );
        });
    });
}

#[gpui::test]
fn failed_recovery_delayed_read_to_another_thread_drops_quietly(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (_sink, _commands) = command_sink([]);
    let old_thread = ThreadId::parse("forge-t1").expect("old thread");
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.selected_thread = Some(ThreadId::parse("forge-t3").expect("other thread"));
            application.pending_failed_recovery = Some(PendingFailedRecovery {
                old_thread: old_thread.clone(),
                message_id: artisan_domain::MessageId::parse("message-1").expect("message"),
                original_request_id: request("queue-1"),
                policy: None,
                new_thread: Some(ThreadId::parse("forge-t2").expect("new thread")),
                recalled: true,
            });
            let payload = artisan_domain::QueueMessagePayload::text_only("hello").expect("payload");
            assert!(
                application
                    .accept_failed_recovery_result(&recovery_result(Some(payload)), application_cx),
                "the mismatched destination consumes the stale read"
            );
            assert!(application.pending_failed_recovery.is_none());
            assert!(application.message_failure.is_none());
        });
    });
}

#[gpui::test]
fn failed_recovery_navigation_cancels_pending(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (_sink, _commands) = command_sink([]);
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.pending_failed_recovery = Some(PendingFailedRecovery {
                old_thread: ThreadId::parse("forge-t1").expect("old thread"),
                message_id: artisan_domain::MessageId::parse("message-1").expect("message"),
                original_request_id: request("queue-1"),
                policy: None,
                new_thread: None,
                recalled: false,
            });
            application.selected_thread = Some(ThreadId::parse("forge-t1").expect("old thread"));
            application.begin_thread_transition(
                Some(ThreadId::parse("forge-t3").expect("other thread")),
                ThreadId::parse("forge-t1").expect("old thread"),
                false,
                application_cx,
            );
            assert!(application.pending_failed_recovery.is_none());

            application.pending_failed_recovery = Some(PendingFailedRecovery {
                old_thread: ThreadId::parse("forge-t1").expect("old thread"),
                message_id: artisan_domain::MessageId::parse("message-1").expect("message"),
                original_request_id: request("queue-1"),
                policy: None,
                new_thread: None,
                recalled: false,
            });
            application.prepare_shutdown(application_cx);
            assert!(application.pending_failed_recovery.is_none());
        });
    });
}

#[gpui::test]
fn failed_recovery_continue_waits_for_armed_mount(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([]);
    let old_thread = ThreadId::parse("forge-t1").expect("old thread");
    let new_thread = ThreadId::parse("forge-t2").expect("new thread");
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.test_command_sink = Some(sink);
            application.selected_thread = Some(new_thread.clone());
            application.pending_failed_recovery = Some(PendingFailedRecovery {
                old_thread: old_thread.clone(),
                message_id: artisan_domain::MessageId::parse("message-1").expect("message"),
                original_request_id: request("queue-1"),
                policy: None,
                new_thread: None,
                recalled: false,
            });
            application.continue_failed_recovery(application_cx);
            assert!(commands.borrow().is_empty());
            assert!(
                application.pending_failed_recovery.is_some(),
                "unarmed recovery waits"
            );

            application
                .pending_failed_recovery
                .as_mut()
                .expect("pending")
                .new_thread = Some(new_thread.clone());
            application.continue_failed_recovery(application_cx);
            assert!(commands.borrow().is_empty());
            assert!(
                application.pending_failed_recovery.is_some(),
                "unmounted recovery waits"
            );
        });
    });
}

#[gpui::test]
fn initially_empty_project_moves_its_draft_into_a_new_destination_thread(
    cx: &mut TestAppContext,
) {
    let (view, _) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            let projects = ProjectListing::new(vec![
                project("empty-alpha", "Alpha"),
                project("empty-beta", "Beta"),
            ])
            .unwrap();
            let alpha = ProjectId::parse("empty-alpha").unwrap();
            let beta = ProjectId::parse("empty-beta").unwrap();
            let destination = ThreadId::parse("beta-transferred-draft").unwrap();
            application.handle_projects(&projects, cx);
            application.handle_empty_threads(&alpha, cx);
            application
                .composer
                .update(cx, |composer, _| composer.set_draft("Alpha idea"));
            application.select_project_from_sidebar(beta.clone(), cx);
            assert_eq!(application.selected_project.as_ref(), Some(&beta));
            assert!(commands.borrow().iter().any(|command| matches!(
                command,
                NativeTransportCommand::CreateTask(project) if project == &beta
            )));
            assert_eq!(application.composer.read(cx).draft(), "Alpha idea");
            let threads = ThreadListing::new(vec![thread(
                destination.as_str(),
                beta.as_str(),
                "New task",
            )])
            .unwrap();
            application.handle_intake_ready(&projects, beta, &threads, destination.clone(), cx);
            application.handle_service_event(
                fresh_start_event(&destination, "home-draft-start", 1),
                cx,
            );
            assert_eq!(application.selected_thread.as_ref(), Some(&destination));
            assert_eq!(application.composer.read(cx).draft(), "Alpha idea");
            assert!(matches!(application.state, NativeViewState::Ready));
            assert!(application.project_picker_action_is_admissible());
            application.composer.update(cx, |composer, cx| {
                composer.switch_thread(&format!("project:{}", alpha.as_str()), false, cx);
            });
            assert_eq!(application.composer.read(cx).draft(), "");
            application.composer.update(cx, |composer, cx| {
                composer.switch_thread(destination.as_str(), false, cx);
            });
            assert_eq!(application.composer.read(cx).draft(), "Alpha idea");
            assert!(!commands.borrow().iter().any(|command| matches!(
                command,
                NativeTransportCommand::QueueMessage(_) | NativeTransportCommand::StopRun(_)
            )));
        });
    });
}
