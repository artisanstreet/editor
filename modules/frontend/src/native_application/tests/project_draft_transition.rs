use super::*;
use base64::Engine as _;

fn image_draft() -> artisan_domain::QueueMessagePayload {
    let image = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
        .unwrap();
    artisan_domain::QueueMessagePayload::new(
        Some(artisan_domain::AuthoredText::parse("Keep this unsent prompt").unwrap()),
        vec![artisan_domain::ImageAttachment::new("image/png", image, "draft.png").unwrap()],
    )
    .unwrap()
}

fn restore_image_draft(application: &NativeApplication, cx: &mut Context<NativeApplication>) {
    application.composer.update(cx, |composer, cx| {
        composer.set_disabled(false, cx);
        let target = composer.capture_recall_target().expect("empty composer");
        composer
            .restore_recalled_payload(&target, image_draft(), cx)
            .unwrap();
    });
}

fn assert_image_draft(application: &NativeApplication, cx: &Context<NativeApplication>) {
    let composer = application.composer.read(cx);
    assert_eq!(composer.attachment_count(), 1);
    assert!(composer.snapshot_ordered_ready_attachments().is_ok());
    assert!(composer.draft_matches_payload(&image_draft()));
}

fn assert_draft_can_send(application: &NativeApplication, cx: &Context<NativeApplication>) {
    assert!(application.message_submission_is_admissible(cx));
    assert!(application.composer.read(cx).send_ready());
    assert!(application.composer_controls.read(cx).snapshot().send_ready);
}

fn finish_new_thread(
    application: &mut NativeApplication,
    source: Option<&ThreadId>,
    target: &ThreadId,
    cx: &mut Context<NativeApplication>,
) {
    if let Some(source) = source {
        application.handle_service_event(
            NativeTransportEvent::ConversationSubscriptionStopped {
                thread_id: source.clone(),
                request_id: request("intake-source-stop"),
                stopped: ConversationSubscriptionStopped {
                    thread_id: source.clone(),
                },
            },
            cx,
        );
    }
    application.try_mount_pending_thread(cx);
    application.handle_service_event(fresh_start_event(target, "intake-target-start", 1), cx);
}

#[gpui::test]
fn new_thread_intake_retires_full_scroll_queue_and_unlocks_projects(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    let project = ProjectId::parse("draft-workspace").unwrap();
    let source = ThreadId::parse("draft-source").unwrap();
    let target = ThreadId::parse("draft-new-thread").unwrap();
    let projects = ProjectListing::new(vec![self::project(project.as_str(), "Workspace")]).unwrap();
    let old_host = cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.project_options = project_options_from_listing(&projects);
            application.selected_project = Some(project.clone());
            application.thread_listing = Some(
                ThreadListing::new(vec![thread(source.as_str(), project.as_str(), "Source")])
                    .unwrap(),
            );
            application.pending_thread = Some(source.clone());
            application.try_mount_pending_thread(cx);
            let old_host = application.conversation_host.clone().unwrap();
            application.dispatch_snapshot(&old_host, snapshot_for(&source, 1), cx);
            restore_image_draft(application, cx);
            old_host
        })
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            assert_image_draft(application, cx);
            let old_surface = old_host.read(cx).surface().clone();
            old_surface.update(cx, |surface, cx| {
                for index in 0..CONVERSATION_SURFACE_MAX_SCROLL_TARGETS {
                    assert!(surface.schedule_scroll_target(
                        ConversationSurfaceTarget::Scene(
                            SceneId::parse(format!("intake-old-scroll-{index}")).unwrap(),
                        ),
                        cx,
                    ));
                }
            });
            application
                .conversation_effects
                .push(ConversationHostEffect::ScrollIntent {
                    target: ConversationSurfaceTarget::Scene(
                        SceneId::parse("intake-blocked-scroll").unwrap(),
                    ),
                });

            application.begin_new_task(cx);
            assert!(commands.borrow().iter().any(|command| matches!(
                command,
                NativeTransportCommand::CreateTask(id) if id == &project
            )));
            let threads = ThreadListing::new(vec![
                thread(target.as_str(), project.as_str(), "New task"),
                thread(source.as_str(), project.as_str(), "Source"),
            ])
            .unwrap();
            application.handle_intake_ready(&projects, project, &threads, target.clone(), cx);
            finish_new_thread(application, Some(&source), &target, cx);

            assert_eq!(application.selected_thread.as_ref(), Some(&target));
            assert_ne!(application.conversation_host.as_ref(), Some(&old_host));
            assert!(matches!(application.state, NativeViewState::Ready));
            assert!(application.project_picker_action_is_admissible());
            assert_image_draft(application, cx);
            assert_draft_can_send(application, cx);
            assert!(application.conversation_effects.is_empty());
            assert!(
                !commands
                    .borrow()
                    .iter()
                    .any(|command| matches!(command, NativeTransportCommand::StopRun(_)))
            );
        });
    });
}

#[derive(Clone, Copy)]
enum DraftLocation {
    NewThread,
    ExistingThread,
    Home,
}

fn project_choice_moves_draft_and_image(cx: &mut TestAppContext, location: DraftLocation) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    let alpha = ProjectId::parse("draft-alpha").unwrap();
    let beta = ProjectId::parse("draft-beta").unwrap();
    let source = ThreadId::parse("alpha-empty-draft").unwrap();
    let target = ThreadId::parse("beta-new-draft").unwrap();
    let existing = ThreadId::parse("beta-existing-thread").unwrap();
    let projects = ProjectListing::new(vec![
        project(alpha.as_str(), "Alpha"),
        project(beta.as_str(), "Beta"),
    ])
    .unwrap();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.project_options = project_options_from_listing(&projects);
            application.selected_project = Some(alpha.clone());
            application
                .project_navigation
                .last_threads
                .insert(beta.clone(), existing.clone());
            match location {
                DraftLocation::NewThread | DraftLocation::ExistingThread => {
                    let mut source_row = thread(source.as_str(), alpha.as_str(), "Source");
                    source_row.has_started_response =
                        matches!(location, DraftLocation::ExistingThread);
                    source_row.last_message_at = matches!(location, DraftLocation::ExistingThread)
                        .then_some(UnixMillis::from_millis(100));
                    application.thread_listing =
                        Some(ThreadListing::new(vec![source_row]).unwrap());
                    application.pending_thread = Some(source.clone());
                    application.try_mount_pending_thread(cx);
                    let host = application.conversation_host.clone().unwrap();
                    application.dispatch_snapshot(&host, snapshot_for(&source, 1), cx);
                }
                DraftLocation::Home => {
                    application.thread_listing = Some(ThreadListing::new(Vec::new()).unwrap());
                    application.state = NativeViewState::EmptyThreads;
                    application.composer.update(cx, |composer, cx| {
                        composer.switch_thread(&format!("project:{}", alpha.as_str()), false, cx);
                    });
                }
            }
            let route = match location {
                DraftLocation::ExistingThread => NativeRoute::Thread {
                    project: alpha,
                    thread: source.clone(),
                },
                DraftLocation::NewThread | DraftLocation::Home => NativeRoute::NewThread {
                    project: Some(alpha),
                },
            };
            application.navigate(route, cx);
            application.sync_project_pickers(cx);
            restore_image_draft(application, cx);
            cx.notify();
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(800.0)));
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| assert_image_draft(application, cx));
    });
    // Choosing a project (the new-task picker and the command menu both
    // route here) carries the unsent prompt into a fresh task there.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.choose_project(beta.clone(), cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            assert_eq!(application.selected_project.as_ref(), Some(&beta));
            assert_eq!(
                commands
                    .borrow()
                    .iter()
                    .filter(|command| matches!(
                        command,
                        NativeTransportCommand::CreateTask(project) if project == &beta
                    ))
                    .count(),
                1
            );
            assert!(!commands.borrow().iter().any(|command| matches!(
                command,
                NativeTransportCommand::Subscribe { thread_id, .. } if thread_id == &existing
            )));
            assert_image_draft(application, cx);

            let threads = ThreadListing::new(vec![
                thread(target.as_str(), beta.as_str(), "New task"),
                thread(existing.as_str(), beta.as_str(), "Existing work"),
            ])
            .unwrap();
            application.handle_intake_ready(&projects, beta.clone(), &threads, target.clone(), cx);
            let source = (!matches!(location, DraftLocation::Home)).then_some(&source);
            finish_new_thread(application, source, &target, cx);
            assert_eq!(application.selected_project.as_ref(), Some(&beta));
            assert_eq!(application.selected_thread.as_ref(), Some(&target));
            assert!(matches!(application.state, NativeViewState::Ready));
            assert!(application.project_picker_action_is_admissible());
            assert_image_draft(application, cx);
            assert_draft_can_send(application, cx);
            assert!(
                !commands
                    .borrow()
                    .iter()
                    .any(|command| matches!(command, NativeTransportCommand::StopRun(_)))
            );
        });
    });
}

#[gpui::test]
fn project_choice_moves_an_unsent_new_thread_draft_and_image(cx: &mut TestAppContext) {
    project_choice_moves_draft_and_image(cx, DraftLocation::NewThread);
}

#[gpui::test]
fn project_choice_moves_an_unsent_existing_thread_draft_and_image(cx: &mut TestAppContext) {
    project_choice_moves_draft_and_image(cx, DraftLocation::ExistingThread);
}

#[gpui::test]
fn project_choice_moves_an_unsent_home_draft_and_image(cx: &mut TestAppContext) {
    project_choice_moves_draft_and_image(cx, DraftLocation::Home);
}
