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
    ConversationStateEvent, MAX_PENDING_EFFECTS, SceneFact, SceneFactKind,
};
use conversation_steering_machine::{
    SourceReference, SteeringEffect, SteeringEvent, SteeringLabelKind, SteeringPlacement,
};
use conversation_turn_machine::{TurnError, TurnEvent};
use conversation_view_machine::{
    DisclosureEvent, DisclosureState, ViewportEffect, ViewportEvent, ViewportGeneration,
    ViewportState,
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
        // Frozen native contract: the send-time source message id lives on
        // the durable item; fixtures leave it absent unless a correlation
        // case specifically needs one.
        source_message_id: None,
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
            &steering_source(command),
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
    // Production provenance derives a session: the empty-details group sits
    // at the anchor before the reply.
    assert!(matches!(
        &turn_scene.blocks[1],
        TurnBlock::WorkGroup(group) if group.session.as_ref().map(SceneId::as_str) == Some("session-turn_a")
            && group.session_details.is_empty()
    ));
    assert!(matches!(
        &turn_scene.blocks[2],
        TurnBlock::AssistantMessage(message)
            if message.id.as_str() == ASSISTANT_A
                && message.body == "hello back"
                && matches!(message.phase, conversation_scene::AssistantPhase::Final)
                && message.provenance.as_ref().is_some_and(|provenance| provenance.run_id.as_ref().map(RunId::as_str) == Some("run_controller"))
    ));
    assert!(matches!(&turn_scene.blocks[3], TurnBlock::TurnStatus(_)));
    assert!(matches!(&turn_scene.blocks[4], TurnBlock::TurnFooter(_)));
    assert_eq!(
        scene
            .promoted_reply_id(&turn_id(TURN_A))
            .map(|id| id.as_str().to_owned()),
        Some(ASSISTANT_A.to_owned())
    );
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
fn resumed_delivery_event_debug_equality_and_routing_preserve_scene_boundary() {
    let event = ConversationStateEvent::Delivery(ConversationDeliveryEvent::SubscriptionResumed {
        thread_id: thread_id(),
        cursor: ConversationCursor::new(1),
    });
    let equal_event =
        ConversationStateEvent::Delivery(ConversationDeliveryEvent::SubscriptionResumed {
            thread_id: thread_id(),
            cursor: ConversationCursor::new(1),
        });
    let wrong_cursor =
        ConversationStateEvent::Delivery(ConversationDeliveryEvent::SubscriptionResumed {
            thread_id: thread_id(),
            cursor: ConversationCursor::new(2),
        });

    assert_eq!(event, equal_event);
    assert_ne!(event, wrong_cursor);
    let debug = format!("{event:?}");
    assert!(debug.contains("DeliverySubscriptionResumed"));
    assert!(debug.contains(THREAD));
    assert!(debug.contains("cursor"));

    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(
            baseline_snapshot(),
        ))
        .expect("baseline delivery");
    let _ = controller.drain_effects();
    let before_scene = controller.scene().expect("baseline scene");

    controller
        .on_delivery(ConversationDeliveryEvent::BatchReceived(gap_batch()))
        .expect("gap enters recovery");
    let _ = controller.drain_effects();
    assert_eq!(controller.delivery_view().phase, DeliveryPhase::Recovering);

    controller
        .dispatch(event)
        .expect("resume routes to delivery child");
    assert_eq!(controller.delivery_view().phase, DeliveryPhase::Ready);
    assert_eq!(
        controller.delivery_view().cursor,
        Some(ConversationCursor::new(1))
    );
    assert_eq!(controller.scene().expect("recovered scene"), before_scene);
    assert!(
        controller.drain_effects().is_empty(),
        "status-only delivery does not cross the scene/effect boundary"
    );
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
fn duplicate_unknown_and_refused_events_are_atomic() {
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
}

#[test]
fn closed_owner_registration_is_atomic() {
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
            &steering_source("closed_command"),
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
}

#[test]
fn pending_effect_capacity_is_atomic() {
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

fn make_turn_updated(
    id: &str,
    ordinal: u64,
    lifecycle: ConversationLifecycle,
    updated_millis: i64,
) -> ConversationTurn {
    make_turn_full(id, ordinal, 0, lifecycle, 0, updated_millis)
}

fn make_turn_full(
    id: &str,
    ordinal: u64,
    revision: u64,
    lifecycle: ConversationLifecycle,
    created_millis: i64,
    updated_millis: i64,
) -> ConversationTurn {
    ConversationTurn {
        turn_id: turn_id(id),
        ordinal: TurnOrdinal::new(ordinal),
        revision: Revision::new(revision),
        lifecycle,
        created_at: stamp(created_millis),
        updated_at: stamp(updated_millis),
    }
}

fn activity_fact(id: &str, turn: &str, ordinal: u64, body: &str) -> SceneFact {
    SceneFact::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneFactKind::Activity {
            body: body.to_owned(),
            kind: None,
            detail: None,
        },
    )
    .expect("valid activity fact")
}

fn turn_lifecycle_batch(
    from: u64,
    to: u64,
    patch: &str,
    turn: &str,
    revision: u64,
    lifecycle: ConversationLifecycle,
    updated_millis: i64,
) -> PatchBatch {
    PatchBatch::new(
        thread_id(),
        ConversationCursor::new(from),
        ConversationCursor::new(to),
        vec![ConversationPatch::TurnLifecycle {
            patch_id: patch_id(patch),
            sequence: PatchSequence::new(to).expect("valid patch sequence"),
            turn_id: turn_id(turn),
            revision: Revision::new(revision),
            lifecycle,
            updated_at: stamp(updated_millis),
        }],
    )
    .expect("valid lifecycle batch envelope")
}

fn delivered_snapshot(
    controller: &mut ConversationStateController,
    cursor: u64,
    watermark_millis: i64,
    turns: Vec<ConversationTurn>,
    items: Vec<ConversationItem>,
) {
    let snapshot = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(cursor),
        turns,
        items,
        stamp(watermark_millis),
    )
    .expect("valid authoritative snapshot");
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
}

#[expect(
    clippy::too_many_arguments,
    reason = "the fixture names each field explicitly so call sites read as data rather than a positional tuple"
)]
fn make_assistant_settled(
    id: &str,
    turn: &str,
    ordinal: u64,
    body: &str,
    phase: AssistantMessagePhase,
    lifecycle: ConversationLifecycle,
    revision: u64,
    updated_millis: i64,
) -> ConversationItem {
    ConversationItem::AssistantMessage(AssistantMessageItem {
        item_id: item_id(id),
        turn_id: turn_id(turn),
        run_id: RunId::parse("run_controller").expect("valid run id"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(revision),
        lifecycle,
        body: AssistantBody::parse(body.to_owned()).expect("valid assistant body"),
        phase,
        created_at: stamp(2),
        updated_at: stamp(updated_millis),
    })
}

fn turn_footer_settlement(
    controller: &ConversationStateController,
    turn: &str,
) -> Option<(String, i64)> {
    let scene = controller.scene().expect("scene builds");
    let turn_scene = scene.turn_scene(&turn_id(turn)).expect("turn present");
    let footer = turn_scene
        .blocks
        .iter()
        .find_map(|block| match block {
            TurnBlock::TurnFooter(footer) => Some(footer),
            _ => None,
        })
        .expect("footer present");
    footer.settlement.as_ref().map(|settlement| {
        (
            settlement.response_text().to_owned(),
            settlement.settled_at_ms(),
        )
    })
}

fn turn_status_basis(controller: &ConversationStateController, turn: &str) -> Option<i64> {
    let scene = controller.scene().expect("scene builds");
    let turn_scene = scene.turn_scene(&turn_id(turn)).expect("turn present");
    turn_scene
        .blocks
        .iter()
        .find_map(|block| match block {
            TurnBlock::TurnStatus(status) => Some(status.active_started_at_ms),
            _ => None,
        })
        .expect("status present")
}

fn user_block_count(controller: &ConversationStateController, item: &str) -> usize {
    let scene = controller.scene().expect("scene builds");
    scene
        .turn_scenes()
        .iter()
        .flat_map(|turn_scene| turn_scene.blocks.iter())
        .filter(|block| {
            matches!(
                block,
                TurnBlock::UserMessage(message) if message.id.as_str() == item
            )
        })
        .count()
}

fn reasoning_fact(id: &str, turn: &str, ordinal: u64, body: &str) -> SceneFact {
    SceneFact::new(
        scene_id(id),
        turn_id(turn),
        ordinal,
        SceneFactKind::Reasoning {
            body: body.to_owned(),
        },
    )
    .expect("valid reasoning fact")
}

fn turn_status_engine_label(
    controller: &ConversationStateController,
    turn: &str,
) -> Option<String> {
    let scene = controller.scene().expect("scene builds");
    scene
        .turn_scene(&turn_id(turn))
        .expect("turn present")
        .blocks
        .iter()
        .find_map(|block| match block {
            TurnBlock::TurnStatus(status) => Some(status.engine_label.clone()),
            _ => None,
        })
        .expect("status present")
}

fn assistant_bodies(controller: &ConversationStateController, turn: &str) -> Vec<String> {
    let scene = controller.scene().expect("scene builds");
    scene
        .turn_scene(&turn_id(turn))
        .expect("turn present")
        .blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::AssistantMessage(message) => Some(message.body.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn completed_turn_with_settled_final_reply_exposes_footer_settlement() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    // Watermark 50 sits above the turn settlement time 42, proving the footer
    // carries the turn's own authoritative updated_at rather than a clock.
    let settled = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(1),
        vec![make_turn_updated(
            TURN_A,
            0,
            ConversationLifecycle::Completed,
            42,
        )],
        vec![
            make_user(USER_A, TURN_A, 1, "hello"),
            make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                2,
                "hello back",
                AssistantMessagePhase::Final,
                ConversationLifecycle::Completed,
                0,
                10,
            ),
        ],
        stamp(50),
    )
    .expect("valid authoritative snapshot");
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(settled))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    assert_eq!(
        turn_footer_settlement(&controller, TURN_A),
        Some(("hello back".to_owned(), 42))
    );
}

#[test]
fn unsettled_turns_keep_unsettled_footers() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    // Baseline: completed turn but its Final reply never completed, so no
    // footer facts may appear.
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(
            baseline_snapshot(),
        ))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    assert_eq!(turn_footer_settlement(&controller, TURN_A), None);

    // Failed turn with a completed Final reply: still no footer, matching the
    // reference which settles footers on completed turns only. A fresh owner
    // takes the failed baseline directly: replaying failure over a completed
    // turn would violate the sealed-terminal lifecycle instead.
    let mut failed_controller = ConversationStateController::new(thread_id());
    let _ = failed_controller.drain_effects();
    let failed = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(2),
        vec![make_turn_updated(
            TURN_A,
            0,
            ConversationLifecycle::Failed,
            42,
        )],
        vec![
            make_user(USER_A, TURN_A, 1, "hello"),
            make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                2,
                "hello back",
                AssistantMessagePhase::Final,
                ConversationLifecycle::Completed,
                0,
                10,
            ),
        ],
        stamp(50),
    )
    .expect("valid authoritative snapshot");
    failed_controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(failed))
        .expect("snapshot delivery succeeds");
    let _ = failed_controller.drain_effects();
    assert_eq!(turn_footer_settlement(&failed_controller, TURN_A), None);
}

#[test]
fn single_prompt_renders_once_after_receipt_and_replay() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(
            baseline_snapshot(),
        ))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    assert_eq!(user_block_count(&controller, USER_A), 1);

    // An identical replay reinstalls without changing visible state: still one
    // prompt bubble, delivery still ready.
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(
            baseline_snapshot(),
        ))
        .expect("replay delivery succeeds");
    let _ = controller.drain_effects();
    assert_eq!(user_block_count(&controller, USER_A), 1);
    assert_eq!(controller.delivery_view().phase, DeliveryPhase::Ready);
}

#[test]
fn streamed_append_preserves_segment_bytes_without_spacing_heuristics() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot(
            1,
            vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)],
            vec![
                make_user(USER_A, TURN_A, 1, "hi"),
                make_assistant(
                    ASSISTANT_A,
                    TURN_A,
                    2,
                    "naturally",
                    AssistantMessagePhase::Unspecified,
                ),
            ],
        )))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();

    // Two streamed token chunks for the SAME logical part concatenate exactly:
    // no spacing heuristic may enter token continuity. Part separation across
    // distinct provider parts happens upstream in the backend text helper.
    let batch = PatchBatch::new(
        thread_id(),
        ConversationCursor::new(1),
        ConversationCursor::new(2),
        vec![ConversationPatch::ItemAppend {
            patch_id: patch_id("append_boundary"),
            sequence: PatchSequence::new(2).expect("valid patch sequence"),
            item_id: item_id(ASSISTANT_A),
            revision: Revision::new(1),
            text: IncrementalText::parse("I'm").expect("valid fragment"),
            updated_at: stamp(11),
        }],
    )
    .expect("valid append batch envelope");
    controller
        .on_delivery(ConversationDeliveryEvent::BatchReceived(batch))
        .expect("append batch applies");
    let _ = controller.drain_effects();

    let scene = controller.scene().expect("scene builds");
    let bodies: Vec<&str> = scene.turn_scenes()[0]
        .blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::AssistantMessage(message) => Some(message.body.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(bodies, vec!["naturallyI'm"]);
}

#[test]
fn active_elapsed_basis_survives_snapshot_refresh_without_reset() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .register_turn(turn_id(TURN_A))
        .expect("turn registration succeeds");
    controller
        .on_turn(
            turn_id(TURN_A),
            TurnEvent::Thinking {
                at: 1000,
                revision: 1,
            },
        )
        .expect("thinking event succeeds");
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot(
            1,
            vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)],
            vec![make_user(USER_A, TURN_A, 1, "hi")],
        )))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    let (narration, _) = scene_status(&controller, TURN_A);
    assert_eq!(narration, SceneTurnNarration::Thinking);
    assert_eq!(turn_status_basis(&controller, TURN_A), Some(1000));

    // A refresh carrying the same durable turn must not reset the basis.
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot(
            2,
            vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)],
            vec![make_user(USER_A, TURN_A, 1, "hi")],
        )))
        .expect("refresh delivery succeeds");
    let _ = controller.drain_effects();
    assert_eq!(turn_status_basis(&controller, TURN_A), Some(1000));

    // Settlement drops the live basis and carries its own terminal duration.
    // Thinking-only work completes as ThoughtFor: no Working event ever set
    // work_seen, so the chart correctly reports thought rather than work.
    controller
        .on_turn(
            turn_id(TURN_A),
            TurnEvent::Completed {
                at: 1005,
                revision: 2,
            },
        )
        .expect("completion succeeds");
    let _ = controller.drain_effects();
    let (settled, _) = scene_status(&controller, TURN_A);
    assert_eq!(settled, SceneTurnNarration::ThoughtFor { millis: 5 });
    assert_eq!(turn_status_basis(&controller, TURN_A), None);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end scenario drives every delivery-derived status; splitting it would hide the causal ordering the test asserts"
)]
fn delivery_drives_pending_waiting_work_stream_and_completed_without_manual_drive() {
    // Production path only: snapshots, batches, and facts. No register_turn,
    // no on_turn. Every status below comes from delivery-derived chart drive.
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();

    // Launch: a Pending turn with a visible durable user message waits on
    // the provider, counting from the turn's own creation.
    delivered_snapshot(
        &mut controller,
        1,
        100,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Pending,
            90,
            95,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    let (waiting, _) = scene_status(&controller, TURN_A);
    assert_eq!(waiting, SceneTurnNarration::ProviderWait);
    assert_eq!(turn_status_basis(&controller, TURN_A), Some(90));

    // Activation without output waits on the provider, counting from the
    // turn's own creation.
    delivered_snapshot(
        &mut controller,
        2,
        120,
        vec![make_turn_full(
            TURN_A,
            0,
            1,
            ConversationLifecycle::Active,
            90,
            115,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    let (waiting, _) = scene_status(&controller, TURN_A);
    assert_eq!(waiting, SceneTurnNarration::ProviderWait);
    assert_eq!(turn_status_basis(&controller, TURN_A), Some(90));

    // Reported tool activity moves the same turn to working without resetting
    // the basis.
    controller
        .register_fact(activity_fact("work_fact", TURN_A, 100, "tool"))
        .expect("activity fact registers");
    let _ = controller.drain_effects();
    delivered_snapshot(
        &mut controller,
        3,
        140,
        vec![make_turn_full(
            TURN_A,
            0,
            2,
            ConversationLifecycle::Active,
            90,
            135,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    let (working, _) = scene_status(&controller, TURN_A);
    assert_eq!(working, SceneTurnNarration::Working);
    assert_eq!(turn_status_basis(&controller, TURN_A), Some(90));

    // A live reply suppresses the status row; its text is the status.
    // (Streaming lifecycle: a Pending item has not arrived yet, so only a
    // genuinely streaming item counts as a live reply. Ordinal 101 puts the
    // reply after the ordinal-100 tool fact, so newest-phase progress stays
    // Reply and the reply remains the promoted top-level row.)
    let streaming = PatchBatch::new(
        thread_id(),
        ConversationCursor::new(3),
        ConversationCursor::new(4),
        vec![ConversationPatch::ItemUpsert {
            patch_id: patch_id("upsert_streaming"),
            sequence: PatchSequence::new(4).expect("valid patch sequence"),
            item: make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                101,
                "draft",
                AssistantMessagePhase::Unspecified,
                ConversationLifecycle::Streaming,
                0,
                10,
            ),
        }],
    )
    .expect("valid upsert batch envelope");
    controller
        .on_delivery(ConversationDeliveryEvent::BatchReceived(streaming))
        .expect("streaming batch applies");
    let _ = controller.drain_effects();
    let scene = controller.scene().expect("scene builds");
    let turn_scene = scene.turn_scene(&turn_id(TURN_A)).expect("turn present");
    assert!(
        turn_scene
            .blocks
            .iter()
            .all(|block| !matches!(block, TurnBlock::TurnStatus(_))),
        "streaming reply suppresses the status row"
    );
    assert!(
        turn_scene.blocks.iter().any(|block| matches!(
            block,
            TurnBlock::AssistantMessage(message) if message.body == "draft"
        )),
        "streaming text stays visible"
    );

    // Settling the reply completes the turn as worked time from creation.
    let settle = PatchBatch::new(
        thread_id(),
        ConversationCursor::new(4),
        ConversationCursor::new(6),
        vec![
            ConversationPatch::ItemUpsert {
                patch_id: patch_id("upsert_final"),
                sequence: PatchSequence::new(5).expect("valid patch sequence"),
                item: make_assistant_settled(
                    ASSISTANT_A,
                    TURN_A,
                    101,
                    "done",
                    AssistantMessagePhase::Final,
                    ConversationLifecycle::Completed,
                    1,
                    150,
                ),
            },
            ConversationPatch::TurnLifecycle {
                patch_id: patch_id("turn_completed"),
                sequence: PatchSequence::new(6).expect("valid patch sequence"),
                turn_id: turn_id(TURN_A),
                revision: Revision::new(3),
                lifecycle: ConversationLifecycle::Completed,
                updated_at: stamp(155),
            },
        ],
    )
    .expect("valid settle batch envelope");
    controller
        .on_delivery(ConversationDeliveryEvent::BatchReceived(settle))
        .expect("settle batch applies");
    let _ = controller.drain_effects();
    // The settle frame must advance the projection cursor: a refusal would
    // keep the last-good streaming scene and fail below without context.
    assert_eq!(
        controller.delivery_view().cursor,
        Some(ConversationCursor::new(6))
    );
    let (settled, _) = scene_status(&controller, TURN_A);
    assert_eq!(settled, SceneTurnNarration::WorkedFor { millis: 65 });
    assert_eq!(turn_status_basis(&controller, TURN_A), None);
    assert_eq!(
        turn_footer_settlement(&controller, TURN_A),
        Some(("done".to_owned(), 155))
    );
}

#[test]
fn delivery_derived_terminal_state_freezes_first_settlement() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    delivered_snapshot(
        &mut controller,
        1,
        1100,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Completed,
            1000,
            1100,
        )],
        vec![
            make_user(USER_A, TURN_A, 1, "hi"),
            make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                2,
                "done",
                AssistantMessagePhase::Final,
                ConversationLifecycle::Completed,
                0,
                10,
            ),
        ],
    );
    let (settled, _) = scene_status(&controller, TURN_A);
    assert_eq!(settled, SceneTurnNarration::ThoughtFor { millis: 100 });
    assert_eq!(
        turn_footer_settlement(&controller, TURN_A),
        Some(("done".to_owned(), 1100))
    );

    // A later window that only advances the watermark must neither move the
    // frozen settlement nor fabricate a footer change.
    delivered_snapshot(
        &mut controller,
        2,
        5000,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Completed,
            1000,
            1100,
        )],
        vec![
            make_user(USER_A, TURN_A, 1, "hi"),
            make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                2,
                "done",
                AssistantMessagePhase::Final,
                ConversationLifecycle::Completed,
                0,
                10,
            ),
        ],
    );
    let (frozen, _) = scene_status(&controller, TURN_A);
    assert_eq!(frozen, SceneTurnNarration::ThoughtFor { millis: 100 });
    assert_eq!(
        turn_footer_settlement(&controller, TURN_A),
        Some(("done".to_owned(), 1100))
    );
}

#[test]
fn interrupted_turn_resumes_through_delivery_without_basis_reset() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    delivered_snapshot(
        &mut controller,
        1,
        200,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Active,
            100,
            150,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    controller
        .register_fact(activity_fact("resume_fact", TURN_A, 100, "tool"))
        .expect("activity fact registers");
    let _ = controller.drain_effects();
    delivered_snapshot(
        &mut controller,
        2,
        250,
        vec![make_turn_full(
            TURN_A,
            0,
            1,
            ConversationLifecycle::Active,
            100,
            240,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    let (working, _) = scene_status(&controller, TURN_A);
    assert_eq!(working, SceneTurnNarration::Working);
    assert_eq!(turn_status_basis(&controller, TURN_A), Some(100));

    controller
        .on_delivery(ConversationDeliveryEvent::BatchReceived(
            turn_lifecycle_batch(
                2,
                3,
                "turn_interrupted",
                TURN_A,
                2,
                ConversationLifecycle::Interrupted,
                260,
            ),
        ))
        .expect("interruption applies");
    let _ = controller.drain_effects();
    let (stopped, _) = scene_status(&controller, TURN_A);
    assert_eq!(stopped, SceneTurnNarration::Interrupted);
    assert_eq!(turn_status_basis(&controller, TURN_A), None);

    // Resuming reuses the original creation basis instead of restarting it.
    controller
        .on_delivery(ConversationDeliveryEvent::BatchReceived(
            turn_lifecycle_batch(
                3,
                4,
                "turn_resumed",
                TURN_A,
                3,
                ConversationLifecycle::Active,
                270,
            ),
        ))
        .expect("resume applies");
    let _ = controller.drain_effects();
    let (resumed, _) = scene_status(&controller, TURN_A);
    assert_eq!(resumed, SceneTurnNarration::Working);
    assert_eq!(turn_status_basis(&controller, TURN_A), Some(100));
}

#[test]
fn history_settlement_uses_the_turn_span_not_the_window_age() {
    // An old turn first seen under a much newer window must keep its own
    // span: created 0, updated 1000 settles as one second, not one day.
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    delivered_snapshot(
        &mut controller,
        1,
        86_400_000,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Completed,
            0,
            1000,
        )],
        vec![
            make_user(USER_A, TURN_A, 1, "hi"),
            make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                2,
                "done",
                AssistantMessagePhase::Final,
                ConversationLifecycle::Completed,
                0,
                10,
            ),
        ],
    );
    let (settled, _) = scene_status(&controller, TURN_A);
    assert_eq!(settled, SceneTurnNarration::ThoughtFor { millis: 1000 });
    assert_eq!(
        turn_footer_settlement(&controller, TURN_A),
        Some(("done".to_owned(), 1000))
    );
}

#[test]
fn accepted_fact_updates_active_status_without_followup_delivery() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    delivered_snapshot(
        &mut controller,
        1,
        120,
        vec![make_turn_full(
            TURN_A,
            0,
            1,
            ConversationLifecycle::Active,
            90,
            115,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    let (waiting, _) = scene_status(&controller, TURN_A);
    assert_eq!(waiting, SceneTurnNarration::ProviderWait);

    // No further delivery: the accepted fact alone must move the status.
    controller
        .register_fact(activity_fact("live_fact", TURN_A, 100, "tool"))
        .expect("activity fact registers");
    let _ = controller.drain_effects();
    let (working, _) = scene_status(&controller, TURN_A);
    assert_eq!(working, SceneTurnNarration::Working);
    assert_eq!(turn_status_basis(&controller, TURN_A), Some(90));
}

#[test]
fn first_snapshot_streaming_yields_suppressed_status_and_creation_basis() {
    // A history controller takes StreamingReply straight from idle: the chart
    // accepts it from Pending, so no refusal is swallowed here.
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    delivered_snapshot(
        &mut controller,
        1,
        140,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Active,
            100,
            135,
        )],
        vec![
            make_user(USER_A, TURN_A, 1, "hi"),
            make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                2,
                "draft",
                AssistantMessagePhase::Unspecified,
                ConversationLifecycle::Streaming,
                0,
                10,
            ),
        ],
    );
    let scene = controller.scene().expect("scene builds");
    let turn_scene = scene.turn_scene(&turn_id(TURN_A)).expect("turn present");
    assert!(
        turn_scene
            .blocks
            .iter()
            .all(|block| !matches!(block, TurnBlock::TurnStatus(_))),
        "first-snapshot streaming suppresses the status row"
    );

    // Settling proves the creation basis planted during suppression: the
    // terminal span counts from 100, not from settlement.
    delivered_snapshot(
        &mut controller,
        2,
        500,
        vec![make_turn_full(
            TURN_A,
            0,
            1,
            ConversationLifecycle::Completed,
            100,
            500,
        )],
        vec![
            make_user(USER_A, TURN_A, 1, "hi"),
            make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                2,
                "done",
                AssistantMessagePhase::Final,
                ConversationLifecycle::Completed,
                1,
                450,
            ),
        ],
    );
    let (settled, _) = scene_status(&controller, TURN_A);
    assert_eq!(settled, SceneTurnNarration::ThoughtFor { millis: 400 });
}

fn activity_fact_with_run(id: &str, turn: &str, ordinal: u64, body: &str, run: &str) -> SceneFact {
    SceneFact::new(
        SceneId::parse(id).expect("valid scene id"),
        turn_id(turn),
        ordinal,
        SceneFactKind::Activity {
            body: body.to_owned(),
            kind: None,
            detail: None,
        },
    )
    .expect("valid activity fact")
    .with_run_id(RunId::parse(run).expect("valid run id"))
}

fn session_anchor(turn: &str) -> SceneId {
    SceneId::parse(format!("session-{turn}")).expect("valid session anchor")
}

fn turn_blocks(controller: &ConversationStateController, turn: &str) -> Vec<String> {
    let scene = controller.scene().expect("scene builds");
    scene
        .turn_scene(&turn_id(turn))
        .expect("turn present")
        .blocks
        .iter()
        .map(|block| match block {
            TurnBlock::UserMessage(_) => "user".to_owned(),
            TurnBlock::AssistantMessage(_) => "reply".to_owned(),
            TurnBlock::WorkGroup(group) if group.session.is_some() => "session".to_owned(),
            TurnBlock::WorkGroup(_) => "work".to_owned(),
            TurnBlock::SteeringLabel(_) => "steer-label".to_owned(),
            TurnBlock::TurnStatus(_) => "status".to_owned(),
            TurnBlock::TurnFooter(_) => "footer".to_owned(),
            _ => "other".to_owned(),
        })
        .collect()
}

#[test]
fn commentary_phase_preserved_without_collapsing_into_streaming() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot(
            1,
            vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)],
            vec![
                make_user(USER_A, TURN_A, 1, "hi"),
                make_assistant(
                    ASSISTANT_A,
                    TURN_A,
                    2,
                    "checking sources",
                    AssistantMessagePhase::Commentary,
                ),
            ],
        )))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();

    // Commentary arrives as commentary with run/lifecycle attribution intact:
    // never collapsed into a streaming marker, never text-inferred. It folds
    // into the session trace, so no top-level assistant row exists for it —
    // but the status row stays, because commentary is work, not a reply.
    let scene = controller.scene().expect("scene builds");
    let turn_scene = scene.turn_scene(&turn_id(TURN_A)).expect("turn present");
    assert!(
        turn_scene
            .blocks
            .iter()
            .any(|block| matches!(block, TurnBlock::TurnStatus(_)))
    );
    assert!(
        !turn_scene
            .blocks
            .iter()
            .any(|block| matches!(block, TurnBlock::AssistantMessage(_)))
    );
    let group = turn_scene
        .blocks
        .iter()
        .find_map(|block| match block {
            TurnBlock::WorkGroup(group) if group.session.is_some() => Some(group),
            _ => None,
        })
        .expect("session group");
    assert_eq!(group.session_details.len(), 1);
    let message = match &group.session_details[0] {
        conversation_scene::SessionDetail::Assistant {
            phase, provenance, ..
        } => (phase, provenance),
        other => panic!("commentary folds as assistant prose, got {other:?}"),
    };
    assert!(matches!(
        message.0,
        conversation_scene::AssistantPhase::Commentary
    ));
    let provenance = message.1.as_ref().expect("production provenance");
    assert_eq!(
        provenance
            .run_id
            .as_ref()
            .map(artisan_domain::RunId::as_str),
        Some("run_controller")
    );
    // The fixture assistant carries a Pending lifecycle: phase preservation
    // holds regardless of lifecycle, and the mapping never coerces it.
    assert_eq!(provenance.lifecycle, Some(ConversationLifecycle::Pending));
}

#[test]
fn commentary_tool_final_flow_groups_folds_and_promotes() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot(
            1,
            vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)],
            vec![
                make_user(USER_A, TURN_A, 1, "hi"),
                make_assistant(
                    "commentary_a",
                    TURN_A,
                    2,
                    "checking",
                    AssistantMessagePhase::Commentary,
                ),
                make_assistant_settled(
                    ASSISTANT_A,
                    TURN_A,
                    4,
                    "done",
                    AssistantMessagePhase::Final,
                    ConversationLifecycle::Completed,
                    0,
                    10,
                ),
            ],
        )))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    controller
        .register_fact(activity_fact_with_run(
            "tool_a",
            TURN_A,
            3,
            "ran",
            "run_controller",
        ))
        .expect("tool fact registers");
    let _ = controller.drain_effects();

    // One session: commentary + tool in trace order, the Final reply
    // top-level and promoted, nothing rendered twice.
    assert_eq!(
        turn_blocks(&controller, TURN_A),
        vec!["user", "session", "reply", "status", "footer"]
    );
    let scene = controller.scene().expect("scene builds");
    let group = scene.turn_scenes()[0]
        .blocks
        .iter()
        .find_map(|block| match block {
            TurnBlock::WorkGroup(group) if group.session.is_some() => Some(group),
            _ => None,
        })
        .expect("session group");
    assert_eq!(group.session_details.len(), 2);
    assert_eq!(
        scene
            .promoted_reply_id(&turn_id(TURN_A))
            .map(|id| id.as_str().to_owned()),
        Some(ASSISTANT_A.to_owned())
    );
}

#[test]
fn explicit_final_newer_promotes_and_settles() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot(
            1,
            vec![make_turn(TURN_A, 0, ConversationLifecycle::Completed)],
            vec![
                make_user(USER_A, TURN_A, 1, "hi"),
                make_assistant_settled(
                    "older_a",
                    TURN_A,
                    2,
                    "draft",
                    AssistantMessagePhase::Unspecified,
                    ConversationLifecycle::Completed,
                    0,
                    10,
                ),
                make_assistant_settled(
                    ASSISTANT_A,
                    TURN_A,
                    3,
                    "final answer",
                    AssistantMessagePhase::Final,
                    ConversationLifecycle::Completed,
                    0,
                    10,
                ),
            ],
        )))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();

    assert_eq!(
        turn_footer_settlement(&controller, TURN_A),
        Some(("final answer".to_owned(), 10))
    );
    // Exactly one top-level reply: the older prose folded into the session.
    let scene = controller.scene().expect("scene builds");
    let replies: Vec<&str> = scene.turn_scenes()[0]
        .blocks
        .iter()
        .filter_map(|block| match block {
            TurnBlock::AssistantMessage(message) => Some(message.body.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(replies, vec!["final answer"]);
}

#[test]
fn steering_boundary_keeps_post_steer_work_top_level() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot(
            1,
            vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)],
            vec![
                make_user(USER_A, TURN_A, 1, "go"),
                make_user(USER_B, TURN_A, 5, "actually, stop"),
            ],
        )))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    register_steering(&mut controller, "cmd_steer", 1);
    controller
        .on_steering(SteeringEvent::DispatchStarted {
            command_id: steering_request("cmd_steer"),
            generation: 1,
            at_ms: 1,
        })
        .expect("dispatch starts");
    controller
        .on_steering(SteeringEvent::DispatchAccepted {
            command_id: steering_request("cmd_steer"),
            generation: 1,
            at_ms: 2,
        })
        .expect("dispatch accepts");
    controller
        .on_steering(SteeringEvent::DurableItemAnchored {
            command_id: steering_request("cmd_steer"),
            generation: 1,
            item_id: item_id(USER_B),
            at_ms: 3,
        })
        .expect("steer anchors");
    controller
        .register_fact(
            activity_fact_with_run("tool_pre", TURN_A, 3, "ran", "run_controller")
                .with_activity_lifecycle(ConversationLifecycle::Completed),
        )
        .expect("pre-steer tool registers");
    controller
        .register_fact(
            activity_fact_with_run("tool_post", TURN_A, 8, "stopping", "run_controller")
                .with_activity_lifecycle(ConversationLifecycle::Active),
        )
        .expect("post-steer tool registers");
    let _ = controller.drain_effects();

    // Pre-steer work joins the session; post-steer work stays top-level
    // below the steering label and supersedes the session. The live tool
    // chain carries progress, so no status row renders.
    assert_eq!(
        turn_blocks(&controller, TURN_A),
        vec!["user", "session", "user", "steer-label", "work", "footer"]
    );
    let scene = controller.scene().expect("scene builds");
    let group = scene.turn_scenes()[0]
        .blocks
        .iter()
        .find_map(|block| match block {
            TurnBlock::WorkGroup(group) if group.session.is_some() => Some(group),
            _ => None,
        })
        .expect("session group");
    assert!(group.superseded);
    assert_eq!(group.session_details.len(), 1);
}

#[test]
fn live_to_settled_disclosure_reconcile_respects_user_choice() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot(
            1,
            vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)],
            vec![
                make_user(USER_A, TURN_A, 1, "hi"),
                make_assistant(
                    ASSISTANT_A,
                    TURN_A,
                    2,
                    "draft",
                    AssistantMessagePhase::Unspecified,
                ),
            ],
        )))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();

    // Live session work starts open through the derived anchor.
    let anchor = session_anchor(TURN_A);
    let state = controller
        .view()
        .disclosure_views
        .into_iter()
        .find(|view| view.scene_id == anchor)
        .expect("session disclosure auto-registered")
        .state;
    assert_eq!(state, DisclosureState::AutoOpen);

    // Settlement folds it closed.
    controller
        .on_delivery(ConversationDeliveryEvent::BatchReceived(
            PatchBatch::new(
                thread_id(),
                ConversationCursor::new(1),
                ConversationCursor::new(2),
                vec![ConversationPatch::TurnLifecycle {
                    patch_id: patch_id("turn_done"),
                    sequence: PatchSequence::new(2).expect("valid patch sequence"),
                    turn_id: turn_id(TURN_A),
                    revision: Revision::new(1),
                    lifecycle: ConversationLifecycle::Completed,
                    updated_at: stamp(20),
                }],
            )
            .expect("valid settle batch"),
        ))
        .expect("settle applies");
    let _ = controller.drain_effects();
    let state = controller
        .view()
        .disclosure_views
        .into_iter()
        .find(|view| view.scene_id == anchor)
        .expect("session disclosure retained")
        .state;
    assert_eq!(state, DisclosureState::AutoClosed);

    // Explicit user choice is authoritative across refreshes: the settled
    // event routes into a user-held controller and changes nothing.
    controller
        .on_disclosure(
            anchor.clone(),
            conversation_view_machine::DisclosureEvent::UserOpen,
        )
        .expect("user opens");
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(
            ConversationSnapshot::new(
                thread_id(),
                ConversationCursor::new(2),
                vec![ConversationTurn {
                    turn_id: turn_id(TURN_A),
                    ordinal: TurnOrdinal::new(0),
                    revision: Revision::new(1),
                    lifecycle: ConversationLifecycle::Completed,
                    created_at: stamp(0),
                    updated_at: stamp(20),
                }],
                vec![
                    make_user(USER_A, TURN_A, 1, "hi"),
                    make_assistant(
                        ASSISTANT_A,
                        TURN_A,
                        2,
                        "draft",
                        AssistantMessagePhase::Unspecified,
                    ),
                ],
                stamp(20),
            )
            .expect("valid refresh snapshot"),
        ))
        .expect("refresh succeeds");
    let _ = controller.drain_effects();
    let state = controller
        .view()
        .disclosure_views
        .into_iter()
        .find(|view| view.scene_id == anchor)
        .expect("session disclosure retained")
        .state;
    assert_eq!(state, DisclosureState::UserOpen);
}

#[test]
fn duplicate_replay_keeps_single_blocks_and_settlement() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    let frame = snapshot(
        1,
        vec![make_turn(TURN_A, 0, ConversationLifecycle::Completed)],
        vec![
            make_user(USER_A, TURN_A, 1, "hi"),
            make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                2,
                "done",
                AssistantMessagePhase::Final,
                ConversationLifecycle::Completed,
                0,
                10,
            ),
        ],
    );
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(frame.clone()))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    controller
        .register_fact(activity_fact_with_run(
            "tool_a",
            TURN_A,
            3,
            "ran",
            "run_controller",
        ))
        .expect("tool fact registers");
    let _ = controller.drain_effects();
    let before = controller.scene().expect("scene builds");
    let settlement_before = turn_footer_settlement(&controller, TURN_A);

    // Identical reinstall plus identical fact upsert: replay idempotence,
    // one block per identity, settlement untouched.
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(frame))
        .expect("replay succeeds");
    let _ = controller.drain_effects();
    controller
        .upsert_fact(activity_fact_with_run(
            "tool_a",
            TURN_A,
            3,
            "ran",
            "run_controller",
        ))
        .expect("identical upsert is a no-op");
    let _ = controller.drain_effects();
    let after = controller.scene().expect("scene builds");
    assert_eq!(after, before);
    assert_eq!(
        turn_footer_settlement(&controller, TURN_A),
        settlement_before
    );
    assert_eq!(user_block_count(&controller, USER_A), 1);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end scenario drives the launch-to-terminal lifecycle; splitting it would hide the causal ordering the test asserts"
)]
fn launched_pending_waiting_through_streaming_to_terminal() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();

    // Stage 1, launch: Pending turn with a visible durable user message
    // waits on the provider, counting from the turn's own creation, with
    // no engine label to name yet.
    delivered_snapshot(
        &mut controller,
        1,
        100,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Pending,
            90,
            95,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    let (waiting, _) = scene_status(&controller, TURN_A);
    assert_eq!(waiting, SceneTurnNarration::ProviderWait);
    assert_eq!(turn_status_basis(&controller, TURN_A), Some(90));
    assert_eq!(turn_status_engine_label(&controller, TURN_A), None);
    assert_eq!(user_block_count(&controller, USER_A), 1);

    // Stage 2, provider activity: reasoning then a tool move the same
    // Pending turn through Thinking to Working without touching the basis.
    controller
        .register_fact(reasoning_fact("reason_a", TURN_A, 100, "thinking aloud"))
        .expect("reasoning fact registers");
    let _ = controller.drain_effects();
    delivered_snapshot(
        &mut controller,
        2,
        120,
        vec![make_turn_full(
            TURN_A,
            0,
            1,
            ConversationLifecycle::Pending,
            90,
            115,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    let (thinking, _) = scene_status(&controller, TURN_A);
    assert_eq!(thinking, SceneTurnNarration::Thinking);
    controller
        .register_fact(activity_fact("tool_a", TURN_A, 101, "ran"))
        .expect("activity fact registers");
    let _ = controller.drain_effects();
    delivered_snapshot(
        &mut controller,
        3,
        140,
        vec![make_turn_full(
            TURN_A,
            0,
            2,
            ConversationLifecycle::Pending,
            90,
            135,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    let (working, _) = scene_status(&controller, TURN_A);
    assert_eq!(working, SceneTurnNarration::Working);
    assert_eq!(turn_status_basis(&controller, TURN_A), Some(90));

    // Stage 3, incremental response: sequential versions of the SAME
    // assistant item arrive in separate snapshots, each asserted before
    // anything terminal.
    delivered_snapshot(
        &mut controller,
        4,
        150,
        vec![make_turn_full(
            TURN_A,
            0,
            3,
            ConversationLifecycle::Pending,
            90,
            145,
        )],
        vec![
            make_user(USER_A, TURN_A, 1, "hi"),
            make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                102,
                "first",
                AssistantMessagePhase::Unspecified,
                ConversationLifecycle::Streaming,
                0,
                145,
            ),
        ],
    );
    let scene = controller.scene().expect("scene builds");
    let turn_scene = scene.turn_scene(&turn_id(TURN_A)).expect("turn present");
    assert!(
        turn_scene
            .blocks
            .iter()
            .all(|block| !matches!(block, TurnBlock::TurnStatus(_))),
        "streaming reply suppresses the status row"
    );
    assert_eq!(
        assistant_bodies(&controller, TURN_A),
        vec!["first".to_owned()]
    );
    delivered_snapshot(
        &mut controller,
        5,
        160,
        vec![make_turn_full(
            TURN_A,
            0,
            4,
            ConversationLifecycle::Pending,
            90,
            150,
        )],
        vec![
            make_user(USER_A, TURN_A, 1, "hi"),
            make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                102,
                "first second",
                AssistantMessagePhase::Unspecified,
                ConversationLifecycle::Streaming,
                1,
                155,
            ),
        ],
    );
    let scene = controller.scene().expect("scene builds");
    let turn_scene = scene.turn_scene(&turn_id(TURN_A)).expect("turn present");
    assert!(
        turn_scene
            .blocks
            .iter()
            .all(|block| !matches!(block, TurnBlock::TurnStatus(_))),
        "streaming reply suppresses the status row"
    );
    assert_eq!(
        assistant_bodies(&controller, TURN_A),
        vec!["first second".to_owned()]
    );

    // Stage 4, terminal: the same item settles as Final at revision 2.
    // Worked time still counts from the send, the final body settles the
    // footer, and the basis is retired with it.
    delivered_snapshot(
        &mut controller,
        6,
        170,
        vec![make_turn_full(
            TURN_A,
            0,
            5,
            ConversationLifecycle::Completed,
            90,
            160,
        )],
        vec![
            make_user(USER_A, TURN_A, 1, "hi"),
            make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                102,
                "first second",
                AssistantMessagePhase::Final,
                ConversationLifecycle::Completed,
                2,
                165,
            ),
        ],
    );
    let (settled, _) = scene_status(&controller, TURN_A);
    assert_eq!(settled, SceneTurnNarration::WorkedFor { millis: 70 });
    assert_eq!(turn_status_basis(&controller, TURN_A), None);
    assert_eq!(
        turn_footer_settlement(&controller, TURN_A),
        Some(("first second".to_owned(), 160))
    );
    assert_eq!(user_block_count(&controller, USER_A), 1);
}

#[test]
fn pending_turn_without_user_message_stays_quiet() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    // No launch evidence: a Pending turn with nothing visible has no anchor
    // for a status row, so it stays quiet.
    delivered_snapshot(
        &mut controller,
        1,
        100,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Pending,
            90,
            95,
        )],
        vec![],
    );
    let (quiet, _) = scene_status(&controller, TURN_A);
    assert_eq!(quiet, SceneTurnNarration::Quiet);
    assert_eq!(turn_status_basis(&controller, TURN_A), None);
}

#[test]
fn duplicate_pending_snapshot_is_effect_quiet_and_stable() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    delivered_snapshot(
        &mut controller,
        1,
        100,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Pending,
            90,
            95,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    let before = controller.scene().expect("scene builds");
    let (waiting, _) = scene_status(&controller, TURN_A);
    assert_eq!(waiting, SceneTurnNarration::ProviderWait);

    // Redelivering the identical frame changes nothing visible and emits
    // nothing: same narration, same basis, empty outbox.
    delivered_snapshot(
        &mut controller,
        1,
        100,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Pending,
            90,
            95,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    assert!(controller.drain_effects().is_empty());
    assert_eq!(controller.scene().expect("scene builds"), before);
    let (still_waiting, _) = scene_status(&controller, TURN_A);
    assert_eq!(still_waiting, SceneTurnNarration::ProviderWait);
    assert_eq!(turn_status_basis(&controller, TURN_A), Some(90));
}

#[test]
fn terminal_snapshot_reinstall_keeps_frozen_settlement() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    let frame = snapshot(
        1,
        vec![make_turn(TURN_A, 0, ConversationLifecycle::Completed)],
        vec![
            make_user(USER_A, TURN_A, 1, "hi"),
            make_assistant_settled(
                ASSISTANT_A,
                TURN_A,
                2,
                "done",
                AssistantMessagePhase::Final,
                ConversationLifecycle::Completed,
                0,
                10,
            ),
        ],
    );
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(frame.clone()))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    let before = controller.scene().expect("scene builds");
    let settlement_before = turn_footer_settlement(&controller, TURN_A);
    assert_eq!(settlement_before, Some(("done".to_owned(), 10)));

    // Identical terminal reinstall: frozen settlement, identical scene,
    // quiet outbox.
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(frame))
        .expect("replay succeeds");
    assert!(controller.drain_effects().is_empty());
    assert_eq!(controller.scene().expect("scene builds"), before);
    assert_eq!(
        turn_footer_settlement(&controller, TURN_A),
        settlement_before
    );
}

#[test]
fn failed_after_completed_is_refused_without_state_change() {
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
    controller
        .on_turn(
            turn_id(TURN_A),
            TurnEvent::Completed {
                at: 14,
                revision: 1,
            },
        )
        .expect("completion succeeds");
    let _ = controller.drain_effects();
    let before_view = controller.view();
    let before_scene = controller.scene().expect("scene builds");
    let before_effects = controller.pending_effects().to_vec();

    // A terminal state is sealed: a later failure is refused and changes
    // nothing, instead of rewriting history.
    assert!(matches!(
        controller.on_turn(
            turn_id(TURN_A),
            TurnEvent::Failed {
                at: 14,
                revision: 2,
                kind: None
            }
        ),
        Err(ConversationStateError::Turn {
            error: TurnError::Sealed { .. },
            ..
        })
    ));
    assert_eq!(controller.view(), before_view);
    assert_eq!(controller.scene().expect("scene remains"), before_scene);
    assert_eq!(controller.pending_effects(), before_effects.as_slice());
}

#[test]
fn unknown_turn_engine_label_is_rejected() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    assert!(matches!(
        controller.on_turn_engine_label(turn_id(TURN_A), Some("Claude".to_owned())),
        Err(ConversationStateError::UnknownTurn { .. })
    ));
    assert!(controller.drain_effects().is_empty());
}

#[test]
fn engine_label_names_waiting_row_and_clears() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    delivered_snapshot(
        &mut controller,
        1,
        100,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Pending,
            90,
            95,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    assert_eq!(turn_status_engine_label(&controller, TURN_A), None);

    controller
        .on_turn_engine_label(turn_id(TURN_A), Some("Claude".to_owned()))
        .expect("label sets");
    let effects = controller.drain_effects();
    assert_eq!(effects.len(), 1);
    assert!(matches!(
        effects[0],
        ConversationStateEffect::SceneInvalidated
    ));
    let (waiting, _) = scene_status(&controller, TURN_A);
    assert_eq!(waiting, SceneTurnNarration::ProviderWait);
    assert_eq!(
        turn_status_engine_label(&controller, TURN_A),
        Some("Claude".to_owned())
    );

    // A repeated identical label is a no-op success without effects.
    controller
        .on_turn_engine_label(turn_id(TURN_A), Some("Claude".to_owned()))
        .expect("repeated label succeeds");
    assert!(controller.drain_effects().is_empty());

    // Clearing restores the generic copy, again invalidating once.
    controller
        .on_turn_engine_label(turn_id(TURN_A), None)
        .expect("clear succeeds");
    let cleared = controller.drain_effects();
    assert_eq!(cleared.len(), 1);
    assert_eq!(turn_status_engine_label(&controller, TURN_A), None);
}

#[test]
fn invalid_engine_label_is_rejected_without_state_change() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .register_turn(turn_id(TURN_A))
        .expect("turn registration succeeds");
    let _ = controller.drain_effects();
    let before_effects = controller.pending_effects().to_vec();

    assert!(matches!(
        controller.on_turn_engine_label(turn_id(TURN_A), Some(String::new())),
        Err(ConversationStateError::Scene(..))
    ));
    assert!(matches!(
        controller.on_turn_engine_label(turn_id(TURN_A), Some("x".repeat(1025))),
        Err(ConversationStateError::Scene(..))
    ));
    assert_eq!(controller.pending_effects(), before_effects.as_slice());
    // A valid label still applies afterwards: refusals changed nothing.
    controller
        .on_turn_engine_label(turn_id(TURN_A), Some("Claude".to_owned()))
        .expect("valid label applies");
}

#[test]
fn engine_label_pruned_when_turn_leaves_snapshot() {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    delivered_snapshot(
        &mut controller,
        1,
        100,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Pending,
            90,
            95,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    controller
        .on_turn_engine_label(turn_id(TURN_A), Some("Claude".to_owned()))
        .expect("label sets");
    let _ = controller.drain_effects();
    assert!(format!("{controller:?}").contains("engine_label_count: 1"));

    // The turn leaving the authoritative snapshot retires its label: nothing
    // stale can be inherited afterwards.
    delivered_snapshot(
        &mut controller,
        2,
        200,
        vec![make_turn_full(
            TURN_B,
            3,
            0,
            ConversationLifecycle::Pending,
            150,
            190,
        )],
        vec![make_user(USER_B, TURN_B, 4, "hey")],
    );
    assert!(format!("{controller:?}").contains("engine_label_count: 0"));

    // Returning without a fresh label renders generic again, while the
    // clock basis kept by the retained controller does not reset.
    delivered_snapshot(
        &mut controller,
        3,
        300,
        vec![make_turn_full(
            TURN_A,
            0,
            0,
            ConversationLifecycle::Pending,
            90,
            95,
        )],
        vec![make_user(USER_A, TURN_A, 1, "hi")],
    );
    assert_eq!(turn_status_engine_label(&controller, TURN_A), None);
    assert_eq!(turn_status_basis(&controller, TURN_A), Some(90));
}
