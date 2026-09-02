//! Deterministic contract tests for the native conversation surface.
//!
//! The pure selector, status, ordering, and action projections avoid a window;
//! the final two tests use the existing in-memory GPUI harness. Tests cross the
//! public frontend crate boundary.

use artisan_frontend::{conversation_scene, conversation_surface};

use artisan_domain::{ConversationLifecycle, ItemId, TurnId};
use artisan_ui::theme::ThemeMode;
use conversation_scene::{
    AssistantPhase, ConversationScene, FileChangeStatus, SceneDisclosure, SceneFileChange, SceneId,
    SceneItem, SceneItemKind, SceneTurn, SteeringPlacement, TurnBlock, TurnNarration,
    TurnNarrationEntry,
};
use conversation_surface::{
    CONVERSATION_SURFACE_MAX_ACTIONS, ConversationSurface, ConversationSurfaceAction,
    ConversationSurfaceTarget, JUMP_TO_LATEST_SELECTOR, ROOT_SELECTOR, RenderedBlockKind,
    TURN_NAVIGATOR_CONTROL_PREFIX, TURN_NAVIGATOR_SELECTOR, VIEWPORT_SELECTOR, ViewportObservation,
    block_selector, changed_file_selector, file_change_status_label, format_elapsed_millis,
    ordered_block_kinds, steering_selector, turn_selector, turn_status_copy,
};
use gpui::{KeyUpEvent, Keystroke, Modifiers, TestAppContext, VisualTestContext, point, px, size};

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

fn markdown_replacement_scenes() -> (ConversationScene, ConversationScene) {
    let initial = scene(
        vec![item(
            "old-user",
            "turn_a",
            1,
            SceneItemKind::UserMessage {
                body: "old authoritative body".to_owned(),
            },
            None,
        )],
        TurnNarration::Quiet,
    );
    let replacement = scene(
        vec![
            item(
                "new-assistant",
                "turn_a",
                1,
                SceneItemKind::AssistantMessage {
                    body: "new authoritative body with `code`".to_owned(),
                    phase: AssistantPhase::Final,
                },
                None,
            ),
            item(
                "replacement-change",
                "turn_a",
                2,
                SceneItemKind::ChangeSet {
                    files: vec![
                        SceneFileChange::new("src/replacement.rs", FileChangeStatus::Modified)
                            .expect("replacement path is valid"),
                    ],
                },
                Some(SceneDisclosure::Open),
            ),
        ],
        TurnNarration::Quiet,
    );
    (initial, replacement)
}

struct SurfaceWindowHost {
    surface: gpui::Entity<ConversationSurface>,
}

impl gpui::Render for SurfaceWindowHost {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        self.surface.clone()
    }
}

fn drain_surface_actions(
    surface: &gpui::Entity<ConversationSurface>,
    cx: &mut gpui::VisualTestContext,
) -> Vec<ConversationSurfaceAction> {
    cx.update(|_, app| surface.update(app, |surface, _| surface.take_actions()))
}

fn complete_key_press(cx: &mut VisualTestContext, key: &'static str) {
    // Pinned GPUI's simulate_keystrokes sends the down half; Button's shared
    // Enter/Space activation is synthesized from the physical key release.
    cx.simulate_keystrokes(key);
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(key).expect("known keyboard activation key"),
    });
}

fn tall_scene() -> ConversationScene {
    let body = (0..160)
        .map(|line| format!("Transcript line {line} keeps the viewport measurable."))
        .collect::<Vec<_>>()
        .join("\n");
    scene(
        vec![item(
            "tall-user",
            "turn_a",
            1,
            SceneItemKind::UserMessage { body },
            None,
        )],
        TurnNarration::Quiet,
    )
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
    let scene = scene(ordered_scene_items(), TurnNarration::Quiet);

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

fn ordered_scene_items() -> Vec<SceneItem> {
    vec![
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
    ]
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
    let _ = drain_surface_actions(&surface, cx);

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

#[gpui::test]
fn jump_to_latest_is_an_overlay_and_pointer_keyboard_activation_is_typed(cx: &mut TestAppContext) {
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
    let root_before = cx.debug_bounds(ROOT_SELECTOR).expect("surface root bounds");
    let viewport_before = cx
        .debug_bounds(VIEWPORT_SELECTOR)
        .expect("surface viewport bounds");
    let _ = drain_surface_actions(&surface, cx);

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.set_jump_to_latest_visible(true, surface_cx);
        });
    });
    cx.run_until_parked();

    assert_eq!(cx.debug_bounds(ROOT_SELECTOR), Some(root_before));
    assert_eq!(cx.debug_bounds(VIEWPORT_SELECTOR), Some(viewport_before));
    let button = cx
        .debug_bounds(JUMP_TO_LATEST_SELECTOR)
        .expect("visible jump control must paint a stable selector");

    cx.simulate_click(button.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(
        drain_surface_actions(&surface, cx)
            .iter()
            .any(|action| matches!(action, ConversationSurfaceAction::JumpToLatestRequested))
    );

    cx.update(|window, app| {
        let focus = surface.read(app).transcript_focus_handle().clone();
        window.focus(&focus);
        window.focus_next();
    });
    complete_key_press(cx, "enter");
    complete_key_press(cx, "space");
    let keyboard_actions = drain_surface_actions(&surface, cx);
    assert_eq!(
        keyboard_actions
            .iter()
            .filter(|action| matches!(action, ConversationSurfaceAction::JumpToLatestRequested))
            .count(),
        2
    );
}

#[gpui::test]
fn viewport_geometry_reports_top_near_bottom_and_detached_positions(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(tall_scene(), ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    cx.run_until_parked();

    let handle = cx.update(|_, app| surface.read(app).scroll_handle().clone());
    let (max_height, viewport_height) = cx.update(|_, app| {
        let surface = surface.read(app);
        (
            surface.scroll_handle().max_offset().height,
            surface.scroll_handle().bounds().size.height,
        )
    });
    assert!(max_height > viewport_height * 0.06 + px(64.0));
    let tolerance = (viewport_height * 0.06).max(px(64.0));
    let initial_observations = drain_surface_actions(&surface, cx);
    let initial_observation = initial_observations.iter().find_map(|action| match action {
        ConversationSurfaceAction::ViewportObserved(observation) => Some(observation),
        _ => None,
    });
    assert_eq!(
        initial_observation.map(|observation| observation.at_bottom),
        Some(false)
    );
    let positions = [
        (max_height - (tolerance - px(1.0)), true),
        (max_height - (tolerance + px(1.0)), false),
    ];

    for (scroll_top, expected_at_bottom) in positions {
        handle.set_offset(point(px(0.0), -scroll_top));
        cx.update(|_, app| surface.update(app, |_, surface_cx| surface_cx.notify()));
        cx.run_until_parked();
        let observations = drain_surface_actions(&surface, cx);
        let observation = observations.iter().find_map(|action| match action {
            ConversationSurfaceAction::ViewportObserved(observation) => Some(observation),
            _ => None,
        });
        assert_eq!(
            observation.map(|observation| observation.at_bottom),
            Some(expected_at_bottom)
        );
        assert!(observation.is_some_and(|observation| {
            observation.first_visible.is_none() && observation.last_visible.is_none()
        }));
    }
}

#[gpui::test]
fn viewport_observations_deduplicate_and_retry_after_queue_backpressure(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(Vec::new(), TurnNarration::Quiet),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.run_until_parked();
    let _ = drain_surface_actions(&surface, cx);

    let first = ViewportObservation {
        first_visible: None,
        last_visible: None,
        at_bottom: false,
    };
    let retry = ViewportObservation {
        first_visible: None,
        last_visible: None,
        at_bottom: true,
    };
    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            assert!(surface.observe_viewport(first.clone(), surface_cx));
            assert!(!surface.observe_viewport(first.clone(), surface_cx));
            assert_eq!(surface.pending_actions().len(), 1);
            let _ = surface.take_actions();
            for _ in 0..conversation_surface::CONVERSATION_SURFACE_MAX_ACTIONS {
                assert!(surface.request_scroll(
                    ConversationSurfaceTarget::Scene(scene_id("fill")),
                    surface_cx,
                ));
            }
            assert!(!surface.observe_viewport(retry.clone(), surface_cx));
            assert_eq!(
                surface.pending_actions().len(),
                conversation_surface::CONVERSATION_SURFACE_MAX_ACTIONS
            );
        });
    });

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            let _ = surface.take_next_action();
            assert!(!surface.observe_viewport(retry.clone(), surface_cx));
            assert!(matches!(
                surface.pending_actions().last(),
                Some(ConversationSurfaceAction::ViewportObserved(observation))
                    if observation == &retry
            ));
        });
    });
}

#[gpui::test]
fn markdown_message_body_mounts_heading_paragraph_inline_code_and_fence(cx: &mut TestAppContext) {
    const USER_MARKDOWN: &str = "artisan-conversation-surface-turn-turn_a-block-user-user-markdown";
    const USER_HEADING: &str =
        "artisan-conversation-surface-turn-turn_a-block-user-user-markdown-block-0";
    const USER_PARAGRAPH: &str =
        "artisan-conversation-surface-turn-turn_a-block-user-user-markdown-block-1";
    const USER_CODE: &str =
        "artisan-conversation-surface-turn-turn_a-block-user-user-markdown-block-2-code";
    const ASSISTANT_MARKDOWN: &str =
        "artisan-conversation-surface-turn-turn_a-block-assistant-assistant-markdown";
    const ASSISTANT_HEADING: &str =
        "artisan-conversation-surface-turn-turn_a-block-assistant-assistant-markdown-block-0";
    const ASSISTANT_PARAGRAPH: &str =
        "artisan-conversation-surface-turn-turn_a-block-assistant-assistant-markdown-block-1";
    const ASSISTANT_CODE: &str =
        "artisan-conversation-surface-turn-turn_a-block-assistant-assistant-markdown-block-2-code";
    let body =
        "# Message heading\n\nParagraph with `inline code`.\n\n```rust\nlet answer = 42;\n```";
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(
                vec![
                    item(
                        "user",
                        "turn_a",
                        1,
                        SceneItemKind::UserMessage {
                            body: body.to_owned(),
                        },
                        None,
                    ),
                    item(
                        "assistant",
                        "turn_a",
                        2,
                        SceneItemKind::AssistantMessage {
                            body: body.to_owned(),
                            phase: AssistantPhase::Final,
                        },
                        None,
                    ),
                ],
                TurnNarration::Quiet,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(640.0)));
    cx.run_until_parked();

    for selector in [
        USER_MARKDOWN,
        USER_HEADING,
        USER_PARAGRAPH,
        USER_CODE,
        ASSISTANT_MARKDOWN,
        ASSISTANT_HEADING,
        ASSISTANT_PARAGRAPH,
        ASSISTANT_CODE,
    ] {
        let bounds = cx
            .debug_bounds(selector)
            .expect("accepted Markdown structure must paint bounds");
        assert!(bounds.size.height > px(0.0), "{selector} must be visible");
    }
    let _ = drain_surface_actions(&surface, cx);
    cx.update(|_, app| assert!(surface.read(app).pending_actions().is_empty()));
}

#[gpui::test]
fn markdown_open_unknown_fence_and_html_are_inert(cx: &mut TestAppContext) {
    const OPEN_MARKDOWN: &str = "artisan-conversation-surface-turn-turn_a-block-user-open-markdown";
    const UNKNOWN_MARKDOWN: &str =
        "artisan-conversation-surface-turn-turn_a-block-assistant-unknown-markdown";
    const HTML_MARKDOWN: &str = "artisan-conversation-surface-turn-turn_a-block-user-html-markdown";
    const HTML_BLOCK: &str =
        "artisan-conversation-surface-turn-turn_a-block-user-html-markdown-block-0";
    const OPEN_CODE: &str =
        "artisan-conversation-surface-turn-turn_a-block-user-open-markdown-block-0-code";
    const UNKNOWN_CODE: &str =
        "artisan-conversation-surface-turn-turn_a-block-assistant-unknown-markdown-block-0-code";
    const HTML_CODE: &str =
        "artisan-conversation-surface-turn-turn_a-block-user-html-markdown-block-0-code";
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(
                vec![
                    item(
                        "open",
                        "turn_a",
                        1,
                        SceneItemKind::UserMessage {
                            body: "```rust\nlet value = 1;\n".to_owned(),
                        },
                        None,
                    ),
                    item(
                        "unknown",
                        "turn_a",
                        2,
                        SceneItemKind::AssistantMessage {
                            body: "```not-a-language\nlet value = 2;\n```".to_owned(),
                            phase: AssistantPhase::Final,
                        },
                        None,
                    ),
                    item(
                        "html",
                        "turn_a",
                        3,
                        SceneItemKind::UserMessage {
                            body: "<div>literal HTML</div>".to_owned(),
                        },
                        None,
                    ),
                ],
                TurnNarration::Quiet,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    cx.run_until_parked();

    for selector in [OPEN_MARKDOWN, UNKNOWN_MARKDOWN, HTML_MARKDOWN, HTML_BLOCK] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "inert source must remain mounted at {selector}"
        );
    }
    for selector in [OPEN_CODE, UNKNOWN_CODE, HTML_CODE] {
        assert!(
            cx.debug_bounds(selector).is_none(),
            "inert source must not expose a highlighted code selector: {selector}"
        );
    }
    let _ = drain_surface_actions(&surface, cx);
    cx.update(|_, app| assert!(surface.read(app).pending_actions().is_empty()));
}

#[gpui::test]
fn markdown_scene_replacement_preserves_authority_and_actions(cx: &mut TestAppContext) {
    const OLD_MARKDOWN: &str =
        "artisan-conversation-surface-turn-turn_a-block-user-old-user-markdown";
    const NEW_MARKDOWN: &str =
        "artisan-conversation-surface-turn-turn_a-block-assistant-new-assistant-markdown";
    const DISCLOSURE_TRIGGER: &str = "artisan-conversation-surface-turn-turn_a-block-change-replacement-change-disclosure-trigger";
    let (initial, replacement) = markdown_replacement_scenes();
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(initial, ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    cx.run_until_parked();
    let _ = drain_surface_actions(&surface, cx);
    assert!(cx.debug_bounds(OLD_MARKDOWN).is_some());

    let mut replacement_app = (*cx).clone();
    let (_, replacement_cx) = replacement_app.add_window_view(|_, _| SurfaceWindowHost {
        surface: surface.clone(),
    });
    assert!(replacement_cx.debug_bounds(OLD_MARKDOWN).is_some());

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.replace_scene(replacement, surface_cx);
        });
    });
    cx.run_until_parked();

    assert!(cx.debug_bounds(NEW_MARKDOWN).is_some());
    let _ = drain_surface_actions(&surface, cx);

    // Pinned GPUI 0.2.2's `Frame::clear` does not clear `debug_bounds`, while
    // `VisualTestContext::debug_bounds` reads the rendered frame's map. After
    // the original window has painted the old scene in both alternating frame
    // maps, its old key is historical rather than current-frame geometry. The
    // fresh test window painted the old scene once before replacement, so its
    // clean alternate frame is an honest current-frame retirement probe for
    // the same authoritative surface entity.
    assert!(replacement_cx.debug_bounds(OLD_MARKDOWN).is_none());
    assert!(replacement_cx.debug_bounds(NEW_MARKDOWN).is_some());

    let trigger = cx
        .debug_bounds(DISCLOSURE_TRIGGER)
        .expect("replacement disclosure trigger must remain mounted");
    cx.simulate_click(trigger.center(), Modifiers::none());
    cx.run_until_parked();

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            assert!(surface.observe_viewport(
                ViewportObservation {
                    first_visible: Some(ConversationSurfaceTarget::Scene(scene_id("turn_a"))),
                    last_visible: None,
                    at_bottom: false,
                },
                surface_cx,
            ));
            assert!(surface.request_scroll(
                ConversationSurfaceTarget::Scene(scene_id("turn_a")),
                surface_cx,
            ));
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        assert_eq!(
            surface.read(app).pending_actions(),
            &[
                ConversationSurfaceAction::DisclosureToggleRequested {
                    id: scene_id("replacement-change"),
                    requested_open: false,
                },
                ConversationSurfaceAction::ViewportObserved(ViewportObservation {
                    first_visible: Some(ConversationSurfaceTarget::Scene(scene_id("turn_a"))),
                    last_visible: None,
                    at_bottom: false,
                }),
                ConversationSurfaceAction::ScrollIntent {
                    target: ConversationSurfaceTarget::Scene(scene_id("turn_a")),
                },
            ]
        );
    });
}

fn item_id(value: &str) -> ItemId {
    ItemId::parse(value).expect("item id is valid")
}

fn navigator_scene() -> ConversationScene {
    ConversationScene::build(
        vec![
            SceneTurn::new(turn_id("turn_a"), 0, ConversationLifecycle::Completed),
            SceneTurn::new(turn_id("turn_b"), 1, ConversationLifecycle::Completed),
        ],
        vec![
            item(
                "nav-first",
                "turn_a",
                1,
                SceneItemKind::UserMessage {
                    body: "first question".to_owned(),
                },
                None,
            ),
            item(
                "nav-assistant",
                "turn_a",
                2,
                SceneItemKind::AssistantMessage {
                    body: "reply".to_owned(),
                    phase: AssistantPhase::Final,
                },
                None,
            ),
            item(
                "nav-second",
                "turn_b",
                3,
                SceneItemKind::UserMessage {
                    body: "second question".to_owned(),
                },
                None,
            ),
        ],
        vec![
            narration("turn_a", TurnNarration::Quiet),
            narration("turn_b", TurnNarration::Quiet),
        ],
        Vec::new(),
    )
    .expect("navigator scene is valid")
}

fn navigator_control_selector(id: &str) -> String {
    format!("{TURN_NAVIGATOR_CONTROL_PREFIX}-{id}")
}

fn mount_navigator_scene(
    scene: ConversationScene,
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<ConversationSurface>,
    &mut VisualTestContext,
) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(scene, ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    cx.run_until_parked();
    let _ = drain_surface_actions(&surface, cx);
    (surface, cx)
}

#[gpui::test]
fn loaded_turn_navigator_hides_without_user_messages(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(
                vec![item(
                    "solo-assistant",
                    "turn_a",
                    1,
                    SceneItemKind::AssistantMessage {
                        body: "reply".to_owned(),
                        phase: AssistantPhase::Final,
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
    let _ = drain_surface_actions(&surface, cx);
    assert!(cx.debug_bounds(TURN_NAVIGATOR_SELECTOR).is_none());
}

#[gpui::test]
fn loaded_turn_navigator_hides_for_a_single_marker(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(
                vec![item(
                    "lone-user",
                    "turn_a",
                    1,
                    SceneItemKind::UserMessage {
                        body: "only question".to_owned(),
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
    let _ = drain_surface_actions(&surface, cx);
    assert!(cx.debug_bounds(TURN_NAVIGATOR_SELECTOR).is_none());
}

#[gpui::test]
fn loaded_turn_navigator_renders_oldest_first_item_controls(cx: &mut TestAppContext) {
    let (_surface, cx) = mount_navigator_scene(navigator_scene(), cx);
    assert!(cx.debug_bounds(TURN_NAVIGATOR_SELECTOR).is_some());
    let first = cx
        .debug_bounds(navigator_control_selector("nav-first"))
        .expect("oldest marker must paint a stable control");
    let second = cx
        .debug_bounds(navigator_control_selector("nav-second"))
        .expect("newest marker must paint a stable control");
    assert!(
        first.center().y < second.center().y,
        "oldest marker paints above the newest marker"
    );
    assert!(cx.debug_bounds(navigator_control_selector("nav-assistant")).is_none());
}

#[gpui::test]
fn loaded_turn_navigator_suppresses_empty_labels(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            ConversationScene::build(
                vec![
                    SceneTurn::new(turn_id("turn_a"), 0, ConversationLifecycle::Completed),
                    SceneTurn::new(turn_id("turn_b"), 1, ConversationLifecycle::Completed),
                ],
                vec![
                    item(
                        "nav-blank",
                        "turn_a",
                        1,
                        SceneItemKind::UserMessage {
                            body: "   ".to_owned(),
                        },
                        None,
                    ),
                    item(
                        "nav-first",
                        "turn_a",
                        2,
                        SceneItemKind::UserMessage {
                            body: "first question".to_owned(),
                        },
                        None,
                    ),
                    item(
                        "nav-second",
                        "turn_b",
                        3,
                        SceneItemKind::UserMessage {
                            body: "second question".to_owned(),
                        },
                        None,
                    ),
                ],
                vec![
                    narration("turn_a", TurnNarration::Quiet),
                    narration("turn_b", TurnNarration::Quiet),
                ],
                Vec::new(),
            )
            .expect("navigator scene is valid"),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    cx.run_until_parked();
    let _ = drain_surface_actions(&surface, cx);
    assert!(cx.debug_bounds(TURN_NAVIGATOR_SELECTOR).is_some());
    assert!(cx.debug_bounds(navigator_control_selector("nav-blank")).is_none());
    assert!(cx.debug_bounds(navigator_control_selector("nav-first")).is_some());
    assert!(cx.debug_bounds(navigator_control_selector("nav-second")).is_some());
}

#[gpui::test]
fn loaded_turn_navigator_pointer_activation_emits_exact_item_scroll_intent(
    cx: &mut TestAppContext,
) {
    let (surface, cx) = mount_navigator_scene(navigator_scene(), cx);
    let offset_before = cx.update(|_, app| surface.read(app).scroll_handle().offset());
    let button = cx
        .debug_bounds(navigator_control_selector("nav-first"))
        .expect("oldest marker must paint a stable control");
    cx.simulate_click(button.center(), Modifiers::none());
    cx.run_until_parked();
    let offset_after = cx.update(|_, app| surface.read(app).scroll_handle().offset());
    assert_eq!(offset_before, offset_after);
    let intents: Vec<ConversationSurfaceTarget> = drain_surface_actions(&surface, cx)
        .into_iter()
        .filter_map(|action| match action {
            ConversationSurfaceAction::ScrollIntent { target } => Some(target),
            _ => None,
        })
        .collect();
    assert_eq!(
        intents,
        [ConversationSurfaceTarget::Item(item_id("nav-first"))]
    );
}

#[gpui::test]
fn loaded_turn_navigator_keyboard_activation_matches_pointer(cx: &mut TestAppContext) {
    let (surface, cx) = mount_navigator_scene(navigator_scene(), cx);
    let target = ConversationSurfaceTarget::Item(item_id("nav-second"));
    cx.update(|window, app| {
        let focus = surface
            .read(app)
            .navigator_focus_handle(&target)
            .expect("navigator control retains its focus handle");
        window.focus(&focus);
    });
    complete_key_press(cx, "enter");
    complete_key_press(cx, "space");
    let intents: Vec<ConversationSurfaceTarget> = drain_surface_actions(&surface, cx)
        .into_iter()
        .filter_map(|action| match action {
            ConversationSurfaceAction::ScrollIntent { target } => Some(target),
            _ => None,
        })
        .collect();
    assert_eq!(intents, [target.clone(), target]);
}

#[gpui::test]
fn loaded_turn_navigator_replacement_prunes_stale_focus_without_selection(
    cx: &mut TestAppContext,
) {
    let (surface, cx) = mount_navigator_scene(navigator_scene(), cx);
    let first_target = ConversationSurfaceTarget::Item(item_id("nav-first"));
    cx.update(|window, app| {
        let focus = surface
            .read(app)
            .navigator_focus_handle(&first_target)
            .expect("navigator control retains its focus handle");
        window.focus(&focus);
    });
    cx.run_until_parked();
    let mut replacement_app = (*cx).clone();
    let (_, replacement_cx) = replacement_app.add_window_view(|_, _| SurfaceWindowHost {
        surface: surface.clone(),
    });
    assert!(replacement_cx.debug_bounds(TURN_NAVIGATOR_SELECTOR).is_some());

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.replace_scene(
                scene(
                    vec![item(
                        "nav-second",
                        "turn_a",
                        1,
                        SceneItemKind::UserMessage {
                            body: "lone survivor".to_owned(),
                        },
                        None,
                    )],
                    TurnNarration::Quiet,
                ),
                surface_cx,
            );
        });
    });
    cx.run_until_parked();

    assert!(replacement_cx.debug_bounds(TURN_NAVIGATOR_SELECTOR).is_none());
    cx.update(|window, app| {
        let surface = surface.read(app);
        assert!(surface.navigator_focus_handle(&first_target).is_none());
        assert!(
            surface
                .transcript_focus_handle()
                .is_focused(window)
        );
    });
    let _ = drain_surface_actions(&surface, cx);
}

#[gpui::test]
fn loaded_turn_navigator_activation_respects_action_backpressure(cx: &mut TestAppContext) {
    let (surface, cx) = mount_navigator_scene(navigator_scene(), cx);
    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            for _ in 0..CONVERSATION_SURFACE_MAX_ACTIONS {
                assert!(surface.request_scroll(
                    ConversationSurfaceTarget::Scene(scene_id("fill")),
                    surface_cx,
                ));
            }
            assert_eq!(
                surface.pending_actions().len(),
                CONVERSATION_SURFACE_MAX_ACTIONS
            );
        });
    });
    let button = cx
        .debug_bounds(navigator_control_selector("nav-first"))
        .expect("oldest marker must paint a stable control");
    cx.simulate_click(button.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        surface.update(app, |surface, _| {
            let pending = surface.pending_actions();
            assert_eq!(pending.len(), CONVERSATION_SURFACE_MAX_ACTIONS);
            assert!(
                !pending.iter().any(|action| matches!(
                    action,
                    ConversationSurfaceAction::ScrollIntent {
                        target: ConversationSurfaceTarget::Item(_)
                    }
                ))
            );
        });
    });
}
