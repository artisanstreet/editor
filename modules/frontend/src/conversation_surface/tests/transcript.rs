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
                    matches!(action, ConversationSurfaceAction::ViewportObserved(_)),
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
                    matches!(action, ConversationSurfaceAction::ViewportObserved(_)),
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
    // Two geometry passes follow the reveal: the probe reports the capped
    // list height, then the metrics listener re-centers the rail from it.
    settle(cx);
    let expanded = cx
        .debug_bounds(TURN_NAVIGATOR_SELECTOR)
        .expect("expanded rail paints");
    let viewport = cx
        .debug_bounds(CONVERSATION_VIEWPORT_SELECTOR)
        .expect("viewport paints");
    let height = f64::from(expanded.size.height);
    let top = f64::from(expanded.origin.y) - f64::from(viewport.origin.y);
    assert!(
        (height - 336.0).abs() <= 4.0,
        "the expanded list caps at 70 % of the 480 px viewport, got {height}"
    );
    assert!(
        (top - 72.0).abs() <= 4.0,
        "the capped rail centers instead of pinning top at zero, got {top}"
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
    assert_eq!(footer.left(), body.left());
    assert_eq!(footer.top() - body.bottom(), px(4.0));
    assert!(footer.bottom() <= turn.bottom());
    cx.update(|_, app| assert_eq!(surface.read(app).scroll_handle().max_offset().y, px(0.0)));
}
