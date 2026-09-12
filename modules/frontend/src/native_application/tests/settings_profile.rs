use super::*;

#[gpui::test]
fn settings_rail_lists_real_engines_without_a_thread(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            // The normal entry point from the profile menu: the Models
            // section with no engine and no selected thread.
            application.navigate(
                NativeRoute::Settings {
                    section: SettingsRoute::Models,
                    engine: None,
                },
                cx,
            );
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let screen = application
                .settings_screen
                .clone()
                .expect("settings screen mounted");
            let ids: Vec<String> = screen
                .read(cx)
                .engines()
                .iter()
                .map(|entry| entry.id.clone())
                .collect();
            // Every real catalog engine is reachable; the mock fixture
            // identity never enters production navigation.
            for expected in ["codex", "claude", "cursor", "grok", "opencode2"] {
                assert!(
                    ids.iter().any(|id| id == expected),
                    "rail must enumerate {expected}: {ids:?}"
                );
            }
            assert!(
                !ids.iter().any(|id| id == "fixture-engine"),
                "rail must not list fixture identities: {ids:?}"
            );
            // Entering Settings requested the global readiness refresh
            // even with no thread selected.
            assert!(
                commands.borrow().iter().any(|command| matches!(
                    command,
                    NativeTransportCommand::ReadAccountUsage { .. }
                )),
                "settings entry must refresh account readiness"
            );
        });
    });
    // The real engine nav button routes through its click handler to
    // the live engine page, not a directly invoked navigate call.
    let engine_nav = cx
        .debug_bounds("settings-nav-engines-codex")
        .expect("engine nav mounted");
    cx.simulate_click(engine_nav.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, _| {
            assert!(matches!(
                application.route(),
                NativeRoute::Settings {
                    section: SettingsRoute::Engines,
                    engine: Some(engine),
                } if engine == "codex"
            ));
        });
    });
}

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end scenario drives the full event chain; splitting it would hide the causal ordering the test asserts"
)]
fn settings_model_choice_saves_acknowledges_and_reloads(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("settings-choice-task").expect("thread");
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                thread_id.clone(),
                "settings draft",
                sink,
            );
            admit_probed_codex_usage(application, cx);
            mount_settings_engine(application, cx, "codex");
        });
    });
    cx.run_until_parked();
    // The mounted choice travels the shared SelectPolicy plus
    // typed-save flow through the real model-row button handler, not
    // a Settings-only bypass or a directly emitted screen event.
    // Delivery is deferred through the effect queue, so the save is
    // asserted after the click and a parked flush.
    let model_row = cx
        .debug_bounds("settings-engine-model-codex-sol")
        .expect("settings model row mounted");
    cx.simulate_click(model_row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let save_request = admitted_save_request(application);
            let retained = application
                .engine_settings
                .pending_save()
                .map(|(_, config)| config.clone())
                .expect("settings choice save tracked");
            application.handle_engine_config_set(
                &artisan_protocol::SetThreadEngineConfigResult {
                    request_id: save_request,
                    thread_id: thread_id.clone(),
                    revision: artisan_domain::EngineConfigRevision::new(1).expect("revision"),
                    disposition: artisan_domain::ReceiptDisposition::Accepted,
                },
                retained.clone(),
                cx,
            );
            assert!(application.engine_settings.authoritative_config().is_some());
        });
    });
    // The typed save went out with the first-send preconditionâ€¦
    let save = commands
        .borrow()
        .iter()
        .find_map(|command| match command {
            NativeTransportCommand::SetThreadEngineConfig(command) => Some(command.clone()),
            _ => None,
        })
        .expect("settings choice save");
    assert_eq!(
        save.precondition(),
        artisan_domain::EngineConfigUpdatePrecondition::Unconfigured
    );
    // â€¦and reopening the thread restores the saved Codex model from
    // durable storage instead of the cleared in-memory choice.
    // `select_thread(None)` clears the authoritative config, so the
    // saved config is preserved first for the reload reply.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let saved = application
                .engine_settings
                .authoritative_config()
                .cloned()
                .expect("saved configuration");
            application.composer_model_choice = None;
            application.engine_settings.select_thread(None);
            application.engine_settings.select_thread(Some(&thread_id));
            application.submit_settings_load(thread_id.clone());
            let generation = application
                .engine_settings
                .active_settings_generation()
                .expect("settings load admitted");
            application.handle_engine_settings(
                generation,
                artisan_protocol::ThreadEngineSettingsResult::Configured {
                    thread_id: thread_id.clone(),
                    revision: artisan_domain::EngineConfigRevision::new(1).expect("revision"),
                    config: Box::new(saved),
                },
                cx,
            );
            let policy = application
                .model_selector
                .read(cx)
                .state()
                .policy()
                .cloned()
                .expect("reloaded policy");
            assert_eq!(policy.model_id, "codex-sol");
            assert_eq!(policy.profile_id.as_deref(), Some("default"));
            assert!(
                application
                    .model_selector
                    .read(cx)
                    .state()
                    .status()
                    .authoritative
            );
            let screen = application
                .settings_screen
                .clone()
                .expect("settings screen mounted");
            let snapshot = screen
                .read(cx)
                .engine_snapshot()
                .cloned()
                .expect("engine snapshot");
            assert_eq!(snapshot.saved_model.as_deref(), Some("codex-sol"));
            assert!(
                snapshot
                    .models
                    .iter()
                    .any(|row| row.id == "codex-sol" && row.saved)
            );
        });
    });
    // A saved native configuration restores from the independent
    // probed/static path: the reload must never request managed
    // OpenCode catalog discovery or favorites.
    assert!(
        commands.borrow().iter().all(|command| !matches!(
            command,
            NativeTransportCommand::ReadComposerCatalog { .. }
                | NativeTransportCommand::ReadModelFavorites { .. }
        )),
        "native reload must not request managed catalog discovery"
    );
}

#[gpui::test]
fn signed_out_refresh_removes_admission_and_updates_settings(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("settings-signout-task").expect("thread");
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(application, cx, thread_id.clone(), "draft", sink);
            admit_probed_codex_usage(application, cx);
            assert!(
                application
                    .effective_catalog_snapshot(cx)
                    .selectability("codex-sol")
                    .is_available()
            );
            mount_settings_engine(application, cx, "codex");
        });
    });
    cx.run_until_parked();
    // The mounted Settings refresh button forces a probed re-read
    // through the real transport command; the reply is fed after the
    // click delivery flushes.
    let refresh = cx
        .debug_bounds("settings-installation-refresh")
        .expect("settings refresh mounted");
    cx.simulate_click(refresh.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let forced = commands.borrow();
            assert!(
                forced.iter().any(|command| matches!(
                    command,
                    NativeTransportCommand::ReadAccountUsage { force: true, .. }
                )),
                "refresh action must force an account re-read"
            );
            drop(forced);
            // The signed-out reply removes the admission and updates the
            // mounted page through the real response handler.
            let generation = application.profile_usage_generation;
            let request_seq = application
                .profile_usage
                .pending_seq("codex")
                .expect("forced codex re-read admitted");
            application.handle_account_usage(
                "codex",
                generation,
                request_seq,
                reported_usage_entry_with_auth(
                    "codex",
                    "Codex",
                    NativeUsageAuthentication::Unauthenticated,
                    Vec::new(),
                ),
                cx,
            );
            assert!(
                !application
                    .effective_catalog_snapshot(cx)
                    .selectability("codex-sol")
                    .is_available()
            );
            let screen = application
                .settings_screen
                .clone()
                .expect("settings screen mounted");
            let snapshot = screen
                .read(cx)
                .engine_snapshot()
                .cloned()
                .expect("engine snapshot");
            assert_eq!(
                snapshot.readiness,
                crate::native_profile_usage::EngineReadiness::NeedsSignIn
            );
            assert_ne!(snapshot.saved_model.as_deref(), Some("codex-sol"));
        });
    });
}

#[gpui::test]
fn sidebar_task_links_share_sliding_hover_surface(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    cx.run_until_parked();
    let new_thread = cx
        .debug_bounds("artisan-workspace-navigation")
        .expect("New thread navigation");
    let marketplace = cx
        .debug_bounds("artisan-marketplace-navigation")
        .expect("Marketplace navigation");
    assert!(cx.debug_bounds("artisan-workspace-tabs-list").is_none());

    cx.simulate_mouse_move(new_thread.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        let hover = application.sidebar_hover.borrow();
        assert_eq!(hover.active_id(), Some("new-thread"));
    });

    cx.simulate_mouse_move(marketplace.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        let hover = application.sidebar_hover.borrow();
        assert_eq!(hover.active_id(), Some("marketplace"));
    });
}

#[gpui::test]
fn sidebar_footer_shares_sliding_hover_and_spacer_clears(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    cx.run_until_parked();
    let trigger = cx
        .debug_bounds("artisan-desktop-profile-trigger")
        .expect("profile footer trigger");
    let spacer = cx
        .debug_bounds("artisan-sidebar-spacer")
        .expect("sidebar spacer");
    let marketplace = cx
        .debug_bounds("artisan-marketplace-navigation")
        .expect("Marketplace navigation");

    // The footer trigger joins the shared pill with its own target.
    cx.simulate_mouse_move(trigger.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        let hover = application.sidebar_hover.borrow();
        assert_eq!(hover.active_id(), Some("profile"));
        assert!(hover.visible());
    });

    // Entering the blank spacer hides the pill instead of stranding it,
    // retaining geometry so the next row keeps sliding.
    cx.simulate_mouse_move(spacer.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        let hover = application.sidebar_hover.borrow();
        assert_eq!(hover.active_id(), None);
        assert!(!hover.visible());
    });

    // Reentering a nav row retargets the same shared pill, sliding from
    // the retained rect instead of placing instantly.
    cx.simulate_mouse_move(marketplace.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        let hover = application.sidebar_hover.borrow();
        assert_eq!(hover.active_id(), Some("marketplace"));
        assert!(hover.visible());
        assert!(
            hover.transition().is_some(),
            "row-to-row must slide, not jump"
        );
    });

    // The footer seal spans the sidebar edges: exactly 10px past the
    // trigger on each side, matching the sidebar padding it bleeds.
    let divider = cx
        .debug_bounds("artisan-sidebar-footer-divider")
        .expect("footer divider");
    assert_eq!(f32::from(divider.left()), f32::from(trigger.left()) - 10.0);
    assert_eq!(
        f32::from(divider.right()),
        f32::from(trigger.right()) + 10.0
    );
}

#[gpui::test]
fn profile_actions_share_sliding_hover_and_keyboard_syncs_pill(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));

    let settings = cx
        .debug_bounds("artisan-desktop-profile-action-0")
        .expect("profile Settings action");
    let usage = cx
        .debug_bounds("artisan-desktop-profile-action-1")
        .expect("profile Usage action");

    cx.simulate_mouse_move(settings.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert_eq!(application.profile_menu.highlighted_index(), Some(0));
        assert_eq!(
            application.profile_hover.borrow().active_id(),
            Some("profile-settings")
        );
        assert!(application.profile_hover.borrow().visible());
    });

    cx.simulate_mouse_move(usage.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert_eq!(application.profile_menu.highlighted_index(), Some(1));
        assert_eq!(
            application.profile_hover.borrow().active_id(),
            Some("profile-usage")
        );
    });

    // Leaving the action surface clears a pointer-owned pill while the
    // open menu keeps its keyboard highlight.
    let trigger = cx
        .debug_bounds("artisan-desktop-profile-trigger")
        .expect("profile trigger");
    cx.simulate_mouse_move(trigger.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert!(application.profile_menu.is_open());
        assert_eq!(application.profile_menu.highlighted_index(), Some(1));
        assert_eq!(application.profile_hover.borrow().active_id(), None);
        assert!(!application.profile_hover.borrow().visible());
    });

    // Re-entering restores the pointer-owned pill under the cursor.
    cx.simulate_mouse_move(usage.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert_eq!(application.profile_menu.highlighted_index(), Some(1));
        assert_eq!(
            application.profile_hover.borrow().active_id(),
            Some("profile-usage")
        );
        assert!(application.profile_hover.borrow().visible());
    });

    cx.simulate_keystrokes("home");
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert_eq!(application.profile_menu.highlighted_index(), Some(0));
        assert_eq!(
            application.profile_hover.borrow().active_id(),
            Some("profile-settings")
        );
        assert!(application.profile_hover.borrow().visible());
    });

    // A keyboard-owned pill survives a real surface leave: the pointer
    // resting elsewhere must not discard keyboard navigation.
    let usage_region = cx
        .debug_bounds(crate::native_profile_usage::PROFILE_USAGE_SELECTOR)
        .expect("profile usage region");
    cx.simulate_mouse_move(usage_region.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert!(application.profile_menu.is_open());
        assert_eq!(application.profile_menu.highlighted_index(), Some(0));
        assert_eq!(
            application.profile_hover.borrow().active_id(),
            Some("profile-settings")
        );
        assert!(application.profile_hover.borrow().visible());
    });

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert!(!application.profile_menu.is_open());
        assert_eq!(application.profile_hover.borrow().active_id(), None);
        assert!(!application.profile_hover.borrow().visible());
    });
}

#[gpui::test]
fn profile_usage_small_content_keeps_natural_height(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
    let scroller = cx
        .debug_bounds("artisan-profile-usage-scroll")
        .expect("usage scroller");
    let height = f32::from(scroller.size.height);
    assert!(
        height > 0.0 && height < 120.0,
        "short usage content must keep its natural height, got {height}"
    );
    cx.update(|_, app| {
        assert_eq!(
            f32::from(view.read(app).profile_usage_scroll.max_offset().y),
            0.0
        );
    });
    assert!(cx.debug_bounds("artisan-desktop-profile-header").is_some());
    assert!(
        cx.debug_bounds("artisan-desktop-profile-actions-hover-surface")
            .is_some()
    );
}

#[gpui::test]
fn profile_usage_tall_content_caps_with_viewport(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, _| {
            install_connected_profile_usage(application, sink);
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
    cx.run_until_parked();
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
    let tall = cx
        .debug_bounds("artisan-profile-usage-scroll")
        .expect("usage scroller");
    assert!(
        f32::from(tall.size.height) > 280.0,
        "tall usage content must expand past the old 280px cap"
    );
    cx.update(|_, app| {
        assert!(
            f32::from(view.read(app).profile_usage_scroll.max_offset().y) > 0.0,
            "the tall usage fixture must scroll"
        );
    });
    assert!(cx.debug_bounds("artisan-desktop-profile-header").is_some());
    assert!(
        cx.debug_bounds("artisan-desktop-profile-actions-hover-surface")
            .is_some()
    );
    // The inline refresh control is present once an engine answered.
    assert!(
        cx.debug_bounds("artisan-profile-usage-refresh-profile-test-alpha")
            .is_some()
    );
    // The zero-percent window is real data: its meter row is rendered.
    let zero_meter = cx
        .debug_bounds("artisan-profile-usage-meter-profile-test-alpha-session")
        .expect("zero-percent meter row");
    // The meter bar keeps the exact source 72px width.
    let zero_bar = cx
        .debug_bounds("artisan-profile-usage-meter-profile-test-alpha-session-bar")
        .expect("zero-percent meter bar");
    assert_eq!(f32::from(zero_bar.size.width), 72.0);
    // No tooltip until a meter is hovered.
    assert!(cx.debug_bounds("artisan-profile-usage-tooltip").is_none());
    cx.simulate_mouse_move(zero_meter.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("artisan-profile-usage-tooltip").is_some());
    // Leaving the meter row dismisses its tooltip.
    let header_top = cx
        .debug_bounds("artisan-desktop-profile-header")
        .expect("profile header");
    cx.simulate_mouse_move(header_top.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("artisan-profile-usage-tooltip").is_none());

    cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(320.0)));
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
    let capped = cx
        .debug_bounds("artisan-profile-usage-scroll")
        .expect("usage scroller");
    assert!(
        f32::from(capped.size.height) < 120.0,
        "a short window must shrink the usage area instead of overflowing"
    );
    let header = cx
        .debug_bounds("artisan-desktop-profile-header")
        .expect("profile header");
    assert!(
        header.top() >= gpui::px(0.0),
        "the header must stay inside a short window"
    );
    assert!(
        cx.debug_bounds("artisan-desktop-profile-actions-hover-surface")
            .is_some()
    );

    cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(200.0)));
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
    let collapsed = cx
        .debug_bounds("artisan-profile-usage-scroll")
        .expect("usage scroller");
    assert!(
        f32::from(collapsed.size.height) <= 1.0,
        "a tiny window must clamp the usage area instead of going negative"
    );
}

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end scenario drives the full event chain; splitting it would hide the causal ordering the test asserts"
)]
fn profile_usage_wheel_scrolls_once_and_dismiss_cancels(cx: &mut TestAppContext) {
    cx.update(|app| app.set_reduce_motion(true));
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, _| {
            install_connected_profile_usage(application, sink);
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
    cx.run_until_parked();
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
    let maximum = cx.update(|_, app| {
        let maximum = f32::from(view.read(app).profile_usage_scroll.max_offset().y);
        assert!(maximum > 0.0, "the tall usage fixture must scroll");
        maximum
    });
    let scroll_center = cx
        .debug_bounds("artisan-profile-usage-scroll")
        .expect("usage scroller")
        .center();

    // A lines wheel queues a bounded target without jumping there.
    cx.update(|_, app| app.set_reduce_motion(false));
    let line = cx.update(|window, _| f32::from(window.line_height()));
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroll_center,
        delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -3.0)),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.update(|_, app| {
        let application = view.read(app);
        let target = application.profile_usage_scroll_state.target();
        assert_eq!(target, (-3.0 * line).clamp(-maximum, 0.0));
        let offset = f32::from(application.profile_usage_scroll.offset().y);
        assert!(
            offset >= target && offset <= 0.0,
            "the wheel must not jump straight to its target"
        );
    });

    // A precise pixel wheel applies exactly once and cancels inertia.
    let expected_pixel = cx.update(|_, app| {
        let application = view.read(app);
        (f32::from(application.profile_usage_scroll.offset().y) - 7.0).clamp(
            -f32::from(application.profile_usage_scroll.max_offset().y),
            0.0,
        )
    });
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroll_center,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(-7.0))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.update(|_, app| {
        let application = view.read(app);
        assert_eq!(
            f32::from(application.profile_usage_scroll.offset().y),
            expected_pixel
        );
        assert!(!application.profile_usage_scroll_state.active());
    });

    // Reduced motion settles a lines wheel directly.
    cx.update(|_, app| app.set_reduce_motion(true));
    let expected_reduced = cx.update(|_, app| {
        let application = view.read(app);
        (f32::from(application.profile_usage_scroll.offset().y) - line).clamp(
            -f32::from(application.profile_usage_scroll.max_offset().y),
            0.0,
        )
    });
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroll_center,
        delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -1.0)),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.update(|_, app| {
        let application = view.read(app);
        assert_eq!(
            f32::from(application.profile_usage_scroll.offset().y),
            expected_reduced
        );
        assert!(!application.profile_usage_scroll_state.active());
    });

    // Dismissing with a queued target cancels the pending motion.
    cx.update(|_, app| app.set_reduce_motion(false));
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroll_center,
        delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -3.0)),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert!(!application.profile_menu.is_open());
        assert_eq!(
            application.profile_usage_scroll_state.target(),
            f32::from(application.profile_usage_scroll.offset().y)
        );
        assert!(!application.profile_usage_scroll_state.active());
    });
}

#[gpui::test]
fn profile_usage_hides_providers_without_data(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, _| {
            application.test_command_sink = Some(sink);
            application.profile_usage.entries.push(reported_usage_entry(
                "profile-test-hidden",
                "Hidden",
                Vec::new(),
            ));
            application.profile_usage.entries.push(NativeUsageEntry {
                engine_id: "profile-test-unauth".to_owned(),
                display_name: "Unauth".to_owned(),
                report: Some(NativeUsageReport {
                    engine_id: "profile-test-unauth".to_owned(),
                    display_name: "Unauth".to_owned(),
                    authentication: NativeUsageAuthentication::Unauthenticated,
                    account_email: None,
                    quota_surface: NativeUsageQuotaSurface::Supported,
                    windows: vec![reported_usage_window(
                        "session",
                        NativeUsageCadence::Session,
                        None,
                        40.0,
                    )],
                    failure: None,
                }),
                failure: None,
                fetched_at_ms: Some(1_000_000),
            });
        });
    });
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
    // Neither the empty nor the unauthenticated provider paints a meter.
    assert!(
        cx.debug_bounds("artisan-profile-usage-meter-profile-test-hidden-session")
            .is_none()
    );
    assert!(
        cx.debug_bounds("artisan-profile-usage-meter-profile-test-unauth-session")
            .is_none()
    );
    assert!(
        cx.debug_bounds("artisan-profile-usage-refresh-profile-test-hidden")
            .is_none()
    );
    // The menu stays mounted with header and actions around the source
    // empty state instead of fabricated rows.
    assert!(cx.debug_bounds("artisan-desktop-profile-header").is_some());
    assert!(
        cx.debug_bounds("artisan-desktop-profile-actions-hover-surface")
            .is_some()
    );
}

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end scenario drives the full event chain; splitting it would hide the causal ordering the test asserts"
)]
fn profile_refresh_swap_interrupts_from_current_values(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, _| {
            application.test_command_sink = Some(sink);
            application.profile_usage.entries.push(reported_usage_entry(
                "swap-test",
                "Swap",
                vec![reported_usage_window(
                    "session",
                    NativeUsageCadence::Session,
                    None,
                    40.0,
                )],
            ));
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
    cx.run_until_parked();
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
    let control = cx
        .debug_bounds("artisan-profile-usage-refresh-swap-test")
        .expect("refresh control");

    // Hovering arms the action target from the resting reading values.
    cx.simulate_mouse_move(control.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let swaps = view.read(app).profile_refresh_swap.borrow();
        let swap = swaps.get("swap-test").expect("swap state");
        assert_eq!(swap.to, [0.0, 1.0, 0.0]);
        assert_eq!(swap.from, [1.0, 0.0, 0.0]);
    });

    // A mid-flight step moves values without jumping to either end.
    let now = super::profile_usage_now_ms();
    cx.update(|_, app| {
        let application = view.read(app);
        application
            .profile_refresh_swap
            .borrow_mut()
            .get_mut("swap-test")
            .expect("swap state")
            .started_ms = now - 75;
        assert!(application.step_profile_swaps(now));
    });
    cx.update(|_, app| {
        let binding = view.read(app).profile_refresh_swap.borrow();
        let swap = binding.get("swap-test").expect("swap state");
        assert!(swap.displayed[0] > 0.0 && swap.displayed[0] < 1.0);
        assert!(swap.displayed[1] > 0.0 && swap.displayed[1] < 1.0);
        // Offsets travel with opacity: the leaving reading heads
        // upward from zero while the entering action arrives from
        // below, neither jumping to an endpoint.
        assert!(swap.off_displayed[0] > -4.0 && swap.off_displayed[0] < 0.0);
        assert!(swap.off_displayed[1] > 0.0 && swap.off_displayed[1] < 4.0);
    });

    // Leaving retargets from the mid-flight values, not from rest.
    let header = cx
        .debug_bounds("artisan-desktop-profile-header")
        .expect("profile header");
    cx.simulate_mouse_move(header.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let binding = view.read(app).profile_refresh_swap.borrow();
        let swap = binding.get("swap-test").expect("swap state");
        assert_eq!(swap.to, [1.0, 0.0, 0.0]);
        assert!(swap.from[0] > 0.0 && swap.from[0] < 1.0);
        assert!(swap.from[1] > 0.0 && swap.from[1] < 1.0);
        // The retained offsets continue without sign flips: the
        // reading keeps leaving upward, the action returns downward.
        assert!(swap.off_from[0] > -4.0 && swap.off_from[0] < 0.0);
        assert!(swap.off_from[1] > 0.0 && swap.off_from[1] < 4.0);
        assert_eq!(swap.off_to, [0.0, 4.0, 4.0]);
    });

    // A far-future step settles exactly on the reading values.
    cx.update(|_, app| {
        let application = view.read(app);
        assert!(!application.step_profile_swaps(now + 100_000));
        let binding = application.profile_refresh_swap.borrow();
        let swap = binding.get("swap-test").expect("swap state");
        assert_eq!(swap.displayed, [1.0, 0.0, 0.0]);
        assert_eq!(swap.off_displayed, [0.0, 4.0, 4.0]);
    });

    // A reduced-motion retarget with a changed target settles every
    // endpoint at once, and a queued step afterwards cannot regress.
    cx.update(|_, app| {
        let application = view.read(app);
        application.retarget_profile_swap("swap-test", super::RefreshSwapTarget::Loading, true);
        let now = super::profile_usage_now_ms();
        application
            .profile_refresh_swap
            .borrow_mut()
            .get_mut("swap-test")
            .expect("swap state")
            .started_ms = now;
        assert!(!application.step_profile_swaps(now));
        let binding = application.profile_refresh_swap.borrow();
        let swap = binding.get("swap-test").expect("swap state");
        assert_eq!(swap.displayed, [0.0, 0.0, 1.0]);
        assert_eq!(swap.to, [0.0, 0.0, 1.0]);
        assert_eq!(swap.off_displayed, [-4.0, 4.0, 0.0]);
    });

    // A reduced-motion change settles mid-flight even though the target
    // itself is unchanged.
    cx.simulate_mouse_move(control.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    let mid = super::profile_usage_now_ms();
    cx.update(|_, app| {
        let application = view.read(app);
        application
            .profile_refresh_swap
            .borrow_mut()
            .get_mut("swap-test")
            .expect("swap state")
            .started_ms = mid - 75;
        assert!(application.step_profile_swaps(mid));
        application.retarget_profile_swap("swap-test", super::RefreshSwapTarget::Action, true);
        let binding = application.profile_refresh_swap.borrow();
        let swap = binding.get("swap-test").expect("swap state");
        assert_eq!(swap.displayed, [0.0, 1.0, 0.0]);
        assert_eq!(swap.off_displayed, [-4.0, 0.0, 4.0]);
    });

    // Reduced motion settles a retarget instantly.
    cx.update(|_, app| app.set_reduce_motion(true));
    cx.simulate_mouse_move(control.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let binding = view.read(app).profile_refresh_swap.borrow();
        let swap = binding.get("swap-test").expect("swap state");
        assert_eq!(swap.to, [0.0, 1.0, 0.0]);
        assert_eq!(swap.displayed, [0.0, 1.0, 0.0]);
    });
}

#[gpui::test]
fn profile_engine_blocks_share_consistent_spacing(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, _| {
            application.test_command_sink = Some(sink);
            // Three byte-identical providers in first, middle, and last
            // position: only consistent gap/padding keeps every block
            // the same height with the same rhythm between them.
            for engine_id in ["spacing-a", "spacing-b", "spacing-c"] {
                application.profile_usage.entries.push(reported_usage_entry(
                    engine_id,
                    "Same",
                    vec![
                        reported_usage_window("session", NativeUsageCadence::Session, None, 30.0),
                        reported_usage_window(
                            "weekly-model",
                            NativeUsageCadence::Weekly,
                            Some("Model"),
                            50.0,
                        ),
                    ],
                ));
            }
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
    cx.run_until_parked();
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
    let scroller = cx
        .debug_bounds("artisan-profile-usage-scroll")
        .expect("usage scroller");
    let first = cx
        .debug_bounds("artisan-profile-usage-engine-spacing-a")
        .expect("first engine block");
    let middle = cx
        .debug_bounds("artisan-profile-usage-engine-spacing-b")
        .expect("middle engine block");
    let last = cx
        .debug_bounds("artisan-profile-usage-engine-spacing-c")
        .expect("last engine block");
    assert_eq!(f32::from(first.size.height), f32::from(middle.size.height));
    assert_eq!(f32::from(middle.size.height), f32::from(last.size.height));
    // Engine separators keep one 1px rule with 4px margins on each side.
    assert_eq!(f32::from(middle.top() - first.bottom()), 9.0);
    assert_eq!(f32::from(last.top() - middle.bottom()), 9.0);
    // The section contributes the single outer 4px inset on both ends.
    assert_eq!(f32::from(first.top() - scroller.top()), 4.0);
    assert_eq!(f32::from(scroller.bottom() - last.bottom()), 4.0);
}

#[gpui::test]
fn profile_tip_tween_runs_up_once_then_carries_across_rows(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, _| {
            install_connected_profile_usage(application, sink);
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
    cx.run_until_parked();
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));

    // Alpha's session window is unused: 100% remains, so the first
    // reading runs up from just short of it.
    let alpha = cx
        .debug_bounds("artisan-profile-usage-meter-profile-test-alpha-session")
        .expect("alpha meter row");
    cx.simulate_mouse_move(alpha.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let tween = *view.read(app).profile_tip_tween.borrow();
        assert_eq!(tween.to, 100.0);
        assert_eq!(tween.from, 92.0);
        assert!(tween.seen);
    });

    // Beta's session window is 21% used: the displayed value carries
    // across while only the target moves.
    let beta = cx
        .debug_bounds("artisan-profile-usage-meter-profile-test-beta-session")
        .expect("beta meter row");
    cx.simulate_mouse_move(beta.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let tween = *view.read(app).profile_tip_tween.borrow();
        assert_eq!(tween.to, 79.0);
        assert_eq!(tween.from, 92.0);
        let displayed = tween.displayed;
        assert!(
            (79.0..=92.0).contains(&displayed),
            "the carried value must ease toward its target, never restart"
        );
    });

    // Reduced motion settles the shared value directly.
    cx.update(|_, app| app.set_reduce_motion(true));
    cx.simulate_mouse_move(alpha.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let tween = *view.read(app).profile_tip_tween.borrow();
        assert_eq!(tween.to, 100.0);
        assert_eq!(tween.displayed, 100.0);
    });
}

#[gpui::test]
fn profile_refresh_spinner_keeps_control_width(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, _| {
            install_connected_profile_usage(application, sink);
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
    cx.run_until_parked();
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let idle = cx
        .debug_bounds("artisan-profile-usage-refresh-profile-test-alpha")
        .expect("refresh control");
    let idle_width = f32::from(idle.size.width);
    assert!(
        cx.debug_bounds("artisan-profile-usage-refresh-profile-test-alpha-spinner")
            .is_some()
    );

    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application
                .profile_usage
                .refreshing_engine_ids
                .push("profile-test-alpha".to_owned());
            cx.notify();
        });
    });
    cx.run_until_parked();
    let loading = cx
        .debug_bounds("artisan-profile-usage-refresh-profile-test-alpha")
        .expect("refresh control");
    assert_eq!(f32::from(loading.size.width), idle_width);
    assert!(
        cx.debug_bounds("artisan-profile-usage-refresh-profile-test-alpha-spinner")
            .is_some()
    );
}

#[gpui::test]
fn profile_refresh_focus_enter_refreshes_once(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, _| {
            application.test_command_sink = Some(sink);
            application.profile_usage.entries.push(reported_usage_entry(
                "codex",
                "Codex",
                vec![reported_usage_window(
                    "session",
                    NativeUsageCadence::Session,
                    None,
                    50.0,
                )],
            ));
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
    cx.run_until_parked();
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
    // The fresh Codex reading needs no load on open; only the focused
    // Enter may dispatch its refresh.
    let focus = cx.update(|_, app| {
        view.read(app)
            .profile_refresh_focus
            .borrow()
            .iter()
            .find(|(id, _)| id == "codex")
            .map(|(_, handle)| handle.clone())
            .expect("refresh focus handle")
    });
    let codex_reads = || {
        commands
            .borrow()
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    NativeTransportCommand::ReadAccountUsage { engine_id, .. }
                    if engine_id == "codex"
                )
            })
            .count()
    };
    assert_eq!(codex_reads(), 0);
    cx.update(|window, app| window.focus(&focus, app));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(codex_reads(), 1);
}

#[gpui::test]
fn profile_menu_plays_shared_popup_motion(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.update(|_, app| app.set_reduce_motion(true));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert!(application.profile_menu.is_open());
        assert_eq!(
            application.profile_menu_motion.borrow().phase(),
            crate::native_model_selector::PickerMenuPhase::Open
        );
    });
    assert!(cx.debug_bounds("artisan-desktop-profile-menu").is_some());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert!(!application.profile_menu.is_open());
        assert_eq!(
            application.profile_menu_motion.borrow().phase(),
            crate::native_model_selector::PickerMenuPhase::Hidden
        );
    });
    assert!(cx.debug_bounds("artisan-desktop-profile-menu").is_none());

    cx.update(|_, app| app.set_reduce_motion(false));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert!(application.profile_menu.is_open());
        assert_eq!(
            application.profile_menu_motion.borrow().phase(),
            crate::native_model_selector::PickerMenuPhase::Opening
        );
    });
    assert!(cx.debug_bounds("artisan-desktop-profile-menu").is_some());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert!(!application.profile_menu.is_open());
        assert_eq!(
            application.profile_menu_motion.borrow().phase(),
            crate::native_model_selector::PickerMenuPhase::Closing
        );
    });
    // The retained exit presentation stays mounted through Closing.
    assert!(cx.debug_bounds("artisan-desktop-profile-menu").is_some());
}

#[gpui::test]
fn profile_tip_clamps_into_a_narrow_viewport(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, _| {
            install_connected_profile_usage(application, sink);
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(900.0)));
    cx.run_until_parked();
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
    let alpha = cx
        .debug_bounds("artisan-profile-usage-meter-profile-test-alpha-session")
        .expect("alpha meter row");
    cx.simulate_mouse_move(alpha.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    let tooltip = cx
        .debug_bounds("artisan-profile-usage-tooltip")
        .expect("usage tooltip");
    assert!(f32::from(tooltip.left()) <= 169.0);
    assert!(f32::from(tooltip.right()) <= 393.0);
}

#[test]
fn profile_name_capitalizes_first_letter_only() {
    assert_eq!(super::capitalize_label("sander"), "Sander");
    assert_eq!(
        super::capitalize_label("DESKTOP-96USC6J"),
        "Desktop-96usc6j"
    );
    assert_eq!(super::capitalize_label(""), "");
}

#[gpui::test]
fn profile_menu_keyboard_opens_settings_and_closes(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, app| view.read(app).profile_menu.is_open())
        .then_some(())
        .expect("menu opens");
    assert!(cx.debug_bounds("artisan-desktop-profile-menu").is_some());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!cx.update(|_, app| view.read(app).profile_menu.is_open()));
    let trigger = cx
        .debug_bounds("artisan-desktop-profile-trigger")
        .expect("profile trigger");
    cx.simulate_click(trigger.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
    cx.simulate_click(trigger.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(!cx.update(|_, app| view.read(app).profile_menu.is_open()));
    cx.simulate_keystrokes("enter home enter");
    cx.run_until_parked();
    cx.update(|_, app| {
        assert!(!view.read(app).profile_menu.is_open());
        assert!(matches!(
            view.read(app).route(),
            NativeRoute::Settings {
                section: SettingsRoute::Models,
                ..
            }
        ));
    });
}

#[gpui::test]
fn profile_menu_usage_action_keeps_menu_open_and_never_invents_readings(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
    cx.update(|window, app| {
        view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert!(application.profile_menu.is_open());
        assert!(application.profile_usage.entries.is_empty());
        let item_ids = application
            .profile_menu
            .entries()
            .iter()
            .filter_map(|entry| entry.as_item())
            .map(|item| item.id.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(item_ids, vec!["settings", "usage"]);
    });
    assert!(
        cx.debug_bounds(crate::native_profile_usage::PROFILE_USAGE_SELECTOR)
            .is_some()
    );
    cx.simulate_keystrokes("end enter");
    cx.run_until_parked();
    assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!cx.update(|_, app| view.read(app).profile_menu.is_open()));
}

#[gpui::test]
fn command_shortcut_opens_palette_without_persistent_titlebar_search(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    cx.update(|_, app| super::bind_native_actions(app));
    cx.run_until_parked();
    // The titlebar no longer paints a persistent search input; the
    // palette lives behind the keyboard shortcut.
    cx.update(|_, app| {
        assert!(!view.read(app).command_menu.read(app).state().is_open());
    });
    assert!(cx.debug_bounds(COMMAND_MENU_INPUT_SELECTOR).is_none());

    // Ctrl+K opens the working palette: focused input, result list, and
    // dialog scrim â€” with no titlebar dropdown.
    cx.simulate_keystrokes("ctrl-k");
    cx.run_until_parked();
    cx.update(|window, app| {
        let application = view.read(app);
        let menu = application.command_menu.read(app);
        assert!(menu.state().is_open());
        assert!(menu.input_focus().is_focused(window));
    });
    assert!(cx.debug_bounds(COMMAND_MENU_INPUT_SELECTOR).is_some());
    assert!(cx.debug_bounds(COMMAND_MENU_LIST_SELECTOR).is_some());
    assert!(cx.debug_bounds(COMMAND_MENU_DROPDOWN_SELECTOR).is_none());
    assert!(
        cx.debug_bounds("artisan-native-command-menu-scrim")
            .is_some()
    );

    // Escape closes the palette and restores root focus, removing the
    // transient input with it.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, app| {
        let application = view.read(app);
        assert!(!application.command_menu.read(app).state().is_open());
        assert!(application.focus_handle.is_focused(window));
    });
    assert!(cx.debug_bounds(COMMAND_MENU_INPUT_SELECTOR).is_none());
}

#[gpui::test]
fn command_activation_routes_settings_through_the_application(cx: &mut TestAppContext) {
    let (view, cx) =
        cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
    cx.update(|_, app| super::bind_native_actions(app));
    cx.simulate_keystrokes("ctrl-k");
    cx.run_until_parked();
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    cx.update(|_, app| {
        view.update(app, |application, _| {
            assert_eq!(
                application.route(),
                &NativeRoute::Settings {
                    section: SettingsRoute::Models,
                    engine: None,
                }
            );
        });
    });
}
