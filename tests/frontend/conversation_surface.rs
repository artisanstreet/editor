//! Deterministic contract tests for the native conversation surface.
//!
//! The pure selector, status, ordering, and action projections avoid a window;
//! the final two tests use the existing in-memory GPUI harness. Registration
//! remains integration-owned work.

#[path = "../../modules/frontend/src/conversation_scene.rs"]
mod conversation_scene;
#[path = "../../modules/frontend/src/conversation_surface.rs"]
mod conversation_surface;

use artisan_domain::{ConversationLifecycle, ItemId, TurnId};
use artisan_ui::theme::ThemeMode;
use conversation_scene::{
    AssistantPhase, ConversationScene, FileChangeStatus, SceneDisclosure, SceneFileChange, SceneId,
    SceneItem, SceneItemKind, SceneTurn, SteeringPlacement, TurnBlock, TurnNarration,
    TurnNarrationEntry,
};
use conversation_surface::{
    ConversationSurface, ConversationSurfaceAction, ROOT_SELECTOR, RenderedBlockKind,
    VIEWPORT_SELECTOR, block_selector, changed_file_selector, file_change_status_label,
    format_elapsed_millis, ordered_block_kinds, steering_selector, turn_selector, turn_status_copy,
};
use gpui::{Modifiers, TestAppContext, px, size};

fn scene_id(value: &str) -> SceneId {
    SceneId::parse(value).expect("scene id is valid")
}

fn turn_id(value: &str) -> TurnId {
    TurnId::parse(value).expect("turn id is valid")
}

fn item(
    id: &str,
    turn: &str,
    ordinal: u64,
    kind: SceneItemKind,
    disclosure: Option<SceneDisclosure>,
) -> SceneItem {
    SceneItem::new(scene_id(id), turn_id(turn), ordinal, kind, disclosure)
        .expect("scene item is valid")
}

fn turn(id: &str) -> SceneTurn {
    SceneTurn::new(turn_id(id), 0, ConversationLifecycle::Completed)
}

fn narration(turn: &str, value: TurnNarration) -> TurnNarrationEntry {
    TurnNarrationEntry::new(turn_id(turn), value)
}

fn scene(items: Vec<SceneItem>, narration_value: TurnNarration) -> ConversationScene {
    ConversationScene::build(
        vec![turn("turn_a")],
        items,
        vec![narration("turn_a", narration_value)],
        Vec::new(),
    )
    .expect("conversation scene is valid")
}

#[test]
fn status_copy_is_exhaustive_and_suppression_has_no_row() {
    let cases = [
        (TurnNarration::Quiet, Some("Quiet")),
        (
            TurnNarration::ProviderWait,
            Some("Waiting for provider to respond…"),
        ),
        (
            TurnNarration::Compacting,
            Some("Compacting the conversation…"),
        ),
        (TurnNarration::Thinking, Some("Thinking")),
        (TurnNarration::Working, Some("Working")),
        (TurnNarration::StreamingSuppression, None),
        (
            TurnNarration::BackgroundWait,
            Some("Waiting for background agents…"),
        ),
        (
            TurnNarration::WorkedFor { millis: 1_000 },
            Some("Worked for 1s"),
        ),
        (
            TurnNarration::ThoughtFor { millis: 60_000 },
            Some("Thought for 1m 0s"),
        ),
        (TurnNarration::Failed, Some("Failed")),
        (TurnNarration::Interrupted, Some("Interrupted")),
        (TurnNarration::Cancelled, Some("Cancelled")),
    ];

    for (narration, expected) in cases {
        assert_eq!(turn_status_copy(narration).as_deref(), expected);
    }
}

#[test]
fn ordered_block_projection_preserves_scene_order() {
    let items = vec![
        item(
            "user",
            "turn_a",
            1,
            SceneItemKind::UserMessage {
                body: "hello".to_owned(),
            },
            None,
        ),
        item(
            "reasoning",
            "turn_a",
            2,
            SceneItemKind::ReasoningSummary {
                body: "reasoning".to_owned(),
            },
            None,
        ),
        item(
            "assistant",
            "turn_a",
            3,
            SceneItemKind::AssistantMessage {
                body: "reply".to_owned(),
                phase: AssistantPhase::Final,
            },
            None,
        ),
        item(
            "plan",
            "turn_a",
            4,
            SceneItemKind::Plan {
                title: "Plan".to_owned(),
                entries: vec!["step".to_owned()],
            },
            None,
        ),
        item(
            "approval",
            "turn_a",
            5,
            SceneItemKind::Approval {
                prompt: "approve".to_owned(),
            },
            None,
        ),
        item(
            "question",
            "turn_a",
            6,
            SceneItemKind::Question {
                prompt: "question".to_owned(),
            },
            None,
        ),
        item(
            "error",
            "turn_a",
            7,
            SceneItemKind::Error {
                message: "error".to_owned(),
            },
            None,
        ),
        item(
            "usage",
            "turn_a",
            8,
            SceneItemKind::UsageInterruption {
                detail: "usage".to_owned(),
            },
            None,
        ),
        item(
            "transition",
            "turn_a",
            9,
            SceneItemKind::ModelTransition {
                from_model: "one".to_owned(),
                to_model: "two".to_owned(),
            },
            None,
        ),
        item(
            "fact",
            "turn_a",
            10,
            SceneItemKind::NativeFact {
                text: "fact".to_owned(),
            },
            None,
        ),
    ];
    let scene = scene(items, TurnNarration::Quiet);

    assert_eq!(
        ordered_block_kinds(&scene),
        vec![
            RenderedBlockKind::UserMessage,
            RenderedBlockKind::WorkGroup,
            RenderedBlockKind::AssistantMessage,
            RenderedBlockKind::Plan,
            RenderedBlockKind::Approval,
            RenderedBlockKind::Question,
            RenderedBlockKind::Error,
            RenderedBlockKind::UsageInterruption,
            RenderedBlockKind::ModelTransition,
            RenderedBlockKind::NativeFact,
            RenderedBlockKind::TurnStatus,
            RenderedBlockKind::TurnFooter,
        ]
    );
}

#[test]
fn changed_files_keep_exact_count_status_and_path_order() {
    let files = vec![
        SceneFileChange::new("src/added.rs", FileChangeStatus::Added).expect("path is valid"),
        SceneFileChange::new("src/modified.rs", FileChangeStatus::Modified).expect("path is valid"),
        SceneFileChange::new("src/removed.rs", FileChangeStatus::Removed).expect("path is valid"),
        SceneFileChange::new("src/renamed.rs", FileChangeStatus::Renamed).expect("path is valid"),
    ];
    let scene = scene(
        vec![item(
            "changes",
            "turn_a",
            1,
            SceneItemKind::ChangeSet {
                files: files.clone(),
            },
            Some(SceneDisclosure::Open),
        )],
        TurnNarration::Quiet,
    );

    let change = scene.turn_scenes()[0]
        .blocks()
        .iter()
        .find_map(|block| match block {
            TurnBlock::ChangeSet(change) => Some(change),
            _ => None,
        })
        .expect("terminal change card is present");
    assert_eq!(change.files.len(), 4);
    assert_eq!(
        change
            .files
            .iter()
            .map(|file| (file.path.as_str(), file_change_status_label(file.status)))
            .collect::<Vec<_>>(),
        vec![
            ("src/added.rs", "Added"),
            ("src/modified.rs", "Modified"),
            ("src/removed.rs", "Removed"),
            ("src/renamed.rs", "Renamed"),
        ]
    );
    assert_eq!(
        changed_file_selector(&scene_id("changes"), 1),
        "artisan-conversation-surface-change-changes-file-1"
    );
}

#[test]
fn steering_selector_and_label_follow_the_scene_block_position() {
    let user = item(
        "user",
        "turn_a",
        1,
        SceneItemKind::UserMessage {
            body: "before steering".to_owned(),
        },
        None,
    );
    let assistant = item(
        "assistant",
        "turn_a",
        2,
        SceneItemKind::AssistantMessage {
            body: "after steering".to_owned(),
            phase: AssistantPhase::Final,
        },
        None,
    );
    let scene = ConversationScene::build(
        vec![turn("turn_a")],
        vec![user, assistant],
        vec![narration("turn_a", TurnNarration::Quiet)],
        vec![
            SteeringPlacement::new(scene_id("steer"), item_id("user"), "Continue here")
                .expect("steering is valid"),
        ],
    )
    .expect("scene is valid");
    let blocks = scene.turn_scenes()[0].blocks();

    assert!(matches!(&blocks[0], TurnBlock::UserMessage(_)));
    let steering_block = &blocks[1];
    assert!(matches!(steering_block, TurnBlock::SteeringLabel(_)));
    assert!(matches!(&blocks[2], TurnBlock::AssistantMessage(_)));
    if let TurnBlock::SteeringLabel(steering) = steering_block {
        assert_eq!(steering.label, "Continue here");
        assert_eq!(
            block_selector(&turn_id("turn_a"), steering_block),
            steering_selector(&turn_id("turn_a"), &scene_id("steer"))
        );
    }
}

#[test]
fn disclosure_action_contains_only_stable_id_and_requested_value() {
    let scene = scene(
        vec![item(
            "card",
            "turn_a",
            1,
            SceneItemKind::Plan {
                title: "authoritative title".to_owned(),
                entries: vec!["authoritative entry".to_owned()],
            },
            Some(SceneDisclosure::Open),
        )],
        TurnNarration::Quiet,
    );
    let before = scene.clone();
    let action = ConversationSurfaceAction::DisclosureToggleRequested {
        id: scene_id("card"),
        requested_open: false,
    };

    assert_eq!(scene, before);
    assert_eq!(
        action,
        ConversationSurfaceAction::DisclosureToggleRequested {
            id: scene_id("card"),
            requested_open: false,
        }
    );
}

#[test]
fn elapsed_formatting_is_deterministic_at_all_unit_boundaries() {
    assert_eq!(format_elapsed_millis(0), "0s");
    assert_eq!(format_elapsed_millis(999), "0s");
    assert_eq!(format_elapsed_millis(1_000), "1s");
    assert_eq!(format_elapsed_millis(59_999), "59s");
    assert_eq!(format_elapsed_millis(60_000), "1m 0s");
    assert_eq!(format_elapsed_millis(3_600_000 + 3_000), "1h 0m 3s");
    assert_eq!(format_elapsed_millis(u64::MAX), "5124095576030h 25m 51s");
}

#[test]
fn error_approval_question_and_transition_are_distinct_kinds() {
    let scene = scene(
        vec![
            item(
                "approval",
                "turn_a",
                1,
                SceneItemKind::Approval {
                    prompt: "approval".to_owned(),
                },
                None,
            ),
            item(
                "question",
                "turn_a",
                2,
                SceneItemKind::Question {
                    prompt: "question".to_owned(),
                },
                None,
            ),
            item(
                "error",
                "turn_a",
                3,
                SceneItemKind::Error {
                    message: "error".to_owned(),
                },
                None,
            ),
            item(
                "transition",
                "turn_a",
                4,
                SceneItemKind::ModelTransition {
                    from_model: "old".to_owned(),
                    to_model: "new".to_owned(),
                },
                None,
            ),
        ],
        TurnNarration::Quiet,
    );
    let kinds = ordered_block_kinds(&scene);
    assert_eq!(
        &kinds[..4],
        &[
            RenderedBlockKind::Approval,
            RenderedBlockKind::Question,
            RenderedBlockKind::Error,
            RenderedBlockKind::ModelTransition,
        ]
    );
    assert_ne!(kinds[0], kinds[1]);
    assert_ne!(kinds[1], kinds[2]);
    assert_ne!(kinds[2], kinds[3]);
}

#[test]
fn selectors_never_include_scene_payload_text() {
    let body = "body text / no-selector";
    let path = "src/payload-path.rs";
    let scene = scene(
        vec![
            item(
                "stable-user",
                "turn_a",
                1,
                SceneItemKind::UserMessage {
                    body: body.to_owned(),
                },
                None,
            ),
            item(
                "stable-change",
                "turn_a",
                2,
                SceneItemKind::ChangeSet {
                    files: vec![
                        SceneFileChange::new(path, FileChangeStatus::Modified)
                            .expect("path is valid"),
                    ],
                },
                None,
            ),
        ],
        TurnNarration::Quiet,
    );
    let blocks = scene.turn_scenes()[0].blocks();
    let user_selector = block_selector(&turn_id("turn_a"), &blocks[0]);
    let change = blocks
        .iter()
        .find(|block| matches!(block, TurnBlock::ChangeSet(_)))
        .expect("change card is present");
    let change_selector = block_selector(&turn_id("turn_a"), change);

    assert_eq!(
        turn_selector(&turn_id("turn_a")),
        "artisan-conversation-surface-turn-turn_a"
    );
    assert!(!user_selector.contains(body));
    assert!(!change_selector.contains(path));
    assert!(!changed_file_selector(&scene_id("stable-change"), 0).contains(path));
    assert!(!steering_selector(&turn_id("turn_a"), &scene_id("steer")).contains(body));
}

#[gpui::test]
fn disclosure_click_emits_request_without_mutating_scene(cx: &mut TestAppContext) {
    const CHANGE_DISCLOSURE_TRIGGER: &str =
        "artisan-conversation-surface-turn-turn_a-block-change-changes-disclosure-trigger";

    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(
                vec![item(
                    "changes",
                    "turn_a",
                    1,
                    SceneItemKind::ChangeSet {
                        files: vec![
                            SceneFileChange::new("src/changed.rs", FileChangeStatus::Modified)
                                .expect("path is valid"),
                        ],
                    },
                    Some(SceneDisclosure::Open),
                )],
                TurnNarration::Quiet,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    cx.run_until_parked();

    let trigger = cx
        .debug_bounds(CHANGE_DISCLOSURE_TRIGGER)
        .expect("controlled disclosure trigger must paint bounds");
    cx.simulate_click(trigger.center(), Modifiers::none());
    cx.run_until_parked();

    cx.update(|_, app| {
        let surface = surface.read(app);
        assert_eq!(
            surface.pending_actions(),
            &[ConversationSurfaceAction::DisclosureToggleRequested {
                id: scene_id("changes"),
                requested_open: false,
            }]
        );
        let change = surface.scene().turn_scenes()[0]
            .blocks()
            .iter()
            .find_map(|block| match block {
                TurnBlock::ChangeSet(change) => Some(change),
                _ => None,
            })
            .expect("change card remains in the scene");
        assert_eq!(change.disclosure, Some(SceneDisclosure::Open));
    });
}

#[gpui::test]
fn mounted_surface_exposes_root_viewport_and_keyboard_focus_bounds(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(
                vec![item(
                    "user",
                    "turn_a",
                    1,
                    SceneItemKind::UserMessage {
                        body: "rendered body".to_owned(),
                    },
                    None,
                )],
                TurnNarration::Quiet,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    cx.run_until_parked();

    let root = cx
        .debug_bounds(ROOT_SELECTOR)
        .expect("surface root must paint bounds");
    let viewport = cx
        .debug_bounds(VIEWPORT_SELECTOR)
        .expect("surface viewport must paint bounds");
    assert_eq!(root.size, viewport.size);
    assert!(root.size.width > px(0.0));
    assert!(root.size.height > px(0.0));

    cx.update(|window, app| {
        let focus = surface.read(app).transcript_focus_handle().clone();
        window.focus(&focus);
        assert!(focus.is_focused(window));
    });
}

fn item_id(value: &str) -> ItemId {
    ItemId::parse(value).expect("item id is valid")
}
