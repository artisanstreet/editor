//! Deterministic proof for the transcript render budgets and row windowing.
//!
//! Every assertion here is structural: built-row sets, placeholder selectors,
//! and shaping ledgers. No test times a frame or depends on wall-clock work.

use artisan_ui::markdown_cache::{
    MARKDOWN_PARSE_CACHE_MAX_BYTES, MARKDOWN_PARSE_CACHE_MAX_ENTRIES,
};

use super::*;
use crate::conversation_scene::SCENE_MAX_MESSAGE_BODY_BYTES;
use crate::conversation_surface::render_budget::{
    TRANSCRIPT_MAX_BUILT_ROWS, TRANSCRIPT_MAX_MARKDOWN_BYTES_PER_ROW, TRANSCRIPT_MAX_ROW_HEIGHT_PX,
    TRANSCRIPT_TURN_GAP_PX, plan_transcript_window, transcript_markdown_within_budget,
};
use crate::conversation_surface::transcript_window::TranscriptHeightTable;

/// One assistant body shared by every turn in the long fixture.
const ASSISTANT_BODY: &str = "A windowed transcript shapes only this reply.\n";

/// Stable selectors for the first and last turn of the long fixture.
const FIRST_TURN_SELECTOR: &str = "artisan-conversation-surface-turn-turn_000";
const LAST_TURN_SELECTOR: &str = "artisan-conversation-surface-turn-turn_119";
const LAST_PLACEHOLDER_SELECTOR: &str = "artisan-conversation-surface-turn-turn_119-placeholder";

/// Builds monotonically increasing row offsets for `rows` rows of one extent.
fn uniform_offsets(rows: usize, extent: f64) -> Vec<f64> {
    (0..=rows)
        .map(|index| f64::from(u32::try_from(index).expect("bounded row index")) * extent)
        .collect()
}

/// Builds a transcript with `turns` user/assistant turn pairs.
fn long_scene(turns: usize) -> ConversationScene {
    turn_range_scene(0..turns)
}

/// Builds the turns `range` of the long fixture: the window a thread shows
/// while its older turns are still unread.
fn turn_range_scene(range: std::ops::Range<usize>) -> ConversationScene {
    let turns = range.len();
    let mut turn_inputs = Vec::with_capacity(turns);
    let mut items = Vec::with_capacity(turns * 2);
    let mut narrations = Vec::with_capacity(turns);
    // Turn ordinals share the global ordinal namespace with items, so item
    // ordinals continue after every turn ordinal.
    let mut ordinal: u64 = u64::try_from(range.end).expect("bounded turn count");
    for index in range {
        let id = turn_id(&format!("turn_{index:03}"));
        turn_inputs.push(SceneTurn::new(
            id.clone(),
            u64::try_from(index).expect("bounded turn index"),
            ConversationLifecycle::Completed,
        ));
        narrations.push(TurnNarrationEntry::new(id.clone(), TurnNarration::Quiet));
        items.push(
            SceneItem::new(
                scene_id(&format!("user-{index:03}")),
                id.clone(),
                ordinal,
                SceneItemKind::UserMessage {
                    body: format!("Prompt {index}"),
                },
                None,
            )
            .expect("long-scene user message is valid"),
        );
        ordinal = ordinal.saturating_add(1);
        items.push(
            SceneItem::new(
                scene_id(&format!("assistant-{index:03}")),
                id,
                ordinal,
                SceneItemKind::AssistantMessage {
                    body: ASSISTANT_BODY.to_owned(),
                    phase: AssistantPhase::Final,
                },
                None,
            )
            .expect("long-scene assistant message is valid"),
        );
        ordinal = ordinal.saturating_add(1);
    }
    ConversationScene::build(turn_inputs, items, narrations, Vec::new())
        .expect("long transcript scene is valid")
}

#[test]
fn planner_builds_viewport_rows_with_bounded_overscan() {
    let offsets = uniform_offsets(1_000, 132.0);
    let plan = plan_transcript_window(&offsets, 10_000.0, 400.0, false);
    // Row 75 spans 9 900..10 032 and row 79 spans 10 428..10 560, so the
    // viewport edges land in rows 75..79; four overscan rows sit on each side.
    assert_eq!(plan.built, 71..83);
    assert!(!plan.capped);
    assert!(plan.desired_rows <= TRANSCRIPT_MAX_BUILT_ROWS);
}

#[test]
fn planner_caps_a_window_that_would_build_the_whole_scene() {
    let offsets = uniform_offsets(1_000, 132.0);
    let plan = plan_transcript_window(&offsets, 0.0, 1_000_000.0, false);
    assert!(plan.capped, "an unbounded viewport must hit the row cap");
    assert_eq!(plan.desired_rows, 1_000);
    assert_eq!(plan.built.len(), TRANSCRIPT_MAX_BUILT_ROWS);
    assert_eq!(plan.built, 0..TRANSCRIPT_MAX_BUILT_ROWS);
}

#[test]
fn planner_tail_anchors_and_still_honors_the_cap() {
    let offsets = uniform_offsets(1_000, 132.0);
    let tail = plan_transcript_window(&offsets, 0.0, 400.0, true);
    assert!(!tail.capped);
    assert_eq!(tail.built, 992..1_000);

    let capped_tail = plan_transcript_window(&offsets, 0.0, 1_000_000.0, true);
    assert!(capped_tail.capped);
    assert_eq!(capped_tail.built.len(), TRANSCRIPT_MAX_BUILT_ROWS);
    assert_eq!(capped_tail.built.end, 1_000);
}

#[test]
fn shaping_budget_predicate_is_inclusive_at_the_cap() {
    assert!(transcript_markdown_within_budget(
        TRANSCRIPT_MAX_MARKDOWN_BYTES_PER_ROW
    ));
    assert!(!transcript_markdown_within_budget(
        TRANSCRIPT_MAX_MARKDOWN_BYTES_PER_ROW + 1
    ));
    // The budget mirrors the accepted-scene ceiling: valid scenes never take
    // the plain fallback, and a regression past the ceiling cannot silently
    // multiply per-frame parse cost.
    assert_eq!(
        TRANSCRIPT_MAX_MARKDOWN_BYTES_PER_ROW,
        SCENE_MAX_MESSAGE_BODY_BYTES
    );
}

#[gpui::test]
fn long_transcript_builds_only_visible_and_overscan_rows(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(long_scene(120), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);

    let report = cx.update(|_, app| surface.read(app).transcript_window_report());
    assert_eq!(report.total_rows, 120);
    assert!(!report.built_rows.is_empty());
    assert!(
        report.built_rows.len() <= TRANSCRIPT_MAX_BUILT_ROWS,
        "the hard row cap must hold, got {} rows",
        report.built_rows.len()
    );
    assert!(
        report.built_rows.len() < 40,
        "the 480 px viewport must build far fewer than the full transcript, got {}",
        report.built_rows.len()
    );
    assert_eq!(report.built_rows.first(), Some(&0));
    assert!(
        report
            .built_rows
            .windows(2)
            .all(|pair| pair[1] == pair[0] + 1),
        "the primary window must be contiguous, got {:?}",
        report.built_rows
    );

    // The tail is off-window: no real row paints, only its height placeholder.
    assert!(cx.debug_bounds(LAST_TURN_SELECTOR).is_none());
    assert!(cx.debug_bounds(LAST_PLACEHOLDER_SELECTOR).is_some());

    // Only built rows reached the Markdown shaper: one assistant message per
    // built turn and exactly its bytes, nothing for the off-window rows.
    assert_eq!(report.shaped_rows, report.built_rows.len());
    assert_eq!(
        report.shaped_bytes,
        report.built_rows.len() * ASSISTANT_BODY.len()
    );
    assert_eq!(report.over_budget_rows, 0);
}

#[gpui::test]
fn scrolling_moves_the_built_window_to_the_tail(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(long_scene(120), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);

    let handle = cx.update(|_, app| surface.read(app).scroll_handle().clone());
    let maximum = handle.max_offset().y;
    assert!(maximum > px(0.0), "the long fixture must scroll");
    handle.set_offset(point(px(0.0), -maximum));
    // `set_offset` writes the shared scroll state without scheduling a
    // frame; request the repaint the window plan rides on.
    cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
    settle(cx);

    let report = cx.update(|_, app| surface.read(app).transcript_window_report());
    assert_eq!(report.built_rows.last(), Some(&119));
    assert!(!report.built_rows.contains(&0));
    assert!(
        report.built_rows.len() <= TRANSCRIPT_MAX_BUILT_ROWS,
        "the row cap must hold after scrolling, got {}",
        report.built_rows.len()
    );
    assert_eq!(report.shaped_rows, report.built_rows.len());
    assert_eq!(
        report.shaped_bytes,
        report.built_rows.len() * ASSISTANT_BODY.len()
    );

    assert!(cx.debug_bounds(FIRST_TURN_SELECTOR).is_none());
    assert!(cx.debug_bounds(LAST_TURN_SELECTOR).is_some());
}

#[gpui::test]
fn scroll_target_reaches_an_off_window_turn(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(long_scene(120), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);

    // Precondition: the tail row is not built, so the target can only resolve
    // through the forced build path.
    let before = offset(&surface, cx);
    let report = cx.update(|_, app| surface.read(app).transcript_window_report());
    assert!(
        !report.built_rows.contains(&119),
        "precondition: unbuilt tail"
    );

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            assert!(surface.schedule_scroll_target(
                ConversationSurfaceTarget::Scene(scene_id("turn_119")),
                surface_cx,
            ));
        });
    });
    settle(cx);
    settle(cx);

    let after = offset(&surface, cx);
    assert!(
        after.y < before.y,
        "the off-window turn must be reached, before {before:?} after {after:?}"
    );
    let report = cx.update(|_, app| surface.read(app).transcript_window_report());
    assert!(
        report.built_rows.contains(&119),
        "the scrolled-to turn must be built, got {:?}",
        report.built_rows
    );
}

#[gpui::test]
fn message_scroll_targets_reach_off_window_turns(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(long_scene(120), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);

    for (target, selector, turn, down) in [
        (
            "user-119",
            "artisan-conversation-surface-turn-turn_119-block-user-user-119",
            119,
            true,
        ),
        (
            "user-000",
            "artisan-conversation-surface-turn-turn_000-block-user-user-000",
            0,
            false,
        ),
        (
            "assistant-119",
            "artisan-conversation-surface-turn-turn_119-block-assistant-assistant-119",
            119,
            true,
        ),
    ] {
        let before = offset(&surface, cx);
        cx.update(|_, app| {
            assert!(
                !surface
                    .read(app)
                    .transcript_window_report()
                    .built_rows
                    .contains(&turn),
                "target turn must start outside the built window",
            );
            surface.update(app, |surface, surface_cx| {
                assert!(surface.schedule_scroll_target(
                    ConversationSurfaceTarget::Item(ItemId::parse(target).expect("message id")),
                    surface_cx,
                ));
            });
        });
        settle(cx);
        cx.update(|window, app| window.simulate_next_frame(app));
        settle(cx);
        let after = offset(&surface, cx);
        assert!(
            if down {
                after.y < before.y
            } else {
                after.y > before.y
            },
            "{target} must move the viewport from {before:?} to {after:?}",
        );
        let message = cx.debug_bounds(selector).expect("target message is built");
        let viewport = cx
            .debug_bounds(CONVERSATION_VIEWPORT_SELECTOR)
            .expect("viewport paints");
        assert!(
            message.bottom() > viewport.top() && message.top() < viewport.bottom(),
            "{target} must be visible: message {message:?}, viewport {viewport:?}",
        );
        cx.update(|_, app| assert!(surface.read(app).pending_scroll_targets.is_empty()));
    }
}

#[gpui::test]
fn over_budget_bodies_skip_markdown_shaping(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(scene(Vec::new()), ThemeMode::Dark, surface_cx)
    });
    let over_budget = "x".repeat(TRANSCRIPT_MAX_MARKDOWN_BYTES_PER_ROW + 1);
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    cx.update(|_, app| {
        surface.update(app, |surface, _| {
            assert_eq!(
                surface.transcript_shape_ledger(),
                TranscriptShapeLedger::default()
            );
            let _ = surface.render_budgeted_markdown(
                &over_budget,
                &theme,
                "budget-proof".to_owned(),
                MarkdownBodyTone::Muted,
            );
            let ledger = surface.transcript_shape_ledger();
            assert_eq!(ledger.rows, 1);
            assert_eq!(ledger.bytes, TRANSCRIPT_MAX_MARKDOWN_BYTES_PER_ROW + 1);
            assert_eq!(ledger.over_budget_rows, 1);
        });
    });
}

#[gpui::test]
fn markdown_parse_cache_serves_unchanged_bodies_once_and_stays_bounded(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(scene(Vec::new()), ThemeMode::Dark, surface_cx)
    });
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    cx.update(|_, app| {
        surface.update(app, |surface, _| {
            let body = "# heading\n\nA cached reply body.";
            let _ = surface.render_budgeted_markdown(
                body,
                &theme,
                "cache-one".to_owned(),
                MarkdownBodyTone::Foreground,
            );
            let _ = surface.render_budgeted_markdown(
                body,
                &theme,
                "cache-two".to_owned(),
                MarkdownBodyTone::Foreground,
            );
            let report = surface.markdown_parse_report();
            assert_eq!(
                report.parses, 1,
                "an unchanged body parses once: {report:?}"
            );
            assert_eq!(
                report.hits, 1,
                "the repeated frame hits the cache: {report:?}"
            );

            let _ = surface.render_budgeted_markdown(
                "# heading\n\nA revised reply body.",
                &theme,
                "cache-three".to_owned(),
                MarkdownBodyTone::Foreground,
            );
            assert_eq!(
                surface.markdown_parse_report().parses,
                2,
                "a changed body re-parses exactly once"
            );

            for index in 0..(MARKDOWN_PARSE_CACHE_MAX_ENTRIES + 8) {
                let body = format!("distinct cached body {index}");
                let _ = surface.render_budgeted_markdown(
                    &body,
                    &theme,
                    format!("cache-{index}"),
                    MarkdownBodyTone::Muted,
                );
            }
            let report = surface.markdown_parse_report();
            assert_eq!(report.entries, MARKDOWN_PARSE_CACHE_MAX_ENTRIES);
            assert!(report.bytes <= MARKDOWN_PARSE_CACHE_MAX_BYTES);
        });
    });
}

/// Builds a transcript whose replies hold `paragraphs(index)` paragraphs.
fn paragraph_scene(turns: usize, paragraphs: impl Fn(usize) -> usize) -> ConversationScene {
    let mut turn_inputs = Vec::with_capacity(turns);
    let mut items = Vec::with_capacity(turns * 2);
    let mut ordinal: u64 = u64::try_from(turns).expect("bounded turn count");
    for index in 0..turns {
        let id = turn_id(&format!("turn_{index:03}"));
        turn_inputs.push(SceneTurn::new(
            id.clone(),
            u64::try_from(index).expect("bounded turn index"),
            ConversationLifecycle::Completed,
        ));
        items.push(
            SceneItem::new(
                scene_id(&format!("user-{index:03}")),
                id.clone(),
                ordinal,
                SceneItemKind::UserMessage {
                    body: format!("Prompt {index}"),
                },
                None,
            )
            .expect("user message is valid"),
        );
        ordinal = ordinal.saturating_add(1);
        items.push(
            SceneItem::new(
                scene_id(&format!("assistant-{index:03}")),
                id,
                ordinal,
                SceneItemKind::AssistantMessage {
                    body: "A paragraph of the reply.\n\n".repeat(paragraphs(index)),
                    phase: AssistantPhase::Final,
                },
                None,
            )
            .expect("assistant message is valid"),
        );
        ordinal = ordinal.saturating_add(1);
    }
    ConversationScene::build(turn_inputs, items, Vec::new(), Vec::new())
        .expect("paragraph scene is valid")
}

#[test]
fn a_row_taller_than_the_estimate_clamp_keeps_its_exact_height() {
    let mut table = TranscriptHeightTable::default();
    table.scene_replaced(&long_scene(3));
    let tall = 5_816.0_f32;
    assert!(f64::from(tall) > TRANSCRIPT_MAX_ROW_HEIGHT_PX);
    table.record(0, tall);
    table.record(1, 300.0);

    assert_eq!(table.height_px(0), tall);
    assert_eq!(
        table.offsets()[1],
        f64::from(tall) + TRANSCRIPT_TURN_GAP_PX,
        "the next row starts below the whole tall row"
    );
    // The clamp still bounds what the tall row contributes to the estimate
    // for the row that has not been measured.
    assert_eq!(
        f64::from(table.height_px(2)),
        f64::midpoint(TRANSCRIPT_MAX_ROW_HEIGHT_PX, 300.0)
    );
}

#[gpui::test]
fn a_tall_reply_keeps_its_height_as_a_placeholder(cx: &mut TestAppContext) {
    const TALL_TURN: &str = "artisan-conversation-surface-turn-turn_011";
    const TALL_PLACEHOLDER: &str = "artisan-conversation-surface-turn-turn_011-placeholder";
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            paragraph_scene(12, |index| if index == 11 { 120 } else { 1 }),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    cx.update(|_, app| surface.update(app, |surface, cx| surface.scroll_to_bottom(cx)));
    settle(cx);
    let painted = cx
        .debug_bounds(TALL_TURN)
        .expect("the reader at the end builds the last turn")
        .size
        .height;
    assert!(
        f64::from(f32::from(painted)) > TRANSCRIPT_MAX_ROW_HEIGHT_PX,
        "the fixture must outgrow the estimate clamp: {painted:?}"
    );

    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.scroll_handle().set_offset(point(px(0.0), px(0.0)));
            cx.notify();
        });
    });
    settle(cx);
    let placeholder = cx
        .debug_bounds(TALL_PLACEHOLDER)
        .expect("the reader at the top leaves the last turn unbuilt");
    assert_eq!(
        placeholder.size.height, painted,
        "a placeholder occupies exactly the space its row painted"
    );
}

/// The reader sits at the end of a transcript whose earlier replies outgrow
/// the estimate clamp while the last turn is short. The tail-anchored and
/// viewport build windows differ by the first row here, so its placeholder
/// must not change the content height. When it did, the two layouts chased
/// each other and the surface never settled: this test then does not
/// terminate instead of failing.
#[gpui::test]
fn the_reader_at_the_end_of_tall_replies_settles(cx: &mut TestAppContext) {
    const LAST_TURN: &str = "artisan-conversation-surface-turn-turn_005";
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            paragraph_scene(6, |index| match index {
                5 => 2,
                4 => 6,
                _ => 120,
            }),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(1280.0), px(900.0)));
    settle(cx);
    for _ in 0..3 {
        cx.update(|_, app| surface.update(app, |surface, cx| surface.scroll_to_bottom(cx)));
        settle(cx);
        cx.update(|window, app| window.simulate_next_frame(app));
        settle(cx);
    }

    let (offset, max_offset) = cx.update(|_, app| {
        let handle = surface.read(app).scroll_handle().clone();
        (handle.offset().y, handle.max_offset().y)
    });
    assert!(
        (offset + max_offset).abs() < px(0.5),
        "the reader stays at the end: {offset:?} of {max_offset:?}"
    );
    let last = cx
        .debug_bounds(LAST_TURN)
        .expect("the last turn is built at the end");
    assert!(
        last.top() >= px(0.0) && last.bottom() <= px(900.0),
        "the last turn is inside the viewport: {last:?}"
    );
}

fn earlier_turn_requests(
    surface: &Entity<ConversationSurface>,
    cx: &mut VisualTestContext,
) -> usize {
    cx.update(|_, app| {
        surface
            .read(app)
            .pending_actions()
            .iter()
            .filter(|action| matches!(action, ConversationSurfaceAction::EarlierTurnsWanted))
            .count()
    })
}

/// Turns read on demand land above the loaded ones. The offset moves by
/// exactly the height they are planned at, so what the reader is looking at
/// stays where it is.
#[gpui::test]
fn older_turns_added_above_leave_the_reader_in_place(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(turn_range_scene(20..40), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    let handle = cx.update(|_, app| surface.read(app).scroll_handle().clone());
    handle.set_offset(point(px(0.0), px(-300.0)));
    cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
    settle(cx);

    // A turn the reader can see, and where it paints.
    let seen = (20..40)
        .map(|index| format!("artisan-conversation-surface-turn-turn_{index:03}"))
        .find_map(|selector| {
            let selector: &'static str = Box::leak(selector.into_boxed_str());
            cx.debug_bounds(selector)
                .filter(|bounds| bounds.origin.y >= px(0.0))
                .map(|bounds| (selector, bounds.origin.y))
        })
        .expect("a turn paints inside the viewport");
    let before = offset(&surface, cx).y;

    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.replace_scene(turn_range_scene(12..40), cx);
        });
    });
    settle(cx);

    let after = offset(&surface, cx).y;
    assert!(
        after < before - px(8.0 * 32.0),
        "the offset grows by the eight rows added above, got {before:?} -> {after:?}"
    );
    let painted = cx
        .debug_bounds(seen.0)
        .expect("the turn the reader saw still paints");
    assert!(
        (painted.origin.y - seen.1).abs() <= px(0.5),
        "the turn stays where it was: {:?} -> {:?}",
        seen.1,
        painted.origin.y
    );

    // A replacement that adds nothing above moves nothing.
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.replace_scene(turn_range_scene(12..40), cx);
        });
    });
    settle(cx);
    assert_eq!(offset(&surface, cx).y, after);
}

/// A reader within two viewports of the start of the loaded turns asks for
/// older ones, once per window, and only when the thread has some.
#[gpui::test]
fn the_reader_near_the_start_asks_for_older_turns_once_per_window(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(turn_range_scene(20..40), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    // The surface holds a thread's first turn until it is told otherwise.
    assert_eq!(earlier_turn_requests(&surface, cx), 0);

    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.set_earlier_turns_available(true, cx)
        });
    });
    settle(cx);
    assert_eq!(earlier_turn_requests(&surface, cx), 1);
    // Frames that change nothing do not ask again.
    cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
    settle(cx);
    assert_eq!(earlier_turn_requests(&surface, cx), 1);

    // The page arrives. The reader is held in place, now more than two
    // viewports below the new start, so nothing more is asked for.
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.replace_scene(turn_range_scene(4..40), cx);
        });
    });
    settle(cx);
    assert_eq!(earlier_turn_requests(&surface, cx), 1);

    // Scrolling back to the start of the larger window asks for the next.
    let handle = cx.update(|_, app| surface.read(app).scroll_handle().clone());
    handle.set_offset(point(px(0.0), px(0.0)));
    cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
    settle(cx);
    assert_eq!(earlier_turn_requests(&surface, cx), 2);
}

/// Draws every pending frame without the refresh [`settle`] ends with: a
/// refresh re-renders every row, which is exactly what these tests rule out.
fn run_frames(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.run_until_parked();
}

/// Render counts for every row of the long fixture's built window.
fn built_row_renders(
    surface: &Entity<ConversationSurface>,
    cx: &mut VisualTestContext,
) -> Vec<(usize, u64)> {
    cx.update(|_, app| {
        let surface = surface.read(app);
        surface
            .transcript_window_report()
            .built_rows
            .iter()
            .map(|index| {
                let turn = turn_id(&format!("turn_{index:03}"));
                let renders = surface
                    .turn_row_renders(&turn)
                    .expect("every built turn has a row");
                (*index, renders)
            })
            .collect()
    })
}

/// A surface frame that changes nothing a row paints replays every row: the
/// surface shell re-renders, the rows do not.
#[gpui::test]
fn unchanged_rows_replay_while_the_surface_re_renders(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(long_scene(120), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    // One more frame lets every row reach its cached steady state: a row's
    // first cached frame still renders once to record what later replays.
    cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
    run_frames(cx);

    let before = built_row_renders(&surface, cx);
    assert!(before.len() > 1, "the fixture builds several rows");
    for _ in 0..3 {
        cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
        run_frames(cx);
    }
    assert_eq!(
        built_row_renders(&surface, cx),
        before,
        "notifying the surface must not re-render unchanged rows"
    );
    // The report still describes what the built rows paint.
    let report = cx.update(|_, app| surface.read(app).transcript_window_report());
    assert_eq!(report.shaped_rows, report.built_rows.len());
}

/// A changed turn re-renders its own row and no other.
#[gpui::test]
fn a_changed_turn_re_renders_only_its_row(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(long_scene(120), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
    run_frames(cx);
    let before = built_row_renders(&surface, cx);
    let changed = before[1].0;

    // The same transcript with one reply's text edited in place, at the
    // same height so nothing else moves.
    let edited = ConversationScene::build(
        (0..120)
            .map(|index| {
                SceneTurn::new(
                    turn_id(&format!("turn_{index:03}")),
                    u64::try_from(index).expect("bounded turn index"),
                    ConversationLifecycle::Completed,
                )
            })
            .collect(),
        (0..120_usize)
            .flat_map(|index| {
                let id = turn_id(&format!("turn_{index:03}"));
                let base = 120 + 2 * u64::try_from(index).expect("bounded turn index");
                let reply = if index == changed {
                    "A windowed transcript shapes only this edit.\n"
                } else {
                    ASSISTANT_BODY
                };
                [
                    SceneItem::new(
                        scene_id(&format!("user-{index:03}")),
                        id.clone(),
                        base,
                        SceneItemKind::UserMessage {
                            body: format!("Prompt {index}"),
                        },
                        None,
                    )
                    .expect("user message is valid"),
                    SceneItem::new(
                        scene_id(&format!("assistant-{index:03}")),
                        id.clone(),
                        base + 1,
                        SceneItemKind::AssistantMessage {
                            body: reply.to_owned(),
                            phase: AssistantPhase::Final,
                        },
                        None,
                    )
                    .expect("assistant message is valid"),
                ]
            })
            .collect(),
        (0..120)
            .map(|index| {
                TurnNarrationEntry::new(turn_id(&format!("turn_{index:03}")), TurnNarration::Quiet)
            })
            .collect(),
        Vec::new(),
    )
    .expect("edited scene is valid");
    cx.update(|_, app| surface.update(app, |surface, cx| surface.replace_scene(edited, cx)));
    run_frames(cx);

    let after = built_row_renders(&surface, cx);
    for ((index, was), (_, now)) in before.iter().zip(&after) {
        if *index == changed {
            assert!(now > was, "the edited turn {index} must re-render");
        } else {
            assert_eq!(now, was, "turn {index} did not change and must replay");
        }
    }
}

/// The long fixture with its last turn live and thinking: the status line
/// shimmers, asking for a frame every frame.
fn live_tail_scene(turns: usize) -> ConversationScene {
    let mut turn_inputs = Vec::with_capacity(turns);
    let mut items = Vec::with_capacity(turns);
    let mut narrations = Vec::with_capacity(turns);
    let mut ordinal: u64 = u64::try_from(turns).expect("bounded turn count");
    for index in 0..turns {
        let id = turn_id(&format!("turn_{index:03}"));
        let live = index + 1 == turns;
        turn_inputs.push(SceneTurn::new(
            id.clone(),
            u64::try_from(index).expect("bounded turn index"),
            if live {
                ConversationLifecycle::Active
            } else {
                ConversationLifecycle::Completed
            },
        ));
        narrations.push(TurnNarrationEntry::new(
            id.clone(),
            if live {
                TurnNarration::Thinking
            } else {
                TurnNarration::Quiet
            },
        ));
        items.push(
            SceneItem::new(
                scene_id(&format!("user-{index:03}")),
                id,
                ordinal,
                SceneItemKind::UserMessage {
                    body: format!("Prompt {index}"),
                },
                None,
            )
            .expect("user message is valid"),
        );
        ordinal = ordinal.saturating_add(1);
    }
    ConversationScene::build(turn_inputs, items, narrations, Vec::new())
        .expect("live tail scene is valid")
}

/// A live status line animates its own row: every other built row replays
/// while the shimmer asks for frame after frame.
#[gpui::test]
fn a_live_shimmer_re_renders_only_its_own_row(cx: &mut TestAppContext) {
    const TURNS: usize = 30;
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(live_tail_scene(TURNS), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    cx.update(|_, app| surface.update(app, |surface, cx| surface.scroll_to_bottom(cx)));
    settle(cx);
    cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
    run_frames(cx);

    let before = built_row_renders(&surface, cx);
    let live = TURNS - 1;
    assert!(
        before.iter().any(|(index, _)| *index == live),
        "the live turn is built: {before:?}"
    );
    assert!(before.len() > 1, "settled rows are built beside it");
    for _ in 0..3 {
        cx.update(|window, app| window.simulate_next_frame(app));
        run_frames(cx);
    }
    let after = built_row_renders(&surface, cx);
    for ((index, was), (_, now)) in before.iter().zip(&after) {
        if *index == live {
            assert!(now > was, "the shimmering row must keep animating");
        } else {
            assert_eq!(now, was, "settled turn {index} must replay");
        }
    }
}
