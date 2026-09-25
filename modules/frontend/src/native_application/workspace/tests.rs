use super::*;
use crate::native_transport_service::QueuedCommand;
use gpui::{TestAppContext, VisualTestContext};

fn selected(
    workspace: &Entity<NativeWorkspace>,
    cx: &mut VisualTestContext,
) -> Entity<NativeApplication> {
    cx.update(|_, cx| {
        let workspace = workspace.read(cx);
        workspace.sessions[workspace.selected].view.clone()
    })
}

fn add_test_host(view: &Entity<NativeApplication>, cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.machine_menu.entries.push(CommandMenuEntry {
                id: "test-ubuntu".into(),
                title: "Ubuntu".into(),
                keywords: Vec::new(),
                action: CommandMenuAction::OpenHost {
                    home: Some(PathBuf::from("/test/ubuntu")),
                },
            });
            cx.notify();
        });
    });
    cx.run_until_parked();
}

#[gpui::test]
fn machine_dropdown_click_switches_in_place_and_preserves_drafts(cx: &mut TestAppContext) {
    let (workspace, cx) =
        cx.add_window_view(|window, cx| NativeWorkspace::new(None, None, window, cx));
    cx.run_until_parked();
    let local = selected(&workspace, cx);
    cx.update(|_, cx| {
        local.update(cx, |view, cx| {
            view.composer
                .update(cx, |composer, _| composer.set_draft("local unsent draft"));
        });
    });
    open_profile(cx);
    let trigger = cx
        .debug_bounds("machine-selector")
        .expect("machine trigger");
    let header = cx.debug_bounds("artisan-desktop-profile-header").unwrap();
    assert!(trigger.origin.y >= header.origin.y && trigger.bottom() <= header.bottom());
    // The avatar side of the header is part of the same selector.
    cx.simulate_click(
        gpui::point(trigger.left() + px(20.0), trigger.center().y),
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("machine-dropdown").is_some(),
        "real click must open dropdown"
    );
    add_test_host(&local, cx);
    let row = cx
        .debug_bounds("machine-option-test-ubuntu")
        .expect("Ubuntu row");
    cx.simulate_click(row.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let remote = selected(&workspace, cx);
    assert_ne!(local, remote);
    cx.update(|_, cx| {
        assert_eq!(
            cx.windows().len(),
            1,
            "host selection must not open windows"
        );
        assert_eq!(workspace.read(cx).sessions.len(), 2);
        assert_eq!(
            crate::editor_settings::get(cx).reopen_host(),
            Some(std::path::Path::new("/test/ubuntu")),
            "a host switch records the reopen-host hint"
        );
        assert_eq!(
            local.read(cx).composer.read(cx).draft(),
            "local unsent draft"
        );
        remote.update(cx, |view, cx| {
            view.composer
                .update(cx, |composer, _| composer.set_draft("Ubuntu unsent draft"));
        });
    });
    let trigger = cx.debug_bounds("machine-selector").unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let local_row = cx.debug_bounds("machine-option-local-host").unwrap();
    cx.simulate_click(local_row.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(selected(&workspace, cx), local);
    cx.update(|_, cx| {
        assert_eq!(crate::editor_settings::get(cx).reopen_host(), None);
        assert_eq!(
            local.read(cx).composer.read(cx).draft(),
            "local unsent draft"
        );
        assert_eq!(
            remote.read(cx).composer.read(cx).draft(),
            "Ubuntu unsent draft"
        );
        assert!(!local.read(cx).shutdown_prepared);
        assert!(!remote.read(cx).shutdown_prepared);
    });
}

#[gpui::test]
fn machine_dropdown_keyboard_selection_and_escape_work_without_forge(cx: &mut TestAppContext) {
    let (workspace, cx) =
        cx.add_window_view(|window, cx| NativeWorkspace::new(None, None, window, cx));
    cx.run_until_parked();
    let local = selected(&workspace, cx);
    open_profile(cx);
    let trigger = cx.debug_bounds("machine-selector").unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    add_test_host(&local, cx);
    cx.simulate_keystrokes("end enter");
    cx.run_until_parked();
    assert_ne!(selected(&workspace, cx), local);
    let trigger = cx.debug_bounds("machine-selector").unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("machine-dropdown").is_some());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("machine-dropdown").is_none());
    cx.update(|_, cx| {
        let view = &workspace.read(cx).sessions[workspace.read(cx).selected].view;
        assert!(
            view.read(cx).profile_menu.is_open(),
            "Escape closes only the nested menu"
        );
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("machine-dropdown").is_some(),
        "focus returns to the ghost trigger"
    );
    cx.simulate_keystrokes("escape escape");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert_eq!(cx.windows().len(), 1);
        let view = &workspace.read(cx).sessions[workspace.read(cx).selected].view;
        assert!(!view.read(cx).profile_menu.is_open());
    });
}

#[gpui::test]
fn machine_shutdown_prepares_all_retained_views(cx: &mut TestAppContext) {
    let (workspace, cx) =
        cx.add_window_view(|window, cx| NativeWorkspace::new(None, None, window, cx));
    let local = selected(&workspace, cx);
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.select(Some(PathBuf::from("/test/ubuntu")), window, cx);
        });
    });
    let remote = selected(&workspace, cx);
    cx.update(|_, cx| {
        workspace.update(cx, |workspace, cx| {
            assert!(workspace.prepare_shutdown(cx).is_empty());
            assert!(local.read(cx).shutdown_prepared);
            assert!(remote.read(cx).shutdown_prepared);
        });
    });
}

#[gpui::test]
fn machine_commands_and_events_stay_with_their_origin(cx: &mut TestAppContext) {
    let (workspace, cx) =
        cx.add_window_view(|window, cx| NativeWorkspace::new(None, None, window, cx));
    let local = selected(&workspace, cx);
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.select(Some(PathBuf::from("/test/ubuntu")), window, cx);
        });
    });
    let remote = selected(&workspace, cx);
    cx.update(|_, cx| {
        let commands = Rc::new(RefCell::new(Vec::new()));
        local.update(cx, |view, cx| {
            view.test_command_sink = Some(NativeTestCommandSink {
                commands: commands.clone(),
                outcomes: Rc::new(RefCell::new(std::collections::VecDeque::new())),
            });
            view.state = NativeViewState::EmptyProjects;
            view.handle_service_event(
                NativeTransportEvent::Failed(command_failure(CommandSendError::Stopped)),
                cx,
            );
        });
        remote.update(cx, |view, _| {
            let remote_commands = Rc::new(RefCell::new(Vec::new()));
            view.test_command_sink = Some(NativeTestCommandSink {
                commands: remote_commands.clone(),
                outcomes: Rc::new(RefCell::new(std::collections::VecDeque::new())),
            });
            view.state = NativeViewState::EmptyProjects;
            view.submit_command(NativeTransportCommand::SelectProject(
                ProjectId::parse("same-project-id").unwrap(),
            ))
            .unwrap();
            assert_eq!(remote_commands.borrow().len(), 1);
        });
        assert!(
            commands.borrow().is_empty(),
            "remote command must not reach local Forge"
        );
        assert!(matches!(local.read(cx).state, NativeViewState::Failure(_)));
        assert!(matches!(
            remote.read(cx).state,
            NativeViewState::EmptyProjects
        ));
        assert_eq!(
            workspace.read(cx).sessions[workspace.read(cx).selected].view,
            remote
        );
    });
}

fn open_profile(cx: &mut VisualTestContext) {
    let trigger = cx.debug_bounds("artisan-desktop-profile-trigger").unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn account_name_overrides_local_identity_without_changing_host(cx: &mut TestAppContext) {
    let (workspace, cx) =
        cx.add_window_view(|window, cx| NativeWorkspace::new(None, None, window, cx));
    let local = selected(&workspace, cx);
    cx.update(|_, cx| {
        local.update(cx, |view, _| view.profile_name = Some("Theo".into()));
        assert_eq!(local.read(cx).profile_display_name(cx), "Theo");
        cx.set_global(crate::native_account_identity::ArtisanAccountIdentity {
            display_name: "sanderAST".into(),
        });
        assert_eq!(local.read(cx).profile_display_name(cx), "sanderAST");
        assert_eq!(local.read(cx).machine_label, "This computer");
        cx.remove_global::<crate::native_account_identity::ArtisanAccountIdentity>();
        assert_eq!(local.read(cx).profile_display_name(cx), "Theo");
    });
}

#[gpui::test]
fn nested_host_menu_stays_in_small_window_and_outside_click_dismisses_both(
    cx: &mut TestAppContext,
) {
    let (workspace, cx) =
        cx.add_window_view(|window, cx| NativeWorkspace::new(None, None, window, cx));
    cx.simulate_resize(gpui::size(px(400.0), px(320.0)));
    cx.run_until_parked();
    open_profile(cx);
    let trigger = cx.debug_bounds("machine-selector").unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let menu = cx.debug_bounds("machine-dropdown").unwrap();
    assert!(menu.left() >= px(0.0) && menu.right() <= px(400.0));
    assert!(menu.top() >= px(0.0) && menu.bottom() <= px(320.0));
    cx.simulate_click(gpui::point(px(390.0), px(10.0)), gpui::Modifiers::none());
    cx.run_until_parked();
    let view = selected(&workspace, cx);
    cx.update(|_, cx| {
        assert!(!view.read(cx).machine_menu.is_open());
        assert!(!view.read(cx).profile_menu.is_open());
    });
}

#[gpui::test]
fn profile_tab_reaches_host_selector_with_shipping_keybindings(cx: &mut TestAppContext) {
    cx.update(crate::native_application::app_entry::bind_native_actions);
    let (workspace, cx) =
        cx.add_window_view(|window, cx| NativeWorkspace::new(None, None, window, cx));
    cx.run_until_parked();
    open_profile(cx);
    cx.simulate_keystrokes("tab enter");
    cx.run_until_parked();
    assert!(cx.debug_bounds("machine-dropdown").is_some());
    let original = selected(&workspace, cx);
    add_test_host(&original, cx);
    cx.simulate_keystrokes("end enter");
    cx.run_until_parked();
    assert_ne!(selected(&workspace, cx), original);
}

#[gpui::test]
fn profile_usage_is_absent_without_forge_and_disappears_on_disconnect(cx: &mut TestAppContext) {
    let (workspace, cx) =
        cx.add_window_view(|window, cx| NativeWorkspace::new(None, None, window, cx));
    cx.run_until_parked();
    open_profile(cx);
    assert!(cx.debug_bounds("artisan-profile-usage-scroll").is_none());
    assert!(
        cx.debug_bounds("artisan-desktop-profile-action-1")
            .is_none()
    );
    let view = selected(&workspace, cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.test_command_sink = Some(NativeTestCommandSink {
                commands: Rc::new(RefCell::new(Vec::new())),
                outcomes: Rc::new(RefCell::new(std::collections::VecDeque::new())),
            });
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("artisan-profile-usage-scroll").is_none());
    assert!(
        cx.debug_bounds("artisan-desktop-profile-action-1")
            .is_none()
    );
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.service_stopped = true;
            cx.notify();
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("artisan-profile-usage-scroll").is_none());
    assert!(
        cx.debug_bounds("artisan-desktop-profile-action-1")
            .is_none()
    );
    cx.update(|_, cx| assert_eq!(view.read(cx).profile_menu.entries().len(), 1));
}

#[gpui::test]
fn host_header_shares_glass_track_and_submenu_preserves_pointer_travel(cx: &mut TestAppContext) {
    let (workspace, cx) =
        cx.add_window_view(|window, cx| NativeWorkspace::new(None, None, window, cx));
    cx.run_until_parked();
    open_profile(cx);
    let settings = cx.debug_bounds("artisan-desktop-profile-action-0").unwrap();
    let header = cx.debug_bounds("machine-selector").unwrap();
    cx.simulate_mouse_move(settings.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_move(header.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    let view = selected(&workspace, cx);
    cx.update(|_, cx| {
        let view = view.read(cx);
        assert_eq!(
            view.profile_hover.borrow().active_id(),
            Some("profile-host")
        );
        assert!(
            view.profile_hover.borrow().transition().is_some(),
            "header uses the existing sliding track"
        );
        assert!(view.machine_menu.is_open());
    });
    let parent = cx.debug_bounds("artisan-desktop-profile-menu").unwrap();
    let header = cx.debug_bounds("machine-selector").unwrap();
    cx.update(|_, cx| {
        let view = view.read(cx);
        let hover = view.profile_hover.borrow();
        let rect = hover
            .transition()
            .map_or_else(|| hover.visual_rect(), |motion| motion.to);
        let surface = view.profile_hover_surface_bounds.borrow().unwrap();
        assert!((rect.left - f32::from(header.left() - surface.left())).abs() < 1.0);
        assert!((rect.top - f32::from(header.top() - surface.top())).abs() < 1.0);
        assert!((rect.width - f32::from(header.size.width)).abs() < 1.0);
        assert!((rect.height - f32::from(header.size.height)).abs() < 1.0);
    });
    let menu = cx.debug_bounds("machine-dropdown").unwrap();
    assert!(
        menu.left() >= parent.right(),
        "submenu opens beside the parent"
    );
    let row = cx.debug_bounds("machine-option-local-host").unwrap();
    cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let menu = &view.read(cx).machine_menu;
        assert!(menu.is_open());
        assert_eq!(menu.hover.borrow().active_id(), Some("local-host"));
    });
    let separator = cx.debug_bounds("machine-add-separator").unwrap();
    let add = cx.debug_bounds("machine-option-add-host").unwrap();
    assert!(separator.top() >= row.bottom());
    assert!(separator.bottom() <= add.top());
    cx.simulate_mouse_move(add.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, cx| {
        let menu = &view.read(cx).machine_menu;
        assert_eq!(menu.hover.borrow().active_id(), Some("add-host"));
        assert!(menu.hover.borrow().transition().is_some());
    });
    cx.simulate_mouse_move(settings.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(!view.read(cx).machine_menu.is_open());
        assert_eq!(
            view.read(cx).profile_hover.borrow().active_id(),
            Some("profile-settings")
        );
    });
}

#[gpui::test]
fn busy_connection_retry_keeps_the_same_host_view(cx: &mut TestAppContext) {
    let (workspace, cx) =
        cx.add_window_view(|window, cx| NativeWorkspace::new(None, None, window, cx));
    let view = selected(&workspace, cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_failure(
                ServiceFailure {
                    stage: ServiceFailureStage::Credentials,
                    category: ServiceFailureCategory::ConnectionBusy,
                },
                cx,
            );
        })
    });
    cx.run_until_parked();
    let retry = cx
        .debug_bounds("retry-forge-connection")
        .expect("retry action is visible");
    cx.simulate_click(retry.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(selected(&workspace, cx), view);
}

#[gpui::test]
fn retry_closes_a_failed_live_worker_before_replacing_it(cx: &mut TestAppContext) {
    let (workspace, cx) =
        cx.add_window_view(|window, cx| NativeWorkspace::new(None, None, window, cx));
    let view = selected(&workspace, cx);
    let (service, mut commands, finished) = NativeTransportService::pending_for_test();
    let service = Arc::new(service);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.service = Some(service.clone());
            view.set_failure(command_failure(CommandSendError::Stopped), cx);
        });
        workspace.update(cx, |workspace, cx| workspace.select(None, window, cx));
    });
    assert!(matches!(
        commands.try_recv().as_ref().map(QueuedCommand::command),
        Ok(NativeTransportCommand::Shutdown)
    ));
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(view.read(cx).connection_retry_pending);
        assert!(Arc::ptr_eq(
            view.read(cx).service.as_ref().unwrap(),
            &service
        ));
    });
    finished.store(true, std::sync::atomic::Ordering::Release);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    assert_eq!(selected(&workspace, cx), view);
    cx.update(|_, cx| {
        assert!(!view.read(cx).connection_retry_pending);
        assert!(view.read(cx).service.is_none());
    });
}

#[gpui::test]
fn closed_worker_channel_preserves_its_final_authentication_failure(cx: &mut TestAppContext) {
    let (workspace, cx) =
        cx.add_window_view(|window, cx| NativeWorkspace::new(None, None, window, cx));
    let view = selected(&workspace, cx);
    let failure = ServiceFailure {
        stage: ServiceFailureStage::Credentials,
        category: ServiceFailureCategory::Authentication,
    };
    let service = NativeTransportService::completed_for_test(vec![
        NativeTransportEvent::Failed(failure),
        NativeTransportEvent::Stopped(ServiceStopStatus::Failed),
    ]);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.service = Some(Arc::new(service));
            view.service_stopped = false;
            assert!(!view.poll_service(cx));
            assert!(matches!(&view.state, NativeViewState::Failure(actual) if *actual == failure));
        })
    });
}
