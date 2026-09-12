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
    TRANSCRIPT_MAX_BUILT_ROWS, TRANSCRIPT_MAX_MARKDOWN_BYTES_PER_ROW, plan_transcript_window,
    transcript_markdown_within_budget,
};

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
    let mut turn_inputs = Vec::with_capacity(turns);
    let mut items = Vec::with_capacity(turns * 2);
    let mut narrations = Vec::with_capacity(turns);
    // Turn ordinals share the global ordinal namespace with items, so item
    // ordinals continue after every turn ordinal.
    let mut ordinal: u64 = u64::try_from(turns).expect("bounded turn count");
    for index in 0..turns {
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
