use super::{
    NARROW_LOGICAL_WIDTH, ProofSceneCase, case_facts, case_snapshot, navigator_candidates,
    parse_selection, reference_run_id,
};
use crate::conversation_delivery_machine::ConversationDeliveryEvent;
use crate::conversation_scene::{
    ConversationScene, TurnBlock, TurnNarration, WorkGroupBlock, WorkGroupLabel,
};
use crate::conversation_state_machine::{
    ConversationStateController, ConversationStateEvent, SceneFactCommand, SceneFactKind,
};
use crate::conversation_surface::ConversationSurfaceTarget;
use artisan_domain::{ItemId, ThreadId, UnixMillis};

fn thread() -> ThreadId {
    ThreadId::parse("parity-proof-test").expect("fixture thread id is valid")
}

fn now() -> UnixMillis {
    UnixMillis::from_millis(1_700_000_000_000)
}

/// Drives one case through the real aggregate exactly as the runner's
/// `seed_case` does, minus the GPUI surface: snapshot then facts through
/// the delivery-owned path, then the projected scene.
fn project(case: ProofSceneCase) -> ConversationScene {
    let thread = thread();
    let mut controller = ConversationStateController::new(thread.clone());
    let snapshot = case_snapshot(case, &thread, now())
        .expect("snapshot builds")
        .expect("case carries a snapshot");
    controller
        .dispatch(ConversationStateEvent::Delivery(
            ConversationDeliveryEvent::SnapshotReceived(snapshot),
        ))
        .expect("snapshot accepted");
    for fact in case_facts(case).expect("facts build") {
        controller
            .dispatch(ConversationStateEvent::Fact(SceneFactCommand::Register(
                fact,
            )))
            .expect("fact accepted");
    }
    controller.scene().expect("scene projects")
}

/// Returns the run-attributed session group of a single-turn scene.
fn session_group(scene: &ConversationScene) -> &WorkGroupBlock {
    let turn = scene.turn_scenes().first().expect("one turn");
    turn.blocks()
        .iter()
        .find_map(|block| match block {
            TurnBlock::WorkGroup(group) if group.session_run.is_some() => Some(group),
            _ => None,
        })
        .expect("one run-attributed session group")
}

/// Returns the turn status narration of a single-turn scene.
fn status_narration(scene: &ConversationScene) -> TurnNarration {
    let turn = scene.turn_scenes().first().expect("one turn");
    turn.blocks()
        .iter()
        .find_map(|block| match block {
            TurnBlock::TurnStatus(status) => Some(status.narration),
            _ => None,
        })
        .expect("one status row")
}

#[test]
fn empty_case_dispatches_nothing() {
    assert!(
        case_snapshot(ProofSceneCase::Empty, &thread(), now())
            .expect("empty builds")
            .is_none()
    );
    assert!(
        case_facts(ProofSceneCase::Empty)
            .expect("empty facts")
            .is_empty()
    );
}

#[test]
fn completed_case_needs_no_facts() {
    assert!(
        case_snapshot(ProofSceneCase::Completed, &thread(), now())
            .expect("completed builds")
            .is_some()
    );
    assert!(
        case_facts(ProofSceneCase::Completed)
            .expect("completed facts")
            .is_empty()
    );
}

#[test]
fn thinking_case_registers_a_reasoning_fact() {
    let facts = case_facts(ProofSceneCase::Thinking).expect("thinking facts");
    assert_eq!(facts.len(), 1);
    assert!(matches!(facts[0].kind, SceneFactKind::Reasoning { .. }));
}

#[test]
fn working_case_registers_an_activity_fact() {
    let facts = case_facts(ProofSceneCase::Working).expect("working facts");
    assert_eq!(facts.len(), 1);
    assert!(matches!(facts[0].kind, SceneFactKind::Activity { .. }));
}

#[test]
fn streaming_case_comes_from_a_live_item_not_facts() {
    assert!(
        case_snapshot(ProofSceneCase::Streaming, &thread(), now())
            .expect("streaming builds")
            .is_some()
    );
    assert!(
        case_facts(ProofSceneCase::Streaming)
            .expect("streaming facts")
            .is_empty()
    );
}

#[test]
fn error_case_registers_an_error_fact() {
    let facts = case_facts(ProofSceneCase::Error).expect("error facts");
    assert_eq!(facts.len(), 1);
    assert!(matches!(facts[0].kind, SceneFactKind::Error { .. }));
}

#[test]
fn selection_parses_the_exact_cli_pair() {
    let selection = parse_selection(&[
        String::from("--case"),
        String::from("thinking"),
        String::from("--viewport"),
        String::from("narrow"),
    ])
    .expect("exact pair parses");
    assert_eq!(selection.case, ProofSceneCase::Thinking);
    assert_eq!(selection.viewport_slug, "narrow");
    assert_eq!(selection.width.to_bits(), NARROW_LOGICAL_WIDTH.to_bits());
}

#[test]
fn selection_fails_closed_on_anything_else() {
    let bad = [
        vec![],
        vec![String::from("--case")],
        vec![
            String::from("--case"),
            String::from("thinking"),
            String::from("--viewport"),
            String::from("narrow"),
            String::from("extra"),
        ],
        vec![
            String::from("--case"),
            String::from("nope"),
            String::from("--viewport"),
            String::from("narrow"),
        ],
        vec![
            String::from("--case"),
            String::from("thinking"),
            String::from("--viewport"),
            String::from("huge"),
        ],
        vec![
            String::from("--viewport"),
            String::from("narrow"),
            String::from("--case"),
            String::from("thinking"),
        ],
    ];
    for args in bad {
        assert!(
            parse_selection(&args).is_err(),
            "must fail closed, got {args:?}"
        );
    }
}

#[test]
fn every_slug_round_trips_through_parse() {
    for case in ProofSceneCase::all() {
        assert_eq!(ProofSceneCase::parse(case.slug()), Some(case));
    }
    assert_eq!(ProofSceneCase::parse("bogus"), None);
}

#[test]
fn reference_settled_projects_session_with_thought_for_six_seconds() {
    let scene = project(ProofSceneCase::ReferenceSettled);
    let group = session_group(&scene);
    assert!(
        matches!(&group.session_run, Some(run) if *run == reference_run_id()),
        "session group carries the attributed run"
    );
    assert!(group.session.is_some(), "session carries its anchor");
    assert!(
        matches!(
            group.label,
            Some(WorkGroupLabel::ThoughtFor { millis: 6_000 })
        ),
        "settled span is six seconds"
    );
    assert!(
        group.reasoning_summary.is_none(),
        "settled rows carry no live summary line"
    );
    assert_eq!(
        status_narration(&scene),
        TurnNarration::ThoughtFor { millis: 6_000 }
    );
    let reply = scene
        .turn_scenes()
        .first()
        .expect("one turn")
        .blocks()
        .iter()
        .find_map(|block| match block {
            TurnBlock::AssistantMessage(message) => Some(message.body.clone()),
            _ => None,
        });
    assert_eq!(reply.as_deref(), Some("Whoopty! \u{1F604} Whats up?"));
}

#[test]
fn reference_thinking_projects_live_markdown_summary() {
    let scene = project(ProofSceneCase::ReferenceThinking);
    let group = session_group(&scene);
    assert!(
        matches!(&group.session_run, Some(run) if *run == reference_run_id()),
        "session group carries the attributed run"
    );
    assert_eq!(
        group.reasoning_summary.as_deref(),
        Some("Checking `mood` for **playful** *tone* before replying.")
    );
    assert_eq!(status_narration(&scene), TurnNarration::Thinking);
}

#[test]
fn reference_navigator_has_two_turns_and_many_markers() {
    let scene = project(ProofSceneCase::ReferenceNavigator);
    assert_eq!(scene.turn_scenes().len(), 2);
    let user =
        |id: &str| ConversationSurfaceTarget::Item(ItemId::parse(id).expect("item id parses"));
    assert_eq!(
        navigator_candidates(&scene),
        vec![user("parity-proof-user-1"), user("parity-proof-user-4")],
        "exactly the two user markers in transcript order"
    );
}

#[test]
fn reference_settled_stays_single_turn() {
    let scene = project(ProofSceneCase::ReferenceSettled);
    assert_eq!(scene.turn_scenes().len(), 1);
}

#[test]
fn single_user_message_yields_one_collapsed_candidate() {
    let scene = project(ProofSceneCase::ReferenceSettled);
    let user = ItemId::parse("parity-proof-user-1").expect("item id parses");
    assert_eq!(
        navigator_candidates(&scene),
        vec![ConversationSurfaceTarget::Item(user)],
        "one user marker is the collapsed rail the capture refuses"
    );
}

#[test]
fn reference_cases_share_one_attributed_run() {
    for case in [
        ProofSceneCase::ReferenceSettled,
        ProofSceneCase::ReferenceThinking,
    ] {
        assert!(
            case_snapshot(case, &thread(), now())
                .expect("snapshot builds")
                .is_some()
        );
        assert!(
            !case_facts(case).expect("facts build").is_empty(),
            "case {} must attribute its session facts",
            case.slug()
        );
    }
}

#[test]
fn longform_case_carries_markdown_and_one_attachment() {
    let snapshot = case_snapshot(ProofSceneCase::Longform, &thread(), now())
        .expect("longform builds")
        .expect("longform snapshot builds");
    assert!(
        case_facts(ProofSceneCase::Longform)
            .expect("longform facts")
            .is_empty()
    );
    let debug = format!("{snapshot:?}");
    assert!(
        debug.contains("MultimodalUserMessage"),
        "longform user item must be multimodal, got {debug}"
    );
}

#[test]
fn capture_pixel_dimensions_reject_invalid_values() {
    assert_eq!(super::physical_pixels(1024.0, 1.5), Some(1536));
    for value in [f32::NAN, f32::INFINITY, -1.0, 0.0, f32::MAX] {
        assert_eq!(super::physical_pixels(value, 1.0), None);
    }
}
