use super::*;

#[test]
fn real_project_summaries_become_identity_preserving_options() {
    let listing = ProjectListing::new(vec![
        project("forge-p1", "First"),
        project("forge-p2", "Second"),
    ])
    .expect("listing");
    let options = project_options_from_listing(&listing);
    assert_eq!(options.len(), 2);
    assert_eq!(options[0].id.as_str(), "forge-p1");
    assert_eq!(options[0].name.as_ref(), "First");
    assert_eq!(options[1].id.as_str(), "forge-p2");
}

#[test]
fn picker_choose_routes_the_real_project_id_and_new_begins_intake() {
    let first = ProjectOption {
        id: ProjectId::parse("forge-p1").expect("project"),
        name: "First".into(),
    };
    let options = vec![first.clone()];
    assert_eq!(
        picker_route(&ProjectPickerAction::Choose(first.id.clone()), &options),
        Ok(PickerRoute::Select(first.id))
    );
    assert_eq!(
        picker_route(&ProjectPickerAction::NewProject, &options),
        Ok(PickerRoute::BeginProjectIntake)
    );
}

#[test]
fn intake_actions_use_begin_then_the_single_retained_retry_command() {
    let options = vec![ProjectOption {
        id: ProjectId::parse("forge-p1").expect("project"),
        name: "First".into(),
    }];
    assert_eq!(
        picker_route(&ProjectPickerAction::NewProject, &options),
        Ok(PickerRoute::BeginProjectIntake)
    );
    assert_eq!(
        intake_command(false),
        crate::native_transport_service::NativeTransportCommand::BeginProjectIntake
    );
    assert_eq!(
        intake_command(true),
        crate::native_transport_service::NativeTransportCommand::RetryProjectIntake
    );
}

#[test]
fn intake_bridge_refusals_stay_typed_and_redacted() {
    let busy = super::command_failure(super::CommandSendError::Busy);
    let stopped = super::command_failure(super::CommandSendError::Stopped);
    assert_eq!(busy.category, super::ServiceFailureCategory::Backpressure);
    assert_eq!(
        stopped.category,
        super::ServiceFailureCategory::ChannelClosed
    );
    assert!(!busy.to_string().contains("127.0.0.1"));
    assert!(!stopped.to_string().contains("directory"));
}

#[gpui::test]
fn native_rail_add_project_has_stable_metadata_and_admission_policy(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    let (sink, _) = command_sink([Ok(())]);

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            let disabled_button = application.add_project_button(application_cx);
            assert_eq!(
                disabled_button.accessible_label(),
                NATIVE_RAIL_ADD_PROJECT_LABEL
            );
            assert_eq!(application.add_project_focus_handle.tab_index, 0);
            assert!(!application.add_project_action_is_admissible());

            application.test_command_sink = Some(sink);
            let enabled_button = application.add_project_button(application_cx);
            assert_eq!(
                enabled_button.accessible_label(),
                NATIVE_RAIL_ADD_PROJECT_LABEL
            );
            assert_eq!(
                enabled_button.visual_style(),
                ButtonStyle::resolve(
                    application.theme,
                    ButtonVariant::Ghost,
                    ButtonSize::IconSmall,
                    MotionPolicy::Reduced,
                )
            );
            assert_eq!(application.add_project_focus_handle.tab_index, 0);
            assert!(application.add_project_focus_handle.tab_stop);
            assert!(application.add_project_action_is_admissible());
            application_cx.notify();
        });
    });
    cx.run_until_parked();

    // The desktop workspace owns the active frame; the shared button
    // metadata and admission policy remain the contract for the sidebar.
}

#[gpui::test]
fn home_project_choice_updates_app_selection_and_scope(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    let (sink, commands) = command_sink([Ok(())]);
    let beta = ProjectId::parse("home-beta").expect("fixture project");

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.test_command_sink = Some(sink);
            let options = vec![
                ProjectOption {
                    id: ProjectId::parse("home-alpha").expect("fixture project"),
                    name: "alpha".to_owned().into(),
                },
                ProjectOption {
                    id: beta.clone(),
                    name: "beta".to_owned().into(),
                },
            ];
            application.project_options.clone_from(&options);
            application.install_home_picker(options, None, application_cx);
            let home = application
                .home_picker
                .clone()
                .expect("home picker installed");
            // Drive the window-free controller seams exactly as the
            // pointer/keyboard wrappers do, then route the drained action.
            home.update(application_cx, |picker, picker_cx| {
                picker.toggle_menu(picker_cx);
                assert!(picker.state().is_open());
                picker.commit_row(PickerRow::Project(1), picker_cx);
            });
            application.route_home_picker_action(&home, application_cx);
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |application, _| {
            assert_eq!(application.selected_project, Some(beta.clone()));
            assert!(
                commands.borrow().iter().any(
                    |command| matches!(command, NativeTransportCommand::SelectProject(id) if id == &beta)
                ),
                "choosing a home row submits the real selection command"
            );
        });
    });
}

#[gpui::test]
fn thread_open_focuses_composer_for_immediate_typing(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    let project = ProjectId::parse("thread-focus-project").expect("fixture project");
    let thread = ThreadId::parse("thread-focus-thread").expect("fixture thread");
    let host = cx.update(|_, app| {
        ConversationHost::mount(thread.clone(), ThemeMode::Dark, app).expect("host")
    });

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.project_options = vec![ProjectOption {
                id: project.clone(),
                name: "focus".to_owned().into(),
            }];
            application.selected_project = Some(project.clone());
            application.conversation_host = Some(host.clone());
            application.navigate(
                NativeRoute::Thread {
                    project: project.clone(),
                    thread: thread.clone(),
                },
                application_cx,
            );
        });
    });
    cx.run_until_parked();

    // No manual focus step: opening the thread must focus the composer
    // itself, because typed text only reaches the surface holding
    // window focus. This is the reported production path with zero
    // admission bypasses: no set_disabled call anywhere.
    cx.simulate_input("hello");
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            assert_eq!(application.composer.read(application_cx).draft(), "hello");
        });
    });
}

#[gpui::test]
fn thread_composer_click_focuses_for_typing_after_other_control(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    let project = ProjectId::parse("thread-click-project").expect("fixture project");
    let thread = ThreadId::parse("thread-click-thread").expect("fixture thread");
    let host = cx.update(|_, app| {
        ConversationHost::mount(thread.clone(), ThemeMode::Dark, app).expect("host")
    });

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.project_options = vec![ProjectOption {
                id: project.clone(),
                name: "click".to_owned().into(),
            }];
            application.selected_project = Some(project.clone());
            application.conversation_host = Some(host.clone());
            application.navigate(
                NativeRoute::Thread {
                    project: project.clone(),
                    thread: thread.clone(),
                },
                application_cx,
            );
        });
    });
    cx.run_until_parked();
    // Deliberately park focus on another real control first, so this
    // exercises the pointer path rather than any prior focus state. No
    // direct focus call or handler invocation on the composer itself.
    cx.update(|window, app| {
        view.update(app, |application, cx| {
            window.focus(&application.profile_focus, cx);
        });
    });
    cx.run_until_parked();
    let editor = cx
        .debug_bounds(crate::native_composer::NATIVE_COMPOSER_EDITOR_SELECTOR)
        .expect("composer editor paints");
    cx.simulate_click(editor.center(), gpui::Modifiers::default());
    cx.run_until_parked();

    cx.simulate_input("hello");
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            assert_eq!(application.composer.read(application_cx).draft(), "hello");
        });
    });
}

#[gpui::test]
fn new_thread_composer_accepts_typed_draft_while_send_unready(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    cx.run_until_parked();
    // Focus exactly as a pointer press does; the composer starts
    // enabled and no test bypass touches admission.
    cx.update(|window, app| {
        view.update(app, |application, cx| {
            let focus = application.composer.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        });
    });
    cx.run_until_parked();

    cx.simulate_input("hello");
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            assert_eq!(application.composer.read(application_cx).draft(), "hello");
            assert!(
                !application.message_submission_is_admissible(application_cx),
                "typing a local draft must not imply send readiness"
            );
        });
    });
}

#[gpui::test]
fn home_project_intake_row_submits_real_intake_command(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    let (sink, commands) = command_sink([Ok(())]);

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.test_command_sink = Some(sink);
            let options = vec![ProjectOption {
                id: ProjectId::parse("home-alpha").expect("fixture project"),
                name: "alpha".to_owned().into(),
            }];
            application.project_options.clone_from(&options);
            application.install_home_picker(options, None, application_cx);
            let home = application
                .home_picker
                .clone()
                .expect("home picker installed");
            home.update(application_cx, |picker, picker_cx| {
                picker.toggle_menu(picker_cx);
                picker.commit_row(PickerRow::NewProject, picker_cx);
            });
            application.route_home_picker_action(&home, application_cx);
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |application, _| {
            assert!(
                commands
                    .borrow()
                    .iter()
                    .any(|command| matches!(command, NativeTransportCommand::BeginProjectIntake)),
                "the home New project row starts the genuine intake flow"
            );
            assert_eq!(
                application.intake_stage,
                Some(NativeProjectIntakeStage::PickingDirectory)
            );
        });
    });
}

#[gpui::test]
fn navigation_changes_the_mounted_route_identity(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));

    // Fresh windows mount the default route.
    assert!(cx.debug_bounds("route-new-thread").is_some());

    // Navigating swaps the mounted route identity.
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.navigate(
                NativeRoute::Settings {
                    section: SettingsRoute::Appearance,
                    engine: None,
                },
                application_cx,
            );
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("route-settings-appearance").is_some());

    // Going back restores the default route identity.
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            assert!(application.go_back(application_cx));
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("route-new-thread").is_some());
}

#[gpui::test]
fn ready_without_host_mounts_surface_on_new_thread_route(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds(DESKTOP_OFFLINE_SELECTOR).is_none());
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.state = NativeViewState::Ready;
            application_cx.notify();
        });
    });
    cx.run_until_parked();

    // Default route is NewThread: the concise home and actual composer
    // mount inside the desktop workspace, not the legacy activity recipe.
    assert!(cx.debug_bounds(DESKTOP_HOME_SELECTOR).is_some());
    assert!(cx.debug_bounds(DESKTOP_COMPOSER_SELECTOR).is_some());
    assert!(cx.debug_bounds(DESKTOP_SIDEBAR_SELECTOR).is_some());
    assert!(cx.debug_bounds(DESKTOP_TITLEBAR_SELECTOR).is_some());
    assert!(
        cx.debug_bounds(NATIVE_STATUS_SELECTOR).is_none(),
        "the Ready stub must not mount beside the surface"
    );

    // Other routes keep the status card.
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.navigate(
                NativeRoute::Settings {
                    section: SettingsRoute::Models,
                    engine: None,
                },
                application_cx,
            );
        });
    });
    cx.run_until_parked();
    // NOTE: `debug_bounds` keeps stale entries for selectors that have
    // left the tree, so absence of the surface is not assertable here;
    // the settings screen mounting (and the status card staying gone)
    // is the observable navigation outcome.
    assert!(cx.debug_bounds("route-settings-models").is_some());
    assert!(cx.debug_bounds(NATIVE_STATUS_SELECTOR).is_none());
}

#[gpui::test]
fn wordmark_returns_to_start(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    cx.update(|_, app| {
        view.update(app, |view, cx| {
            view.navigate(
                NativeRoute::Settings {
                    section: SettingsRoute::Models,
                    engine: None,
                },
                cx,
            );
        });
    });
    cx.run_until_parked();
    let brand = cx.debug_bounds("artisan-brand-home").expect("wordmark");
    cx.simulate_click(brand.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(cx.update(|_, app| matches!(
        view.read(app).route(),
        NativeRoute::NewThread { project: None }
    )));
}

#[gpui::test]
fn conversation_header_paints_summary_then_stored_title(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, _commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_unnamed_title_task(application, cx, "", sink);
        });
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("artisan-desktop-route-title:New task")
            .is_some(),
        "the titlebar header paints the stored title before a summary exists"
    );
    assert!(
        cx.debug_bounds("artisan-thread-screen-title:New task")
            .is_none(),
        "the conversation screen carries no duplicate title header"
    );

    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let thread_id = ThreadId::parse("title-task").expect("thread");
            install_summary_title(application, cx, &thread_id, "Ship the port");
            assert_eq!(
                application.desktop_route_title(cx),
                "Ship the port",
                "summary mode selects the harness title for the route"
            );
        });
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("artisan-desktop-route-title:Ship the port")
            .is_some(),
        "the harness summary replaces the stored title in the titlebar header"
    );
}

#[gpui::test]
fn conversation_header_refines_the_creation_placeholder(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, _commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let thread_id = install_unnamed_title_task(application, cx, "", sink);
            application.handle_service_event(
                NativeTransportEvent::Snapshot(snapshot_for(&thread_id, 1)),
                cx,
            );
            application.handle_service_event(
                NativeTransportEvent::PatchBatch(echo_batch(
                    &thread_id,
                    1,
                    "item-title",
                    None,
                    "turn-title",
                    0,
                    1,
                    "Fix the header title",
                )),
                cx,
            );
            assert_eq!(
                application.desktop_route_title(cx),
                "Fix the header title",
                "the stored title is the latest user message until the harness names the thread"
            );
        });
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("artisan-desktop-route-title:Fix the header title")
            .is_some(),
        "the refined fallback paints in the titlebar header"
    );
    assert!(
        cx.debug_bounds("artisan-thread-screen-title:Fix the header title")
            .is_none(),
        "the conversation screen carries no duplicate title header"
    );

    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let thread_id = ThreadId::parse("title-task").expect("thread");
            install_summary_title(application, cx, &thread_id, "Ship the port");
            assert_eq!(
                application.desktop_route_title(cx),
                "Ship the port",
                "the harness summary wins over the refined stored title"
            );
        });
    });
}

#[gpui::test]
fn new_thread_route_names_the_thread_being_created(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, _commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let thread_id = install_unnamed_title_task(application, cx, "", sink);
            application.pending_thread = Some(thread_id.clone());
            application.navigate(
                NativeRoute::NewThread {
                    project: application.selected_project.clone(),
                },
                cx,
            );
            // The new-thread route names the thread being created, not
            // the generic draft label, once the harness has named it.
            install_summary_title(application, cx, &thread_id, "Ship the port");
            assert_eq!(application.desktop_route_title(cx), "Ship the port");
            assert_eq!(
                application.desktop_header_title(cx).as_deref(),
                Some("Ship the port")
            );
        });
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("artisan-desktop-route-title:Ship the port")
            .is_some(),
        "the desktop header names the thread being created"
    );

    // Without a thread the route keeps the unnamed draft label and the
    // header stays on the bare wordmark.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.pending_thread = None;
            assert_eq!(
                application.desktop_route_title(cx),
                super::UNNAMED_THREAD_TITLE
            );
            assert_eq!(application.desktop_header_title(cx), None);
        });
    });
}

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end scenario drives the full event chain; splitting it would hide the causal ordering the test asserts"
)]
fn titlebar_workspace_header_starts_inset_from_the_sidebar_edge_and_truncates_the_thread_name(
    cx: &mut TestAppContext,
) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    let (sink, _commands) = command_sink([]);
    let thread_id = cx.update(|_, app| {
        view.update(app, |application, cx| {
            let thread_id = install_unnamed_title_task(application, cx, "", sink);
            install_project_option(application, "artisan-street");
            thread_id
        })
    });
    cx.run_until_parked();

    let titlebar = cx
        .debug_bounds(DESKTOP_TITLEBAR_SELECTOR)
        .expect("titlebar");
    let wordmark = cx.debug_bounds("artisan-brand-home").expect("wordmark");
    let sidebar = cx.debug_bounds(DESKTOP_SIDEBAR_SELECTOR).expect("sidebar");
    let header = cx
        .debug_bounds(TITLEBAR_HEADER_SELECTOR)
        .expect("workspace header");
    let folder = cx
        .debug_bounds(TITLEBAR_PROJECT_FOLDER_SELECTOR)
        .expect("folder fallback");
    let separator = cx
        .debug_bounds(TITLEBAR_THREAD_SEPARATOR_SELECTOR)
        .expect("thread separator");
    let short_title = cx
        .debug_bounds(Box::leak(
            format!(
                "{TITLEBAR_ROUTE_TITLE_SELECTOR}:{}",
                super::UNNAMED_THREAD_TITLE
            )
            .into_boxed_str(),
        ))
        .expect("thread title");
    let controls = cx
        .debug_bounds("artisan-desktop-titlebar-controls")
        .expect("caption controls");

    // The wordmark owns the leading sidebar section above the sidebar;
    // the workspace header lives in the titlebar's content section,
    // anchored to the primary card's left edge plus the section's inset,
    // never following the wordmark into the sidebar.
    let sidebar_right = sidebar.origin.x + sidebar.size.width;
    assert!(
        wordmark.origin.x + wordmark.size.width <= sidebar_right,
        "the wordmark stays in the leading sidebar section"
    );
    assert!(
        (f32::from(header.origin.x) - f32::from(sidebar_right) - DESKTOP_TITLEBAR_CONTENT_INSET_PX)
            .abs()
            <= 1.0,
        "the workspace header starts at the primary card's left edge plus the content inset"
    );
    assert!(
        header.origin.x < titlebar.origin.x + titlebar.size.width / 2.0,
        "the workspace header starts on the leading half of the strip"
    );
    assert!(folder.origin.x <= separator.origin.x);
    assert!(separator.origin.x <= short_title.origin.x);

    // With no inspected repository facts the folder fallback paints and
    // the VCS mark/link never do.
    assert!(
        cx.debug_bounds(TITLEBAR_REPOSITORY_MARK_SELECTOR).is_none(),
        "a repository mark without repository facts would be fabricated"
    );
    assert!(
        cx.debug_bounds(Box::leak(
            format!("{TITLEBAR_REPOSITORY_LABEL_SELECTOR}:artisanstreet/editor").into_boxed_str()
        ))
        .is_none(),
        "a repository link without repository facts would be fabricated"
    );

    // A long thread title grows elastically on the one line and then
    // truncates: it never runs under the caption controls.
    let long_title = format!(
        "{} padding padding",
        "very long conversation title ".repeat(7)
    );
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_summary_title(application, cx, &thread_id, &long_title);
        });
    });
    cx.run_until_parked();
    let truncated_title = cx
        .debug_bounds(Box::leak(
            format!("{TITLEBAR_ROUTE_TITLE_SELECTOR}:{long_title}").into_boxed_str(),
        ))
        .expect("long thread title");
    assert!(
        truncated_title.size.width > short_title.size.width,
        "the thread name is elastic while the strip has room"
    );
    assert!(
        truncated_title.origin.x + truncated_title.size.width <= controls.origin.x,
        "the truncating thread name never crosses the caption controls"
    );

    // The shipping window opens at the 1024x720 surface; the header must
    // keep the whole line seated there, left of the strip's midpoint.
    cx.simulate_resize(gpui::size(
        gpui::px(super::SURFACE_WIDTH),
        gpui::px(super::SURFACE_HEIGHT),
    ));
    cx.run_until_parked();
    let default_title = cx
        .debug_bounds(Box::leak(
            format!("{TITLEBAR_ROUTE_TITLE_SELECTOR}:{long_title}").into_boxed_str(),
        ))
        .expect("thread title at the shipping window size");
    let default_titlebar = cx
        .debug_bounds(DESKTOP_TITLEBAR_SELECTOR)
        .expect("titlebar at the shipping window size");
    let default_controls = cx
        .debug_bounds("artisan-desktop-titlebar-controls")
        .expect("caption controls at the shipping window size");
    assert!(
        default_title.size.width > gpui::px(0.0),
        "the subject keeps visible width at the shipping window size"
    );
    assert!(
        default_title.origin.x + default_title.size.width <= default_controls.origin.x,
        "the subject stays inside the strip at the shipping window size"
    );
    assert!(
        default_title.origin.x < default_titlebar.origin.x + default_titlebar.size.width / 2.0,
        "the subject starts on the leading half at the shipping window size"
    );
}

#[gpui::test]
fn titlebar_workspace_header_names_the_repository_when_facts_exist(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    let (sink, _commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_unnamed_title_task(application, cx, "", sink);
            install_project_option(application, "artisan-street");
            // Injected fixture shaped exactly like the attached-project
            // repository query's response; the transport handler retains
            // the same facts from the bounded Git read.
            application.titlebar_repository = Some(TitlebarRepository::new(
                RepositoryHost::GitHub,
                "https://github.com/artisanstreet/editor",
            ));
        });
    });
    cx.run_until_parked();

    let mark = cx
        .debug_bounds(TITLEBAR_REPOSITORY_MARK_SELECTOR)
        .expect("repository mark");
    let link = cx
        .debug_bounds(Box::leak(
            format!("{TITLEBAR_REPOSITORY_LABEL_SELECTOR}:artisanstreet/editor").into_boxed_str(),
        ))
        .expect("qualified repository link");
    let separator = cx
        .debug_bounds(TITLEBAR_THREAD_SEPARATOR_SELECTOR)
        .expect("thread separator");
    let title = cx
        .debug_bounds(Box::leak(
            format!(
                "{TITLEBAR_ROUTE_TITLE_SELECTOR}:{}",
                super::UNNAMED_THREAD_TITLE
            )
            .into_boxed_str(),
        ))
        .expect("thread title");

    assert!(mark.origin.x + mark.size.width <= link.origin.x);
    assert!(link.origin.x <= separator.origin.x);
    assert!(separator.origin.x <= title.origin.x);
    assert!(
        cx.debug_bounds(TITLEBAR_PROJECT_FOLDER_SELECTOR).is_none(),
        "inspected repository facts replace the project-folder fallback"
    );
}

#[test]
fn titlebar_workspace_context_paints_muted_instead_of_foreground() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let context = super::titlebar_context_tone(&theme);
    assert_eq!(
        context,
        theme.colors.muted_foreground.to_paint(),
        "the folder fallback and thread subject inherit muted foreground"
    );
    assert_ne!(
        context,
        theme.colors.foreground.to_paint(),
        "workspace context must not paint as primary foreground"
    );
}

#[gpui::test]
fn selecting_a_project_requests_repository_facts_and_clears_stale_ones(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let project = ProjectId::parse("varde").expect("project");
            application.test_command_sink = Some(sink);
            application.selected_project = Some(project.clone());
            application.titlebar_repository = Some(TitlebarRepository::new(
                RepositoryHost::GitHub,
                "https://github.com/old/stale",
            ));

            application.request_project_repository(cx);
            assert!(matches!(
                commands.borrow().as_slice(),
                [NativeTransportCommand::QueryProjectRepository { project_id }]
                    if project_id == &project
            ));
            assert!(
                application.titlebar_repository.is_none(),
                "stale facts are cleared while the new read is in flight"
            );

            // The same selection does not submit a duplicate read.
            application.request_project_repository(cx);
            assert_eq!(commands.borrow().len(), 1);

            let other = ProjectId::parse("other").expect("project");
            application.selected_project = Some(other.clone());
            application.request_project_repository(cx);
            assert_eq!(commands.borrow().len(), 2);
            assert!(matches!(
                &commands.borrow()[1],
                NativeTransportCommand::QueryProjectRepository { project_id }
                    if project_id == &other
            ));
        });
    });
}

#[gpui::test]
fn repository_observations_update_the_titlebar_seam_for_the_selected_project(
    cx: &mut TestAppContext,
) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let project = ProjectId::parse("varde").expect("project");
            let other = ProjectId::parse("other").expect("project");
            application.selected_project = Some(project.clone());

            let repository =
                artisan_protocol::ProjectRepository::Repository(protocol_repository_snapshot());
            application.handle_project_repository(&project, Some(&repository), cx);
            assert_eq!(
                application.titlebar_repository,
                Some(TitlebarRepository::new(
                    RepositoryHost::GitHub,
                    "https://github.com/artisanstreet/varde",
                ))
            );

            // A reply for a project that is no longer selected is dropped.
            application.selected_project = Some(other.clone());
            application.handle_project_repository(&project, None, cx);
            assert!(
                application.titlebar_repository.is_some(),
                "a stale reply cannot clear the newer selection"
            );

            // The current project's not-repository observation falls back
            // to the project folder.
            application.handle_project_repository(
                &other,
                Some(&artisan_protocol::ProjectRepository::NotRepository),
                cx,
            );
            assert!(application.titlebar_repository.is_none());

            // A repository with no browsable remote keeps the fallback.
            let local_only = ProtocolRepositorySnapshot::new(
                RepositoryBranchState::unborn("main").expect("branch"),
                None,
                vec![],
            )
            .expect("snapshot");
            application.selected_project = Some(project.clone());
            application.handle_project_repository(
                &project,
                Some(&artisan_protocol::ProjectRepository::Repository(local_only)),
                cx,
            );
            assert!(application.titlebar_repository.is_none());
        });
    });
}

#[gpui::test]
fn subject_less_routes_keep_the_bare_wordmark(cx: &mut TestAppContext) {
    let (_view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("artisan-brand-home").is_some(),
        "the wordmark keeps the home identity"
    );
    assert!(
        cx.debug_bounds(TITLEBAR_HEADER_SELECTOR).is_none(),
        "a route naming no conversation paints no workspace header"
    );
    assert!(
        cx.debug_bounds(Box::leak(
            format!(
                "{TITLEBAR_ROUTE_TITLE_SELECTOR}:{}",
                super::UNNAMED_THREAD_TITLE
            )
            .into_boxed_str(),
        ))
        .is_none(),
        "the unnamed draft label never becomes a titlebar subject"
    );
}

#[test]
fn thread_separator_is_darker_than_muted_foreground() {
    let muted_foreground = ArtisanTheme::for_mode(ThemeMode::Dark)
        .colors
        .muted_foreground
        .l;
    assert!(
        artisan_ui::theme::SurfaceStep::S600.oklch().l < muted_foreground,
        "the separator ramp step must paint darker than the muted line"
    );
}

#[gpui::test]
fn admitted_rail_activation_submits_once_and_retains_restore_state(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    let (sink, commands) = command_sink([Ok(())]);

    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.state = NativeViewState::EmptyProjects;
            application.test_command_sink = Some(sink);
            application_cx.notify();
        });
    });
    cx.run_until_parked();

    // The rail button is retired pending the legacy rail re-homing;
    // drive the same activation handler it invoked.
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.activate_add_project(application_cx);
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |application, _| {
            assert!(matches!(
                commands.borrow().as_slice(),
                [NativeTransportCommand::BeginProjectIntake]
            ));
            assert!(matches!(
                application.intake_stage,
                Some(NativeProjectIntakeStage::PickingDirectory)
            ));
            assert!(matches!(
                application.intake_restore_state.as_ref(),
                Some(NativeViewState::EmptyProjects)
            ));
            assert!(!application.add_project_action_is_admissible());
        });
    });

    // Activation is now inadmissible: driving it again must not queue a
    // second intake command.
    cx.update(|_, app| {
        view.update(app, |application, application_cx| {
            application.activate_add_project(application_cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(commands.borrow().len(), 1);
}

#[gpui::test]
fn rail_busy_and_stopped_admission_preserve_typed_failures(cx: &mut TestAppContext) {
    let (view, _) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    let (sink, commands) = command_sink([
        Err(super::CommandSendError::Busy),
        Err(super::CommandSendError::Stopped),
    ]);

    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.state = NativeViewState::EmptyProjects;
            application.test_command_sink = Some(sink);
            application.activate_add_project(application_cx);
            assert!(matches!(
                commands.borrow().as_slice(),
                [NativeTransportCommand::BeginProjectIntake]
            ));
            assert!(application.intake_stage.is_none());
            assert!(matches!(
                &application.state,
                NativeViewState::Failure(failure)
                    if failure.stage == super::ServiceFailureStage::EventBridge
                        && failure.category == super::ServiceFailureCategory::Backpressure
            ));

            application.activate_add_project(application_cx);
            assert!(matches!(
                commands.borrow().as_slice(),
                [
                    NativeTransportCommand::BeginProjectIntake,
                    NativeTransportCommand::BeginProjectIntake
                ]
            ));
            assert!(application.intake_stage.is_none());
            assert!(matches!(
                &application.state,
                NativeViewState::Failure(failure)
                    if failure.stage == super::ServiceFailureStage::EventBridge
                        && failure.category == super::ServiceFailureCategory::ChannelClosed
            ));
        });
    });
}

#[gpui::test]
fn every_project_action_fence_disables_the_native_rail_action(cx: &mut TestAppContext) {
    let (view, _) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    let (sink, commands) = command_sink([Ok(())]);

    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.test_command_sink = Some(sink);
            assert!(application.add_project_action_is_admissible());

            application.shutdown_prepared = true;
            assert!(!application.add_project_action_is_admissible());
            application.shutdown_prepared = false;

            application.service_stopped = true;
            assert!(!application.add_project_action_is_admissible());
            application.service_stopped = false;

            application.intake_stage = Some(NativeProjectIntakeStage::PickingDirectory);
            assert!(!application.add_project_action_is_admissible());
            application.intake_stage = None;

            application.thread_switch_flight = Some(ThreadSwitchFlight {
                source_thread: ThreadId::parse("rail-source").expect("thread"),
                target_thread: Some(ThreadId::parse("rail-target").expect("thread")),
                generation: 1,
                phase: ThreadSwitchPhase::UnsubscribeAdmission {
                    retry_pending: false,
                    retry_used: false,
                },
            });
            assert!(!application.add_project_action_is_admissible());
            application.thread_switch_flight = None;

            application.ordinary_unsubscribe_thread =
                Some(ThreadId::parse("rail-unsubscribe").expect("thread"));
            assert!(!application.add_project_action_is_admissible());
            application.ordinary_unsubscribe_thread = None;

            assert!(application.add_project_action_is_admissible());
            assert!(commands.borrow().is_empty());
            application_cx.notify();
        });
    });
}

#[test]
fn ready_membership_requires_the_exact_project_and_thread_rows() {
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
    assert!(ready_membership_is_valid(
        &projects,
        &ProjectId::parse("forge-p2").expect("project"),
        &threads,
        &ThreadId::parse("forge-t2").expect("thread")
    ));
    assert!(!ready_membership_is_valid(
        &projects,
        &ProjectId::parse("missing-project").expect("project"),
        &threads,
        &ThreadId::parse("forge-t2").expect("thread")
    ));
    let cross_project_threads =
        ThreadListing::new(vec![thread("forge-t2", "forge-p1", "New thread")]).expect("threads");
    assert!(!ready_membership_is_valid(
        &projects,
        &ProjectId::parse("forge-p2").expect("project"),
        &cross_project_threads,
        &ThreadId::parse("forge-t2").expect("thread")
    ));
}

#[gpui::test]
fn picker_is_disabled_for_every_intake_progress_stage(cx: &mut TestAppContext) {
    let project_id = ProjectId::parse("forge-p1").expect("project");
    let (view, _) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.install_picker(
                vec![ProjectOption {
                    id: project_id.clone(),
                    name: "First".into(),
                }],
                Some(project_id),
                application_cx,
            );
            for stage in [
                NativeProjectIntakeStage::PickingDirectory,
                NativeProjectIntakeStage::AttachingProject,
                NativeProjectIntakeStage::RefreshingProjects,
                NativeProjectIntakeStage::CreatingThread,
                NativeProjectIntakeStage::RefreshingThreads,
            ] {
                application.handle_intake_progress(stage, application_cx);
                let picker = application.picker.clone().expect("picker");
                assert!(picker.read(application_cx).state().is_disabled());
            }
        });
    });
}

#[gpui::test]
fn cancellation_restores_the_prior_catalog_and_host_and_clears_picker_action(
    cx: &mut TestAppContext,
) {
    let project_id = ProjectId::parse("forge-p1").expect("project");
    let thread_id = ThreadId::parse("forge-t1").expect("thread");
    let options = vec![ProjectOption {
        id: project_id.clone(),
        name: "First".into(),
    }];
    let (view, _) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.project_options = options.clone();
            application.selected_project = Some(project_id.clone());
            application.pending_thread = Some(thread_id.clone());
            application.try_mount_pending_thread(application_cx);
            let host_before = application.conversation_host.clone().expect("host");
            application.state = NativeViewState::Ready;
            application.install_picker(options.clone(), Some(project_id.clone()), application_cx);
            application.last_picker_action = Some(ProjectPickerAction::NewProject);
            application
                .handle_intake_progress(NativeProjectIntakeStage::PickingDirectory, application_cx);
            assert!(
                application
                    .picker
                    .as_ref()
                    .expect("picker")
                    .read(application_cx)
                    .state()
                    .is_disabled()
            );

            application.handle_intake_cancelled(application_cx);

            assert!(matches!(&application.state, NativeViewState::Ready));
            assert_eq!(application.project_options, options);
            assert_eq!(application.selected_project.as_ref(), Some(&project_id));
            assert_eq!(application.selected_thread.as_ref(), Some(&thread_id));
            assert_eq!(application.conversation_host.as_ref(), Some(&host_before));
            let picker = application
                .picker
                .as_ref()
                .expect("picker")
                .read(application_cx);
            assert!(!picker.state().is_disabled());
            assert_eq!(picker.last_action(), None);
            assert_eq!(application.intake_stage, None);
            assert_eq!(application.intake_failure_operation, None);
        });
    });
}

#[gpui::test]
fn retryable_intake_failure_keeps_a_picker_for_the_retry_command(cx: &mut TestAppContext) {
    let project_id = ProjectId::parse("forge-p1").expect("project");
    let (view, _) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    cx.update(|app| {
        view.update(app, |application, application_cx| {
            application.install_picker(
                vec![ProjectOption {
                    id: project_id.clone(),
                    name: "First".into(),
                }],
                Some(project_id),
                application_cx,
            );
            application
                .handle_intake_progress(NativeProjectIntakeStage::CreatingThread, application_cx);
            application.handle_intake_failed(
                NativeProjectIntakeOperation::CreateThread,
                ServiceFailure {
                    stage: super::ServiceFailureStage::Request,
                    category: super::ServiceFailureCategory::Peer,
                },
                true,
                application_cx,
            );
            assert!(application.intake_retry_available);
            assert_eq!(
                intake_command(true),
                super::NativeTransportCommand::RetryProjectIntake
            );
            assert!(
                !application
                    .picker
                    .as_ref()
                    .expect("picker")
                    .read(application_cx)
                    .state()
                    .is_disabled()
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
