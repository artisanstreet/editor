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

/// The Forge stops the retired source thread's subscription.
fn stop_source(
    application: &mut NativeApplication,
    source: &ThreadId,
    cx: &mut Context<NativeApplication>,
) {
    application.handle_service_event(
        NativeTransportEvent::ConversationSubscriptionStopped {
            thread_id: source.clone(),
            request_id: request("new-thread-source-stop"),
            stopped: ConversationSubscriptionStopped {
                thread_id: source.clone(),
            },
        },
        cx,
    );
}

/// No task is created: the source (if any) stopped and the new-thread
/// screen shows the project's new-task draft, ready to send.
fn assert_on_new_task_draft(
    application: &NativeApplication,
    project: &ProjectId,
    commands: &std::rc::Rc<std::cell::RefCell<Vec<NativeTransportCommand>>>,
    cx: &Context<NativeApplication>,
) {
    assert!(
        !commands
            .borrow()
            .iter()
            .any(|command| matches!(command, NativeTransportCommand::CreateTask(_))),
        "a new thread is created by its first send, not by opening the screen"
    );
    assert_eq!(application.selected_project.as_ref(), Some(project));
    assert_eq!(application.selected_thread, None);
    assert!(application.conversation_host.is_none());
    assert!(matches!(
        application.route(),
        NativeRoute::NewThread { project: Some(shown) } if shown == project
    ));
    assert_eq!(
        application.composer.read(cx).draft_scope(),
        Some(artisan_domain::ComposerDraftScope::Project(project.clone()))
    );
    assert!(matches!(application.state, NativeViewState::EmptyThreads));
    assert!(application.project_picker_action_is_admissible());
    assert_image_draft(application, cx);
    assert_draft_can_send(application, cx);
    assert!(
        !commands
            .borrow()
            .iter()
            .any(|command| matches!(command, NativeTransportCommand::StopRun(_)))
    );
}

#[gpui::test]
fn new_thread_retires_the_open_thread_and_its_full_scroll_queue(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    let project = ProjectId::parse("draft-workspace").unwrap();
    let source = ThreadId::parse("draft-source").unwrap();
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
                NativeTransportCommand::Unsubscribe { thread_id } if thread_id == &source
            )));
            stop_source(application, &source, cx);
            assert!(application.conversation_effects.is_empty());
            assert_on_new_task_draft(application, &project, &commands, cx);
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
    // route here) carries the unsent prompt into its new-task draft.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.choose_project(beta.clone(), cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            assert_eq!(application.selected_project.as_ref(), Some(&beta));
            assert!(!commands.borrow().iter().any(|command| matches!(
                command,
                NativeTransportCommand::Subscribe { thread_id, .. } if thread_id == &existing
            )));
            assert_image_draft(application, cx);
            if !matches!(location, DraftLocation::Home) {
                stop_source(application, &source, cx);
            }
            assert_on_new_task_draft(application, &beta, &commands, cx);
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
