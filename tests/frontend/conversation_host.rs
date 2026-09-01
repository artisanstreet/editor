//! Black-box GPUI coverage for the native conversation host boundary.
//!
//! These tests mount the production [`ConversationHost`] and its production
//! [`ConversationSurface`] child. They inspect only public controller views,
//! scenes, effects, and entities; no test maintains a parallel conversation
//! state model.

use artisan_domain::{
    AssistantBody, AssistantMessageItem, AssistantMessagePhase, ConversationCursor,
    ConversationItem, ConversationLifecycle, ConversationPatch, ConversationSnapshot,
    ConversationTurn, IncrementalText, ItemId, ItemOrdinal, MessageBody, PatchBatch, PatchId,
    PatchSequence, RequestId, Revision, RunId, ThreadId, TurnId, TurnOrdinal, UnixMillis,
    UserMessageItem,
};
use artisan_frontend::{
    conversation_delivery_machine, conversation_host, conversation_scene,
    conversation_state_machine, conversation_steering_machine, conversation_surface,
    conversation_turn_machine, conversation_view_machine,
};
use artisan_ui::theme::ThemeMode;
use conversation_delivery_machine::{
    ConversationDeliveryEffect, ConversationDeliveryEvent, DeliveryPhase,
};
use conversation_host::{
    ConversationHost, ConversationHostEffect, ConversationHostError, ConversationHostRefusal,
};
use conversation_scene::{SceneDisclosure, SceneId, TurnBlock};
use conversation_state_machine::{
    ConversationStateEffect, ConversationStateError, ConversationStateEvent, SceneFact,
    SceneFactCommand, SceneFactKind,
};
use conversation_steering_machine::{
    SourceReference, SteeringEffect, SteeringEvent, SteeringLabelKind,
};
use conversation_surface::{
    ConversationSurfaceAction, ConversationSurfaceTarget, JUMP_TO_LATEST_SELECTOR,
    ViewportObservation, ordered_block_kinds,
};
use conversation_turn_machine::TurnEvent;
use conversation_view_machine::{DisclosureState, ViewportEffect};
use gpui::{Modifiers, TestAppContext, px, size};

const THREAD: &str = "thread_host";
const TURN_A: &str = "turn_a";
const USER_A: &str = "user_a";
const ASSISTANT_A: &str = "assistant_a";

fn thread_id() -> ThreadId {
    ThreadId::parse(THREAD).expect("test thread id is valid")
}

fn turn_id(value: &str) -> TurnId {
    TurnId::parse(value).expect("test turn id is valid")
}

fn item_id(value: &str) -> ItemId {
    ItemId::parse(value).expect("test item id is valid")
}

fn scene_id(value: &str) -> SceneId {
    SceneId::parse(value).expect("test scene id is valid")
}

fn request_id(value: &str) -> RequestId {
    RequestId::parse(value).expect("test request id is valid")
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
        body: MessageBody::parse(body.to_owned()).expect("test user body is valid"),
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
        run_id: RunId::parse("run_host").expect("test run id is valid"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Pending,
        body: AssistantBody::parse(body.to_owned()).expect("test assistant body is valid"),
        phase,
        created_at: stamp(2),
        updated_at: stamp(10),
    })
}

fn baseline_snapshot() -> ConversationSnapshot {
    ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(1),
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
        stamp(10),
    )
    .expect("test snapshot is valid")
}

fn gap_batch() -> PatchBatch {
    PatchBatch::new(
        thread_id(),
        ConversationCursor::new(2),
        ConversationCursor::new(3),
        vec![ConversationPatch::ItemAppend {
            patch_id: PatchId::parse("gap_host").expect("test patch id is valid"),
            sequence: PatchSequence::new(3).expect("test patch sequence is valid"),
            item_id: item_id(USER_A),
            revision: Revision::new(1),
            text: IncrementalText::parse("ignored").expect("test fragment is valid"),
            updated_at: stamp(11),
        }],
    )
    .expect("test gap batch is valid")
}

fn add_host(
    cx: &mut TestAppContext,
) -> (gpui::Entity<ConversationHost>, &mut gpui::VisualTestContext) {
    cx.add_window_view(|_, host_cx| {
        ConversationHost::new(thread_id(), ThemeMode::Dark, host_cx)
            .expect("fresh controller projects its empty scene")
    })
}

fn dispatch_snapshot(host: &gpui::Entity<ConversationHost>, cx: &mut gpui::VisualTestContext) {
    let snapshot = baseline_snapshot();
    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Delivery(ConversationDeliveryEvent::SnapshotReceived(
                    snapshot,
                )),
                host_cx,
            )
            .expect("snapshot dispatch succeeds");
        });
    });
}

fn dispatch_following_viewport_burst(
    host: &gpui::Entity<ConversationHost>,
    cx: &mut gpui::VisualTestContext,
    count: usize,
) {
    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            for _ in 0..count {
                host.dispatch(
                    ConversationStateEvent::Viewport(
                        conversation_view_machine::ViewportEvent::UserScrolled { at_bottom: true },
                    ),
                    host_cx,
                )
                .expect("bounded viewport burst is accepted");
            }
        });
    });
    cx.run_until_parked();
}

fn bottom_scroll_requests(effects: &[ConversationHostEffect]) -> usize {
    effects
        .iter()
        .filter(|effect| {
            matches!(
                effect,
                ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                    ViewportEffect::RequestBottomScroll { .. }
                ))
            )
        })
        .count()
}

fn proof_host(
    proof: &gpui::Entity<artisan_frontend::proof::ProofSurface>,
    cx: &mut gpui::VisualTestContext,
) -> gpui::Entity<ConversationHost> {
    cx.update(|_, app| {
        proof
            .read(app)
            .conversation_host()
            .cloned()
            .expect("proof mounts the genuine conversation host")
    })
}

fn drain_proof_effects(
    proof: &gpui::Entity<artisan_frontend::proof::ProofSurface>,
    cx: &mut gpui::VisualTestContext,
) -> Vec<ConversationHostEffect> {
    cx.update(|_, app| {
        proof.update(app, |proof, proof_cx| {
            proof.drain_conversation_effects(proof_cx)
        })
    })
}

fn assert_proof_boundary_is_full_and_scroll_is_blocked(
    proof: &gpui::Entity<artisan_frontend::proof::ProofSurface>,
    host: &gpui::Entity<ConversationHost>,
    cx: &mut gpui::VisualTestContext,
    maximum: usize,
) {
    cx.update(|_, app| {
        let proof = proof.read(app);
        assert_eq!(proof.pending_conversation_effects().len(), maximum);
        let host = host.read(app);
        assert_eq!(
            host.pending_effect_count(),
            conversation_host::CONVERSATION_HOST_MAX_EFFECTS
        );
        assert!(matches!(
            host.surface().read(app).pending_actions(),
            [ConversationSurfaceAction::ScrollIntent {
                target: ConversationSurfaceTarget::Scene(target)
            }] if target.as_str() == "proof-pump"
        ));
    });
}

#[gpui::test]
fn initial_scene_and_request_cross_the_host_boundary(cx: &mut TestAppContext) {
    let (host, cx) = add_host(cx);
    cx.run_until_parked();

    cx.update(|_, app| {
        let host = host.read(app);
        let controller_scene = host
            .controller_scene()
            .expect("fresh controller scene projects");
        let surface = host.surface().read(app);
        assert_eq!(surface.scene(), &controller_scene);
        assert_eq!(host.pending_effect_count(), 1);
        assert!(matches!(
            host.pending_effects(),
            [ConversationHostEffect::Controller(
                ConversationStateEffect::Delivery(
                    ConversationDeliveryEffect::RequestSnapshot {
                        thread_id: request_thread,
                        generation: 1,
                        after: None,
                    }
                )
            )] if request_thread == &thread_id()
        ));
    });
}

#[gpui::test]
fn snapshot_replaces_the_surface_from_pure_scene_order(cx: &mut TestAppContext) {
    let (host, cx) = add_host(cx);
    dispatch_snapshot(&host, cx);
    cx.run_until_parked();

    cx.update(|_, app| {
        let host = host.read(app);
        let controller_scene = host
            .controller_scene()
            .expect("accepted snapshot scene projects");
        assert!(host.controller_view().delivery.has_snapshot);
        let surface = host.surface().read(app);
        assert_eq!(surface.scene(), &controller_scene);
        assert_eq!(surface.scene().turn_scenes().len(), 1);
        assert_eq!(
            ordered_block_kinds(surface.scene()),
            vec![
                conversation_surface::RenderedBlockKind::UserMessage,
                conversation_surface::RenderedBlockKind::AssistantMessage,
                conversation_surface::RenderedBlockKind::TurnStatus,
                conversation_surface::RenderedBlockKind::TurnFooter,
            ]
        );
        assert!(host.pending_effects().iter().any(|effect| matches!(
            effect,
            ConversationHostEffect::Controller(ConversationStateEffect::Delivery(
                ConversationDeliveryEffect::Invalidate
            ))
        )));
    });
}

#[gpui::test]
fn generic_delivery_and_steering_invalidations_are_retained(cx: &mut TestAppContext) {
    let (host, cx) = add_host(cx);
    let command_id = request_id("command_host");
    let source_reference = SourceReference::parse("source_host").expect("test source is valid");
    let snapshot = baseline_snapshot();
    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::RegisterTurn {
                    turn_id: turn_id(TURN_A),
                },
                host_cx,
            )
            .expect("turn registration succeeds");
            host.dispatch(
                ConversationStateEvent::Delivery(ConversationDeliveryEvent::SnapshotReceived(
                    snapshot,
                )),
                host_cx,
            )
            .expect("snapshot dispatch succeeds");
            host.dispatch(
                ConversationStateEvent::RegisterSteering {
                    command_id: command_id.clone(),
                    generation: 1,
                    source_reference,
                    started_at_ms: 0,
                    label_kind: SteeringLabelKind::Steering,
                },
                host_cx,
            )
            .expect("steering registration succeeds");
            host.dispatch(
                ConversationStateEvent::Steering(SteeringEvent::DispatchStarted {
                    command_id,
                    generation: 1,
                    at_ms: 1,
                }),
                host_cx,
            )
            .expect("steering dispatch succeeds");
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        let host = host.read(app);
        assert!(host.pending_effects().iter().any(|effect| matches!(
            effect,
            ConversationHostEffect::Controller(ConversationStateEffect::SceneInvalidated)
        )));
        assert!(host.pending_effects().iter().any(|effect| matches!(
            effect,
            ConversationHostEffect::Controller(ConversationStateEffect::Delivery(
                ConversationDeliveryEffect::Invalidate
            ))
        )));
        assert!(host.pending_effects().iter().any(|effect| matches!(
            effect,
            ConversationHostEffect::Controller(ConversationStateEffect::Steering {
                effect: SteeringEffect::RenderInvalidation { generation: 1, .. },
                ..
            })
        )));
    });
}

#[gpui::test]
fn controller_refusal_is_atomic_and_retained_as_a_typed_effect(cx: &mut TestAppContext) {
    let (host, cx) = add_host(cx);
    let disclosure_id = scene_id("refusal");
    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::RegisterDisclosure {
                    scene_id: disclosure_id.clone(),
                    initially_working: false,
                },
                host_cx,
            )
            .expect("first disclosure registration succeeds");
            let before = host.controller_view();
            let refusal = host.dispatch(
                ConversationStateEvent::RegisterDisclosure {
                    scene_id: disclosure_id,
                    initially_working: false,
                },
                host_cx,
            );
            assert!(matches!(
                refusal,
                Err(ConversationHostError::Controller(
                    ConversationStateError::DuplicateDisclosure { .. }
                ))
            ));
            assert_eq!(host.controller_view(), before);
            assert!(host.pending_effects().iter().any(|effect| matches!(
                effect,
                ConversationHostEffect::Refused {
                    refusal: ConversationHostRefusal::Controller(
                        ConversationStateError::DuplicateDisclosure { .. }
                    )
                }
            )));
        });
    });
}

#[gpui::test]
fn disclosure_click_routes_user_open_and_close_through_controller(cx: &mut TestAppContext) {
    const TRIGGER: &str =
        "artisan-conversation-surface-turn-turn_a-block-user-user_a-disclosure-trigger";

    let (host, cx) = add_host(cx);
    let snapshot = baseline_snapshot();
    let disclosure_id = scene_id(USER_A);
    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Delivery(ConversationDeliveryEvent::SnapshotReceived(
                    snapshot,
                )),
                host_cx,
            )
            .expect("snapshot dispatch succeeds");
            host.dispatch(
                ConversationStateEvent::RegisterDisclosure {
                    scene_id: disclosure_id,
                    initially_working: false,
                },
                host_cx,
            )
            .expect("disclosure registration succeeds");
        });
    });
    cx.simulate_resize(size(px(720.0), px(520.0)));
    cx.run_until_parked();

    let trigger = cx
        .debug_bounds(TRIGGER)
        .expect("hosted disclosure trigger must paint bounds");
    cx.simulate_click(trigger.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let host = host.read(app);
        let disclosure = host
            .controller_view()
            .disclosure_views
            .into_iter()
            .find(|view| view.scene_id.as_str() == USER_A)
            .expect("registered disclosure view remains visible");
        assert_eq!(disclosure.state, DisclosureState::UserOpen);
        assert!(host.surface().read(app).pending_actions().is_empty());
        assert!(matches!(
            &host.surface().read(app).scene().turn_scenes()[0].blocks()[0],
            TurnBlock::UserMessage(message)
                if message.disclosure == Some(SceneDisclosure::Open)
        ));
    });

    let trigger = cx
        .debug_bounds(TRIGGER)
        .expect("closed/open trigger remains mounted");
    cx.simulate_click(trigger.center(), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let host = host.read(app);
        let disclosure = host
            .controller_view()
            .disclosure_views
            .into_iter()
            .find(|view| view.scene_id.as_str() == USER_A)
            .expect("registered disclosure view remains visible");
        assert_eq!(disclosure.state, DisclosureState::UserClosed);
        assert!(matches!(
            &host.surface().read(app).scene().turn_scenes()[0].blocks()[0],
            TurnBlock::UserMessage(message)
                if message.disclosure == Some(SceneDisclosure::Closed)
        ));
    });
}

#[gpui::test]
fn explicit_viewport_observations_and_scroll_intents_stay_typed(cx: &mut TestAppContext) {
    let (host, cx) = add_host(cx);
    cx.run_until_parked();
    let surface = cx.update(|_, app| host.read(app).surface().clone());

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            assert!(surface.observe_viewport(
                ViewportObservation {
                    first_visible: None,
                    last_visible: None,
                    at_bottom: false,
                },
                surface_cx,
            ));
            assert!(surface.request_scroll(
                ConversationSurfaceTarget::Scene(scene_id("latest")),
                surface_cx,
            ));
        });
    });
    cx.run_until_parked();

    cx.update(|_, app| {
        let host = host.read(app);
        assert!(host.controller_view().viewport_state.is_detached());
        assert!(host.pending_effects().iter().any(|effect| matches!(
            effect,
            ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                ViewportEffect::ShowJumpToLatest
            ))
        )));
        assert!(host.pending_effects().iter().any(|effect| matches!(
            effect,
            ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                ViewportEffect::InvalidateRender
            ))
        )));
        assert!(host.pending_effects().iter().any(|effect| matches!(
            effect,
            ConversationHostEffect::ScrollIntent {
                target: ConversationSurfaceTarget::Scene(target)
            } if target.as_str() == "latest"
        )));
        assert!(host.surface().read(app).pending_actions().is_empty());
    });

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            assert!(surface.observe_viewport(
                ViewportObservation {
                    first_visible: None,
                    last_visible: None,
                    at_bottom: true,
                },
                surface_cx,
            ));
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let host = host.read(app);
        assert!(host.controller_view().viewport_state.is_following());
        assert!(host.pending_effects().iter().any(|effect| matches!(
            effect,
            ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                ViewportEffect::HideJumpToLatest
            ))
        )));
    });
}

#[gpui::test]
fn jump_to_latest_routes_to_viewport_controller_and_enters_scrolling(cx: &mut TestAppContext) {
    let (host, cx) = add_host(cx);
    cx.run_until_parked();
    let surface = cx.update(|_, app| host.read(app).surface().clone());

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.set_jump_to_latest_visible(true, surface_cx);
        });
    });
    cx.run_until_parked();
    let button = cx
        .debug_bounds(JUMP_TO_LATEST_SELECTOR)
        .expect("hosted jump control must paint");
    cx.simulate_click(button.center(), Modifiers::none());
    cx.run_until_parked();

    cx.update(|_, app| {
        let host = host.read(app);
        assert!(host.controller_view().viewport_state.is_scrolling());
        assert_eq!(bottom_scroll_requests(host.pending_effects()), 1);
        assert!(host.pending_effects().iter().any(|effect| matches!(
            effect,
            ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                ViewportEffect::HideJumpToLatest
            ))
        )));
    });
}

#[gpui::test]
fn extent_changed_is_dispatched_once_for_render_invalidations_and_not_for_viewport_only(
    cx: &mut TestAppContext,
) {
    let (host, cx) = add_host(cx);
    cx.run_until_parked();
    cx.update(|_, app| {
        host.update(app, |host, _| {
            let _ = host.drain_effects();
        });
    });

    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::RegisterTurn {
                    turn_id: turn_id("extent-following"),
                },
                host_cx,
            )
            .expect("following scene invalidation is accepted");
        });
    });
    let following_effects = cx.update(|_, app| host.update(app, |host, _| host.drain_effects()));
    assert_eq!(bottom_scroll_requests(&following_effects), 1);

    let surface = cx.update(|_, app| host.read(app).surface().clone());
    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            assert!(surface.observe_viewport(
                ViewportObservation {
                    first_visible: None,
                    last_visible: None,
                    at_bottom: false,
                },
                surface_cx,
            ));
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        assert!(
            host.read(app)
                .controller_view()
                .viewport_state
                .is_detached()
        );
    });
    let detached_observation_effects =
        cx.update(|_, app| host.update(app, |host, _| host.drain_effects()));
    assert_eq!(bottom_scroll_requests(&detached_observation_effects), 0);

    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::RegisterTurn {
                    turn_id: turn_id("extent-detached"),
                },
                host_cx,
            )
            .expect("detached scene invalidation is accepted");
        });
    });
    let detached_effects = cx.update(|_, app| host.update(app, |host, _| host.drain_effects()));
    assert_eq!(bottom_scroll_requests(&detached_effects), 0);

    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Viewport(
                    conversation_view_machine::ViewportEvent::UserScrolled { at_bottom: true },
                ),
                host_cx,
            )
            .expect("physical bottom observation returns to following");
        });
    });
    let _ = cx.update(|_, app| host.update(app, |host, _| host.drain_effects()));
    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Viewport(
                    conversation_view_machine::ViewportEvent::UserScrolled { at_bottom: false },
                ),
                host_cx,
            )
            .expect("viewport-only invalidation is accepted");
        });
    });
    let viewport_only_effects =
        cx.update(|_, app| host.update(app, |host, _| host.drain_effects()));
    assert_eq!(bottom_scroll_requests(&viewport_only_effects), 0);
    assert!(viewport_only_effects.iter().any(|effect| matches!(
        effect,
        ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
            ViewportEffect::InvalidateRender
        ))
    )));
}

#[gpui::test]
fn effect_backpressure_preserves_controller_tail_and_refusal_atomicity(cx: &mut TestAppContext) {
    let (host, cx) = add_host(cx);
    cx.run_until_parked();

    for _ in 0..conversation_host::CONVERSATION_HOST_MAX_EFFECTS.saturating_sub(1) {
        cx.update(|_, app| {
            host.update(app, |host, host_cx| {
                host.dispatch(
                    ConversationStateEvent::Viewport(
                        conversation_view_machine::ViewportEvent::UserScrolled { at_bottom: true },
                    ),
                    host_cx,
                )
                .expect("accepted effects fit the controller");
            });
        });
    }
    let controller_tail_capacity =
        conversation_state_machine::MAX_PENDING_EFFECTS.saturating_sub(3);
    for _ in 0..controller_tail_capacity {
        cx.update(|_, app| {
            host.update(app, |host, host_cx| {
                host.dispatch(
                    ConversationStateEvent::Viewport(
                        conversation_view_machine::ViewportEvent::UserScrolled { at_bottom: true },
                    ),
                    host_cx,
                )
                .expect("accepted effects remain in the controller tail");
            });
        });
    }

    let before = cx.update(|_, app| host.read(app).controller_view());
    let refusal = cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Viewport(
                    conversation_view_machine::ViewportEvent::ExtentChanged,
                ),
                host_cx,
            )
        })
    });
    assert!(matches!(
        refusal,
        Err(ConversationHostError::EffectOutboxFull { .. })
    ));
    cx.update(|_, app| {
        let host = host.read(app);
        assert_eq!(host.controller_view(), before);
        assert_eq!(
            host.pending_effect_count(),
            conversation_host::CONVERSATION_HOST_MAX_EFFECTS
        );
        assert_eq!(
            host.pending_controller_effect_count(),
            controller_tail_capacity
        );
    });

    let drained = cx.update(|_, app| host.update(app, |host, _| host.drain_effects()));
    assert_eq!(
        drained.len(),
        conversation_host::CONVERSATION_HOST_MAX_EFFECTS
    );
    cx.update(|_, app| {
        let host = host.read(app);
        assert_eq!(host.pending_controller_effect_count(), 0);
        assert_eq!(host.pending_effect_count(), controller_tail_capacity);
    });
}

#[gpui::test]
fn blocked_surface_action_remains_until_the_host_retries_it(cx: &mut TestAppContext) {
    let (host, cx) = add_host(cx);
    cx.run_until_parked();
    let surface = cx.update(|_, app| host.read(app).surface().clone());

    for _ in 0..conversation_host::CONVERSATION_HOST_MAX_EFFECTS {
        cx.update(|_, app| {
            host.update(app, |host, host_cx| {
                host.dispatch(
                    ConversationStateEvent::Viewport(
                        conversation_view_machine::ViewportEvent::UserScrolled { at_bottom: true },
                    ),
                    host_cx,
                )
                .expect("host queue accepts the first bounded wave");
            });
        });
    }
    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            assert!(surface.request_scroll(
                ConversationSurfaceTarget::Scene(scene_id("blocked-first")),
                surface_cx,
            ));
            assert!(surface.request_scroll(
                ConversationSurfaceTarget::Scene(scene_id("blocked-second")),
                surface_cx,
            ));
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let pending_actions = host.read(app).surface().read(app).pending_actions();
        assert_eq!(pending_actions.len(), 2);
        assert!(matches!(
            &pending_actions[0],
            ConversationSurfaceAction::ScrollIntent {
                target: ConversationSurfaceTarget::Scene(target)
            } if target.as_str() == "blocked-first"
        ));
        assert!(matches!(
            &pending_actions[1],
            ConversationSurfaceAction::ScrollIntent {
                target: ConversationSurfaceTarget::Scene(target)
            } if target.as_str() == "blocked-second"
        ));
    });

    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            let _ = host.drain_effects();
            host.process_pending_actions(host_cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let host = host.read(app);
        assert!(host.surface().read(app).pending_actions().is_empty());
        let scroll_targets: Vec<_> = host
            .pending_effects()
            .iter()
            .filter_map(|effect| match effect {
                ConversationHostEffect::ScrollIntent {
                    target: ConversationSurfaceTarget::Scene(target),
                } => Some(target.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(scroll_targets, vec!["blocked-first", "blocked-second"]);
    });
}

#[gpui::test]
fn recovery_keeps_the_last_good_scene_and_close_is_typed(cx: &mut TestAppContext) {
    let (host, cx) = add_host(cx);
    dispatch_snapshot(&host, cx);
    cx.run_until_parked();
    let before = cx.update(|_, app| host.read(app).surface().read(app).scene().clone());

    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Delivery(ConversationDeliveryEvent::BatchReceived(
                    gap_batch(),
                )),
                host_cx,
            )
            .expect("gap is reported through typed recovery effects");
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let host = host.read(app);
        assert_eq!(
            host.controller_view().delivery_phase,
            DeliveryPhase::Recovering
        );
        assert_eq!(host.surface().read(app).scene(), &before);
        assert!(host.pending_effects().iter().any(|effect| matches!(
            effect,
            ConversationHostEffect::Controller(ConversationStateEffect::Delivery(
                ConversationDeliveryEffect::RequestSnapshot { .. }
            ))
        )));
    });

    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(ConversationStateEvent::Close, host_cx)
                .expect("close dispatch succeeds");
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let host = host.read(app);
        assert!(host.controller_view().closed);
        assert_eq!(host.surface().read(app).scene(), &before);
        assert!(host.pending_effects().iter().any(|effect| matches!(
            effect,
            ConversationHostEffect::Controller(ConversationStateEffect::Delivery(
                ConversationDeliveryEffect::OwnerClosed { .. }
            ))
        )));
    });
}

#[gpui::test]
fn compaction_and_streaming_narration_are_projected_by_the_controller_scene(
    cx: &mut TestAppContext,
) {
    let (host, cx) = add_host(cx);
    let snapshot = baseline_snapshot();
    let compaction = SceneFact::new(
        scene_id("compaction_host"),
        turn_id(TURN_A),
        3,
        SceneFactKind::Compaction {
            summary: "context compacted".to_owned(),
        },
    )
    .expect("test compaction fact is valid");
    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Delivery(ConversationDeliveryEvent::SnapshotReceived(
                    snapshot,
                )),
                host_cx,
            )
            .expect("snapshot dispatch succeeds");
            host.dispatch(
                ConversationStateEvent::Fact(SceneFactCommand::Register(compaction)),
                host_cx,
            )
            .expect("compaction fact dispatch succeeds");
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let host = host.read(app);
        let surface = host.surface().read(app);
        assert!(
            ordered_block_kinds(surface.scene())
                .contains(&conversation_surface::RenderedBlockKind::Compaction)
        );
        assert_eq!(
            surface.scene(),
            &host.controller_scene().expect("scene projects")
        );
    });
}

#[gpui::test]
fn streaming_narration_is_projected_by_the_controller_scene(cx: &mut TestAppContext) {
    let (host, cx) = add_host(cx);
    let streaming_snapshot = ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(1),
        vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)],
        vec![make_user(USER_A, TURN_A, 1, "hello")],
        stamp(10),
    )
    .expect("streaming snapshot is valid");
    cx.update(|_, app| {
        host.update(app, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Delivery(ConversationDeliveryEvent::SnapshotReceived(
                    streaming_snapshot,
                )),
                host_cx,
            )
            .expect("streaming snapshot dispatch succeeds");
            host.dispatch(
                ConversationStateEvent::RegisterTurn {
                    turn_id: turn_id(TURN_A),
                },
                host_cx,
            )
            .expect("streaming turn registration succeeds");
            host.dispatch(
                ConversationStateEvent::Turn {
                    turn_id: turn_id(TURN_A),
                    event: TurnEvent::Thinking { at: 1, revision: 1 },
                },
                host_cx,
            )
            .expect("thinking event dispatch succeeds");
            host.dispatch(
                ConversationStateEvent::Turn {
                    turn_id: turn_id(TURN_A),
                    event: TurnEvent::StreamingReply { at: 2, revision: 2 },
                },
                host_cx,
            )
            .expect("streaming event dispatch succeeds");
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let host = host.read(app);
        assert_eq!(
            ordered_block_kinds(host.surface().read(app).scene()),
            vec![
                conversation_surface::RenderedBlockKind::UserMessage,
                conversation_surface::RenderedBlockKind::TurnStatus,
                conversation_surface::RenderedBlockKind::TurnFooter,
            ]
        );
        assert_eq!(
            host.surface().read(app).scene(),
            &host.controller_scene().expect("streaming scene projects")
        );
    });
}

#[gpui::test]
fn proof_mounts_the_genuine_host_and_retains_the_initial_effect(cx: &mut TestAppContext) {
    let (proof, cx) = cx.add_window_view(artisan_frontend::proof::ProofSurface::new);
    cx.run_until_parked();

    cx.update(|window, app| {
        let proof_ref = proof.read(app);
        assert!(proof_ref.conversation_host().is_some());
        assert!(matches!(
            proof_ref.pending_conversation_effects(),
            [ConversationHostEffect::Controller(
                ConversationStateEffect::Delivery(ConversationDeliveryEffect::RequestSnapshot {
                    thread_id: request_thread,
                    generation: 1,
                    after: None,
                })
            )] if request_thread.as_str() == "proof-conversation"
        ));
        assert!(proof_ref.root_focus().is_focused(window));
        assert!(
            !proof_ref
                .picker()
                .read(app)
                .trigger_focus()
                .is_focused(window)
        );
        assert_eq!(proof_ref.clicks(), 0);
    });

    cx.simulate_click(gpui::point(px(40.0), px(40.0)), Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| assert_eq!(proof.read(app).clicks(), 1));
}

#[gpui::test]
fn proof_drain_pumps_full_boundary_without_unrelated_activity(cx: &mut TestAppContext) {
    let (proof, cx) = cx.add_window_view(artisan_frontend::proof::ProofSurface::new);
    cx.run_until_parked();
    let host = proof_host(&proof, cx);
    let surface = cx.update(|_, app| host.read(app).surface().clone());
    let maximum = artisan_frontend::proof::PROOF_MAX_CONVERSATION_EFFECTS;

    dispatch_following_viewport_burst(&host, cx, maximum.saturating_sub(1));
    cx.update(|_, app| {
        assert_eq!(
            proof.read(app).pending_conversation_effects().len(),
            maximum
        );
    });

    dispatch_following_viewport_burst(&host, cx, conversation_host::CONVERSATION_HOST_MAX_EFFECTS);
    cx.update(|_, app| {
        let host = host.read(app);
        assert_eq!(
            host.pending_effect_count(),
            conversation_host::CONVERSATION_HOST_MAX_EFFECTS
        );
        assert_eq!(host.pending_controller_effect_count(), 0);
    });

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            assert!(surface.request_scroll(
                ConversationSurfaceTarget::Scene(scene_id("proof-pump")),
                surface_cx,
            ));
        });
    });
    cx.run_until_parked();
    assert_proof_boundary_is_full_and_scroll_is_blocked(&proof, &host, cx, maximum);

    let first = drain_proof_effects(&proof, cx);
    assert_eq!(first.len(), maximum);
    assert!(matches!(
        first.first(),
        Some(ConversationHostEffect::Controller(
            ConversationStateEffect::Delivery(ConversationDeliveryEffect::RequestSnapshot {
                thread_id,
                generation: 1,
                after: None,
            })
        )) if thread_id.as_str() == "proof-conversation"
    ));
    assert!(first.iter().skip(1).all(|effect| matches!(
        effect,
        ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
            ViewportEffect::HideJumpToLatest
        ))
    )));
    cx.update(|_, app| {
        let host = host.read(app);
        assert!(host.surface().read(app).pending_actions().is_empty());
        assert!(matches!(
            host.pending_effects(),
            [ConversationHostEffect::ScrollIntent {
                target: ConversationSurfaceTarget::Scene(target)
            }] if target.as_str() == "proof-pump"
        ));
    });

    let second = drain_proof_effects(&proof, cx);
    assert_eq!(second.len(), maximum);
    assert!(second.iter().all(|effect| matches!(
        effect,
        ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
            ViewportEffect::HideJumpToLatest
        ))
    )));
    let third = drain_proof_effects(&proof, cx);
    assert!(matches!(
        third.as_slice(),
        [ConversationHostEffect::ScrollIntent {
            target: ConversationSurfaceTarget::Scene(target)
        }] if target.as_str() == "proof-pump"
    ));
    cx.update(|_, app| {
        assert!(proof.read(app).pending_conversation_effects().is_empty());
    });
}

#[test]
fn refusal_effects_are_typed_and_redacted() {
    let refusal = ConversationHostEffect::Refused {
        refusal: ConversationHostRefusal::EffectOutboxFull {
            count: conversation_host::CONVERSATION_HOST_MAX_EFFECTS,
            maximum: conversation_host::CONVERSATION_HOST_MAX_EFFECTS,
        },
    };
    assert!(matches!(
        refusal,
        ConversationHostEffect::Refused {
            refusal: ConversationHostRefusal::EffectOutboxFull { .. }
        }
    ));
}
