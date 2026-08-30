//! Black-box tests for the pure conversation render scene.
//!
//! Structural enum assertions only: no string snapshots of debug output.

#[path = "../../modules/frontend/src/conversation_scene.rs"]
mod conversation_scene;

use artisan_domain::{ConversationLifecycle, ItemId, MESSAGE_BODY_MAX_BYTES, MessageBody, TurnId};
use conversation_scene::{
    AssistantPhase, FileChangeStatus, SCENE_MAX_CHANGED_FILES_PER_CARD,
    SCENE_MAX_DISPLAY_PATH_BYTES, SCENE_MAX_ITEMS, SCENE_MAX_MESSAGE_BODY_BYTES,
    SCENE_MAX_NARRATIONS, SCENE_MAX_NATIVE_FACT_BYTES, SCENE_MAX_PLAN_ENTRIES,
    SCENE_MAX_STEERING_PLACEMENTS, SCENE_MAX_TURNS, SCENE_MAX_WORK_GROUP_ITEMS, SceneBuildError,
    SceneDisclosure, SceneFileChange, SceneId, SceneItem, SceneItemKind, SceneTurn,
    SteeringPlacement, TurnBlock, TurnNarration, TurnNarrationEntry, WorkGroupBlock,
    WorkGroupLabel, WorkItem,
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
        reasoning_item("r1", "turn_a", 1, "reasoning"),
        assistant_item("assist", "turn_a", 2, "partial", AssistantPhase::Streaming),
    ];
    let narrations = vec![narration("turn_a", TurnNarration::StreamingSuppression)];
    let scene = ConversationScene::build(turns, items, narrations, Vec::new()).expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    // Still has work group and assistant message, but no status row
    assert!(blocks.iter().any(|b| matches!(b, TurnBlock::WorkGroup(_))));
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
        reasoning_item("r1", "turn_a", 1, "r"),
        assistant_item("assist", "turn_a", 2, "partial", AssistantPhase::Streaming),
    ];
    // narration is Quiet, not suppression – status should remain
    let narrations = vec![narration("turn_a", TurnNarration::Quiet)];
    let scene = ConversationScene::build(turns, items, narrations, Vec::new()).expect("builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(blocks.iter().any(|b| matches!(b, TurnBlock::WorkGroup(_))));
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
    let turns2 = vec![scene_turn("turn_b", 1, ConversationLifecycle::Completed)];
    let items2 = vec![
        reasoning_item("r1", "turn_b", 1, "r1"),
        activity_item("a1", "turn_b", 2, "tool"),
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
    let items: Vec<SceneItem> = (0..(SCENE_MAX_ITEMS + 1))
        .map(|i| user_item(&format!("id_{i}"), "turn_a", i as u64 + 1, "hi"))
        .collect();
    let err = ConversationScene::build(turns, items, Vec::new(), Vec::new()).expect_err("overflow");
    assert!(matches!(err, SceneBuildError::TooManyItems { .. }));
}

#[test]
fn work_group_overflow_is_typed_error() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Active)];
    let items: Vec<SceneItem> = (0..(SCENE_MAX_WORK_GROUP_ITEMS + 1))
        .map(|i| reasoning_item(&format!("r{i}"), "turn_a", i as u64 + 1, "r"))
        .collect();
    let err =
        ConversationScene::build(turns, items, Vec::new(), Vec::new()).expect_err("work overflow");
    assert!(matches!(err, SceneBuildError::TooManyWorkItems { .. }));
}

#[test]
fn changed_files_overflow_is_typed_error() {
    use conversation_scene::ConversationScene;

    let turns = vec![scene_turn("turn_a", 0, ConversationLifecycle::Completed)];
    let files: Vec<SceneFileChange> = (0..(SCENE_MAX_CHANGED_FILES_PER_CARD + 1))
        .map(|i| SceneFileChange::new(format!("file_{i}.txt"), FileChangeStatus::Modified).unwrap())
        .collect();
    let items = vec![change_set_item("cs", "turn_a", 1, files)];
    let err =
        ConversationScene::build(turns, items, Vec::new(), Vec::new()).expect_err("files overflow");
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
    let too_many_entries = (0..(SCENE_MAX_PLAN_ENTRIES + 1))
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

    let too_many_turns = (0..(SCENE_MAX_TURNS + 1))
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

    let too_many_narrations = (0..(SCENE_MAX_NARRATIONS + 1))
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

    let too_many_steerings = (0..(SCENE_MAX_STEERING_PLACEMENTS + 1))
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
