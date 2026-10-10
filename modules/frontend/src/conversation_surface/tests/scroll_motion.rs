//! Programmatic scroll motion: the eased follow, the scroll-range hold,
//! same-frame anchoring, and the live fold-for-reply flight.

use super::*;

const VIEWPORT_SIZE: (f32, f32) = (720.0, 480.0);

fn resize(cx: &mut VisualTestContext) {
    cx.simulate_resize(size(px(VIEWPORT_SIZE.0), px(VIEWPORT_SIZE.1)));
    settle(cx);
}

fn reply_scene(paragraphs: usize) -> ConversationScene {
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

fn at_natural_end(surface: &Entity<ConversationSurface>, cx: &mut VisualTestContext) -> bool {
    cx.update(|_, app| {
        let handle = surface.read(app).scroll_handle();
        (handle.offset().y + handle.max_offset().y).abs() < px(0.5)
    })
}

/// Opening a thread lands at its end in one frame: the transcript never
/// rushes past from the top. Later follows glide as before.
#[gpui::test]
fn an_opening_follow_lands_at_the_end_instead_of_gliding(cx: &mut TestAppContext) {
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(reply_scene(50), ThemeMode::Dark, cx));
    resize(cx);
    cx.update(|_, app| surface.update(app, ConversationSurface::follow_to_bottom));
    settle(cx);
    pump_animation_frame_after(cx, Duration::from_millis(16));
    assert!(
        at_natural_end(&surface, cx),
        "the first frame after layout is already at the end"
    );
    for _ in 0..3 {
        pump_animation_frame_after(cx, Duration::from_millis(16));
    }
    assert!(
        cx.update(|_, app| surface.read(app).landing.is_none()),
        "the opening lands once the end holds still"
    );
}

#[gpui::test]
fn a_follow_glides_to_the_end_instead_of_snapping(cx: &mut TestAppContext) {
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(reply_scene(30), ThemeMode::Dark, cx));
    resize(cx);
    cx.update(|_, app| surface.update(app, ConversationSurface::scroll_to_bottom));
    settle(cx);
    let start = offset(&surface, cx);
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.replace_scene(reply_scene(50), cx);
            surface.follow_to_bottom(cx);
        });
    });
    settle(cx);
    // The follow's first frame has not moved yet: the motion happens on the
    // display frames that follow, not as a jump inside this layout.
    assert_eq!(
        offset(&surface, cx),
        start,
        "a follow never jumps in layout"
    );
    assert!(cx.update(|_, app| surface.read(app).bottom_glide.is_some()));
    pump_animation_frame_after(cx, Duration::from_millis(16));
    let first = offset(&surface, cx);
    let end = cx.update(|_, app| -surface.read(app).natural_scroll_max());
    assert!(
        first.y < start.y && f32::from(first.y) > end,
        "the first frame lands strictly between start and end: {first:?} of {end}"
    );
    run_bottom_glide(&surface, cx);
    assert!(at_natural_end(&surface, cx), "the glide settles on the end");
}

#[gpui::test]
fn a_follow_during_a_wheel_glide_waits_for_it_instead_of_being_dropped(cx: &mut TestAppContext) {
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(reply_scene(40), ThemeMode::Dark, cx));
    resize(cx);
    cx.update(|_, app| surface.update(app, ConversationSurface::scroll_to_bottom));
    settle(cx);
    // A coarse wheel tick toward the end (already within its leeway) starts
    // the reader's own glide.
    let viewport = cx
        .debug_bounds(CONVERSATION_VIEWPORT_SELECTOR)
        .expect("viewport paints");
    cx.update(|_, app| {
        surface.update(app, |surface, _| {
            let maximum = f32::from(surface.scroll_handle.max_offset().y);
            surface
                .scroll_handle
                .set_offset(point(px(0.0), px(-maximum + 40.0)));
        });
    });
    settle(cx);
    cx.simulate_event(ScrollWheelEvent {
        position: viewport.center(),
        delta: ScrollDelta::Lines(point(0.0f32, -3.0f32)),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::default(),
    });
    assert!(cx.update(|_, app| surface.read(app).transcript_scroll.active()));
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.show_queued_text("Sent just now".to_owned(), cx);
            surface.follow_to_bottom(cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let surface = surface.read(app);
        assert!(
            surface.follow_bottom_pending,
            "a follow during the reader's glide is deferred, not dropped"
        );
        assert!(surface.bottom_glide.is_none(), "the reader's glide wins");
    });
    // Let the wheel glide finish; its last frame renders the deferred follow.
    for _ in 0..64 {
        if cx.update(|_, app| !surface.read(app).transcript_scroll.active()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(16));
        cx.update(|window, app| {
            surface.update(app, |surface, cx| {
                surface.advance_transcript_scroll(window, cx);
            });
        });
        cx.run_until_parked();
    }
    settle(cx);
    cx.update(|_, app| {
        assert!(
            !surface.read(app).follow_bottom_pending,
            "the deferred follow runs once the wheel glide ends"
        );
    });
    run_bottom_glide(&surface, cx);
    assert!(
        at_natural_end(&surface, cx),
        "the new tail is brought into view"
    );
}

#[gpui::test]
fn a_shrinking_last_turn_never_clamps_the_reader_and_glides_back(cx: &mut TestAppContext) {
    let (surface, cx) =
        cx.add_window_view(|_, cx| ConversationSurface::new(reply_scene(60), ThemeMode::Dark, cx));
    resize(cx);
    cx.update(|_, app| surface.update(app, ConversationSurface::scroll_to_bottom));
    settle(cx);
    assert!(at_natural_end(&surface, cx));
    let before = offset(&surface, cx);
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.replace_scene(reply_scene(45), cx);
        });
    });
    settle(cx);
    // The frame that shrank the turn kept the scroll range: GPUI had nothing
    // to clamp, so nothing on screen jumped.
    cx.update(|_, app| {
        let surface = surface.read(app);
        assert_eq!(surface.scroll_handle().offset(), before, "no clamp jump");
        assert!(surface.scroll_handle().max_offset().y >= -before.y);
        assert!(
            surface.natural_scroll_max() < -f32::from(before.y),
            "the natural end moved up with the shorter turn"
        );
        assert!(
            surface.bottom_glide.is_some(),
            "a reader left past the natural end glides back"
        );
    });
    run_bottom_glide(&surface, cx);
    assert!(offset(&surface, cx).y > before.y, "the reader moved up");
    assert!(
        at_natural_end(&surface, cx),
        "and landed exactly on the end"
    );
}

fn two_turn_scene(first_lines: usize) -> ConversationScene {
    let lines = |count: usize| {
        (0..count)
            .map(|line| format!("Anchoring line {line} keeps the turn tall."))
            .collect::<Vec<_>>()
            .join("\n")
    };
    ConversationScene::build(
        vec![
            SceneTurn::new(turn_id("turn_a"), 0, ConversationLifecycle::Completed),
            SceneTurn::new(turn_id("turn_b"), 1, ConversationLifecycle::Completed),
        ],
        vec![
            SceneItem::new(
                scene_id("above"),
                turn_id("turn_a"),
                2,
                SceneItemKind::UserMessage {
                    body: lines(first_lines),
                },
                None,
            )
            .expect("first turn item is valid"),
            SceneItem::new(
                scene_id("reading"),
                turn_id("turn_b"),
                3,
                SceneItemKind::UserMessage { body: lines(60) },
                None,
            )
            .expect("second turn item is valid"),
        ],
        Vec::new(),
        Vec::new(),
    )
    .expect("two-turn scene is valid")
}

#[gpui::test]
fn rows_above_the_reader_changing_height_leave_the_reader_in_place(cx: &mut TestAppContext) {
    const READING: &str = "artisan-conversation-surface-turn-turn_b-block-user-reading";
    let (surface, cx) = cx
        .add_window_view(|_, cx| ConversationSurface::new(two_turn_scene(8), ThemeMode::Dark, cx));
    resize(cx);
    // Put the reader 100 px into the second turn.
    let viewport = cx
        .debug_bounds(CONVERSATION_VIEWPORT_SELECTOR)
        .expect("viewport paints");
    let reading = cx.debug_bounds(READING).expect("second turn paints");
    let into = reading.top() - viewport.top() + px(100.0);
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            let offset = surface.scroll_handle.offset();
            surface
                .scroll_handle
                .set_offset(point(offset.x, offset.y - into));
            cx.notify();
        });
    });
    settle(cx);
    let placed = cx.debug_bounds(READING).expect("second turn paints").top() - viewport.top();
    let before = offset(&surface, cx);
    // The turn above the reader grows by 30 lines.
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.replace_scene(two_turn_scene(38), cx);
        });
    });
    settle(cx);
    let after = cx.debug_bounds(READING).expect("second turn paints").top() - viewport.top();
    assert!(
        (after - placed).abs() <= px(1.0),
        "the reader's content stays put: {placed:?} then {after:?}"
    );
    assert!(
        offset(&surface, cx).y < before.y - px(100.0),
        "the offset absorbed the growth above"
    );
}

/// A live section with a command, then a streaming reply with `body`.
fn live_reply_scene(body: &str, phase: AssistantPhase) -> ConversationScene {
    let run_item = |id: &str, ordinal: u64, kind: SceneItemKind, lifecycle| {
        item(id, ordinal, kind, None).with_provenance(ItemProvenance {
            run_id: Some(artisan_domain::RunId::parse("run_a").expect("run id is valid")),
            lifecycle: Some(lifecycle),
            text_settled: false,
        })
    };
    ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Active,
        )],
        vec![
            item(
                "prompt",
                1,
                SceneItemKind::UserMessage {
                    body: "why is the footer wrong".to_owned(),
                },
                None,
            ),
            run_item(
                "likely",
                2,
                SceneItemKind::AssistantMessage {
                    body: "Likely cause found".to_owned(),
                    phase: AssistantPhase::Commentary,
                },
                ConversationLifecycle::Completed,
            ),
            run_item(
                "cmd-a",
                3,
                SceneItemKind::Activity {
                    body: "cargo test".to_owned(),
                    kind: Some("terminal_activity".to_owned()),
                    detail: Some("cargo test --locked".to_owned()),
                },
                ConversationLifecycle::Completed,
            ),
            run_item(
                "reply",
                4,
                SceneItemKind::AssistantMessage {
                    body: body.to_owned(),
                    phase,
                },
                ConversationLifecycle::Streaming,
            ),
        ],
        vec![
            TurnNarrationEntry::new(turn_id("turn_a"), TurnNarration::StreamingSuppression)
                .with_active_started_at_ms(1_000)
                .with_session_disclosure(SceneDisclosure::Open),
        ],
        Vec::new(),
    )
    .expect("live reply scene is valid")
}

#[gpui::test]
fn a_live_section_folds_for_its_reply_on_the_accordion_clock(cx: &mut TestAppContext) {
    const PANEL: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-turn_a-disclosure-panel";
    let (surface, cx) = cx.add_window_view(|_, cx| {
        ConversationSurface::new(
            live_reply_scene("All tests pass. ", AssistantPhase::Unspecified),
            ThemeMode::Dark,
            cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(720.0)));
    settle(cx);
    let open = cx
        .debug_bounds(PANEL)
        .expect("the live panel paints")
        .size
        .height;
    assert!(
        open > px(0.0),
        "the work is open while the reply could be narration"
    );
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.replace_scene(
                live_reply_scene("All tests pass. ", AssistantPhase::Final),
                cx,
            );
        });
    });
    settle(cx);
    let folding = cx
        .debug_bounds(PANEL)
        .expect("the folding panel paints")
        .size
        .height;
    assert!(
        folding > px(0.0) && folding <= open,
        "the fold starts from the open height instead of popping: {folding:?} of {open:?}"
    );
    pump_animation_frame_after(cx, Duration::from_millis(300));
    pump_animation_frame_after(cx, Duration::from_millis(16));
    let folded = cx
        .debug_bounds(PANEL)
        .map_or(px(0.0), |bounds| bounds.size.height);
    assert_eq!(
        folded,
        px(0.0),
        "the fold lands closed after one accordion duration"
    );
}
