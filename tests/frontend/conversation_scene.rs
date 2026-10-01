//! Black-box tests for the pure conversation render scene.
//!
//! Structural enum assertions only: no string snapshots of debug output.

use artisan_domain::{
    ConversationLifecycle, ItemId, MESSAGE_BODY_MAX_BYTES, MessageBody, ObservationId, RunId,
    TurnId,
};
use artisan_frontend::conversation_scene;
use artisan_frontend::conversation_scene::{
    AssistantPhase, FileChangeStatus, ItemProvenance, ProgressPhase,
    SCENE_MAX_CHANGED_FILES_PER_CARD, SCENE_MAX_DISPLAY_PATH_BYTES, SCENE_MAX_ITEMS,
    SCENE_MAX_MESSAGE_BODY_BYTES, SCENE_MAX_NARRATIONS, SCENE_MAX_NATIVE_FACT_BYTES,
    SCENE_MAX_PLAN_ENTRIES, SCENE_MAX_STEERING_PLACEMENTS, SCENE_MAX_TURNS,
    SCENE_MAX_WORK_GROUP_ITEMS, SceneBuildError, SceneDisclosure, SceneFileChange, SceneId,
    SceneItem, SceneItemKind, SceneTurn, SessionDetail, SteeringPlacement, TurnBlock,
    TurnNarration, TurnNarrationEntry, WorkGroupBlock, WorkGroupLabel, WorkItem,
};

fn scene_id(value: &str) -> SceneId {
    SceneId::parse(value).expect("scene id valid")
}

fn turn_id(value: &str) -> TurnId {
    TurnId::parse(value).expect("turn id valid")
}

fn item_id(value: &str) -> ItemId {
    ItemId::parse(value).expect("item id valid")
}

fn scene_turn(id: &str, ordinal: u64, lifecycle: ConversationLifecycle) -> SceneTurn {
    SceneTurn::new(turn_id(id), ordinal, lifecycle)
}

fn user_item(id: &str, turn: &str, ordinal: u64, body: &str) -> SceneItem {
    SceneItem::new(
        item_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::UserMessage {
            body: body.to_owned(),
        },
        None,
    )
    .expect("user item valid")
}

fn assistant_item(
    id: &str,
    turn: &str,
    ordinal: u64,
    body: &str,
    phase: AssistantPhase,
) -> SceneItem {
    SceneItem::new(
        item_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::AssistantMessage {
            body: body.to_owned(),
            phase,
        },
        None,
    )
    .expect("assistant item valid")
}

fn run_id(value: &str) -> RunId {
    RunId::parse(value).expect("run id valid")
}

fn provenance(run: &str, lifecycle: ConversationLifecycle) -> ItemProvenance {
    ItemProvenance {
        run_id: Some(run_id(run)),
        lifecycle: Some(lifecycle),
    }
}

/// Attaches run/lifecycle provenance to one scene item.
fn provenanced(item: SceneItem, run: &str, lifecycle: ConversationLifecycle) -> SceneItem {
    item.with_provenance(provenance(run, lifecycle))
}

fn reasoning_item(id: &str, turn: &str, ordinal: u64, body: &str) -> SceneItem {
    SceneItem::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::ReasoningSummary {
            body: body.to_owned(),
        },
        None,
    )
    .expect("reasoning item valid")
}

fn activity_item(id: &str, turn: &str, ordinal: u64, body: &str) -> SceneItem {
    SceneItem::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::Activity {
            body: body.to_owned(),
            kind: None,
            detail: None,
        },
        None,
    )
    .expect("activity item valid")
}

fn work_session_item(id: &str, turn: &str, ordinal: u64, title: &str) -> SceneItem {
    SceneItem::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::WorkSession {
            title: title.to_owned(),
        },
        None,
    )
    .expect("work session item valid")
}

fn compaction_item(id: &str, turn: &str, ordinal: u64, summary: &str) -> SceneItem {
    SceneItem::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::Compaction {
            summary: summary.to_owned(),
        },
        None,
    )
    .expect("compaction item valid")
}

fn change_set_item(id: &str, turn: &str, ordinal: u64, files: Vec<SceneFileChange>) -> SceneItem {
    SceneItem::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::ChangeSet { files },
        None,
    )
    .expect("change-set item valid")
}

fn file_change_item(id: &str, turn: &str, ordinal: u64, path: &str) -> SceneItem {
    SceneItem::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::FileChange {
            file: SceneFileChange::new(path, FileChangeStatus::Modified).expect("path valid"),
        },
        None,
    )
    .expect("file-change item valid")
}

fn plan_item(id: &str, turn: &str, ordinal: u64) -> SceneItem {
    SceneItem::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::Plan {
            title: "plan".to_owned(),
            entries: vec!["a".to_owned()],
        },
        None,
    )
    .expect("plan item valid")
}

fn approval_item(id: &str, turn: &str, ordinal: u64) -> SceneItem {
    SceneItem::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::Approval {
            prompt: "approve?".to_owned(),
            approval_id: ObservationId::parse(id).expect("approval id"),
        },
        None,
    )
    .expect("approval item valid")
}

fn question_item(id: &str, turn: &str, ordinal: u64) -> SceneItem {
    SceneItem::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::Question {
            prompt: "question?".to_owned(),
            answer: None,
        },
        None,
    )
    .expect("question item valid")
}

fn error_item(id: &str, turn: &str, ordinal: u64) -> SceneItem {
    SceneItem::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::Error {
            message: "boom".to_owned(),
        },
        None,
    )
    .expect("error item valid")
}

fn native_fact_item(id: &str, turn: &str, ordinal: u64, text: &str) -> SceneItem {
    SceneItem::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneItemKind::NativeFact {
            text: text.to_owned(),
        },
        None,
    )
    .expect("native fact item valid")
}

fn narration(turn: &str, narration: TurnNarration) -> TurnNarrationEntry {
    TurnNarrationEntry::new(turn_id(turn), narration)
}

fn steering(id: &str, anchor: &str, label: &str) -> SteeringPlacement {
    SteeringPlacement::new(scene_id(id), item_id(anchor), label.to_owned()).expect("steering valid")
}

// ---- 1. empty and message-only scenes preserve canonical turn/item order ----

#[test]
fn empty_conversation_is_representable_without_dummy_messages() {
    use conversation_scene::ConversationScene;

    let scene = ConversationScene::build(Vec::new(), Vec::new(), Vec::new(), Vec::new())
        .expect("empty scene builds");
    assert!(scene.turn_scenes().is_empty());
    assert!(scene.deferred_change_sets().is_empty());
}

#[test]
fn partially_loaded_turn_without_items_still_has_status_and_footer() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)];
    let scene = ConversationScene::build(
        turns,
        Vec::new(),
        vec![narration("turn_a", TurnNarration::Quiet)],
        Vec::new(),
    )
    .expect("partial turn builds");
    let ts = &scene.turn_scenes()[0];
    assert_eq!(ts.blocks.len(), 2);
    assert!(matches!(&ts.blocks[0], TurnBlock::TurnStatus(_)));
    assert!(matches!(&ts.blocks[1], TurnBlock::TurnFooter(_)));
}

#[test]
fn message_only_scenes_preserve_canonical_turn_and_item_order() {
    use conversation_scene::ConversationScene;

    let turns = vec![
        scene_turn("turn_b", 1, ConversationLifecycle::Pending),
        scene_turn("turn_a", 0, ConversationLifecycle::Pending),
    ];
    let items = vec![
        user_item("item_user_b", "turn_b", 4, "b"),
        user_item("item_user_a", "turn_a", 2, "a"),
        assistant_item("item_assist_a", "turn_a", 3, "reply", AssistantPhase::Final),
    ];
    let scene = ConversationScene::build(turns, items, Vec::new(), Vec::new()).expect("builds");
    assert_eq!(scene.turn_scenes()[0].turn_id.as_str(), "turn_a");
    assert_eq!(scene.turn_scenes()[1].turn_id.as_str(), "turn_b");
    let blocks = &scene.turn_scenes()[0].blocks;
    // first block is user message, second is assistant message
    assert!(matches!(&blocks[0], TurnBlock::UserMessage(_)));
    assert!(matches!(&blocks[1], TurnBlock::AssistantMessage(_)));
}

#[test]
fn message_bodies_use_the_domain_ceiling_without_truncation() {
    use conversation_scene::ConversationScene;

    assert_eq!(SCENE_MAX_MESSAGE_BODY_BYTES, MESSAGE_BODY_MAX_BYTES);
    let max_body = "x".repeat(MESSAGE_BODY_MAX_BYTES);
    assert!(MessageBody::parse(max_body.clone()).is_ok());

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)];
    let items = vec![
        user_item("user_a", "turn_a", 1, &max_body),
        assistant_item("assistant_a", "turn_a", 2, &max_body, AssistantPhase::Final),
    ];
    let scene = ConversationScene::build(turns, items, Vec::new(), Vec::new())
        .expect("maximum domain-sized messages build");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(matches!(
        &blocks[0],
        TurnBlock::UserMessage(message) if message.body.len() == MESSAGE_BODY_MAX_BYTES
    ));
    assert!(matches!(
        &blocks[1],
        TurnBlock::AssistantMessage(message) if message.body.len() == MESSAGE_BODY_MAX_BYTES
    ));

    let too_long = "x".repeat(MESSAGE_BODY_MAX_BYTES + 1);
    let err = SceneItem::new(
        item_id("too_long"),
        turn_id("turn_a"),
        3,
        SceneItemKind::UserMessage { body: too_long },
        None,
    )
    .expect_err("message body above the domain ceiling is refused");
    assert!(matches!(err, SceneBuildError::MessageBodyTooLong { .. }));
}

// ---- 2. thinking/reasoning/activity/work items group without swallowing final reply ----

#[test]
fn reasoning_activity_work_session_coalesce_into_one_group_before_final_reply() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)];
    let items = vec![
        reasoning_item("r1", "turn_a", 1, "thinking"),
        activity_item("a1", "turn_a", 2, "tool"),
        work_session_item("w1", "turn_a", 3, "work"),
        assistant_item("assist", "turn_a", 4, "final", AssistantPhase::Final),
    ];
    let scene = ConversationScene::build(turns, items, Vec::new(), Vec::new()).expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    // Expect: WorkGroup, AssistantMessage, TurnStatus, TurnFooter
    assert!(matches!(&blocks[0], TurnBlock::WorkGroup(_)));
    if let TurnBlock::WorkGroup(group) = &blocks[0] {
        assert_eq!(group.items.len(), 3);
        assert!(matches!(&group.items[0], WorkItem::Reasoning { .. }));
        assert!(matches!(&group.items[1], WorkItem::Activity { .. }));
        assert!(matches!(&group.items[2], WorkItem::WorkSession { .. }));
    }
    assert!(matches!(&blocks[1], TurnBlock::AssistantMessage(_)));
    // final reply not swallowed
    assert!(
        matches!(&blocks[1], TurnBlock::AssistantMessage(b) if b.phase == AssistantPhase::Final)
    );
}

#[test]
fn messages_break_work_groups() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)];
    let items = vec![
        reasoning_item("r1", "turn_a", 1, "r"),
        user_item("u1", "turn_a", 2, "user breaks"),
        reasoning_item("r2", "turn_a", 3, "r2"),
    ];
    let scene = ConversationScene::build(
        turns,
        items,
        vec![narration("turn_a", TurnNarration::WorkedFor { millis: 9 })],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    // WorkGroup, UserMessage, WorkGroup, Status, Footer
    assert!(matches!(&blocks[0], TurnBlock::WorkGroup(_)));
    assert!(matches!(&blocks[1], TurnBlock::UserMessage(_)));
    assert!(matches!(&blocks[2], TurnBlock::WorkGroup(_)));
    assert_eq!(
        blocks
            .iter()
            .filter(|block| matches!(block, TurnBlock::WorkGroup(group) if group.label.is_some()))
            .count(),
        1
    );
    assert!(matches!(
        &blocks[2],
        TurnBlock::WorkGroup(WorkGroupBlock {
            label: Some(WorkGroupLabel::WorkedFor { millis: 9 }),
            ..
        })
    ));
}

// ---- 3. active compaction has one exact card/narration and no generic duplicate ----

#[test]
fn active_compaction_has_one_card_and_compacting_narration_no_duplicate() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)];
    let items = vec![compaction_item("c1", "turn_a", 1, "compacting...")];
    let narrations = vec![narration("turn_a", TurnNarration::Compacting)];
    let scene = ConversationScene::build(turns, items, narrations, Vec::new()).expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    let compaction_count = blocks
        .iter()
        .filter(|b| matches!(b, TurnBlock::Compaction(_)))
        .count();
    let status_count = blocks
        .iter()
        .filter(|b| matches!(b, TurnBlock::TurnStatus(_)))
        .count();
    assert_eq!(compaction_count, 1);
    assert_eq!(status_count, 1);
    assert!(matches!(
        blocks.iter().find(|b| matches!(b, TurnBlock::TurnStatus(_))),
        Some(TurnBlock::TurnStatus(s)) if s.narration == TurnNarration::Compacting
    ));
}

#[test]
fn compaction_cannot_be_paired_with_generic_thinking_or_working_status() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)];
    let items = vec![compaction_item("compact", "turn_a", 1, "summary")];
    let err = ConversationScene::build(
        turns,
        items,
        vec![narration("turn_a", TurnNarration::Thinking)],
        Vec::new(),
    )
    .expect_err("generic active narration conflicts with compaction");
    assert!(matches!(
        err,
        SceneBuildError::CompactionNarrationConflict {
            narration: TurnNarration::Thinking
        }
    ));
}

// ---- 4. streaming reply suppresses only quiet status row ----

#[test]
fn streaming_reply_suppresses_quiet_status_row_only() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Streaming)];
    let items = vec![
        provenanced(
            reasoning_item("r1", "turn_a", 1, "reasoning"),
            "run_a",
            ConversationLifecycle::Active,
        ),
        provenanced(
            assistant_item(
                "assist",
                "turn_a",
                2,
                "partial",
                AssistantPhase::Unspecified,
            ),
            "run_a",
            ConversationLifecycle::Streaming,
        ),
    ];
    let narrations = vec![narration("turn_a", TurnNarration::StreamingSuppression)];
    let scene = ConversationScene::build(turns, items, narrations, Vec::new()).expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    // Session group present with the reply top-level, but no status row.
    assert!(blocks.iter().any(|b| matches!(
        b,
        TurnBlock::WorkGroup(group) if group.session.is_some()
    )));
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, TurnBlock::AssistantMessage(_)))
    );
    assert!(!blocks.iter().any(|b| matches!(b, TurnBlock::TurnStatus(_))));
    // footer still present
    assert!(blocks.iter().any(|b| matches!(b, TurnBlock::TurnFooter(_))));
}

#[test]
fn streaming_reply_does_not_remove_message_or_work_group_when_not_suppressing() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Streaming)];
    let items = vec![
        activity_item("a1", "turn_a", 1, "tool"),
        provenanced(
            assistant_item(
                "assist",
                "turn_a",
                2,
                "partial",
                AssistantPhase::Unspecified,
            ),
            "run_a",
            ConversationLifecycle::Streaming,
        ),
    ];
    // narration is Quiet, not suppression – status should remain
    let narrations = vec![narration("turn_a", TurnNarration::Quiet)];
    let scene = ConversationScene::build(turns, items, narrations, Vec::new()).expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(blocks.iter().any(|b| matches!(
        b,
        TurnBlock::WorkGroup(group) if group.session.is_some()
    )));
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, TurnBlock::AssistantMessage(_)))
    );
    assert!(blocks.iter().any(|b| matches!(b, TurnBlock::TurnStatus(_))));
}

// ---- 5. change cards defer while active and appear in pinned terminal order ----

#[test]
fn change_cards_defer_while_active_and_appear_in_pinned_order_when_settled() {
    use conversation_scene::ConversationScene;

    // Active turn: change card deferred
    let turns_active = vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)];
    let items_active = vec![
        assistant_item("assist", "turn_a", 1, "reply", AssistantPhase::Final),
        change_set_item(
            "cs1",
            "turn_a",
            2,
            vec![SceneFileChange::new("a.txt", FileChangeStatus::Modified).unwrap()],
        ),
    ];
    let scene_active = ConversationScene::build(turns_active, items_active, Vec::new(), Vec::new())
        .expect("builds");
    assert_eq!(scene_active.deferred_change_sets().len(), 1);
    assert!(
        !scene_active.turn_scenes()[0]
            .blocks
            .iter()
            .any(|b| matches!(b, TurnBlock::ChangeSet(_)))
    );

    // Settled turn: change card appears in terminal order assistant -> change -> status -> footer
    let turns_settled = vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)];
    let items_settled = vec![
        assistant_item("assist", "turn_a", 1, "reply", AssistantPhase::Final),
        change_set_item(
            "cs1",
            "turn_a",
            2,
            vec![SceneFileChange::new("a.txt", FileChangeStatus::Modified).unwrap()],
        ),
    ];
    let narrations = vec![narration(
        "turn_a",
        TurnNarration::WorkedFor { millis: 100 },
    )];
    let scene_settled =
        ConversationScene::build(turns_settled, items_settled, narrations, Vec::new())
            .expect("builds");
    assert!(scene_settled.deferred_change_sets().is_empty());
    let blocks = &scene_settled.turn_scenes()[0].blocks;
    let idx_assist = blocks
        .iter()
        .position(|b| matches!(b, TurnBlock::AssistantMessage(_)))
        .expect("assistant");
    let idx_change = blocks
        .iter()
        .position(|b| matches!(b, TurnBlock::ChangeSet(_)))
        .expect("change");
    let idx_status = blocks
        .iter()
        .position(|b| matches!(b, TurnBlock::TurnStatus(_)))
        .expect("status");
    let idx_footer = blocks
        .iter()
        .position(|b| matches!(b, TurnBlock::TurnFooter(_)))
        .expect("footer");
    assert!(idx_assist < idx_change);
    assert!(idx_change < idx_status);
    assert!(idx_status < idx_footer);
}

#[test]
fn changed_files_break_work_groups_and_interrupted_turns_still_defer_them() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)];
    let items = vec![
        reasoning_item("before", "turn_a", 1, "before"),
        file_change_item("file", "turn_a", 2, "changed.rs"),
        reasoning_item("after", "turn_a", 3, "after"),
    ];
    let scene = ConversationScene::build(turns, items, Vec::new(), Vec::new()).expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(matches!(&blocks[0], TurnBlock::WorkGroup(_)));
    assert!(matches!(&blocks[1], TurnBlock::WorkGroup(_)));
    assert_eq!(scene.deferred_change_sets().len(), 1);
    assert!(
        !blocks
            .iter()
            .any(|block| matches!(block, TurnBlock::ChangeSet(_)))
    );

    let interrupted = ConversationScene::build(
        vec![scene_turn("turn_i", 0, ConversationLifecycle::Interrupted)],
        vec![file_change_item("file", "turn_i", 1, "changed.rs")],
        Vec::new(),
        Vec::new(),
    )
    .expect("interrupted scene builds");
    assert_eq!(interrupted.deferred_change_sets().len(), 1);
    assert!(
        !interrupted.turn_scenes()[0]
            .blocks
            .iter()
            .any(|block| matches!(block, TurnBlock::ChangeSet(_)))
    );
}

#[test]
fn terminal_change_card_follows_interactive_cards_before_status_and_footer() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)];
    let items = vec![
        assistant_item("reply", "turn_a", 1, "done", AssistantPhase::Final),
        approval_item("approval", "turn_a", 2),
        file_change_item("file", "turn_a", 3, "changed.rs"),
    ];
    let scene = ConversationScene::build(turns, items, Vec::new(), Vec::new()).expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    let kinds: Vec<&str> = blocks
        .iter()
        .map(|block| match block {
            TurnBlock::AssistantMessage(_) => "assistant",
            TurnBlock::Approval(_) => "approval",
            TurnBlock::ChangeSet(_) => "change",
            TurnBlock::TurnStatus(_) => "status",
            TurnBlock::TurnFooter(_) => "footer",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["assistant", "approval", "change", "status", "footer"]
    );
}

// ---- 6. completed reasoning-only work renders Thought for, ordinary work renders Worked for, never both ----

#[test]
fn reasoning_only_work_renders_thought_for_ordinary_renders_worked_for_never_both() {
    use conversation_scene::ConversationScene;

    // reasoning-only
    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)];
    let items = vec![
        reasoning_item("r1", "turn_a", 1, "r1"),
        reasoning_item("r2", "turn_a", 2, "r2"),
    ];
    let narrations = vec![narration(
        "turn_a",
        TurnNarration::ThoughtFor { millis: 123 },
    )];
    let scene = ConversationScene::build(turns, items, narrations, Vec::new()).expect("builds");
    let block = scene.turn_scenes()[0]
        .blocks
        .iter()
        .find(|b| matches!(b, TurnBlock::WorkGroup(_)))
        .expect("work group");
    if let TurnBlock::WorkGroup(g) = block {
        assert!(
            matches!(g.label, Some(l) if matches!(l, WorkGroupLabel::ThoughtFor { millis: 123 }))
        );
        assert!(!matches!(g.label, Some(WorkGroupLabel::WorkedFor { .. })));
    }

    // ordinary work (activity + reasoning mixed)
    let turns2 = vec![scene_turn("turn_b", 2, ConversationLifecycle::Completed)];
    let items2 = vec![
        reasoning_item("r1", "turn_b", 3, "r1"),
        activity_item("a1", "turn_b", 4, "tool"),
    ];
    let narrations2 = vec![narration(
        "turn_b",
        TurnNarration::WorkedFor { millis: 456 },
    )];
    let scene2 = ConversationScene::build(turns2, items2, narrations2, Vec::new()).expect("builds");
    let block2 = scene2.turn_scenes()[0]
        .blocks
        .iter()
        .find(|b| matches!(b, TurnBlock::WorkGroup(_)))
        .expect("work group");
    if let TurnBlock::WorkGroup(g) = block2 {
        assert!(
            matches!(g.label, Some(l) if matches!(l, WorkGroupLabel::WorkedFor { millis: 456 }))
        );
    }

    // never both in same group – enforced by enum single variant
    // also across scene, each group has exactly one label variant
}

// ---- 7. two steering labels remain immediately after two exact anchors ----

#[test]
fn two_steering_labels_remain_immediately_after_exact_anchors() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)];
    let items = vec![
        user_item("user_a", "turn_a", 1, "first"),
        user_item("user_b", "turn_a", 2, "second"),
    ];
    let steerings = vec![
        steering("steer_a", "user_a", "steer one"),
        steering("steer_b", "user_b", "steer two"),
    ];
    let scene = ConversationScene::build(turns, items, Vec::new(), steerings).expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    // Find positions
    let pos_user_a = blocks
        .iter()
        .position(|b| matches!(b, TurnBlock::UserMessage(m) if m.id.as_str() == "user_a"))
        .expect("user_a");
    let pos_steer_a = blocks
        .iter()
        .position(|b| matches!(b, TurnBlock::SteeringLabel(s) if s.id.as_str() == "steer_a"))
        .expect("steer_a");
    let pos_user_b = blocks
        .iter()
        .position(|b| matches!(b, TurnBlock::UserMessage(m) if m.id.as_str() == "user_b"))
        .expect("user_b");
    let pos_steer_b = blocks
        .iter()
        .position(|b| matches!(b, TurnBlock::SteeringLabel(s) if s.id.as_str() == "steer_b"))
        .expect("steer_b");
    assert_eq!(pos_steer_a, pos_user_a + 1);
    assert_eq!(pos_steer_b, pos_user_b + 1);
}

// ---- 8. unknown/non-user steering anchors are refused ----

#[test]
fn unknown_steering_anchor_is_refused() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)];
    let items = vec![user_item("user_a", "turn_a", 1, "hello")];
    let steerings = vec![steering("steer_x", "missing_id", "label")];
    let err =
        ConversationScene::build(turns, items, Vec::new(), steerings).expect_err("must refuse");
    assert!(matches!(err, SceneBuildError::UnknownSteeringAnchor { .. }));
}

#[test]
fn non_user_steering_anchor_is_refused() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)];
    let items = vec![assistant_item(
        "assist_a",
        "turn_a",
        1,
        "reply",
        AssistantPhase::Final,
    )];
    // Anchor is assistant, not user
    let steerings = vec![steering("steer_x", "assist_a", "label")];
    let err =
        ConversationScene::build(turns, items, Vec::new(), steerings).expect_err("must refuse");
    assert!(matches!(err, SceneBuildError::NonUserSteeringAnchor { .. }));
}

// ---- 9. disclosure is attached to correct group/card ----

#[test]
fn disclosure_is_attached_to_correct_group_and_card() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)];
    let mut r = reasoning_item("r1", "turn_a", 1, "thought");
    r.disclosure = Some(SceneDisclosure::Open);
    let mut plan = plan_item("plan1", "turn_a", 2);
    plan.disclosure = Some(SceneDisclosure::Closed);
    let narrations = vec![narration(
        "turn_a",
        TurnNarration::ThoughtFor { millis: 10 },
    )];
    let scene =
        ConversationScene::build(turns, vec![r, plan], narrations, Vec::new()).expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    let work = blocks
        .iter()
        .find(|b| matches!(b, TurnBlock::WorkGroup(_)))
        .expect("work group");
    if let TurnBlock::WorkGroup(g) = work {
        // group disclosure is from first work item or group-level
        // our implementation copies work item disclosure into items; group disclosure taken from first
        assert_eq!(
            g.items[0],
            WorkItem::Reasoning {
                id: scene_id("r1"),
                body: "thought".to_owned(),
                disclosure: Some(SceneDisclosure::Open)
            }
        );
    }
    let plan_block = blocks
        .iter()
        .find(|b| matches!(b, TurnBlock::Plan(_)))
        .expect("plan");
    if let TurnBlock::Plan(p) = plan_block {
        assert_eq!(p.disclosure, Some(SceneDisclosure::Closed));
    }
}

// ---- 10. duplicate identities/ordinals, unknown turns, boundary overflows are typed failures ----

#[test]
fn duplicate_turn_id_is_typed_error() {
    use conversation_scene::ConversationScene;

    let turns = vec![
        scene_turn("turn_a", 0, ConversationLifecycle::Pending),
        scene_turn("turn_a", 1, ConversationLifecycle::Pending),
    ];
    let err = ConversationScene::build(turns, Vec::new(), Vec::new(), Vec::new())
        .expect_err("duplicate turn id");
    assert!(matches!(err, SceneBuildError::DuplicateTurnId { .. }));
}

#[test]
fn duplicate_item_id_is_typed_error() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)];
    let items = vec![
        user_item("dup", "turn_a", 1, "a"),
        user_item("dup", "turn_a", 2, "b"),
    ];
    let err = ConversationScene::build(turns, items, Vec::new(), Vec::new())
        .expect_err("duplicate item id");
    assert!(matches!(err, SceneBuildError::DuplicateItemId { .. }));
}

#[test]
fn duplicate_steering_id_is_typed_error() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)];
    let items = vec![user_item("user_a", "turn_a", 1, "hello")];
    let steerings = vec![
        steering("duplicate", "user_a", "one"),
        steering("duplicate", "user_a", "two"),
    ];
    let err = ConversationScene::build(turns, items, Vec::new(), steerings)
        .expect_err("duplicate steering id");
    assert!(matches!(err, SceneBuildError::DuplicateSteeringId { .. }));
}

#[test]
fn duplicate_ordinal_is_typed_error() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)];
    let items = vec![
        user_item("a", "turn_a", 1, "a"),
        user_item("b", "turn_a", 1, "b"),
    ];
    let err = ConversationScene::build(turns, items, Vec::new(), Vec::new())
        .expect_err("duplicate ordinal");
    assert!(matches!(err, SceneBuildError::DuplicateOrdinal { .. }));
}

#[test]
fn unknown_turn_is_typed_error() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)];
    let items = vec![user_item("u1", "turn_missing", 1, "hi")];
    let err =
        ConversationScene::build(turns, items, Vec::new(), Vec::new()).expect_err("unknown turn");
    assert!(matches!(err, SceneBuildError::UnknownTurn { .. }));
}

#[test]
fn scene_items_overflow_is_typed_error() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)];
    let items: Vec<SceneItem> = (0..=SCENE_MAX_ITEMS)
        .map(|i| user_item(&format!("id_{i}"), "turn_a", i as u64 + 1, "hi"))
        .collect();
    let err = ConversationScene::build(turns, items, Vec::new(), Vec::new()).expect_err("overflow");
    assert!(matches!(err, SceneBuildError::TooManyItems { .. }));
}

#[test]
fn work_group_overflow_is_typed_error() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)];
    let items: Vec<SceneItem> = (0..=SCENE_MAX_WORK_GROUP_ITEMS)
        .map(|i| reasoning_item(&format!("r{i}"), "turn_a", i as u64 + 1, "r"))
        .collect();
    let err =
        ConversationScene::build(turns, items, Vec::new(), Vec::new()).expect_err("work overflow");
    assert!(matches!(err, SceneBuildError::TooManyWorkItems { .. }));
}

#[test]
fn changed_files_overflow_is_typed_error() {
    let files: Vec<SceneFileChange> = (0..=SCENE_MAX_CHANGED_FILES_PER_CARD)
        .map(|i| SceneFileChange::new(format!("file_{i}.txt"), FileChangeStatus::Modified).unwrap())
        .collect();
    let err = SceneItem::new(
        scene_id("cs"),
        turn_id("turn_a"),
        1,
        SceneItemKind::ChangeSet { files },
        None,
    )
    .expect_err("files overflow");
    assert!(matches!(err, SceneBuildError::TooManyChangedFiles { .. }));
}

#[test]
fn native_fact_text_overflow_is_typed_error() {
    let long = "x".repeat(SCENE_MAX_NATIVE_FACT_BYTES + 1);
    let err = SceneItem::new(
        scene_id("n1"),
        turn_id("turn_a"),
        1,
        SceneItemKind::NativeFact { text: long },
        None,
    )
    .expect_err("native fact overflow");
    assert!(matches!(err, SceneBuildError::NativeFactTooLong { .. }));
}

#[test]
fn display_path_overflow_is_typed_error() {
    let long_path = "a".repeat(SCENE_MAX_DISPLAY_PATH_BYTES + 1);
    let err =
        SceneFileChange::new(long_path, FileChangeStatus::Modified).expect_err("path overflow");
    assert!(matches!(err, SceneBuildError::DisplayPathTooLong { .. }));
}

#[test]
fn public_payload_constructors_refuse_each_bounded_collection_and_label() {
    let too_many_entries = (0..=SCENE_MAX_PLAN_ENTRIES)
        .map(|index| index.to_string())
        .collect();
    let err = SceneItem::new(
        scene_id("plan"),
        turn_id("turn_a"),
        1,
        SceneItemKind::Plan {
            title: "plan".to_owned(),
            entries: too_many_entries,
        },
        None,
    )
    .expect_err("plan entry bound");
    assert!(matches!(err, SceneBuildError::TooManyPlanEntries { .. }));

    let too_long_title = "x".repeat(conversation_scene::SCENE_MAX_TEXT_BYTES + 1);
    let err = SceneItem::new(
        scene_id("work"),
        turn_id("turn_a"),
        1,
        SceneItemKind::WorkSession {
            title: too_long_title,
        },
        None,
    )
    .expect_err("general text bound");
    assert!(matches!(err, SceneBuildError::TextTooLong { .. }));

    let err = SteeringPlacement::new(
        scene_id("steer"),
        item_id("user"),
        "x".repeat(conversation_scene::SCENE_MAX_STEERING_LABEL_BYTES + 1),
    )
    .expect_err("steering label bound");
    assert!(matches!(err, SceneBuildError::SteeringLabelTooLong { .. }));

    let err =
        SceneFileChange::new("", FileChangeStatus::Modified).expect_err("empty display path bound");
    assert!(matches!(err, SceneBuildError::EmptyDisplayPath));
}

#[test]
fn build_rejects_unknown_and_duplicate_narrations_atomically() {
    use conversation_scene::ConversationScene;

    let unknown = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)],
        Vec::new(),
        vec![narration("turn_missing", TurnNarration::Quiet)],
        Vec::new(),
    )
    .expect_err("unknown narration turn");
    assert!(matches!(
        unknown,
        SceneBuildError::UnknownNarrationTurn { .. }
    ));

    let duplicate = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)],
        Vec::new(),
        vec![
            narration("turn_a", TurnNarration::Quiet),
            narration("turn_a", TurnNarration::Working),
        ],
        Vec::new(),
    )
    .expect_err("duplicate narration turn");
    assert!(matches!(
        duplicate,
        SceneBuildError::DuplicateNarration { .. }
    ));
}

#[test]
fn scene_rejects_collection_bounds_before_processing_payloads() {
    use conversation_scene::ConversationScene;

    let too_many_turns = (0..=SCENE_MAX_TURNS)
        .map(|index| {
            scene_turn(
                &format!("turn_{index}"),
                index as u64,
                ConversationLifecycle::Pending,
            )
        })
        .collect();
    let err = ConversationScene::build(too_many_turns, Vec::new(), Vec::new(), Vec::new())
        .expect_err("turn bound");
    assert!(matches!(err, SceneBuildError::TooManyTurns { .. }));

    let too_many_narrations = (0..=SCENE_MAX_NARRATIONS)
        .map(|_| narration("turn_a", TurnNarration::Quiet))
        .collect();
    let err = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)],
        Vec::new(),
        too_many_narrations,
        Vec::new(),
    )
    .expect_err("narration bound");
    assert!(matches!(err, SceneBuildError::TooManyNarrations { .. }));

    let too_many_steerings = (0..=SCENE_MAX_STEERING_PLACEMENTS)
        .map(|index| steering(&format!("steer_{index}"), "user_a", "label"))
        .collect();
    let err = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)],
        vec![user_item("user_a", "turn_a", 1, "hello")],
        Vec::new(),
        too_many_steerings,
    )
    .expect_err("steering bound");
    assert!(matches!(
        err,
        SceneBuildError::TooManySteeringPlacements { .. }
    ));
}

#[test]
fn duplicate_ordinal_across_turn_and_item_is_error() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)];
    let items = vec![user_item("u1", "turn_a", 0, "hi")];
    let err = ConversationScene::build(turns, items, Vec::new(), Vec::new())
        .expect_err("cross ordinal duplicate");
    assert!(matches!(err, SceneBuildError::DuplicateOrdinal { .. }));
}

// ---- 11. interrupted/failed/cancelled and interactive/error cards remain ordered and distinct ----

#[test]
fn interrupted_failed_cancelled_and_interactive_error_cards_remain_ordered_distinct() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Failed)];
    let items = vec![
        approval_item("ap1", "turn_a", 1),
        question_item("q1", "turn_a", 2),
        error_item("e1", "turn_a", 3),
        SceneItem::new(
            scene_id("u1"),
            turn_id("turn_a"),
            4,
            SceneItemKind::UsageInterruption {
                detail: "interrupted".to_owned(),
            },
            None,
        )
        .expect("interruption item valid"),
        SceneItem::new(
            scene_id("m1"),
            turn_id("turn_a"),
            5,
            SceneItemKind::ModelTransition {
                from_model: "m1".to_owned(),
                to_model: "m2".to_owned(),
            },
            None,
        )
        .expect("model transition item valid"),
        native_fact_item("n1", "turn_a", 6, "fact"),
    ];
    let narrations = vec![narration("turn_a", TurnNarration::Failed)];
    let scene = ConversationScene::build(turns, items, narrations, Vec::new()).expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    // Expect distinct ordered blocks: Approval, Question, Error, UsageInterruption, ModelTransition, NativeFact, Status, Footer
    let kinds: Vec<&str> = blocks
        .iter()
        .map(|b| match b {
            TurnBlock::Approval(_) => "approval",
            TurnBlock::Question(_) => "question",
            TurnBlock::Error(_) => "error",
            TurnBlock::UsageInterruption(_) => "interruption",
            TurnBlock::ModelTransition(_) => "model",
            TurnBlock::NativeFact(_) => "native",
            TurnBlock::TurnStatus(_) => "status",
            TurnBlock::TurnFooter(_) => "footer",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "approval",
            "question",
            "error",
            "interruption",
            "model",
            "native",
            "status",
            "footer"
        ]
    );
    // Ensure status narration is Failed (distinct from Interrupted/Cancelled)
    assert!(matches!(
        blocks.iter().find(|b| matches!(b, TurnBlock::TurnStatus(_))),
        Some(TurnBlock::TurnStatus(s)) if s.narration == TurnNarration::Failed
    ));
}

// ---- 12. footer settlement and active clock basis are additive, typed, and exact ----

#[test]
fn footers_start_unsettled_and_settle_only_the_exact_turn() {
    use conversation_scene::{ConversationScene, TurnFooterSettlement};

    let turns = vec![
        scene_turn("turn_a", 0, ConversationLifecycle::Completed),
        scene_turn("turn_b", 10, ConversationLifecycle::Completed),
    ];
    let items = vec![
        user_item("user_a", "turn_a", 1, "hi"),
        assistant_item("assist_a", "turn_a", 2, "hello", AssistantPhase::Final),
        user_item("user_b", "turn_b", 11, "who are you"),
        assistant_item("assist_b", "turn_b", 12, "artisan", AssistantPhase::Final),
    ];
    let mut scene = ConversationScene::build(turns, items, Vec::new(), Vec::new()).expect("builds");
    for turn_scene in scene.turn_scenes() {
        let footer = turn_scene
            .blocks
            .iter()
            .find_map(|block| match block {
                TurnBlock::TurnFooter(footer) => Some(footer),
                _ => None,
            })
            .expect("one footer per turn");
        assert!(footer.settlement.is_none());
    }

    let settlement = TurnFooterSettlement::new("hello".to_owned(), 99).expect("valid settlement");
    assert!(scene.set_turn_footer_settlement(&turn_id("turn_a"), settlement));
    assert!(!scene.set_turn_footer_settlement(
        &turn_id("turn_missing"),
        TurnFooterSettlement::new("x".to_owned(), 1).expect("valid")
    ));

    let settled = scene
        .turn_scene(&turn_id("turn_a"))
        .expect("turn_a present")
        .blocks
        .iter()
        .find_map(|block| match block {
            TurnBlock::TurnFooter(footer) => Some(footer),
            _ => None,
        })
        .expect("turn_a footer");
    let facts = settled.settlement.as_ref().expect("turn_a settled");
    assert_eq!(facts.response_text(), "hello");
    assert_eq!(facts.settled_at_ms(), 99);

    let other = scene
        .turn_scene(&turn_id("turn_b"))
        .expect("turn_b present")
        .blocks
        .iter()
        .find_map(|block| match block {
            TurnBlock::TurnFooter(footer) => Some(footer),
            _ => None,
        })
        .expect("turn_b footer");
    assert!(other.settlement.is_none());
}

#[test]
fn footer_settlement_rejects_response_text_above_the_body_ceiling() {
    use conversation_scene::TurnFooterSettlement;

    let too_long = "x".repeat(SCENE_MAX_MESSAGE_BODY_BYTES + 1);
    let err = TurnFooterSettlement::new(too_long, 7).expect_err("overlong settlement refused");
    assert!(matches!(err, SceneBuildError::MessageBodyTooLong { .. }));
}

#[test]
fn active_clock_basis_flows_to_status_only_for_active_work() {
    use conversation_scene::ConversationScene;

    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![assistant_item(
            "assist_a",
            "turn_a",
            1,
            "draft",
            AssistantPhase::Unspecified,
        )],
        vec![
            TurnNarrationEntry::new(turn_id("turn_a"), TurnNarration::Thinking)
                .with_active_started_at_ms(1_000),
        ],
        Vec::new(),
    )
    .expect("builds");
    assert!(matches!(
        scene.turn_scenes()[0].blocks.iter().find(|block| matches!(
            block,
            TurnBlock::TurnStatus(_)
        )),
        Some(TurnBlock::TurnStatus(status))
            if status.narration == TurnNarration::Thinking
                && status.active_started_at_ms == Some(1_000)
    ));

    // Quiet and terminal narrations refuse a live basis instead of rendering a
    // ticking row or forking a second elapsed source.
    for narration in [
        TurnNarration::Quiet,
        TurnNarration::WorkedFor { millis: 4 },
        TurnNarration::Failed,
    ] {
        let err = ConversationScene::build(
            vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)],
            Vec::new(),
            vec![
                TurnNarrationEntry::new(turn_id("turn_a"), narration)
                    .with_active_started_at_ms(1_000),
            ],
            Vec::new(),
        )
        .expect_err("basis without active work is refused");
        assert!(
            matches!(
                err,
                SceneBuildError::ActiveBasisWithoutActiveNarration { .. }
            ),
            "unexpected error for {narration:?}: {err:?}"
        );
    }
}

#[test]
fn adjacent_assistant_segments_keep_exact_bytes_and_block_boundaries() {
    use conversation_scene::ConversationScene;

    // Two durable response segments must never merge and never gain heuristic
    // spacing: the boundary stays structural (two blocks) and token bytes stay
    // exact. Visual separation between the blocks belongs to the renderer.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            assistant_item(
                "seg_a",
                "turn_a",
                1,
                "naturally",
                AssistantPhase::Unspecified,
            ),
            assistant_item("seg_b", "turn_a", 2, "I'm", AssistantPhase::Unspecified),
        ],
        Vec::new(),
        Vec::new(),
    )
    .expect("builds");
    let bodies: Vec<&str> = scene.turn_scenes()[0]
        .blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::AssistantMessage(message) => Some(message.body.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(bodies, vec!["naturally", "I'm"]);
}

// ---- 12. session-anchored details (R1/H): late work joins the session ----

fn session_group(blocks: &[TurnBlock]) -> &conversation_scene::WorkGroupBlock {
    blocks
        .iter()
        .find_map(|block| match block {
            TurnBlock::WorkGroup(group) if group.session.is_some() => Some(group),
            _ => None,
        })
        .expect("one session group")
}

#[test]
fn session_groups_late_reasoning_before_final_reply() {
    use conversation_scene::ConversationScene;

    // Screenshot shape: the reply settled first, reasoning landed later at a
    // higher ordinal. The late work joins the session trace in place; the
    // transcript shows user → session → final reply → footer, with no work
    // block after the reply and no visible reasoning row anywhere.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)],
        vec![
            user_item("user_a", "turn_a", 1, "Whoopty"),
            provenanced(
                assistant_item(
                    "reply",
                    "turn_a",
                    2,
                    "Whoopty! What's up?",
                    AssistantPhase::Final,
                ),
                "run_a",
                ConversationLifecycle::Completed,
            ),
            provenanced(
                reasoning_item("late", "turn_a", 3, "Planning playful ambiguous response"),
                "run_a",
                ConversationLifecycle::Completed,
            ),
        ],
        vec![narration(
            "turn_a",
            TurnNarration::ThoughtFor { millis: 6_000 },
        )],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    let kinds: Vec<&str> = blocks
        .iter()
        .map(|block| match block {
            TurnBlock::UserMessage(_) => "user",
            TurnBlock::WorkGroup(_) => "session",
            TurnBlock::AssistantMessage(_) => "reply",
            TurnBlock::TurnStatus(_) => "status",
            TurnBlock::TurnFooter(_) => "footer",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, vec!["user", "session", "reply", "status", "footer"]);
    let group = session_group(blocks);
    assert_eq!(
        group.session.as_ref().map(SceneId::as_str),
        Some("session-turn_a")
    );
    assert!(group.items.is_empty());
    assert!(group.session_details.is_empty());
    // The settled group keeps the newest thinking body for its collapsed
    // chip label, and reasoning never becomes a visible work item even
    // though it arrived late.
    assert_eq!(
        group.reasoning_summary.as_deref(),
        Some("Planning playful ambiguous response")
    );
    // Reference store.ts:785 marks the group superseded whenever it is not
    // the last content block; the final reply follows here, so true is the
    // exact value (settled turns show no live line either way).
    assert!(group.superseded);
    assert!(matches!(
        group.label,
        Some(WorkGroupLabel::ThoughtFor { millis: 6_000 })
    ));
}

#[test]
fn live_session_exposes_one_reasoning_summary_line() {
    use conversation_scene::ConversationScene;

    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                reasoning_item("r1", "turn_a", 2, "first thought"),
                "run_a",
                ConversationLifecycle::Active,
            ),
            provenanced(
                reasoning_item("r2", "turn_a", 3, "second thought"),
                "run_a",
                ConversationLifecycle::Active,
            ),
        ],
        vec![narration("turn_a", TurnNarration::Thinking)],
        Vec::new(),
    )
    .expect("builds");
    let group = session_group(&scene.turn_scenes()[0].blocks);
    // Newest non-empty reasoning body only; no visible reasoning rows.
    assert_eq!(group.reasoning_summary.as_deref(), Some("second thought"));
    assert!(group.items.is_empty());
    assert!(!group.superseded);
}

#[test]
fn commentary_folds_into_session_details_without_suppressing() {
    use conversation_scene::ConversationScene;

    // Streaming commentary is intermediate work, not a reply: the status row
    // stays while the commentary folds into the session trace.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("note", "turn_a", 2, "checking", AssistantPhase::Commentary),
                "run_a",
                ConversationLifecycle::Streaming,
            ),
        ],
        vec![narration("turn_a", TurnNarration::StreamingSuppression)],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(blocks.iter().any(|b| matches!(b, TurnBlock::TurnStatus(_))));
    assert!(
        !blocks
            .iter()
            .any(|b| matches!(b, TurnBlock::AssistantMessage(_)))
    );
    let group = session_group(blocks);
    assert_eq!(group.session_details.len(), 1);
    assert!(matches!(
        &group.session_details[0],
        conversation_scene::SessionDetail::Assistant {
            phase: AssistantPhase::Commentary,
            ..
        }
    ));
}

#[test]
fn settled_last_promotes_completed_reply_without_final_phase() {
    use conversation_scene::ConversationScene;

    // Phaseless providers never emit Final: the settled last completed
    // message still promotes to the single top-level reply.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("m1", "turn_a", 2, "interim", AssistantPhase::Unspecified),
                "run_a",
                ConversationLifecycle::Completed,
            ),
            provenanced(
                assistant_item("m2", "turn_a", 3, "settled", AssistantPhase::Unspecified),
                "run_a",
                ConversationLifecycle::Completed,
            ),
        ],
        vec![narration("turn_a", TurnNarration::ThoughtFor { millis: 7 })],
        Vec::new(),
    )
    .expect("builds");
    let turn_scene = &scene.turn_scenes()[0];
    let replies: Vec<&str> = turn_scene
        .blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::AssistantMessage(message) => Some(message.body.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(replies, vec!["settled"]);
    assert_eq!(
        scene
            .promoted_reply_id(&turn_id("turn_a"))
            .map(|id| id.as_str().to_owned()),
        Some("m2".to_owned())
    );
    assert!(
        turn_scene.blocks.iter().any(|block| matches!(
            block,
            TurnBlock::TurnFooter(footer) if footer.settlement.is_none()
        )),
        "settlement is aggregate-owned, never built"
    );
}

#[test]
fn progress_reply_promotes_phaseless_prose_while_current() {
    use conversation_scene::ConversationScene;

    // Newest prose wins while it remains the newest phase, even Unspecified.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("m1", "turn_a", 2, "reply", AssistantPhase::Unspecified),
                "run_a",
                ConversationLifecycle::Streaming,
            ),
        ],
        vec![narration("turn_a", TurnNarration::StreamingSuppression)],
        Vec::new(),
    )
    .expect("builds");
    assert_eq!(
        scene
            .promoted_reply_id(&turn_id("turn_a"))
            .map(|id| id.as_str().to_owned()),
        Some("m1".to_owned())
    );
}

#[test]
fn newer_work_returns_prose_to_session_details() {
    use conversation_scene::ConversationScene;

    // A tool result landing after the prose makes work the newest phase: the
    // prose returns to session details and nothing renders top-level twice.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("m1", "turn_a", 2, "reply", AssistantPhase::Unspecified),
                "run_a",
                ConversationLifecycle::Completed,
            ),
            provenanced(
                activity_item("tool", "turn_a", 3, "ran"),
                "run_a",
                ConversationLifecycle::Active,
            ),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(
        !blocks
            .iter()
            .any(|b| matches!(b, TurnBlock::AssistantMessage(_)))
    );
    let group = session_group(blocks);
    assert_eq!(group.progress, ProgressPhase::Work);
    // The one ordered detail list carries both, prose first in ordinal
    // order — never two sources.
    assert_eq!(group.session_details.len(), 2);
    assert!(matches!(
        &group.session_details[0],
        conversation_scene::SessionDetail::Assistant { .. }
    ));
    assert!(matches!(
        &group.session_details[1],
        conversation_scene::SessionDetail::Activity { .. }
    ));
}

#[test]
fn multi_run_dissolves_session_grouping_without_losing_prose() {
    use conversation_scene::ConversationScene;

    // Two runs with content mirror the reference multi-session rule: no
    // group, every assistant top-level in ordinal order, nothing voided.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("r1", "turn_a", 2, "first", AssistantPhase::Final),
                "run_a",
                ConversationLifecycle::Completed,
            ),
            provenanced(
                assistant_item("r2", "turn_a", 3, "second", AssistantPhase::Final),
                "run_b",
                ConversationLifecycle::Completed,
            ),
        ],
        vec![narration("turn_a", TurnNarration::WorkedFor { millis: 9 })],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(!blocks.iter().any(|b| matches!(
        b,
        TurnBlock::WorkGroup(group) if group.session.is_some()
    )));
    let bodies: Vec<&str> = blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::AssistantMessage(message) => Some(message.body.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(bodies, vec!["first", "second"]);
}

/// Names each block for layout assertions; a session continuation (the
/// segment after a mid-run user message) is named apart from the first
/// segment.
fn steer_layout(blocks: &[TurnBlock]) -> Vec<&'static str> {
    blocks
        .iter()
        .map(|block| match block {
            TurnBlock::UserMessage(_) => "user",
            TurnBlock::WorkGroup(group) if group.continuation.is_some() => "continuation",
            TurnBlock::WorkGroup(group) if group.session.is_some() => "session",
            TurnBlock::WorkGroup(_) => "work",
            TurnBlock::SteeringLabel(_) => "steer-label",
            TurnBlock::AssistantMessage(_) => "assistant",
            TurnBlock::TurnStatus(_) => "status",
            TurnBlock::TurnFooter(_) => "footer",
            _ => "other",
        })
        .collect()
}

/// Returns the detail identities of one session segment in stored order.
fn detail_ids(group: &WorkGroupBlock) -> Vec<&str> {
    group
        .session_details
        .iter()
        .map(|detail| match detail {
            SessionDetail::Assistant { id, .. }
            | SessionDetail::Activity { id, .. }
            | SessionDetail::Compaction { id, .. }
            | SessionDetail::NativeFact { id, .. } => id.as_str(),
        })
        .collect()
}

/// Returns every work group block of one turn in block order.
fn work_groups(blocks: &[TurnBlock]) -> Vec<&WorkGroupBlock> {
    blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::WorkGroup(group) => Some(group),
            _ => None,
        })
        .collect()
}

fn run_activity(id: &str, ordinal: u64, lifecycle: ConversationLifecycle) -> SceneItem {
    provenanced(
        activity_item(id, "turn_a", ordinal, "ran"),
        "run_a",
        lifecycle,
    )
}

fn run_commentary(id: &str, ordinal: u64, body: &str) -> SceneItem {
    provenanced(
        assistant_item(id, "turn_a", ordinal, body, AssistantPhase::Commentary),
        "run_a",
        ConversationLifecycle::Completed,
    )
}

#[test]
fn unacknowledged_post_steer_work_joins_the_session_above_the_bubble() {
    use conversation_scene::ConversationScene;

    // Departs from the old post-steering rule: a steer never starts a new
    // section. Until the model acknowledges it in prose, work after the steer
    // joins the one session above the bubble, and the bubble (with its
    // label) sits after all of it without superseding the session.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("prompt", "turn_a", 1, "go"),
            run_activity("tool", 2, ConversationLifecycle::Completed),
            user_item("steer", "turn_a", 5, "actually, stop"),
            run_activity("tool2", 8, ConversationLifecycle::Active),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        vec![steering("steer_1", "steer", "steering")],
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert_eq!(
        steer_layout(blocks),
        vec!["user", "session", "user", "steer-label", "status", "footer"]
    );
    let group = session_group(blocks);
    assert!(!group.superseded, "a steer never supersedes its session");
    assert!(group.continuation.is_none());
    assert_eq!(detail_ids(group), vec!["tool", "tool2"]);
}

#[test]
fn session_anchor_overlong_is_typed_error() {
    use conversation_scene::ConversationScene;

    // Turn ids admit 128 bytes; the session prefix pushes past the scene
    // identity ceiling instead of fabricating a colliding anchor.
    let long = "t".repeat(128);
    let err = ConversationScene::build(
        vec![scene_turn(&long, 0, ConversationLifecycle::Active)],
        vec![
            user_item("user_long", &long, 1, "hi"),
            provenanced(
                assistant_item("m_long", &long, 2, "reply", AssistantPhase::Unspecified),
                "run_a",
                ConversationLifecycle::Active,
            ),
        ],
        Vec::new(),
        Vec::new(),
    )
    .expect_err("overlong session anchor is refused");
    assert!(matches!(err, SceneBuildError::SessionAnchorTooLong { .. }));
}

#[test]
fn legacy_positional_layout_without_provenance_is_unchanged() {
    use conversation_scene::{ConversationScene, WorkGroupBlock};

    // No provenance anywhere: contiguous work groups, top-level reply, no
    // session fields — byte-for-byte the pre-session contract.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            reasoning_item("r1", "turn_a", 1, "thinking"),
            activity_item("a1", "turn_a", 2, "tool"),
            assistant_item("assist", "turn_a", 3, "final", AssistantPhase::Final),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(matches!(
        &blocks[0],
        TurnBlock::WorkGroup(WorkGroupBlock { session: None, .. })
    ));
    assert!(matches!(&blocks[1], TurnBlock::AssistantMessage(_)));
    assert!(blocks.iter().any(|b| matches!(
        b,
        TurnBlock::WorkGroup(group) if group.session.is_none()
            && group.reasoning_summary.is_none()
            && group.progress == ProgressPhase::None
    )));
}

#[test]
fn waiting_activity_keeps_the_status_block_for_the_header_without_a_summary() {
    use conversation_scene::ConversationScene;

    // A live tool chain newer than model prose carries progress itself, so
    // the row paints nothing of its own even while working: the block stays
    // with its narration and basis, which the group header counts as
    // `Working for …`, but the older thinking summary is dropped so the row
    // is the header's exact duplicate and stands down.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                reasoning_item("r1", "turn_a", 2, "Weighing the options."),
                "run_a",
                ConversationLifecycle::Completed,
            ),
            provenanced(
                activity_item("tool", "turn_a", 3, "running"),
                "run_a",
                ConversationLifecycle::Active,
            ),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    let status = blocks
        .iter()
        .find_map(|b| match b {
            TurnBlock::TurnStatus(status) => Some(status),
            _ => None,
        })
        .expect("the status block stays for the group header's live line");
    assert_eq!(status.narration, TurnNarration::Working);
    assert_eq!(status.reasoning_summary, None);
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, TurnBlock::WorkGroup(group) if group.session.is_some())),
        "the session group owns the live header"
    );
}

#[test]
fn engine_handoff_folds_into_session_header() {
    use conversation_scene::ConversationScene;

    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("m1", "turn_a", 2, "reply", AssistantPhase::Unspecified),
                "run_a",
                ConversationLifecycle::Streaming,
            ),
            SceneItem::new(
                scene_id("hop"),
                turn_id("turn_a"),
                3,
                SceneItemKind::ModelTransition {
                    from_model: "engine-a".to_owned(),
                    to_model: "engine-b".to_owned(),
                },
                None,
            )
            .expect("transition item valid"),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(
        !blocks
            .iter()
            .any(|b| matches!(b, TurnBlock::ModelTransition(_)))
    );
    let group = session_group(blocks);
    assert!(group.transition.is_some());
    let status = blocks.iter().find_map(|block| match block {
        TurnBlock::TurnStatus(status) => Some(status),
        _ => None,
    });
    assert!(status.is_none(), "live reply suppresses the row");
}

#[test]
fn promoted_reply_id_absent_without_candidates() {
    use conversation_scene::ConversationScene;

    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)],
        vec![user_item("user_a", "turn_a", 1, "hi")],
        Vec::new(),
        Vec::new(),
    )
    .expect("builds");
    assert_eq!(scene.promoted_reply_id(&turn_id("turn_a")), None);
}

#[test]
fn unattributed_assistant_renders_top_level_in_session_turn() {
    use conversation_scene::ConversationScene;

    // Promotion (phase-based) still selects the latest reply, but the
    // unattributed message keeps its legacy top-level row.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("m1", "turn_a", 2, "first", AssistantPhase::Final),
                "run_a",
                ConversationLifecycle::Active,
            ),
            assistant_item("m2", "turn_a", 3, "second", AssistantPhase::Final),
        ],
        vec![narration("turn_a", TurnNarration::WorkedFor { millis: 9 })],
        Vec::new(),
    )
    .expect("builds");
    let bodies: Vec<&str> = scene.turn_scenes()[0]
        .blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::AssistantMessage(message) => Some(message.body.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(bodies, vec!["second"]);
    assert_eq!(
        scene
            .promoted_reply_id(&turn_id("turn_a"))
            .map(|id| id.as_str().to_owned()),
        Some("m2".to_owned())
    );
}

#[test]
fn explicit_final_beats_newer_unspecified_while_work_is_newest() {
    use conversation_scene::ConversationScene;

    // Reference first loop records the latest explicit final independently:
    // Final@2 wins even with newer Unspecified@3, and newer Activity@4
    // keeps progress at Work so nothing overrides it.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("fin", "turn_a", 2, "final", AssistantPhase::Final),
                "run_a",
                ConversationLifecycle::Completed,
            ),
            provenanced(
                assistant_item("unsp", "turn_a", 3, "later", AssistantPhase::Unspecified),
                "run_a",
                ConversationLifecycle::Completed,
            ),
            provenanced(
                activity_item("tool", "turn_a", 4, "ran"),
                "run_a",
                ConversationLifecycle::Completed,
            ),
        ],
        Vec::new(),
        Vec::new(),
    )
    .expect("builds");
    assert_eq!(
        scene
            .promoted_reply_id(&turn_id("turn_a"))
            .map(|id| id.as_str().to_owned()),
        Some("fin".to_owned())
    );
    let bodies: Vec<&str> = scene.turn_scenes()[0]
        .blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::AssistantMessage(message) => Some(message.body.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(bodies, vec!["final"]);
}

#[test]
fn cancelled_turn_with_later_work_promotes_nothing() {
    use conversation_scene::ConversationScene;

    // Failed/Cancelled turns are not completed work sessions: without an
    // independent session lifecycle the limitation is stated, so
    // settled-last never fires here even with later work present.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Cancelled)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("draft", "turn_a", 2, "draft", AssistantPhase::Unspecified),
                "run_a",
                ConversationLifecycle::Completed,
            ),
            provenanced(
                activity_item("tool", "turn_a", 4, "ran"),
                "run_a",
                ConversationLifecycle::Completed,
            ),
        ],
        vec![narration("turn_a", TurnNarration::Cancelled)],
        Vec::new(),
    )
    .expect("builds");
    assert_eq!(scene.promoted_reply_id(&turn_id("turn_a")), None);
    assert!(
        !scene.turn_scenes()[0]
            .blocks
            .iter()
            .any(|block| matches!(block, TurnBlock::AssistantMessage(_)))
    );
}

#[test]
fn failed_turn_keeps_explicit_final_reply() {
    use conversation_scene::ConversationScene;

    // The independent latest-final branch does not depend on turn outcome:
    // an explicit Final still promotes (the footer rule separately requires
    // a completed turn, so no settlement is implied).
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Failed)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("fin", "turn_a", 2, "final", AssistantPhase::Final),
                "run_a",
                ConversationLifecycle::Completed,
            ),
        ],
        vec![narration("turn_a", TurnNarration::Failed)],
        Vec::new(),
    )
    .expect("builds");
    assert_eq!(
        scene
            .promoted_reply_id(&turn_id("turn_a"))
            .map(|id| id.as_str().to_owned()),
        Some("fin".to_owned())
    );
    let bodies: Vec<&str> = scene.turn_scenes()[0]
        .blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::AssistantMessage(message) => Some(message.body.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(bodies, vec!["final"]);
}

#[test]
fn completed_tool_after_prose_does_not_wait() {
    use conversation_scene::ConversationScene;

    // A settled tool is history, not progress: only a typed live lifecycle
    // waits, so the status row stays even though the tool ordinal is newer.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("m1", "turn_a", 2, "done", AssistantPhase::Final),
                "run_a",
                ConversationLifecycle::Completed,
            ),
            provenanced(
                activity_item("tool", "turn_a", 4, "ran"),
                "run_a",
                ConversationLifecycle::Completed,
            ),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(blocks.iter().any(|b| matches!(b, TurnBlock::TurnStatus(_))));
}

#[test]
fn stale_streaming_before_newer_tool_is_not_live_reply() {
    use conversation_scene::ConversationScene;

    // The streaming reply is not the newest phase once a tool lands after
    // it: progress is Work and nothing promotes, so has_live_reply
    // (progress Reply AND live text) is false. The tool is already
    // Completed, so no waiting suppression confounds the predicate either:
    // old code hid this row via stale live_reply, corrected code keeps it.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("m1", "turn_a", 2, "draft", AssistantPhase::Unspecified),
                "run_a",
                ConversationLifecycle::Streaming,
            ),
            provenanced(
                activity_item("tool", "turn_a", 4, "ran"),
                "run_a",
                ConversationLifecycle::Completed,
            ),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(blocks.iter().any(|b| matches!(b, TurnBlock::TurnStatus(_))));
    let group = session_group(blocks);
    assert_eq!(group.progress, ProgressPhase::Work);
    assert_eq!(scene.promoted_reply_id(&turn_id("turn_a")), None);
}

#[test]
fn later_reasoning_retires_tool_wait() {
    use conversation_scene::ConversationScene;

    // Model prose is assistant text or reasoning summaries: reasoning newer
    // than the live tool means the wait is over and the row returns.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                activity_item("tool", "turn_a", 2, "running"),
                "run_a",
                ConversationLifecycle::Active,
            ),
            provenanced(
                reasoning_item("r1", "turn_a", 4, "thinking out loud"),
                "run_a",
                ConversationLifecycle::Active,
            ),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(blocks.iter().any(|b| matches!(b, TurnBlock::TurnStatus(_))));
}

fn turn_statuses(
    scene: &conversation_scene::ConversationScene,
) -> Vec<(TurnNarration, Option<String>)> {
    scene.turn_scenes()[0]
        .blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::TurnStatus(status) => Some((status.narration, status.engine_label.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn explicit_engine_label_wins_outside_session_mode() {
    use conversation_scene::ConversationScene;

    // No runs anywhere, so legacy layout applies - yet an explicit
    // send-time label still names the waiting row, ahead of the
    // transition-derived fallback which only fills session rows.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            SceneItem::new(
                scene_id("hop"),
                turn_id("turn_a"),
                2,
                SceneItemKind::ModelTransition {
                    from_model: "engine-a".to_owned(),
                    to_model: "engine-b".to_owned(),
                },
                None,
            )
            .expect("transition item valid"),
        ],
        vec![
            TurnNarrationEntry::new(turn_id("turn_a"), TurnNarration::ProviderWait)
                .with_engine_label("Claude".to_owned()),
        ],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, TurnBlock::ModelTransition(_)))
    );
    assert_eq!(
        turn_statuses(&scene),
        vec![(TurnNarration::ProviderWait, Some("Claude".to_owned()))]
    );
}

#[test]
fn transition_label_fills_status_without_explicit_label() {
    use conversation_scene::ConversationScene;

    // Session mode with no explicit label: the folded handoff still names
    // the row through the existing transition fallback.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("user_a", "turn_a", 1, "hi"),
            provenanced(
                assistant_item("m1", "turn_a", 2, "done", AssistantPhase::Final),
                "run_a",
                ConversationLifecycle::Completed,
            ),
            SceneItem::new(
                scene_id("hop"),
                turn_id("turn_a"),
                3,
                SceneItemKind::ModelTransition {
                    from_model: "engine-a".to_owned(),
                    to_model: "engine-b".to_owned(),
                },
                None,
            )
            .expect("transition item valid"),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        Vec::new(),
    )
    .expect("builds");
    assert_eq!(
        turn_statuses(&scene),
        vec![(TurnNarration::Working, Some("engine-b".to_owned()))]
    );
}

#[test]
fn empty_engine_label_is_rejected() {
    use conversation_scene::ConversationScene;

    let err = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)],
        vec![user_item("user_a", "turn_a", 1, "hi")],
        vec![
            TurnNarrationEntry::new(turn_id("turn_a"), TurnNarration::Quiet)
                .with_engine_label(String::new()),
        ],
        Vec::new(),
    )
    .expect_err("blank engine label is refused");
    assert!(matches!(err, SceneBuildError::EmptyEngineLabel));
}

#[test]
fn overlong_engine_label_is_rejected() {
    use conversation_scene::ConversationScene;

    let err = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Pending)],
        vec![user_item("user_a", "turn_a", 1, "hi")],
        vec![
            TurnNarrationEntry::new(turn_id("turn_a"), TurnNarration::Quiet)
                .with_engine_label("x".repeat(1025)),
        ],
        Vec::new(),
    )
    .expect_err("overlong engine label is refused");
    assert!(matches!(err, SceneBuildError::EngineLabelTooLong { .. }));
}

#[test]
fn acknowledging_prose_continues_the_session_after_the_bubble() {
    use conversation_scene::ConversationScene;

    // Departs from the old reference, where a durable mid-run reply was a
    // boundary: the acknowledging commentary and the tools after it stay in
    // the same session, painted as a headerless continuation after the
    // bubble. Commentary is a row, never top-level prose.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("prompt", "turn_a", 1, "go"),
            run_activity("tool", 2, ConversationLifecycle::Completed),
            user_item("steer", "turn_a", 5, "actually, stop"),
            provenanced(
                assistant_item(
                    "reply-after-steer",
                    "turn_a",
                    7,
                    "Thanks, I will check Windows",
                    AssistantPhase::Commentary,
                ),
                "run_a",
                ConversationLifecycle::Streaming,
            ),
            run_activity("tool2", 8, ConversationLifecycle::Active),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert_eq!(
        steer_layout(blocks),
        vec![
            "user",
            "session",
            "user",
            "continuation",
            "status",
            "footer"
        ]
    );
    let groups = work_groups(blocks);
    let (first, continuation) = (groups[0], groups[1]);
    assert!(!first.superseded);
    assert_eq!(detail_ids(first), vec!["tool"]);
    assert_eq!(detail_ids(continuation), vec!["reply-after-steer", "tool2"]);
    // One section: shared session identity and disclosure, a distinct
    // segment anchor, and nothing the first segment's header owns.
    assert_eq!(continuation.session, first.session);
    assert_eq!(continuation.disclosure, first.disclosure);
    assert_eq!(
        continuation.continuation.as_ref().map(SceneId::as_str),
        Some("session-turn_a.1")
    );
    assert!(continuation.label.is_none());
    assert!(continuation.transition.is_none());
    assert!(continuation.reasoning_summary.is_none());
    assert!(!continuation.superseded);
}

#[test]
fn tools_before_the_acknowledgement_extend_the_chain_above_the_bubble() {
    use conversation_scene::ConversationScene;

    // Seven commands, the steer, four more commands, then the acknowledging
    // prose and two commands: the eleven commands are one uninterrupted run
    // of activity rows above the bubble, and the prose plus the last two sit
    // below it.
    let mut items = vec![
        user_item("prompt", "turn_a", 1, "why is the footer wrong"),
        run_commentary("likely", 2, "Likely cause found"),
    ];
    for index in 0..7 {
        items.push(run_activity(
            &format!("pre_{index}"),
            3 + index,
            ConversationLifecycle::Completed,
        ));
    }
    items.push(user_item("steer", "turn_a", 10, "also check the tray"));
    for index in 0..4 {
        items.push(run_activity(
            &format!("post_{index}"),
            11 + index,
            ConversationLifecycle::Completed,
        ));
    }
    items.push(run_commentary(
        "confirmed",
        15,
        "Claude footer cause confirmed",
    ));
    items.push(run_activity("late_0", 16, ConversationLifecycle::Completed));
    items.push(run_activity("late_1", 17, ConversationLifecycle::Active));
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        items,
        vec![narration("turn_a", TurnNarration::Working)],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert_eq!(
        steer_layout(blocks),
        vec![
            "user",
            "session",
            "user",
            "continuation",
            "status",
            "footer"
        ]
    );
    let groups = work_groups(blocks);
    let first = detail_ids(groups[0]);
    assert_eq!(first[0], "likely");
    assert_eq!(first.len(), 12);
    assert!(
        groups[0].session_details[1..]
            .iter()
            .all(|detail| matches!(detail, SessionDetail::Activity { .. })),
        "the eleven commands are one contiguous activity run"
    );
    assert_eq!(detail_ids(groups[1]), vec!["confirmed", "late_0", "late_1"]);
}

#[test]
fn steers_acknowledged_by_the_same_prose_sit_together_before_it() {
    use conversation_scene::ConversationScene;

    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("prompt", "turn_a", 1, "go"),
            run_activity("tool", 2, ConversationLifecycle::Completed),
            user_item("steer_a", "turn_a", 3, "first"),
            user_item("steer_b", "turn_a", 4, "second"),
            run_activity("tool2", 5, ConversationLifecycle::Completed),
            run_commentary("ack", 6, "Handling both"),
            run_activity("tool3", 7, ConversationLifecycle::Active),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        vec![steering("label_b", "steer_b", "steering")],
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert_eq!(
        steer_layout(blocks),
        vec![
            "user",
            "session",
            "user",
            "user",
            "steer-label",
            "continuation",
            "status",
            "footer"
        ]
    );
    let users: Vec<&str> = blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::UserMessage(message) => Some(message.id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(users, vec!["prompt", "steer_a", "steer_b"]);
    let groups = work_groups(blocks);
    assert_eq!(detail_ids(groups[0]), vec!["tool", "tool2"]);
    assert_eq!(detail_ids(groups[1]), vec!["ack", "tool3"]);
    assert!(!groups[0].superseded);
}

#[test]
fn two_steers_each_find_their_own_seam() {
    use conversation_scene::ConversationScene;

    // The first steer is acknowledged, the next two are not yet: they stack
    // in ordinal order after all work the session holds, and each segment
    // gets its own anchor.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("prompt", "turn_a", 1, "go"),
            run_activity("tool", 2, ConversationLifecycle::Completed),
            user_item("steer_a", "turn_a", 3, "first"),
            run_activity("tool2", 4, ConversationLifecycle::Completed),
            run_commentary("ack_a", 5, "On it"),
            run_activity("tool3", 6, ConversationLifecycle::Completed),
            user_item("steer_b", "turn_a", 7, "second"),
            user_item("steer_c", "turn_a", 8, "third"),
            run_activity("tool4", 9, ConversationLifecycle::Active),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert_eq!(
        steer_layout(blocks),
        vec![
            "user",
            "session",
            "user",
            "continuation",
            "user",
            "user",
            "status",
            "footer"
        ]
    );
    let groups = work_groups(blocks);
    assert_eq!(detail_ids(groups[0]), vec!["tool", "tool2"]);
    assert_eq!(detail_ids(groups[1]), vec!["ack_a", "tool3", "tool4"]);
    assert!(!groups[0].superseded);

    // Once the later steers are acknowledged too, the work after that seam
    // opens a second continuation with its own anchor.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)],
        vec![
            user_item("prompt", "turn_a", 1, "go"),
            run_activity("tool", 2, ConversationLifecycle::Completed),
            user_item("steer_a", "turn_a", 3, "first"),
            run_commentary("ack_a", 4, "On it"),
            user_item("steer_b", "turn_a", 5, "second"),
            run_commentary("ack_b", 6, "Also on that"),
            run_activity("tool2", 7, ConversationLifecycle::Active),
        ],
        vec![narration("turn_a", TurnNarration::Working)],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert_eq!(
        steer_layout(blocks),
        vec![
            "user",
            "session",
            "user",
            "continuation",
            "user",
            "continuation",
            "status",
            "footer"
        ]
    );
    let anchors: Vec<Option<&str>> = work_groups(blocks)
        .iter()
        .map(|group| group.continuation.as_ref().map(SceneId::as_str))
        .collect();
    assert_eq!(
        anchors,
        vec![None, Some("session-turn_a.1"), Some("session-turn_a.2")]
    );
}

#[test]
fn the_promoted_final_reply_can_acknowledge_a_steer() {
    use conversation_scene::ConversationScene;

    // The final reply is the first prose after the steer: the bubble sits
    // directly before it, every tool stays above, and the settled outcome
    // titles the one section.
    let reply = || {
        provenanced(
            assistant_item("reply", "turn_a", 5, "Done", AssistantPhase::Final),
            "run_a",
            ConversationLifecycle::Completed,
        )
    };
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)],
        vec![
            user_item("prompt", "turn_a", 1, "go"),
            run_activity("tool", 2, ConversationLifecycle::Completed),
            user_item("steer", "turn_a", 3, "and the tray"),
            run_activity("tool2", 4, ConversationLifecycle::Completed),
            reply(),
        ],
        vec![narration("turn_a", TurnNarration::WorkedFor { millis: 9 })],
        Vec::new(),
    )
    .expect("builds");
    assert_eq!(
        scene.promoted_reply_id(&turn_id("turn_a")),
        Some(scene_id("reply"))
    );
    let blocks = &scene.turn_scenes()[0].blocks;
    assert_eq!(
        steer_layout(blocks),
        vec!["user", "session", "user", "assistant", "status", "footer"]
    );
    let group = session_group(blocks);
    assert_eq!(detail_ids(group), vec!["tool", "tool2"]);
    assert_eq!(group.label, Some(WorkGroupLabel::WorkedFor { millis: 9 }));

    // Late work after the acknowledging reply continues the section before
    // the reply, exactly like late work joins before it without a steer.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)],
        vec![
            user_item("prompt", "turn_a", 1, "go"),
            run_activity("tool", 2, ConversationLifecycle::Completed),
            user_item("steer", "turn_a", 3, "and the tray"),
            reply(),
            run_activity("late", 6, ConversationLifecycle::Completed),
        ],
        vec![narration("turn_a", TurnNarration::WorkedFor { millis: 9 })],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert_eq!(
        steer_layout(blocks),
        vec![
            "user",
            "session",
            "user",
            "continuation",
            "assistant",
            "status",
            "footer"
        ]
    );
    let groups = work_groups(blocks);
    assert_eq!(detail_ids(groups[1]), vec!["late"]);
    assert_eq!(
        groups[0].label,
        Some(WorkGroupLabel::WorkedFor { millis: 9 })
    );
    assert!(
        groups[1].label.is_none(),
        "only the first segment is titled"
    );
}

#[test]
fn a_session_turn_without_a_steer_keeps_its_exact_blocks() {
    use conversation_scene::ConversationScene;

    // No mid-run user message: one session segment, no continuation, and
    // the pre-existing superseded rule (any later block) is unchanged.
    let scene = ConversationScene::build(
        vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)],
        vec![
            user_item("prompt", "turn_a", 1, "go"),
            run_commentary("note", 2, "Looking"),
            run_activity("tool", 3, ConversationLifecycle::Completed),
            provenanced(
                assistant_item("reply", "turn_a", 4, "Done", AssistantPhase::Final),
                "run_a",
                ConversationLifecycle::Completed,
            ),
        ],
        vec![narration("turn_a", TurnNarration::WorkedFor { millis: 9 })],
        Vec::new(),
    )
    .expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert_eq!(
        steer_layout(blocks),
        vec!["user", "session", "assistant", "status", "footer"]
    );
    let TurnBlock::WorkGroup(group) = &blocks[1] else {
        panic!("session group follows the prompt");
    };
    assert_eq!(
        group,
        &WorkGroupBlock {
            items: Vec::new(),
            label: Some(WorkGroupLabel::WorkedFor { millis: 9 }),
            disclosure: None,
            session: Some(scene_id("session-turn_a")),
            session_run: Some(run_id("run_a")),
            superseded: true,
            continuation: None,
            reasoning_summary: None,
            progress: ProgressPhase::Reply,
            transition: None,
            session_details: group.session_details.clone(),
        }
    );
    assert_eq!(detail_ids(group), vec!["note", "tool"]);
}
