//! Black-box coverage for the bounded conversation composition controller.
//!
//! Tests use only the public frontend crate boundary.

use artisan_frontend::{
    conversation_delivery_machine, conversation_scene, conversation_state_machine,
    conversation_steering_machine, conversation_turn_machine, conversation_view_machine,
};

use artisan_domain::{
    AssistantBody, AssistantMessageItem, AssistantMessagePhase, ConversationCursor,
    ConversationItem, ConversationLifecycle, ConversationPatch, ConversationSnapshot,
    ConversationTurn, IncrementalText, ItemId, ItemOrdinal, MessageBody, PatchBatch, PatchId,
    PatchSequence, Revision, RunId, ThreadId, TurnId, TurnOrdinal, UnixMillis, UserMessageItem,
};

use conversation_delivery_machine::{
    ConversationDeliveryEffect, ConversationDeliveryEvent, DeliveryPhase,
};
use conversation_scene::{
    FileChangeStatus, SceneDisclosure, SceneFileChange, SceneId, TurnBlock,
    TurnNarration as SceneTurnNarration,
};
use conversation_state_machine::{
    CapacityResource, ConversationStateController, ConversationStateEffect, ConversationStateError,
    MAX_PENDING_EFFECTS, SceneFact, SceneFactKind,
};
use conversation_steering_machine::{
    SourceReference, SteeringEffect, SteeringEvent, SteeringLabelKind, SteeringPlacement,
};
use conversation_turn_machine::{TurnError, TurnEvent};
use conversation_view_machine::{
    DisclosureEvent, ViewportEffect, ViewportEvent, ViewportGeneration, ViewportState,
};

const THREAD: &str = "thread_controller";
const TURN_A: &str = "turn_a";
const TURN_B: &str = "turn_b";
const USER_A: &str = "user_a";
const USER_B: &str = "user_b";
const ASSISTANT_A: &str = "assistant_a";

fn thread_id() -> ThreadId {
    ThreadId::parse(THREAD).expect("valid thread id")
}

fn turn_id(value: &str) -> TurnId {
    TurnId::parse(value).expect("valid turn id")
}

fn item_id(value: &str) -> ItemId {
    ItemId::parse(value).expect("valid item id")
}

fn patch_id(value: &str) -> PatchId {
    PatchId::parse(value).expect("valid patch id")
}

fn stamp(millis: i64) -> UnixMillis {
    UnixMillis::from_millis(millis)
}

fn make_turn(id: &str, ordinal: u64, lifecycle: ConversationLifecycle) -> ConversationTurn {
    ConversationTurn {
        turn_id: turn_id(id),
        ordinal: TurnOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle,
        created_at: stamp(0),
        updated_at: stamp(10),
    }
}

fn make_user(id: &str, turn: &str, ordinal: u64, body: &str) -> ConversationItem {
    ConversationItem::UserMessage(UserMessageItem {
        item_id: item_id(id),
        turn_id: turn_id(turn),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Pending,
        body: MessageBody::parse(body.to_owned()).expect("valid user body"),
        created_at: stamp(1),
        updated_at: stamp(10),
    })
}

fn make_assistant(
    id: &str,
    turn: &str,
    ordinal: u64,
    body: &str,
    phase: AssistantMessagePhase,
) -> ConversationItem {
    ConversationItem::AssistantMessage(AssistantMessageItem {
        item_id: item_id(id),
        turn_id: turn_id(turn),
        run_id: RunId::parse("run_controller").expect("valid run id"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Pending,
        body: AssistantBody::parse(body.to_owned()).expect("valid assistant body"),
        phase,
        created_at: stamp(2),
        updated_at: stamp(10),
    })
}

fn snapshot(
    cursor: u64,
    turns: Vec<ConversationTurn>,
    items: Vec<ConversationItem>,
) -> ConversationSnapshot {
    ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(cursor),
        turns,
        items,
        stamp(10),
    )
    .expect("valid authoritative snapshot")
}

fn baseline_snapshot() -> ConversationSnapshot {
    snapshot(
        1,
        vec![make_turn(TURN_A, 0, ConversationLifecycle::Completed)],
        vec![
            make_user(USER_A, TURN_A, 1, "hello"),
            make_assistant(
                ASSISTANT_A,
                TURN_A,
                2,
                "hello back",
                AssistantMessagePhase::Final,
            ),
        ],
    )
}

fn gap_batch() -> PatchBatch {
    PatchBatch::new(
        thread_id(),
        ConversationCursor::new(2),
        ConversationCursor::new(3),
        vec![ConversationPatch::ItemAppend {
            patch_id: patch_id("gap_patch"),
            sequence: PatchSequence::new(3).expect("valid patch sequence"),
            item_id: item_id(USER_A),
            revision: Revision::new(1),
            text: IncrementalText::parse("ignored").expect("valid fragment"),
            updated_at: stamp(11),
        }],
    )
    .expect("valid gap batch envelope")
}

fn scene_status(
    controller: &ConversationStateController,
    turn: &str,
) -> (SceneTurnNarration, usize) {
    let scene = controller.scene().expect("scene builds");
    let turn_scene = scene.turn_scene(&turn_id(turn)).expect("turn scene exists");
    let statuses: Vec<_> = turn_scene
        .blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::TurnStatus(status) => Some(status.narration),
            _ => None,
        })
        .collect();
    (statuses[0], statuses.len())
}

fn steering_source(value: &str) -> SourceReference {
    SourceReference::parse(value.to_owned()).expect("valid source reference")
}

fn register_steering(controller: &mut ConversationStateController, command: &str, generation: u64) {
    controller
        .register_steering(
            artisan_domain::RequestId::parse(command).expect("valid request id"),
            generation,
            steering_source(command),
            0,
            SteeringLabelKind::Steering,
        )
        .expect("steering registration succeeds");
}

fn steering_request(command: &str) -> artisan_domain::RequestId {
    artisan_domain::RequestId::parse(command).expect("valid request id")
}

fn scene_id(value: &str) -> SceneId {
    SceneId::parse(value).expect("valid scene id")
}

fn assert_one_narration(narration: SceneTurnNarration) {
    match narration {
        SceneTurnNarration::Quiet
        | SceneTurnNarration::ProviderWait
        | SceneTurnNarration::Compacting
        | SceneTurnNarration::Thinking
        | SceneTurnNarration::Working
        | SceneTurnNarration::StreamingSuppression
        | SceneTurnNarration::BackgroundWait
        | SceneTurnNarration::WorkedFor { .. }
        | SceneTurnNarration::ThoughtFor { .. }
        | SceneTurnNarration::Failed
        | SceneTurnNarration::Interrupted
        | SceneTurnNarration::Cancelled => {}
    }
}

#[test]
fn initial_delivery_request_is_an_aggregate_effect() {
    let controller = ConversationStateController::new(thread_id());
    assert_eq!(controller.pending_effect_count(), 1);
    assert!(matches!(
        controller.pending_effects(),
        [ConversationStateEffect::Delivery(
            ConversationDeliveryEffect::RequestSnapshot {
                generation: 1,
                after: None,
                ..
            }
        )]
    ));
}

#[test]
fn authoritative_snapshot_projects_deterministic_durable_scene() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(
            baseline_snapshot(),
        ))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();

    let scene = controller.scene().expect("scene builds");
    let turn_scene = &scene.turn_scenes()[0];
    assert_eq!(turn_scene.turn_id.as_str(), TURN_A);
    assert!(matches!(
        &turn_scene.blocks[0],
        TurnBlock::UserMessage(message) if message.id.as_str() == USER_A && message.body == "hello"
    ));
    assert!(matches!(
        &turn_scene.blocks[1],
        TurnBlock::AssistantMessage(message)
            if message.id.as_str() == ASSISTANT_A
                && message.body == "hello back"
                && matches!(message.phase, conversation_scene::AssistantPhase::Final)
    ));
    assert!(matches!(&turn_scene.blocks[2], TurnBlock::TurnStatus(_)));
    assert!(matches!(&turn_scene.blocks[3], TurnBlock::TurnFooter(_)));
}

#[test]
fn delivery_gap_keeps_last_good_scene_and_requests_one_resnapshot() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(
            baseline_snapshot(),
        ))
        .expect("baseline delivery succeeds");
    let _ = controller.drain_effects();
    let before = controller.scene().expect("baseline scene builds");

    controller
        .on_delivery(ConversationDeliveryEvent::BatchReceived(gap_batch()))
        .expect("delivery reports the gap");
    assert_eq!(controller.delivery_view().phase, DeliveryPhase::Recovering);
    assert_eq!(controller.scene().expect("recovery scene builds"), before);

    let effects = controller.drain_effects();
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(
                effect,
                ConversationStateEffect::Delivery(
                    ConversationDeliveryEffect::RequestSnapshot { .. }
                )
            ))
            .count(),
        1
    );
    assert_eq!(effects.len(), 2);
}

#[test]
fn turn_progression_has_one_narration_and_exact_terminal_copy() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot(
            1,
            vec![
                make_turn(TURN_A, 0, ConversationLifecycle::Active),
                make_turn(TURN_B, 3, ConversationLifecycle::Active),
            ],
            vec![make_user(USER_A, TURN_A, 1, "hello")],
        )))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    controller
        .register_turn(turn_id(TURN_A))
        .expect("turn A registration");
    controller
        .register_turn(turn_id(TURN_B))
        .expect("turn B registration");
    let _ = controller.drain_effects();

    let progression = [
        (
            TurnEvent::Compacting {
                at: 10,
                revision: 1,
            },
            SceneTurnNarration::Compacting,
        ),
        (
            TurnEvent::Thinking {
                at: 11,
                revision: 2,
            },
            SceneTurnNarration::Thinking,
        ),
        (
            TurnEvent::Working {
                at: 12,
                revision: 3,
            },
            SceneTurnNarration::Working,
        ),
        (
            TurnEvent::StreamingReply {
                at: 13,
                revision: 4,
            },
            SceneTurnNarration::StreamingSuppression,
        ),
        (
            TurnEvent::Completed {
                at: 14,
                revision: 5,
            },
            SceneTurnNarration::WorkedFor { millis: 4 },
        ),
    ];
    for (event, expected) in progression {
        controller
            .on_turn(turn_id(TURN_A), event)
            .expect("turn event succeeds");
        let (actual, count) = scene_status(&controller, TURN_A);
        assert_eq!(actual, expected);
        assert_eq!(count, 1);
        assert_one_narration(actual);
        let _ = controller.drain_effects();
    }

    for event in [
        TurnEvent::Thinking {
            at: 10,
            revision: 1,
        },
        TurnEvent::StreamingReply {
            at: 11,
            revision: 2,
        },
        TurnEvent::Completed {
            at: 12,
            revision: 3,
        },
    ] {
        controller
            .on_turn(turn_id(TURN_B), event)
            .expect("thought-only event succeeds");
        let (actual, count) = scene_status(&controller, TURN_B);
        assert_eq!(count, 1);
        assert_one_narration(actual);
        if matches!(actual, SceneTurnNarration::ThoughtFor { .. }) {
            assert_eq!(actual, SceneTurnNarration::ThoughtFor { millis: 2 });
        }
        let _ = controller.drain_effects();
    }
}

#[test]
fn simultaneous_steering_views_keep_their_own_anchors_generations_and_effects() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot(
            1,
            vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)],
            vec![
                make_user(USER_A, TURN_A, 1, "first"),
                make_user(USER_B, TURN_A, 2, "second"),
            ],
        )))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    register_steering(&mut controller, "cmd_a", 1);
    register_steering(&mut controller, "cmd_b", 7);
    let _ = controller.drain_effects();

    for (command, generation) in [("cmd_a", 1), ("cmd_b", 7)] {
        controller
            .on_steering(SteeringEvent::DispatchStarted {
                command_id: steering_request(command),
                generation,
                at_ms: 1,
            })
            .expect("dispatch starts");
        controller
            .on_steering(SteeringEvent::DispatchAccepted {
                command_id: steering_request(command),
                generation,
                at_ms: 2,
            })
            .expect("dispatch accepts");
    }
    let pending = controller.view().pending_lip_steering_views;
    assert_eq!(pending.len(), 2);
    assert!(pending.iter().any(|view| view.generation == 1));
    assert!(pending.iter().any(|view| view.generation == 7));
    let _ = controller.drain_effects();

    controller
        .on_steering(SteeringEvent::DurableItemAnchored {
            command_id: steering_request("cmd_a"),
            generation: 1,
            item_id: item_id(USER_A),
            at_ms: 3,
        })
        .expect("A anchors");
    controller
        .on_steering(SteeringEvent::DurableItemAnchored {
            command_id: steering_request("cmd_b"),
            generation: 7,
            item_id: item_id(USER_B),
            at_ms: 3,
        })
        .expect("B anchors");
    let effects = controller.drain_effects();
    assert!(effects.iter().any(|effect| matches!(
        effect,
        ConversationStateEffect::Steering {
            command_id,
            generation: 1,
            effect: SteeringEffect::WatchAcknowledgement { anchor, .. },
        } if command_id.as_str() == "cmd_a" && anchor.as_str() == USER_A
    )));
    assert!(effects.iter().any(|effect| matches!(
        effect,
        ConversationStateEffect::Steering {
            command_id,
            generation: 7,
            effect: SteeringEffect::WatchAcknowledgement { anchor, .. },
        } if command_id.as_str() == "cmd_b" && anchor.as_str() == USER_B
    )));

    let view = controller.view();
    assert!(view.pending_lip_steering_views.is_empty());
    assert!(view.steering_views.iter().any(|entry| {
        entry.view.generation == 1
            && matches!(
                &entry.view.placement,
                SteeringPlacement::AnchoredAfter { anchor } if anchor.as_str() == USER_A
            )
    }));
    assert!(view.steering_views.iter().any(|entry| {
        entry.view.generation == 7
            && matches!(
                &entry.view.placement,
                SteeringPlacement::AnchoredAfter { anchor } if anchor.as_str() == USER_B
            )
    }));
}

#[test]
fn only_an_exact_durable_user_item_can_anchor_a_steering_label() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(
            baseline_snapshot(),
        ))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    register_steering(&mut controller, "cmd_anchor", 1);
    let _ = controller.drain_effects();
    controller
        .on_steering(SteeringEvent::DispatchStarted {
            command_id: steering_request("cmd_anchor"),
            generation: 1,
            at_ms: 1,
        })
        .expect("dispatch starts");
    controller
        .on_steering(SteeringEvent::DispatchAccepted {
            command_id: steering_request("cmd_anchor"),
            generation: 1,
            at_ms: 2,
        })
        .expect("dispatch accepts");
    let _ = controller.drain_effects();

    let before_view = controller.view();
    let before_effects = controller.pending_effects().to_vec();
    let non_user = controller.on_steering(SteeringEvent::DurableItemAnchored {
        command_id: steering_request("cmd_anchor"),
        generation: 1,
        item_id: item_id(ASSISTANT_A),
        at_ms: 3,
    });
    assert!(matches!(
        non_user,
        Err(ConversationStateError::NonUserSteeringAnchor { .. })
    ));
    assert_eq!(controller.view(), before_view);
    assert_eq!(controller.pending_effects(), before_effects.as_slice());

    let unknown = controller.on_steering(SteeringEvent::DurableItemAnchored {
        command_id: steering_request("cmd_anchor"),
        generation: 1,
        item_id: item_id("not_durable"),
        at_ms: 3,
    });
    assert!(matches!(
        unknown,
        Err(ConversationStateError::UnknownSteeringAnchor { .. })
    ));
    assert_eq!(controller.view(), before_view);

    controller
        .on_steering(SteeringEvent::DurableItemAnchored {
            command_id: steering_request("cmd_anchor"),
            generation: 1,
            item_id: item_id(USER_A),
            at_ms: 3,
        })
        .expect("durable user anchors");
    let scene = controller.scene().expect("scene builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(matches!(
        &blocks[1],
        TurnBlock::SteeringLabel(label) if label.anchor.as_str() == USER_A && label.label == "steering"
    ));
}

#[test]
fn changed_file_fact_is_projected_by_the_pure_scene_builder() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(
            baseline_snapshot(),
        ))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();

    let fact = SceneFact::new(
        scene_id("change_fact"),
        turn_id(TURN_A),
        3,
        SceneFactKind::ChangedFiles {
            files: vec![
                SceneFileChange::new("src/main.rs", FileChangeStatus::Modified)
                    .expect("valid changed path"),
            ],
        },
    )
    .expect("valid changed-file fact");
    controller.register_fact(fact).expect("fact registers");
    let _ = controller.drain_effects();

    let scene = controller.scene().expect("scene builds");
    let blocks = &scene.turn_scenes()[0].blocks;
    assert!(matches!(
        blocks.iter().find(|block| matches!(block, TurnBlock::ChangeSet(_))),
        Some(TurnBlock::ChangeSet(change))
            if change.files.len() == 1 && change.files[0].path == "src/main.rs"
    ));
}

#[test]
fn disclosure_user_override_survives_auto_lifecycle_until_retire() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(
            baseline_snapshot(),
        ))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    let key = scene_id(USER_A);
    controller
        .register_disclosure(key.clone(), true)
        .expect("disclosure registers");
    let _ = controller.drain_effects();
    controller
        .on_disclosure(key.clone(), DisclosureEvent::UserOpen)
        .expect("user opens disclosure");
    controller
        .on_disclosure(key.clone(), DisclosureEvent::WorkSettledSuccessfully)
        .expect("auto settle event");
    controller
        .on_disclosure(key.clone(), DisclosureEvent::WorkBecameActive)
        .expect("auto active event");
    controller
        .on_disclosure(key.clone(), DisclosureEvent::WorkFailedOrInterrupted)
        .expect("auto failure event");

    let scene = controller.scene().expect("scene builds");
    assert!(matches!(
        &scene.turn_scenes()[0].blocks[0],
        TurnBlock::UserMessage(message) if message.disclosure == Some(SceneDisclosure::Open)
    ));
    assert!(
        controller
            .view()
            .disclosure_views
            .iter()
            .any(|view| view.scene_id == key && view.state.is_user_controlled())
    );

    controller
        .on_disclosure(key.clone(), DisclosureEvent::Removed)
        .expect("disclosure retires");
    let scene = controller.scene().expect("retired scene builds");
    assert!(matches!(
        &scene.turn_scenes()[0].blocks[0],
        TurnBlock::UserMessage(message) if message.disclosure.is_none()
    ));
}

#[test]
fn viewport_scroll_completion_is_fenced_by_generation() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_viewport(ViewportEvent::JumpToBottomRequested)
        .expect("jump requests a scroll");
    assert_eq!(
        controller.viewport_state(),
        ViewportState::Scrolling {
            generation: ViewportGeneration::new(1)
        }
    );
    let _ = controller.drain_effects();
    controller
        .on_viewport(ViewportEvent::programmatic_scroll_started(
            ViewportGeneration::new(1),
        ))
        .expect("current scroll starts");
    let _ = controller.drain_effects();

    let before = controller.view();
    controller
        .on_viewport(ViewportEvent::scroll_completed(ViewportGeneration::INITIAL))
        .expect("stale completion is reported by child");
    assert_eq!(controller.view().viewport_state, before.viewport_state);
    assert_eq!(
        controller.view().viewport_generation,
        before.viewport_generation
    );
    assert!(controller.pending_effects().iter().any(|effect| matches!(
        effect,
        ConversationStateEffect::Viewport(ViewportEffect::CompletionRejected {
            generation: ViewportGeneration(0),
            reason: conversation_view_machine::CompletionRejection::StaleGeneration,
        })
    )));

    controller
        .on_viewport(ViewportEvent::scroll_completed(ViewportGeneration::new(1)))
        .expect("current completion settles");
    assert!(matches!(
        controller.viewport_state(),
        ViewportState::Settling {
            generation: ViewportGeneration(1)
        }
    ));
}

#[test]
fn duplicate_unknown_capacity_closed_and_refused_events_are_atomic() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(
            baseline_snapshot(),
        ))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    controller
        .register_turn(turn_id(TURN_A))
        .expect("turn registration succeeds");
    let _ = controller.drain_effects();

    let before_view = controller.view();
    let before_scene = controller.scene().expect("scene builds");
    let before_effects = controller.pending_effects().to_vec();
    assert!(matches!(
        controller.register_turn(turn_id(TURN_A)),
        Err(ConversationStateError::DuplicateTurn { .. })
    ));
    assert!(matches!(
        controller.on_turn(
            turn_id("unknown_turn"),
            TurnEvent::Thinking { at: 1, revision: 1 }
        ),
        Err(ConversationStateError::UnknownTurn { .. })
    ));
    assert!(matches!(
        controller.on_steering(SteeringEvent::DispatchStarted {
            command_id: steering_request("unknown_command"),
            generation: 1,
            at_ms: 1,
        }),
        Err(ConversationStateError::UnknownSteering { .. })
    ));
    assert!(matches!(
        controller.on_disclosure(scene_id("unknown_disclosure"), DisclosureEvent::UserOpen),
        Err(ConversationStateError::UnknownDisclosure { .. })
    ));
    assert_eq!(controller.view(), before_view);
    assert_eq!(controller.scene().expect("scene remains"), before_scene);
    assert_eq!(controller.pending_effects(), before_effects.as_slice());

    controller
        .on_turn(
            turn_id(TURN_A),
            TurnEvent::Thinking {
                at: 10,
                revision: 1,
            },
        )
        .expect("first turn event succeeds");
    let _ = controller.drain_effects();
    let before_refused_view = controller.view();
    let before_refused_scene = controller.scene().expect("scene builds");
    let before_refused_effects = controller.pending_effects().to_vec();
    assert!(matches!(
        controller.on_turn(turn_id(TURN_A), TurnEvent::Thinking { at: 9, revision: 2 }),
        Err(ConversationStateError::Turn {
            error: TurnError::TimestampRegression { .. },
            ..
        })
    ));
    assert_eq!(controller.view(), before_refused_view);
    assert_eq!(
        controller.scene().expect("scene remains"),
        before_refused_scene
    );
    assert_eq!(
        controller.pending_effects(),
        before_refused_effects.as_slice()
    );

    let mut closed = ConversationStateController::new(thread_id());
    let _ = closed.drain_effects();
    closed.close().expect("owner closes");
    let _ = closed.drain_effects();
    let closed_view = closed.view();
    let closed_effects = closed.pending_effects().to_vec();
    assert!(matches!(
        closed.register_turn(turn_id(TURN_A)),
        Err(ConversationStateError::OwnerClosed)
    ));
    assert!(matches!(
        closed.register_steering(
            steering_request("closed_command"),
            1,
            steering_source("closed_command"),
            0,
            SteeringLabelKind::Steering,
        ),
        Err(ConversationStateError::OwnerClosed)
    ));
    assert!(matches!(
        closed.register_disclosure(scene_id("closed_disclosure"), false),
        Err(ConversationStateError::OwnerClosed)
    ));
    assert_eq!(closed.view(), closed_view);
    assert_eq!(closed.pending_effects(), closed_effects.as_slice());

    let mut capacity = ConversationStateController::new(thread_id());
    let _ = capacity.drain_effects();
    let mut capacity_error = None;
    for _ in 0..MAX_PENDING_EFFECTS {
        match capacity.on_viewport(ViewportEvent::ExtentChanged) {
            Ok(()) => {}
            Err(error) => {
                capacity_error = Some(error);
                break;
            }
        }
    }
    assert!(matches!(
        capacity_error,
        Some(ConversationStateError::CapacityExhausted {
            resource: CapacityResource::PendingEffects,
            ..
        })
    ));
    let view_before_capacity_refusal = capacity.view();
    let effects_before_capacity_refusal = capacity.pending_effects().to_vec();
    assert!(matches!(
        capacity.on_viewport(ViewportEvent::ExtentChanged),
        Err(ConversationStateError::CapacityExhausted { .. })
    ));
    assert_eq!(capacity.view(), view_before_capacity_refusal);
    assert_eq!(
        capacity.pending_effects(),
        effects_before_capacity_refusal.as_slice()
    );
}
