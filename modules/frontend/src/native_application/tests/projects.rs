use super::*;

fn project_ids(application: &NativeApplication) -> Vec<&str> {
    application
        .project_options
        .iter()
        .map(|option| option.id.as_str())
        .collect()
}

fn workspace_projects() -> Vec<ProjectOption> {
    project_options_from_listing(
        &ProjectListing::new(vec![
            project("workspace-alpha", "Alpha"),
            project("workspace-beta", "Beta"),
            project("workspace-gamma", "Gamma"),
        ])
        .expect("projects"),
    )
}

pub(super) fn finish_project_transition(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
) {
    if let Some((source, generation)) =
        application
            .thread_switch_flight
            .as_ref()
            .and_then(|flight| {
                matches!(
                    flight.phase,
                    ThreadSwitchPhase::AwaitingUnsubscribeStop { .. }
                )
                .then(|| (flight.source_thread.clone(), flight.generation))
            })
    {
        application.handle_service_event(
            NativeTransportEvent::ConversationSubscriptionStopped {
                thread_id: source.clone(),
                request_id: request(&format!("project-stop-{generation}")),
                stopped: ConversationSubscriptionStopped { thread_id: source },
            },
            cx,
        );
    }
    if let Some((target, generation)) =
        application
            .thread_switch_flight
            .as_ref()
            .and_then(|flight| {
                matches!(
                    flight.phase,
                    ThreadSwitchPhase::AwaitingSubscriptionStart { .. }
                )
                .then(|| (flight.target_thread.clone().unwrap(), flight.generation))
            })
    {
        application.handle_service_event(
            fresh_start_event(&target, &format!("project-start-{generation}"), 1),
            cx,
        );
    }
}

#[gpui::test]
fn explicit_project_choices_promote_in_recent_order(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.project_options = workspace_projects();
            application.selected_project = Some(application.project_options[0].id.clone());

            for (chosen, order) in [
                (
                    "workspace-beta",
                    ["workspace-beta", "workspace-alpha", "workspace-gamma"],
                ),
                (
                    "workspace-gamma",
                    ["workspace-gamma", "workspace-beta", "workspace-alpha"],
                ),
            ] {
                application.choose_project(ProjectId::parse(chosen).unwrap(), cx);
                assert_eq!(
                    application.selected_project.as_ref().map(ProjectId::as_str),
                    Some(chosen)
                );
                assert_eq!(project_ids(application), order);
            }
        });
    });
}

#[gpui::test]
fn project_catalog_refresh_keeps_selection_and_recent_order(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.project_options = workspace_projects();
            application.selected_project = Some(application.project_options[0].id.clone());
            let beta = ProjectId::parse("workspace-beta").unwrap();
            application.choose_project(beta.clone(), cx);

            let refreshed = ProjectListing::new(vec![
                project("workspace-gamma", "Gamma renamed"),
                project("workspace-alpha", "Alpha"),
                project("workspace-delta", "Delta"),
                project("workspace-beta", "Beta"),
            ])
            .unwrap();
            application.handle_projects(&refreshed, cx);
            assert_eq!(application.selected_project, Some(beta));
            assert_eq!(
                project_ids(application),
                [
                    "workspace-beta",
                    "workspace-alpha",
                    "workspace-gamma",
                    "workspace-delta"
                ]
            );
            assert_eq!(
                application.project_options[2].name.as_ref(),
                "Gamma renamed"
            );
        });
    });
}

#[gpui::test]
fn late_project_thread_responses_leave_the_current_workspace_intact(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.project_options = workspace_projects();
            let alpha = ProjectId::parse("workspace-alpha").unwrap();
            let beta = ProjectId::parse("workspace-beta").unwrap();
            let beta_thread = ThreadId::parse("beta-thread").unwrap();
            let current = ThreadListing::new(vec![thread(
                beta_thread.as_str(),
                beta.as_str(),
                "Current conversation",
            )])
            .unwrap();
            application.selected_project = Some(beta.clone());
            application.selected_thread = Some(beta_thread.clone());
            application.thread_listing = Some(current.clone());
            application.state = NativeViewState::Ready;
            application.navigate(
                NativeRoute::Thread {
                    project: beta,
                    thread: beta_thread.clone(),
                },
                cx,
            );
            let route = application.route().clone();
            let stale = ThreadListing::new(vec![thread(
                "alpha-thread",
                alpha.as_str(),
                "Previous conversation",
            )])
            .unwrap();

            application.handle_threads(&alpha, &stale, cx);
            application.handle_empty_threads(&alpha, cx);
            assert!(matches!(application.state, NativeViewState::Ready));
            assert_eq!(application.thread_listing, Some(current));
            assert_eq!(application.selected_thread, Some(beta_thread));
            assert_eq!(application.route(), &route);
        });
    });
}

#[gpui::test]
fn switching_projects_with_an_empty_composer_restores_threads_and_dormant_drafts(
    cx: &mut TestAppContext,
) {
    let (view, _) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.project_options = workspace_projects();
            let alpha = ProjectId::parse("workspace-alpha").unwrap();
            let beta = ProjectId::parse("workspace-beta").unwrap();
            let remembered = ThreadId::parse("alpha-second").unwrap();
            let alpha_threads = ThreadListing::new(vec![
                thread("alpha-first", alpha.as_str(), "First conversation"),
                thread(remembered.as_str(), alpha.as_str(), "Last viewed"),
            ])
            .unwrap();
            let beta_threads = ThreadListing::new(vec![
                thread("beta-thread", beta.as_str(), "Other project"),
                thread("beta-dormant", beta.as_str(), "Saved work"),
            ])
            .unwrap();
            application.selected_project = Some(alpha.clone());
            application.thread_listing = Some(alpha_threads.clone());
            application.pending_thread = Some(remembered.clone());
            application.try_mount_pending_thread(cx);
            application.composer.update(cx, |composer, cx| {
                composer.switch_thread("alpha-first", false, cx);
                composer.set_draft("Unsent Alpha work");
                composer.switch_thread("beta-dormant", false, cx);
                composer.set_draft("Unsent Beta work");
                composer.switch_thread(remembered.as_str(), false, cx);
            });
            assert_eq!(application.composer.read(cx).draft(), "");

            application.choose_project(beta.clone(), cx);
            application.handle_threads(&beta, &beta_threads, cx);
            finish_project_transition(application, cx);
            assert_eq!(application.composer.read(cx).draft(), "");
            application.choose_project(alpha.clone(), cx);
            application.handle_threads(&alpha, &alpha_threads, cx);
            finish_project_transition(application, cx);
            assert_eq!(application.selected_thread, Some(remembered));
            assert_eq!(application.composer.read(cx).draft(), "");

            application.choose_project(beta.clone(), cx);
            application.handle_threads(&beta, &beta_threads, cx);
            finish_project_transition(application, cx);
            assert_eq!(application.composer.read(cx).draft(), "");

            let remaining_alpha = ThreadListing::new(vec![thread(
                "alpha-first",
                alpha.as_str(),
                "Remaining conversation",
            )])
            .unwrap();
            application.choose_project(alpha.clone(), cx);
            application.handle_threads(&alpha, &remaining_alpha, cx);
            finish_project_transition(application, cx);
            assert_eq!(
                application.selected_thread.as_ref().map(ThreadId::as_str),
                Some("alpha-first")
            );
            // The composer shows each thread's draft as its Forge returns it.
            assert_eq!(application.composer.read(cx).draft(), "");
            application.reply_forge_draft("Unsent Alpha work", cx);
            assert_eq!(application.composer.read(cx).draft(), "Unsent Alpha work");
            application.composer.update(cx, |composer, cx| {
                composer.switch_thread("beta-dormant", false, cx);
            });
            application.reply_forge_draft("Unsent Beta work", cx);
            assert_eq!(application.composer.read(cx).draft(), "Unsent Beta work");
            assert!(
                !commands
                    .borrow()
                    .iter()
                    .any(|command| matches!(command, NativeTransportCommand::StopRun(_))),
                "project navigation must not stop background runs"
            );
        });
    });
}

#[gpui::test]
fn switching_to_an_empty_project_keeps_an_inflight_payload_in_its_source_thread(
    cx: &mut TestAppContext,
) {
    let (view, _) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.project_options = workspace_projects();
            let alpha = ProjectId::parse("workspace-alpha").unwrap();
            let beta = ProjectId::parse("workspace-beta").unwrap();
            let source = ThreadId::parse("alpha-thread").unwrap();
            let listing = ThreadListing::new(vec![thread(
                source.as_str(),
                alpha.as_str(),
                "Sending conversation",
            )])
            .unwrap();
            application.selected_project = Some(alpha.clone());
            application.thread_listing = Some(listing.clone());
            application.pending_thread = Some(source.clone());
            application.try_mount_pending_thread(cx);
            let token = application
                .composer
                .update(cx, |composer, cx| {
                    composer.set_disabled(false, cx);
                    composer.set_draft("Alpha message awaiting receipt");
                    composer.begin_draft_submission()
                })
                .expect("in-flight submission");
            application.message_flight = Some(NativeMessageFlight {
                thread_id: source.clone(),
                request_id: request("project-switch-send"),
                token,
            });

            application.choose_project(beta.clone(), cx);
            application.handle_threads(&beta, &ThreadListing::new(vec![]).unwrap(), cx);
            application.handle_empty_threads(&beta, cx);
            finish_project_transition(application, cx);
            assert!(application.selected_thread.is_none());
            assert_eq!(application.composer.read(cx).draft(), "");
            application.choose_project(alpha.clone(), cx);
            application.handle_threads(&alpha, &listing, cx);
            finish_project_transition(application, cx);
            assert_eq!(application.selected_thread, Some(source));
            application.reply_forge_draft("Alpha message awaiting receipt", cx);
            assert_eq!(
                application.composer.read(cx).draft(),
                "Alpha message awaiting receipt"
            );
            assert!(!commands.borrow().iter().any(|command| matches!(
                command,
                NativeTransportCommand::CreateTask(_)
                    | NativeTransportCommand::SubmitComposerDraft(_)
                    | NativeTransportCommand::StopRun(_)
            )));
        });
    });
}

#[gpui::test]
fn opening_a_project_remembers_the_previous_projects_last_thread(cx: &mut TestAppContext) {
    let (view, _) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.project_options = workspace_projects();
            let alpha = ProjectId::parse("workspace-alpha").unwrap();
            let beta = ProjectId::parse("workspace-beta").unwrap();
            let source = ThreadId::parse("alpha-second").unwrap();
            let target = ThreadId::parse("beta-thread").unwrap();
            let alpha_threads = ThreadListing::new(vec![
                thread("alpha-first", alpha.as_str(), "First conversation"),
                thread(source.as_str(), alpha.as_str(), "Last viewed conversation"),
            ])
            .unwrap();
            let beta_threads = ThreadListing::new(vec![thread(
                target.as_str(),
                beta.as_str(),
                "Opened conversation",
            )])
            .unwrap();
            application.selected_project = Some(alpha.clone());
            application.thread_listing = Some(alpha_threads.clone());
            application.pending_thread = Some(source.clone());
            application.try_mount_pending_thread(cx);
            let projects = ProjectListing::new(vec![
                project(alpha.as_str(), "Alpha"),
                project(beta.as_str(), "Beta"),
            ])
            .unwrap();

            application.handle_intake_ready(&projects, beta, &beta_threads, target.clone(), cx);
            finish_project_transition(application, cx);
            assert_eq!(application.selected_thread, Some(target));
            application.choose_project(alpha.clone(), cx);
            application.handle_threads(&alpha, &alpha_threads, cx);
            finish_project_transition(application, cx);
            assert_eq!(application.selected_thread, Some(source));
        });
    });
}

#[gpui::test]
fn project_switch_retires_full_scroll_queue_and_handles_either_reply_order(
    cx: &mut TestAppContext,
) {
    for listing_first in [true, false] {
        let (view, _) = cx.add_window_view(|window, cx| test_application(window, cx));
        let (sink, commands) = command_sink([]);
        cx.update(|app| {
            view.update(app, |application, cx| {
                application.test_command_sink = Some(sink);
                application.project_options = workspace_projects();
                let alpha = ProjectId::parse("workspace-alpha").unwrap();
                let beta = ProjectId::parse("workspace-beta").unwrap();
                let source = ThreadId::parse("scrolling-alpha").unwrap();
                let target = ThreadId::parse("destination-beta").unwrap();
                application.selected_project = Some(alpha.clone());
                application.thread_listing = Some(
                    ThreadListing::new(vec![thread(source.as_str(), alpha.as_str(), "Scrolling")])
                        .unwrap(),
                );
                application.pending_thread = Some(source.clone());
                application.try_mount_pending_thread(cx);
                let source_host = application.conversation_host.clone().unwrap();
                let source_surface = source_host.read(cx).surface().clone();
                source_surface.update(cx, |surface, cx| {
                    for index in 0..CONVERSATION_SURFACE_MAX_SCROLL_TARGETS {
                        assert!(surface.schedule_scroll_target(
                            ConversationSurfaceTarget::Scene(
                                SceneId::parse(format!("old-scroll-{index}")).unwrap(),
                            ),
                            cx,
                        ));
                    }
                });
                application.conversation_effects.push(ConversationHostEffect::ScrollIntent {
                    target: ConversationSurfaceTarget::Scene(SceneId::parse("blocked-scroll").unwrap()),
                });
                let destination = ThreadListing::new(vec![thread(
                    target.as_str(),
                    beta.as_str(),
                    "Destination",
                )])
                .unwrap();

                application.choose_project(beta.clone(), cx);
                assert_eq!(application.selected_thread, Some(source.clone()));
                assert_eq!(application.conversation_host.as_ref(), Some(&source_host));
                if listing_first {
                    application.handle_threads(&beta, &destination, cx);
                    assert_eq!(
                        application.thread_switch_flight.as_ref().unwrap().target_thread,
                        Some(target.clone())
                    );
                    finish_project_transition(application, cx);
                } else {
                    finish_project_transition(application, cx);
                    assert!(matches!(application.state, NativeViewState::LoadingThreads));
                    assert!(application.selected_thread.is_none());
                    application.handle_threads(&beta, &destination, cx);
                    application.handle_service_event(
                        fresh_start_event(&target, "late-project-start", 1),
                        cx,
                    );
                }

                assert_eq!(application.selected_thread, Some(target.clone()));
                assert!(matches!(application.state, NativeViewState::Ready));
                assert_eq!(
                    application.conversation_host.as_ref().unwrap().read(cx)
                        .controller_view().delivery.thread_id,
                    target
                );
                assert!(application.conversation_effects.is_empty());
                source_surface.update(cx, |surface, cx| {
                    assert!(surface.schedule_scroll_target(
                        ConversationSurfaceTarget::Scene(SceneId::parse("retirement-cleared").unwrap()),
                        cx,
                    ), "retirement releases the old surface's full scroll queue");
                });
                assert_eq!(
                    commands.borrow().iter().filter(|command| matches!(
                        command, NativeTransportCommand::Unsubscribe { thread_id } if thread_id == &source
                    )).count(),
                    1
                );
                assert!(!commands.borrow().iter().any(|command| matches!(command, NativeTransportCommand::StopRun(_))));
            });
        });
    }
}
