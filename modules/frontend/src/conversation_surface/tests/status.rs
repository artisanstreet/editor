use super::*;

#[test]
fn quiet_and_suppressed_narrations_paint_no_status_row() {
    assert_eq!(turn_status_copy(TurnNarration::Quiet), None);
    assert_eq!(turn_status_copy(TurnNarration::StreamingSuppression), None);
    assert!(!status_row_visible(false, TurnNarration::Quiet));
    assert!(!status_row_visible(true, TurnNarration::Quiet));
    assert!(!status_row_visible(
        false,
        TurnNarration::StreamingSuppression
    ));
    assert!(status_row_visible(false, TurnNarration::Thinking));
    assert!(status_row_visible(false, TurnNarration::Working));
    assert!(status_row_visible(false, TurnNarration::ProviderWait));
}

#[test]
fn terminal_duration_prefers_the_group_header() {
    let worked = TurnNarration::WorkedFor { millis: 65_000 };
    let thought = TurnNarration::ThoughtFor { millis: 5_000 };
    assert_eq!(
        turn_status_copy(worked),
        Some("Worked for 1m 5s".to_owned())
    );
    assert!(!status_row_visible(true, worked));
    assert!(!status_row_visible(true, thought));
    assert!(status_row_visible(false, worked));
    assert!(status_row_visible(false, thought));
    assert!(status_row_visible(true, TurnNarration::Failed));
    assert!(status_row_visible(true, TurnNarration::ProviderWait));
}

#[test]
fn live_line_is_owned_by_the_latest_group_once() {
    assert_eq!(
        live_group_header_copy(TurnNarration::Working, Some(0), Some(65_000)),
        Some("Working for 1m 5s".to_owned())
    );
    assert_eq!(
        live_group_header_copy(TurnNarration::Thinking, None, None),
        Some("Thinking".to_owned())
    );
    assert_eq!(
        live_group_header_copy(TurnNarration::ProviderWait, Some(0), Some(5_000)),
        None
    );
    assert_eq!(
        live_group_header_copy(TurnNarration::Failed, None, None),
        None
    );
    assert!(status_row_visible(true, TurnNarration::Thinking));
    assert!(status_row_visible(true, TurnNarration::Working));
    assert!(status_row_visible(false, TurnNarration::Thinking));
    assert!(status_row_visible(false, TurnNarration::Working));
}

#[test]
fn owning_group_index_selects_the_latest_group() {
    let live_scene = ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Active,
        )],
        vec![
            item(
                "user-a",
                1,
                SceneItemKind::UserMessage {
                    body: "hi".to_owned(),
                },
                None,
            ),
            item(
                "work-a",
                2,
                SceneItemKind::Activity {
                    body: "a".to_owned(),
                    kind: None,
                    detail: None,
                },
                None,
            ),
            item(
                "assistant-a",
                3,
                SceneItemKind::AssistantMessage {
                    body: "hello".to_owned(),
                    phase: AssistantPhase::Final,
                },
                None,
            ),
            item(
                "work-b",
                4,
                SceneItemKind::Activity {
                    body: "b".to_owned(),
                    kind: None,
                    detail: None,
                },
                None,
            ),
        ],
        vec![TurnNarrationEntry::new(
            turn_id("turn_a"),
            TurnNarration::Working,
        )],
        Vec::new(),
    )
    .expect("conversation scene is valid");
    let turn = live_scene
        .turn_scene(&turn_id("turn_a"))
        .expect("turn present");
    assert_eq!(owning_group_index(turn), Some(3));
    assert_eq!(
        ordered_block_kinds(&live_scene)[3],
        RenderedBlockKind::WorkGroup
    );
    let empty = scene(Vec::new());
    let empty_turn = empty.turn_scene(&turn_id("turn_a")).expect("turn present");
    assert_eq!(owning_group_index(empty_turn), None);
}

#[test]
fn live_status_formats_the_authoritative_elapsed_basis() {
    assert_eq!(
        live_status_copy(TurnNarration::Thinking, None, Some(1_000)),
        Some("Thinking".to_owned())
    );
    assert_eq!(
        live_status_copy(TurnNarration::Thinking, Some(0), None),
        Some("Thinking".to_owned())
    );
    assert_eq!(
        live_status_copy(TurnNarration::Thinking, Some(0), Some(65_000)),
        Some("Thinking for 1m 5s".to_owned())
    );
    assert_eq!(
        live_status_copy(TurnNarration::Working, Some(10_000), Some(3_000)),
        Some("Working for 0s".to_owned())
    );
    assert_eq!(
        live_status_copy(TurnNarration::Working, Some(0), Some(3_661_000)),
        Some("Working for 1h 1m 1s".to_owned())
    );
    assert_eq!(
        live_status_copy(TurnNarration::Failed, Some(0), Some(5_000)),
        Some("Failed".to_owned())
    );
    assert_eq!(
        live_status_copy(TurnNarration::Quiet, Some(0), Some(5_000)),
        None
    );
    assert_eq!(
        live_status_copy(TurnNarration::StreamingSuppression, Some(0), Some(5_000)),
        None
    );
    assert_eq!(
        live_status_copy(TurnNarration::ProviderWait, Some(0), Some(5_000)),
        Some("Waiting for provider to respond…".to_owned())
    );
    assert_eq!(
        live_status_copy(TurnNarration::ProviderWait, None, Some(5_000)),
        Some("Waiting for provider to respond…".to_owned())
    );
}

#[test]
fn multiline_reasoning_reduces_to_one_headline() {
    assert_eq!(
        status_summary_copy(Some(
            "**First thought**\n\nSome body.\n\n**Planning playful ambiguous response**"
        )),
        Some("Planning playful ambiguous response".to_owned())
    );
    assert_eq!(
        status_summary_copy(Some("Unfinished thought without end")),
        None
    );
    assert_eq!(status_summary_copy(None), None);
}

#[test]
fn provider_wait_copy_names_the_engine_when_known() {
    assert_eq!(
        provider_wait_copy(None),
        "Waiting for provider to respond…".to_owned()
    );
    assert_eq!(
        provider_wait_copy(Some("Claude")),
        "Waiting for Claude to respond…".to_owned()
    );
}

#[test]
fn production_provider_wait_keeps_the_waiting_sentence() {
    // Production derives ProviderWait while no scene fact has arrived;
    // the basis may still ride along, but the row narrates the wait —
    // counting lives in the group header, never in this sentence.
    let scene = ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Active,
        )],
        Vec::new(),
        vec![
            TurnNarrationEntry::new(turn_id("turn_a"), TurnNarration::ProviderWait)
                .with_active_started_at_ms(0),
        ],
        Vec::new(),
    )
    .expect("provider wait may carry the active basis");
    let (narration, basis) = scene
        .turn_scene(&turn_id("turn_a"))
        .expect("turn present")
        .blocks()
        .iter()
        .find_map(|block| match block {
            TurnBlock::TurnStatus(status) => Some((status.narration, status.active_started_at_ms)),
            _ => None,
        })
        .expect("status block present");
    assert_eq!(narration, TurnNarration::ProviderWait);
    assert_eq!(basis, Some(0));
    assert_eq!(
        live_status_copy(narration, basis, Some(65_000)),
        Some("Waiting for provider to respond…".to_owned())
    );
}

#[test]
fn status_copy_and_paint_share_one_decision() {
    // Summary wins over narration; unfinished phases fall back through.
    assert_eq!(
        turn_status_copy_text(
            TurnNarration::Working,
            Some(0),
            Some(65_000),
            Some("**Planning it**"),
            None,
        ),
        Some("Planning it".to_owned())
    );
    assert_eq!(
        turn_status_copy_text(
            TurnNarration::Working,
            Some(0),
            Some(65_000),
            Some("no ending here"),
            None,
        ),
        Some("Working for 1m 5s".to_owned())
    );
    assert_eq!(
        turn_status_copy_text(TurnNarration::Quiet, None, None, None, None),
        None
    );
    // Identical lines paint once; distinct lines both paint.
    assert!(!turn_status_paints(
        true,
        TurnNarration::Working,
        Some("Working for 1m 5s"),
        Some("Working for 1m 5s"),
    ));
    assert!(turn_status_paints(
        true,
        TurnNarration::Working,
        Some("Planning it"),
        Some("Working for 1m 5s"),
    ));
    assert!(turn_status_paints(
        false,
        TurnNarration::Working,
        Some("Working for 1m 5s"),
        None,
    ));
    assert!(!turn_status_paints(
        true,
        TurnNarration::WorkedFor { millis: 1_000 },
        Some("Worked for 1s"),
        None,
    ));
}

#[test]
fn identical_status_and_header_paint_once() {
    assert!(status_duplicates_owner(
        Some("Working for 1m 5s"),
        Some("Working for 1m 5s")
    ));
    assert!(!status_duplicates_owner(
        Some("Thinking for 1m 5s"),
        Some("Working for 1m 5s")
    ));
    assert!(!status_duplicates_owner(Some("Working"), None));
    assert!(!status_duplicates_owner(None, Some("Working for 1m 5s")));
    assert!(!status_duplicates_owner(None, None));
}

#[gpui::test]
fn status_motion_defaults_to_full_with_reduced_override(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(scene(Vec::new()), ThemeMode::Dark, surface_cx)
    });
    cx.update(|_, app| {
        assert_eq!(surface.read(app).status_motion(), MotionPolicy::Full);
    });
    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.set_status_motion(MotionPolicy::Reduced, surface_cx);
        });
    });
    cx.update(|_, app| {
        assert_eq!(surface.read(app).status_motion(), MotionPolicy::Reduced);
    });
}

#[test]
fn effective_status_motion_matrix() {
    use artisan_ui::shimmer_text::{ShimmerMotionPlan, ShimmerText};
    use artisan_ui::theme::ArtisanTheme;
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    // Stored Full follows the live system signal.
    assert_eq!(
        effective_status_motion(MotionPolicy::Full, false),
        MotionPolicy::Full
    );
    assert_eq!(
        effective_status_motion(MotionPolicy::Full, true),
        MotionPolicy::Reduced
    );
    // An explicit Reduced override always wins.
    assert_eq!(
        effective_status_motion(MotionPolicy::Reduced, false),
        MotionPolicy::Reduced
    );
    assert_eq!(
        effective_status_motion(MotionPolicy::Reduced, true),
        MotionPolicy::Reduced
    );
    // Live rows animate only under effective Full; settled rows and
    // reduced motion always resolve to the immediate static path.
    let animate = ShimmerText::new("Thinking", theme, MotionPolicy::Full)
        .active(true)
        .motion_plan();
    assert!(matches!(animate, ShimmerMotionPlan::Animate(_)));
    let still = ShimmerText::new("Thinking", theme, MotionPolicy::Reduced)
        .active(true)
        .motion_plan();
    assert!(matches!(still, ShimmerMotionPlan::Immediate));
    let settled = ShimmerText::new("Worked for 1m 5s", theme, MotionPolicy::Full)
        .active(false)
        .motion_plan();
    assert!(matches!(settled, ShimmerMotionPlan::Immediate));
}

#[gpui::test]
fn status_shimmer_tracks_system_reduced_motion(cx: &mut TestAppContext) {
    let thinking = ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Active,
        )],
        Vec::new(),
        vec![TurnNarrationEntry::new(
            turn_id("turn_a"),
            TurnNarration::Thinking,
        )],
        Vec::new(),
    )
    .expect("conversation scene is valid");
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(thinking, ThemeMode::Dark, surface_cx)
    });
    settle(cx);
    cx.update(|_, app| {
        app.set_reduce_motion(true);
    });
    settle(cx);
    cx.update(|_, app| {
        // The system signal never mutates the stored preference and
        // emits no surface actions on its own: render only reads it.
        assert_eq!(surface.read(app).status_motion(), MotionPolicy::Full);
        assert!(surface.read(app).pending_actions().is_empty());
    });
    cx.update(|_, app| {
        app.set_reduce_motion(false);
    });
    settle(cx);
    cx.update(|_, app| {
        assert_eq!(surface.read(app).status_motion(), MotionPolicy::Full);
        assert!(surface.read(app).pending_actions().is_empty());
    });
}
