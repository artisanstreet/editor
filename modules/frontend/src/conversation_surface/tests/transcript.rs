use super::*;

#[test]
fn end_space_matches_the_reference_anchoring_formula() {
    assert_eq!(end_space_height(900.0, 0.0, 0.0), 884.0);
    assert_eq!(end_space_height(900.0, 100.0, 800.0), 192.0);
    assert_eq!(end_space_height(0.0, 0.0, 0.0), 192.0);
}

#[gpui::test]
fn short_conversation_has_no_artificial_scroll_room(cx: &mut TestAppContext) {
    // A short conversation fits without a scrollable spacer.
    let user_scene = ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Active,
        )],
        vec![item(
            "user-a",
            1,
            SceneItemKind::UserMessage {
                body: "hi".to_owned(),
            },
            None,
        )],
        vec![TurnNarrationEntry::new(
            turn_id("turn_a"),
            TurnNarration::Quiet,
        )],
        Vec::new(),
    )
    .expect("conversation scene is valid");
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(user_scene, ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(600.0)));
    settle(cx);
    settle(cx);
    let turn_bounds = cx
        .debug_bounds("artisan-conversation-surface-turn-turn_a")
        .expect("turn must paint");
    let spacer_bounds = cx
        .debug_bounds(TRANSCRIPT_END_SPACE_SELECTOR)
        .expect("end space must paint");
    let viewport_height =
        cx.update(|_, app| f64::from(surface.read(app).scroll_handle().bounds().size.height));
    // Window and content spaces agree on differences: the element offset
    // cancels out of the formula exactly like the scroll-offset path.
    assert!(f64::from(turn_bounds.size.height) < viewport_height);
    assert_eq!(spacer_bounds.size.height, px(0.0));
    cx.update(|_, app| {
        assert_eq!(surface.read(app).scroll_handle().max_offset().y, px(0.0));
    });
    cx.update(|_, app| {
        // Settling the viewport legitimately emits viewport observations;
        // drain them and prove nothing else is pending.
        surface.update(app, |surface, _| {
            for action in surface.take_actions() {
                assert!(
                    matches!(
                        action,
                        ConversationSurfaceAction::ViewportObserved(_)
                            | ConversationSurfaceAction::ViewportExtentChanged
                    ),
                    "only legitimate viewport observations may precede assertions, got {action:?}"
                );
            }
        });
        assert!(surface.read(app).pending_actions().is_empty());
    });
}

#[gpui::test]
fn user_body_drag_selects_and_copies_exact_bytes(cx: &mut TestAppContext) {
    // Real pointer drag across the painted user body, then the platform
    // copy keystroke: the clipboard must carry the exact body bytes and
    // no observation may cross the surface action boundary.
    const BODY: &str = "selectable proof body";
    let user_scene = ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Active,
        )],
        vec![item(
            "user-a",
            1,
            SceneItemKind::UserMessage {
                body: BODY.to_owned(),
            },
            None,
        )],
        vec![TurnNarrationEntry::new(
            turn_id("turn_a"),
            TurnNarration::Quiet,
        )],
        Vec::new(),
    )
    .expect("conversation scene is valid");
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(user_scene, ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(240.0)));
    settle(cx);
    // Static literal: debug_bounds takes &'static str. Verified against
    // the selector contract: turn_selector appends "-turn-turn_a" to the
    // surface root, the user arm appends "-block-user-user-a", and the
    // body container appends "-body".
    let bounds = cx
        .debug_bounds("artisan-conversation-surface-turn-turn_a-block-user-user-a-body")
        .expect("user body must paint");
    let left = point(bounds.origin.x + px(1.0), bounds.origin.y + px(10.0));
    // The head lands past the text end on the LAST line inside the
    // bubble padding, so the layout clamps it to the exact whole body:
    // an endpoint on a last glyph resolves to that char start and drops
    // the final character, and a single-line head would miss wrapped
    // lines below it entirely.
    let right = point(
        bounds.origin.x + bounds.size.width + px(8.0),
        bounds.origin.y + bounds.size.height - px(10.0),
    );
    cx.simulate_mouse_down(left, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(right, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(right, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_keystrokes("ctrl-c");
    settle(cx);

    let copied = cx.update(|_, app| {
        app.read_from_clipboard()
            .as_ref()
            .and_then(gpui::ClipboardItem::text)
    });
    assert_eq!(copied, Some(BODY.to_owned()));
    cx.update(|_, app| {
        surface.update(app, |surface, _| {
            for action in surface.take_actions() {
                assert!(
                    matches!(
                        action,
                        ConversationSurfaceAction::ViewportObserved(_)
                            | ConversationSurfaceAction::ViewportExtentChanged
                    ),
                    "only legitimate viewport observations may precede assertions, got {action:?}"
                );
            }
        });
        assert!(surface.read(app).pending_actions().is_empty());
    });
}

#[gpui::test]
fn scene_scroll_target_executes_against_rendered_group_root(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scroll_target_scene(SceneDisclosure::Open),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(240.0)));
    settle(cx);
    let before = offset(&surface, cx);

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            assert!(surface.schedule_scroll_target(
                ConversationSurfaceTarget::Scene(scene_id("work-first")),
                surface_cx,
            ));
        });
    });
    settle(cx);

    let after = offset(&surface, cx);
    assert!(
        after.y < before.y,
        "the rendered work group must be reached"
    );
    cx.update(|_, app| assert!(surface.read(app).pending_scroll_targets.is_empty()));
}

#[gpui::test]
fn item_scroll_target_executes_against_rendered_work_item_root(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scroll_target_scene(SceneDisclosure::Open),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(240.0)));
    settle(cx);
    let before = offset(&surface, cx);

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            assert!(surface.schedule_scroll_target(
                ConversationSurfaceTarget::Item(
                    ItemId::parse("work-target").expect("item id is valid"),
                ),
                surface_cx,
            ));
        });
    });
    settle(cx);

    let after = offset(&surface, cx);
    assert!(after.y < before.y, "the rendered work item must be reached");
    cx.update(|_, app| assert!(surface.read(app).pending_scroll_targets.is_empty()));
}

#[gpui::test]
fn navigator_click_reaches_user_messages_in_both_directions(cx: &mut TestAppContext) {
    cx.update(|app| app.set_reduce_motion(true));
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(tall_navigator_scene(), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    let rail = cx
        .debug_bounds(TURN_NAVIGATOR_SELECTOR)
        .expect("navigator paints");
    cx.simulate_mouse_move(rail.center(), None::<gpui::MouseButton>, Modifiers::none());
    settle(cx);

    for (control, target, message) in [
        (
            "artisan-conversation-surface-turn-navigator-control-nav-second",
            "nav-second",
            "artisan-conversation-surface-turn-turn_b-block-user-nav-second",
        ),
        (
            "artisan-conversation-surface-turn-navigator-control-nav-first",
            "nav-first",
            "artisan-conversation-surface-turn-turn_a-block-user-nav-first",
        ),
    ] {
        let before = offset(&surface, cx);
        let row = cx
            .debug_bounds(control)
            .expect("expanded navigator row paints");
        cx.simulate_click(row.center(), Modifiers::none());
        // Apply the adapter's accepted target to the surface, then verify
        // physical movement and message alignment rather than only its intent.
        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                let targets: Vec<_> = surface
                    .take_actions()
                    .into_iter()
                    .filter_map(|action| {
                        if let ConversationSurfaceAction::ScrollIntent { target } = action {
                            Some(target)
                        } else {
                            None
                        }
                    })
                    .collect();
                assert_eq!(
                    targets,
                    vec![ConversationSurfaceTarget::Item(
                        ItemId::parse(target).expect("user message id"),
                    )]
                );
                for target in targets {
                    assert!(surface.schedule_scroll_target(target, surface_cx));
                }
            });
        });
        settle(cx);
        cx.update(|window, app| window.simulate_next_frame(app));
        settle(cx);
        let reached = cx
            .debug_bounds(message)
            .expect("target user message paints");
        let viewport = cx
            .debug_bounds(CONVERSATION_VIEWPORT_SELECTOR)
            .expect("viewport paints");
        assert!(
            (reached.top() - viewport.top()).abs() <= px(1.0),
            "{target} must align with the viewport top: target {reached:?}, viewport {viewport:?}",
        );
        if target == "nav-second" {
            assert!(
                offset(&surface, cx).y < before.y,
                "second marker must scroll down"
            );
        } else {
            assert!(
                offset(&surface, cx).y > before.y,
                "first marker must scroll back up"
            );
        }
    }
}

#[gpui::test]
fn navigator_active_tracks_the_reader_not_the_tail(cx: &mut TestAppContext) {
    const FIRST_TICK: &str = "artisan-conversation-surface-turn-navigator-control-nav-first-tick";
    const SECOND_TICK: &str = "artisan-conversation-surface-turn-navigator-control-nav-second-tick";
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(tall_navigator_scene(), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    // At the top the first turn owns the position, not the tail.
    let first_tick = cx.debug_bounds(FIRST_TICK).expect("first tick paints");
    let second_tick = cx.debug_bounds(SECOND_TICK).expect("second tick paints");
    assert_eq!(first_tick.size.width, px(24.0));
    assert_eq!(second_tick.size.width, px(16.0));
    // Scrolled to the end the second turn takes over.
    let handle = cx.update(|_, app| surface.read(app).scroll_handle().clone());
    let maximum = cx.update(|_, app| surface.read(app).scroll_handle().max_offset().y);
    assert!(maximum > px(0.0), "the tall fixture must scroll");
    handle.set_offset(point(px(0.0), -maximum));
    // `set_offset` writes the shared scroll state without scheduling a
    // frame; production scroll paths notify, so request the repaint that
    // the geometry-derived active marker rides on.
    cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
    settle(cx);
    let first_tick = cx.debug_bounds(FIRST_TICK).expect("first tick paints");
    let second_tick = cx.debug_bounds(SECOND_TICK).expect("second tick paints");
    assert_eq!(first_tick.size.width, px(16.0));
    assert_eq!(second_tick.size.width, px(24.0));
}

#[gpui::test]
fn navigator_markers_are_cached_across_renders(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(tall_navigator_scene(), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    let first = cx.update(|_, app| Rc::clone(&surface.read(app).navigator_markers));
    cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
    cx.run_until_parked();
    let second = cx.update(|_, app| Rc::clone(&surface.read(app).navigator_markers));
    assert!(
        Rc::ptr_eq(&first, &second),
        "a plain re-render must reuse the scene-derived marker cache"
    );
    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.replace_scene(tall_navigator_scene(), surface_cx);
        });
    });
    cx.run_until_parked();
    let third = cx.update(|_, app| Rc::clone(&surface.read(app).navigator_markers));
    assert!(
        !Rc::ptr_eq(&first, &third),
        "a scene replacement must rebuild the marker cache"
    );
}

#[gpui::test]
fn transcript_wheel_queues_bounded_target_then_settles(cx: &mut TestAppContext) {
    // Manual frame pump, stated honestly: the test harness never runs
    // `on_next_frame` callbacks on dirty draws, so parked frames alone
    // cannot advance the smoothing clock. Each pumped frame calls the
    // same `advance_transcript_scroll` the production callback runs.
    let body = (0..40)
        .map(|line| format!("Wheel line {line} makes the transcript scrollable."))
        .collect::<Vec<_>>()
        .join("\n");
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(vec![item(
                "tall-user",
                1,
                SceneItemKind::UserMessage { body },
                None,
            )]),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(240.0)));
    settle(cx);
    let maximum = cx.update(|_, app| surface.read(app).scroll_handle().max_offset().y);
    assert!(maximum > px(0.0), "the tall fixture must scroll");
    let before = offset(&surface, cx);
    let viewport = cx
        .debug_bounds(CONVERSATION_VIEWPORT_SELECTOR)
        .expect("viewport paints");
    cx.simulate_event(ScrollWheelEvent {
        position: viewport.center(),
        delta: ScrollDelta::Lines(point(0.0f32, -3.0f32)),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::default(),
    });
    // No synchronous jump: smoothing queues a bounded target with
    // frames still pending.
    assert_eq!(offset(&surface, cx), before);
    let target = cx.update(|_, app| {
        let surface_ref = surface.read(app);
        assert!(
            surface_ref.transcript_scroll.active(),
            "a coarse wheel tick must leave an interpolation target outstanding"
        );
        surface_ref.transcript_scroll.target()
    });
    // Pump bounded frames with draws between steps: the first frame
    // lands strictly between start and target, and the run settles
    // exactly onto the queued target before retiring it.
    let pump_frame = |cx: &mut VisualTestContext| {
        // Production smoothing now measures elapsed time, not pump count.
        std::thread::sleep(std::time::Duration::from_millis(16));
        cx.update(|window, app| {
            surface.update(app, |surface, surface_cx| {
                surface.advance_transcript_scroll(window, surface_cx);
            });
        });
        cx.run_until_parked();
    };
    pump_frame(cx);
    let mid = offset(&surface, cx);
    assert!(
        mid != before && f32::from(mid.y) != target,
        "the first pumped frame must sit between start and target"
    );
    for _ in 0..64 {
        let settled_now = cx.update(|_, app| !surface.read(app).transcript_scroll.active());
        if settled_now {
            break;
        }
        pump_frame(cx);
    }
    let settled = offset(&surface, cx);
    assert_eq!(
        f32::from(settled.y),
        target,
        "smoothing must settle exactly onto the queued target"
    );
    cx.update(|_, app| {
        assert!(
            !surface.read(app).transcript_scroll.active(),
            "settling must retire the interpolation target"
        );
    });
}

#[test]
fn navigator_turn_top_accounts_for_host_header_offset() {
    // Standalone tests see a zero viewport origin and would miss the
    // parent offset, so the rebasing math carries an explicit case: a
    // turn 300 px down the content, scrolled 100 px, under a host
    // header offsetting the viewport 50 px, sits 150 px past the
    // viewport top.
    assert_eq!(
        ConversationSurface::navigator_turn_top_viewport(300.0, 100.0, 50.0),
        150.0
    );
    assert_eq!(
        ConversationSurface::navigator_turn_top_viewport(300.0, 100.0, 0.0),
        200.0
    );
}

#[gpui::test]
fn long_navigator_list_centers_the_cap_not_the_content(cx: &mut TestAppContext) {
    // Thirty turns overflow the 70 % cap once expanded: the rail must
    // center the capped box instead of pinning a full-content top at zero.
    let turns: Vec<SceneTurn> = (0..30)
        .map(|index| {
            SceneTurn::new(
                turn_id(&format!("turn_{index:02}")),
                u64::try_from(index).expect("bounded turn index"),
                ConversationLifecycle::Completed,
            )
        })
        .collect();
    let mut items = Vec::new();
    for (index, turn) in turns.iter().enumerate() {
        items.push(
            SceneItem::new(
                scene_id(&format!("nq{index:02}")),
                turn.turn_id.clone(),
                (turns.len() + index + 1) as u64,
                SceneItemKind::UserMessage {
                    body: format!("question {index}"),
                },
                None,
            )
            .expect("navigator item is valid"),
        );
    }
    let narrations = turns
        .iter()
        .map(|turn| TurnNarrationEntry::new(turn.turn_id.clone(), TurnNarration::Quiet))
        .collect();
    let scene = ConversationScene::build(turns, items, narrations, Vec::new())
        .expect("long navigator scene is valid");
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(scene, ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    // The harness does not always repaint just because the window resized;
    // notify once so the rail metrics measure the 480 px viewport before
    // the pointer targets the rail. Without this the rail still carries
    // its creation-frame position at the maximized window height.
    cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
    settle(cx);
    let rail = cx
        .debug_bounds(TURN_NAVIGATOR_SELECTOR)
        .expect("rail paints");
    cx.simulate_mouse_move(rail.center(), None::<gpui::MouseButton>, Modifiers::none());
    settle(cx);
    let menu = cx
        .debug_bounds(TURN_NAVIGATOR_MENU_SELECTOR)
        .expect("the open menu paints");
    let viewport = cx
        .debug_bounds(CONVERSATION_VIEWPORT_SELECTOR)
        .expect("viewport paints");
    let height = f64::from(menu.size.height);
    let top = f64::from(menu.origin.y) - f64::from(viewport.origin.y);
    assert!(
        (height - 336.0).abs() <= 4.0,
        "the menu caps at 70 % of the 480 px viewport, got {height}"
    );
    assert!(
        (top - 72.0).abs() <= 4.0,
        "the capped menu centers instead of pinning top at zero, got {top}"
    );
}

/// A menu whose rows all fit has nothing to scroll to: a wheel over it leaves
/// every row where it was instead of sliding them into empty space.
#[gpui::test]
fn navigator_menu_that_fits_does_not_scroll(cx: &mut TestAppContext) {
    const FIRST_ROW: &str = "artisan-conversation-surface-turn-navigator-control-nav-first-row";
    cx.update(|app| app.set_reduce_motion(true));
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(tall_navigator_scene(), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(1200.0), px(800.0)));
    settle(cx);
    let rail = cx
        .debug_bounds(TURN_NAVIGATOR_SELECTOR)
        .expect("rail paints");
    cx.simulate_mouse_move(rail.center(), None::<gpui::MouseButton>, Modifiers::none());
    settle(cx);
    let first = cx.debug_bounds(FIRST_ROW).expect("first row paints");
    cx.simulate_mouse_move(first.center(), None::<gpui::MouseButton>, Modifiers::none());
    settle(cx);
    for lines in [-6.0f32, 6.0] {
        cx.simulate_event(ScrollWheelEvent {
            position: first.center(),
            delta: ScrollDelta::Lines(point(0.0f32, lines)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::default(),
        });
        settle(cx);
        assert_eq!(
            cx.debug_bounds(FIRST_ROW),
            Some(first),
            "a wheel of {lines} lines moves nothing in a menu that fits"
        );
        cx.update(|_, app| {
            assert_eq!(surface.read(app).navigator_scroll.offset().y, px(0.0));
        });
    }
}

/// The menu floats beside the rail: opening it leaves the rail exactly where
/// it was, the card sits left of the rail inside the transcript column, and
/// the pointer can travel from the rail into the menu and along its rows
/// without the menu closing or moving.
#[gpui::test]
fn navigator_menu_floats_beside_a_rail_that_never_moves(cx: &mut TestAppContext) {
    const FIRST_ROW: &str = "artisan-conversation-surface-turn-navigator-control-nav-first-row";
    const SECOND_ROW: &str = "artisan-conversation-surface-turn-navigator-control-nav-second-row";
    cx.update(|app| app.set_reduce_motion(true));
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(tall_navigator_scene(), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(1200.0), px(800.0)));
    settle(cx);
    let resting = cx
        .debug_bounds(TURN_NAVIGATOR_SELECTOR)
        .expect("rail paints");
    assert!(
        cx.debug_bounds(TURN_NAVIGATOR_MENU_SELECTOR).is_none(),
        "the menu stays closed at rest"
    );

    cx.simulate_mouse_move(
        resting.center(),
        None::<gpui::MouseButton>,
        Modifiers::none(),
    );
    settle(cx);
    let opened = cx
        .debug_bounds(TURN_NAVIGATOR_MENU_SELECTOR)
        .expect("rail hover opens the menu");
    let viewport = cx
        .debug_bounds(CONVERSATION_VIEWPORT_SELECTOR)
        .expect("viewport paints");
    assert_eq!(
        cx.debug_bounds(TURN_NAVIGATOR_SELECTOR),
        Some(resting),
        "opening the menu neither moves nor resizes the rail"
    );
    assert!(
        opened.right() <= resting.left(),
        "the menu {opened:?} sits beside the rail {resting:?}, not over it"
    );
    assert!(
        opened.left() >= viewport.left(),
        "the menu {opened:?} stays inside the transcript column {viewport:?}"
    );
    assert!(
        (opened.center().y - viewport.center().y).abs() <= px(1.0),
        "the menu centers on the viewport like the rail"
    );

    // Across the gap and along the rows: still open, still in place.
    let first = cx.debug_bounds(FIRST_ROW).expect("first row paints");
    let second = cx.debug_bounds(SECOND_ROW).expect("second row paints");
    for position in [
        point(opened.right() + px(2.0), first.center().y),
        first.center(),
        second.center(),
    ] {
        cx.simulate_mouse_move(position, None::<gpui::MouseButton>, Modifiers::none());
        settle(cx);
        assert_eq!(
            cx.debug_bounds(TURN_NAVIGATOR_MENU_SELECTOR),
            Some(opened),
            "the menu holds its place with the pointer at {position:?}"
        );
        assert_eq!(cx.debug_bounds(FIRST_ROW), Some(first));
    }

    // A menu row is a pointer target for the same marker as its tick.
    cx.update(|_, app| {
        surface.update(app, |surface, _| drop(surface.take_actions()));
    });
    cx.simulate_click(second.center(), Modifiers::none());
    let targets = cx.update(|_, app| {
        surface.update(app, |surface, _| {
            surface
                .take_actions()
                .into_iter()
                .filter_map(|action| match action {
                    ConversationSurfaceAction::ScrollIntent { target } => Some(target),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
    });
    assert_eq!(
        targets,
        vec![ConversationSurfaceTarget::Item(
            ItemId::parse("nav-second").expect("user message id"),
        )]
    );

    cx.simulate_mouse_move(
        point(viewport.left() + px(4.0), viewport.top() + px(4.0)),
        None::<gpui::MouseButton>,
        Modifiers::none(),
    );
    settle(cx);
    assert!(
        cx.debug_bounds(TURN_NAVIGATOR_MENU_SELECTOR).is_none(),
        "leaving the rail and the menu closes it"
    );
}

#[gpui::test]
fn transcript_turns_keep_the_centered_prose_column(cx: &mut TestAppContext) {
    // The screen mounts the host full-bleed; prose rhythm lives on the
    // turn roots themselves, so a wide standalone surface centers the
    // same 768 px column the composer card keeps.
    const TURN_A: &str = "artisan-conversation-surface-turn-turn_a";
    let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(tall_navigator_scene(), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(1200.0), px(800.0)));
    settle(cx);
    let root = cx
        .debug_bounds(CONVERSATION_SURFACE_SELECTOR)
        .expect("surface root lays out");
    let turn = cx.debug_bounds(TURN_A).expect("turn lays out");
    assert!(
        f64::from(turn.size.width) <= 769.0,
        "turn keeps the prose max width"
    );
    let turn_center = f64::from(turn.origin.x) + f64::from(turn.size.width) / 2.0;
    let root_center = f64::from(root.origin.x) + f64::from(root.size.width) / 2.0;
    assert!(
        (turn_center - root_center).abs() <= 1.0,
        "turn centers in the surface"
    );
}

#[gpui::test]
fn stale_or_unmounted_scroll_targets_are_benign_no_ops(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scroll_target_scene(SceneDisclosure::Closed),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(240.0)));
    settle(cx);
    let before = offset(&surface, cx);

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            assert!(surface.schedule_scroll_target(
                ConversationSurfaceTarget::Scene(scene_id("not-rendered")),
                surface_cx,
            ));
            assert!(surface.schedule_scroll_target(
                ConversationSurfaceTarget::Item(
                    ItemId::parse("work-target").expect("item id is valid"),
                ),
                surface_cx,
            ));
        });
    });
    settle(cx);

    assert_eq!(offset(&surface, cx), before);
    cx.update(|_, app| assert!(surface.read(app).pending_scroll_targets.is_empty()));
}

#[gpui::test]
fn scroll_target_queue_retains_fifo_head_at_bounded_capacity(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(scene(Vec::new()), ThemeMode::Dark, surface_cx)
    });
    let first = ConversationSurfaceTarget::Scene(scene_id("queued-0"));
    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            for index in 0..CONVERSATION_SURFACE_MAX_SCROLL_TARGETS {
                assert!(surface.schedule_scroll_target(
                    ConversationSurfaceTarget::Scene(scene_id(&format!("queued-{index}"))),
                    surface_cx,
                ));
            }
            assert!(!surface.schedule_scroll_target(
                ConversationSurfaceTarget::Scene(scene_id("refused")),
                surface_cx,
            ));
            assert_eq!(
                surface.pending_scroll_targets.len(),
                CONVERSATION_SURFACE_MAX_SCROLL_TARGETS
            );
            assert_eq!(surface.pending_scroll_targets.first(), Some(&first));
        });
    });
}

#[gpui::test]
fn settled_toolbar_stays_in_the_message_column_and_short_chat_does_not_scroll(
    cx: &mut TestAppContext,
) {
    let mut transcript = scene(vec![item(
        "assistant-a",
        1,
        SceneItemKind::AssistantMessage {
            body: "A short response".to_owned(),
            phase: AssistantPhase::Final,
        },
        None,
    )]);
    assert!(transcript.set_turn_footer_settlement(
        &turn_id("turn_a"),
        TurnFooterSettlement::new("A short response".to_owned(), 99).unwrap()
    ));
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(transcript, ThemeMode::Dark, cx));
    surface.update(cx, |surface, cx| {
        surface.set_footer_relative_age(&turn_id("turn_a"), "1h ago".to_owned(), cx);
        surface.set_footer_speed(&turn_id("turn_a"), Some("51.2 tok/s".to_owned()), cx);
    });
    cx.simulate_resize(size(px(720.0), px(600.0)));
    settle(cx);
    settle(cx);
    let body = cx
        .debug_bounds("artisan-conversation-surface-turn-turn_a-block-assistant-assistant-a")
        .unwrap();
    let footer = cx
        .debug_bounds("artisan-conversation-surface-turn-turn_a-footer")
        .unwrap();
    let turn = cx
        .debug_bounds("artisan-conversation-surface-turn-turn_a")
        .unwrap();
    let copy = cx
        .debug_bounds("artisan-conversation-surface-turn-turn_a-footer-footer-copy")
        .unwrap();
    assert_eq!(copy.left(), body.left());
    assert_eq!(copy.size, size(px(16.0), px(16.0)));
    let metadata = cx
        .debug_bounds("artisan-conversation-surface-turn-turn_a-footer-time-99")
        .unwrap();
    assert_eq!(metadata.left() - copy.right(), px(10.0));
    assert_eq!(footer.left(), body.left());
    assert_eq!(footer.top() - body.bottom(), px(4.0));
    assert!(footer.bottom() <= turn.bottom());
    cx.update(|_, app| assert_eq!(surface.read(app).scroll_handle().max_offset().y, px(0.0)));
}

#[gpui::test]
fn jump_to_latest_interpolates_and_reaches_the_bottom(cx: &mut TestAppContext) {
    let body = "long transcript line\n".repeat(80);
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(vec![item(
                "jump-body",
                1,
                SceneItemKind::UserMessage { body },
                None,
            )]),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(240.0)));
    settle(cx);
    let before = offset(&surface, cx);
    cx.update(|_, app| surface.update(app, |surface, cx| surface.smooth_scroll_to_bottom(cx)));
    assert_eq!(
        offset(&surface, cx),
        before,
        "request must not synchronously jump"
    );
    cx.run_until_parked();
    for frame in 0..80 {
        std::thread::sleep(Duration::from_millis(16));
        cx.update(|window, app| {
            surface.update(app, |surface, cx| {
                surface.advance_transcript_scroll(window, cx)
            })
        });
        cx.run_until_parked();
        let (position, maximum, active) = cx.update(|_, app| {
            let value = surface.read(app);
            (
                value.scroll_handle.offset().y,
                value.scroll_handle.max_offset().y,
                value.smooth_bottom_active,
            )
        });
        if frame == 0 {
            assert!(position < before.y && position > -maximum);
        }
        if !active {
            assert!((position + maximum).abs() < px(0.5));
            return;
        }
    }
    panic!("jump animation did not settle");
}

#[gpui::test]
fn a_travelling_jump_reports_the_end_when_it_settles(cx: &mut TestAppContext) {
    // The controller stays in its scrolling state until the reader is
    // reported at the end. The last report before the jump already said
    // "at the end" (the surface starts that way), so a settled jump must
    // report it again rather than leave the controller ignoring every
    // extent change that streaming brings afterwards.
    let body = "long transcript line\n".repeat(80);
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(vec![item(
                "jump-body",
                1,
                SceneItemKind::UserMessage { body },
                None,
            )]),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(240.0)));
    settle(cx);
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            let _ = surface.take_actions();
            surface.smooth_scroll_to_bottom(cx);
        });
    });
    cx.run_until_parked();
    for _ in 0..80 {
        std::thread::sleep(Duration::from_millis(16));
        cx.update(|window, app| {
            surface.update(app, |surface, cx| {
                surface.advance_transcript_scroll(window, cx);
            });
        });
        cx.run_until_parked();
        let active = cx.update(|_, app| surface.read(app).smooth_bottom_active);
        if !active {
            let actions =
                cx.update(|_, app| surface.update(app, |surface, _| surface.take_actions()));
            assert!(
                actions.contains(&ConversationSurfaceAction::ViewportObserved(
                    ViewportObservation {
                        first_visible: None,
                        last_visible: None,
                        at_bottom: true,
                    }
                )),
                "a settled jump must report the reader at the end: {actions:?}"
            );
            return;
        }
    }
    panic!("jump animation did not settle");
}

#[gpui::test]
fn a_new_tail_turn_is_brought_into_view_through_its_reserved_space(cx: &mut TestAppContext) {
    // A sent message becomes the transcript's new last turn. The end space
    // reserves exactly the room that aligns it at the top of the viewport,
    // and the automatic follow must take the reader there even though that
    // reserve is far larger than the base end space. Growth of that same
    // turn afterwards is absorbed by the reserve, not followed.
    fn transcript(with_new_turn: bool) -> ConversationScene {
        let mut turns = vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Completed,
        )];
        let mut items = vec![
            SceneItem::new(
                scene_id("older"),
                turn_id("turn_a"),
                2,
                SceneItemKind::UserMessage {
                    body: body().repeat(10),
                },
                None,
            )
            .unwrap(),
        ];
        if with_new_turn {
            turns.push(SceneTurn::new(
                turn_id("turn_b"),
                1,
                ConversationLifecycle::Active,
            ));
            items.push(
                SceneItem::new(
                    scene_id("latest"),
                    turn_id("turn_b"),
                    3,
                    SceneItemKind::UserMessage {
                        body: "New turn".to_owned(),
                    },
                    None,
                )
                .unwrap(),
            );
        }
        ConversationScene::build(turns, items, Vec::new(), Vec::new()).unwrap()
    }
    let (surface, cx) = cx
        .add_window_view(|_, cx| ConversationSurface::new(transcript(false), ThemeMode::Dark, cx));
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    assert_eq!(
        offset(&surface, cx).y,
        px(0.0),
        "the reader starts at the top"
    );
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.replace_scene(transcript(true), cx);
            surface.follow_to_bottom(cx);
        });
    });
    settle(cx);
    settle(cx);
    cx.update(|_, app| {
        let handle = surface.read(app).scroll_handle();
        assert!(handle.max_offset().y > px(0.0));
        assert!(
            (handle.offset().y + handle.max_offset().y).abs() < px(0.5),
            "a new tail turn is brought into view: {:?} of {:?}",
            handle.offset().y,
            handle.max_offset().y
        );
    });
    assert!(
        cx.debug_bounds(TRANSCRIPT_END_SPACE_SELECTOR)
            .unwrap()
            .size
            .height
            > px(TRANSCRIPT_END_SPACE_PX),
        "the new turn keeps its reserved reading space"
    );
    let aligned = offset(&surface, cx);
    cx.update(|_, app| surface.update(app, |surface, cx| surface.follow_to_bottom(cx)));
    settle(cx);
    assert_eq!(
        offset(&surface, cx),
        aligned,
        "following the same tail leaves the reserved space in control"
    );
}

#[gpui::test]
fn a_sent_row_is_a_new_tail_to_follow(cx: &mut TestAppContext) {
    // The Forge outbox row of a sent message paints at the transcript tail
    // before its turn is delivered; an automatic follow brings it into view
    // exactly like a new turn.
    let (surface, cx) = cx.add_window_view(|_, cx| {
        ConversationSurface::new(
            scene(vec![item(
                "older",
                1,
                SceneItemKind::UserMessage {
                    body: body().repeat(10),
                },
                None,
            )]),
            ThemeMode::Dark,
            cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    assert_eq!(offset(&surface, cx).y, px(0.0));
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.show_queued_text("Sent just now".to_owned(), cx);
            surface.follow_to_bottom(cx);
        });
    });
    settle(cx);
    settle(cx);
    cx.update(|_, app| {
        let handle = surface.read(app).scroll_handle();
        assert!(handle.max_offset().y > px(0.0));
        assert!(
            (handle.offset().y + handle.max_offset().y).abs() < px(0.5),
            "the sent row is brought into view"
        );
    });
}

#[gpui::test]
fn showing_jump_to_latest_re_reports_the_reader_at_the_end(cx: &mut TestAppContext) {
    // A detach the surface never observed (a disclosure toggle, an
    // interrupted jump) shows the jump control. The reader's next return to
    // the end must be reported even when the last report already said so,
    // or the controller could never re-attach within the follow leeway.
    let long_scene = scene(vec![item(
        "reply",
        1,
        SceneItemKind::AssistantMessage {
            body: "Streaming paragraph.\n\n".repeat(40),
            phase: AssistantPhase::Final,
        },
        None,
    )]);
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(long_scene, ThemeMode::Dark, cx));
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    cx.update(|_, app| surface.update(app, |surface, cx| surface.scroll_to_bottom(cx)));
    settle(cx);
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            let _ = surface.take_actions();
            let maximum = f32::from(surface.scroll_handle.max_offset().y);
            surface.observe_wheel_destination(-maximum + 8.0, maximum, cx);
            assert!(
                surface.pending_actions().is_empty(),
                "an unchanged at-the-end report is not repeated"
            );
            surface.set_jump_to_latest_visible(true, cx);
            surface.observe_wheel_destination(-maximum + 8.0, maximum, cx);
            assert!(
                matches!(
                    surface.pending_actions().last(),
                    Some(ConversationSurfaceAction::ViewportObserved(
                        ViewportObservation {
                            at_bottom: true,
                            ..
                        }
                    )),
                ),
                "{:?}",
                surface.pending_actions()
            );
        });
    });
}

#[gpui::test]
fn a_jump_with_nothing_to_scroll_completes_at_once_and_reports_the_end(cx: &mut TestAppContext) {
    // A transcript that fits its viewport has no travel: the jump is done
    // on request, the reader is reported at the end so the controller
    // leaves its scrolling state, and the next wheel tick is an ordinary
    // scroll, never an interrupted jump that would detach the reader.
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(vec![item(
                "short-body",
                1,
                SceneItemKind::UserMessage {
                    body: "One short line.".to_owned(),
                },
                None,
            )]),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    cx.update(|_, app| {
        assert_eq!(surface.read(app).scroll_handle().max_offset().y, px(0.0));
        surface.update(app, |surface, _| {
            let _ = surface.take_actions();
        });
    });
    cx.update(|_, app| surface.update(app, |surface, cx| surface.smooth_scroll_to_bottom(cx)));
    settle(cx);
    let actions = cx.update(|_, app| {
        surface.update(app, |surface, _| {
            assert!(
                !surface.smooth_bottom_active,
                "a jump with no travel must not stay armed"
            );
            surface.take_actions()
        })
    });
    assert!(
        actions.contains(&ConversationSurfaceAction::ViewportObserved(
            ViewportObservation {
                first_visible: None,
                last_visible: None,
                at_bottom: true,
            }
        )),
        "the completed jump must report the reader at the end: {actions:?}"
    );
    let viewport = cx
        .debug_bounds(CONVERSATION_VIEWPORT_SELECTOR)
        .expect("viewport paints");
    cx.simulate_event(ScrollWheelEvent {
        position: viewport.center(),
        delta: ScrollDelta::Lines(point(0.0f32, 3.0f32)),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::default(),
    });
    settle(cx);
    let actions = cx.update(|_, app| surface.update(app, |surface, _| surface.take_actions()));
    assert!(
        !actions.contains(&ConversationSurfaceAction::BottomScrollInterrupted),
        "a wheel tick after a no-op jump is not an interrupted jump: {actions:?}"
    );
    assert!(
        !actions.iter().any(|action| matches!(
            action,
            ConversationSurfaceAction::ViewportObserved(ViewportObservation {
                at_bottom: false,
                ..
            })
        )),
        "nothing overflows, so the reader can never leave the end: {actions:?}"
    );
}

#[gpui::test]
fn content_that_stops_overflowing_reports_the_end_again(cx: &mut TestAppContext) {
    // The reader detached while the transcript overflowed; once it shrinks
    // to fit, there is nowhere to be but the end. The report repeats even
    // though the last one already said so, because the controller may have
    // detached in between without any observation recording it.
    fn transcript(lines: usize) -> ConversationScene {
        scene(vec![item(
            "shrinking",
            1,
            SceneItemKind::UserMessage {
                body: "A transcript line.\n".repeat(lines),
            },
            None,
        )])
    }
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(transcript(60), ThemeMode::Dark, cx));
    cx.simulate_resize(size(px(720.0), px(240.0)));
    settle(cx);
    cx.update(|_, app| surface.update(app, |surface, cx| surface.scroll_to_bottom(cx)));
    settle(cx);
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            let _ = surface.take_actions();
            // The last report already says "at the end".
            let _ = surface.observe_viewport(
                ViewportObservation {
                    first_visible: None,
                    last_visible: None,
                    at_bottom: true,
                },
                cx,
            );
            let _ = surface.take_actions();
            surface.replace_scene(transcript(1), cx);
        })
    });
    settle(cx);
    let actions = cx.update(|_, app| surface.update(app, |surface, _| surface.take_actions()));
    cx.update(|_, app| assert_eq!(surface.read(app).scroll_handle().max_offset().y, px(0.0)));
    assert!(actions.contains(&ConversationSurfaceAction::ViewportExtentChanged));
    assert!(
        actions.contains(&ConversationSurfaceAction::ViewportObserved(
            ViewportObservation {
                first_visible: None,
                last_visible: None,
                at_bottom: true,
            }
        )),
        "content that fits must report the reader at the end: {actions:?}"
    );
}

#[gpui::test]
fn streaming_resize_requests_follow_without_detaching_the_reader(cx: &mut TestAppContext) {
    fn transcript(paragraphs: usize) -> ConversationScene {
        scene(vec![item(
            "reply",
            1,
            SceneItemKind::AssistantMessage {
                body: "Streaming paragraph.\n\n".repeat(paragraphs),
                phase: AssistantPhase::Final,
            },
            None,
        )])
    }
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(transcript(30), ThemeMode::Dark, cx));
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    cx.update(|_, app| surface.update(app, |surface, cx| surface.scroll_to_bottom(cx)));
    settle(cx);
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            let _ = surface.take_actions();
            surface.replace_scene(transcript(50), cx);
        })
    });
    settle(cx);
    let actions = cx.update(|_, app| surface.update(app, |surface, _| surface.take_actions()));
    assert!(actions.contains(&ConversationSurfaceAction::ViewportExtentChanged));
    assert!(
        !actions.iter().any(|action| matches!(
            action,
            ConversationSurfaceAction::ViewportObserved(ViewportObservation {
                at_bottom: false,
                ..
            })
        )),
        "content growth must not impersonate reader input: {actions:?}"
    );
    cx.update(|_, app| surface.update(app, |surface, cx| surface.follow_to_bottom(cx)));
    settle(cx);
    cx.update(|_, app| {
        let handle = surface.read(app).scroll_handle();
        assert!((handle.offset().y + handle.max_offset().y).abs() < px(0.5));
    });
}

#[gpui::test]
fn wheel_destination_controls_follow_leeway_before_smoothing(cx: &mut TestAppContext) {
    let long_scene = scene(vec![item(
        "reply",
        1,
        SceneItemKind::AssistantMessage {
            body: "Streaming paragraph.\n\n".repeat(40),
            phase: AssistantPhase::Final,
        },
        None,
    )]);
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(long_scene, ThemeMode::Dark, cx));
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    cx.update(|_, app| surface.update(app, |surface, cx| surface.scroll_to_bottom(cx)));
    settle(cx);
    cx.update(|window, app| {
        surface.update(app, |surface, cx| {
            let _ = surface.take_actions();
            let maximum = f32::from(surface.scroll_handle.max_offset().y);
            surface.follow_to_bottom(cx);
            surface.handle_transcript_wheel(
                &ScrollWheelEvent {
                    position: surface.scroll_handle.bounds().center(),
                    delta: ScrollDelta::Lines(point(0.0, 6.0)),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                },
                window,
                cx,
            );
            assert!(maximum + surface.transcript_scroll.target() >= 64.0);
            assert!(
                !surface.follow_bottom_pending,
                "reader intent cancels queued follow"
            );
            assert!(surface.pending_actions().iter().any(|action| matches!(
                action,
                ConversationSurfaceAction::ViewportObserved(ViewportObservation {
                    at_bottom: false,
                    ..
                })
            )));
            // A tiny movement is still inside Electron's 64 px leeway.
            surface.observe_wheel_destination(-maximum + 32.0, maximum, cx);
            assert!(matches!(
                surface.pending_actions().last(),
                Some(ConversationSurfaceAction::ViewportObserved(
                    ViewportObservation {
                        at_bottom: true,
                        ..
                    }
                ))
            ));
        })
    });
}

#[gpui::test]
fn automatic_follow_leaves_reserved_turn_space_in_control(cx: &mut TestAppContext) {
    let reserved_scene = ConversationScene::build(
        vec![
            SceneTurn::new(turn_id("turn_a"), 0, ConversationLifecycle::Completed),
            SceneTurn::new(turn_id("turn_b"), 1, ConversationLifecycle::Active),
        ],
        vec![
            SceneItem::new(
                scene_id("older"),
                turn_id("turn_a"),
                2,
                SceneItemKind::UserMessage {
                    body: body().repeat(10),
                },
                None,
            )
            .unwrap(),
            SceneItem::new(
                scene_id("latest"),
                turn_id("turn_b"),
                3,
                SceneItemKind::UserMessage {
                    body: "New turn".to_owned(),
                },
                None,
            )
            .unwrap(),
        ],
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(reserved_scene, ThemeMode::Dark, cx));
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    assert!(
        cx.debug_bounds(TRANSCRIPT_END_SPACE_SELECTOR)
            .unwrap()
            .size
            .height
            > px(TRANSCRIPT_END_SPACE_PX)
    );
    let before = offset(&surface, cx);
    cx.update(|_, app| surface.update(app, |surface, cx| surface.follow_to_bottom(cx)));
    settle(cx);
    assert_eq!(
        offset(&surface, cx),
        before,
        "follow must not fight reserved reading space"
    );
}
