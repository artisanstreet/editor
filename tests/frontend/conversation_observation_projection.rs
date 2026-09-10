//! Black-box coverage for the persisted engine activity projection.
//!
//! Tests use only the public frontend crate boundary plus public domain
//! types: retained [`EngineObservationState`] rows with real Forge
//! attribution project through [`project_activities`] into atomic
//! [`SceneFactCommand::Upsert`] facts on a [`ConversationStateController`].
//! No clocks are sampled; every timestamp is a persisted fixture value.

use artisan_domain::{
    AgentMessageCompletedObservation, AssistantBody, AssistantMessageItem, AssistantMessagePhase,
    ConversationCursor, ConversationItem, ConversationLifecycle, ConversationSnapshot,
    ConversationTurn, EngineObservationAttribution, EngineObservationEvent, ItemId, ItemOrdinal,
    MessageBody, MessagePhase, Observation, ObservationId, ObservationSequence,
    ReasoningSummaryCompletedObservation, ReasoningSummaryDeltaObservation, Revision, RunId,
    TerminalActivityInput, TerminalActivityObservation, TerminalActivityState, ThreadId, ToolAction,
    ToolObservation, TurnId, TurnOrdinal, UnixMillis, UserMessageItem,
};
use artisan_frontend::conversation_delivery_machine::ConversationDeliveryEvent;
use artisan_frontend::conversation_observation_projection::project_activities;
use artisan_frontend::conversation_scene::{SceneId, TurnBlock, TurnNarration as SceneTurnNarration};
use artisan_frontend::conversation_state_machine::{
    ConversationStateController, SceneFact, SceneFactKind,
};
use artisan_frontend::engine_observation_state::{ApplyOutcome, EngineObservationState};

const THREAD: &str = "thread-activity";
const TURN_A: &str = "turn_a";
const TURN_B: &str = "turn_b";
const RUN_A: &str = "run_a";
const RUN_B: &str = "run_b";

fn thread_id() -> ThreadId {
    ThreadId::parse(THREAD).expect("valid thread id")
}

fn turn_id(value: &str) -> TurnId {
    TurnId::parse(value).expect("valid turn id")
}

fn run_id(value: &str) -> RunId {
    RunId::parse(value).expect("valid run id")
}

fn observation_id(value: &str) -> ObservationId {
    ObservationId::parse(value).expect("valid observation id")
}

fn sequence(value: u64) -> ObservationSequence {
    ObservationSequence::new(value).expect("valid sequence")
}

fn stamp(millis: i64) -> UnixMillis {
    UnixMillis::from_millis(millis)
}

fn attribution(run: &str, turn: &str, committed_at: i64, delivery_sequence: u64) -> EngineObservationAttribution {
    EngineObservationAttribution {
        run_id: run_id(run),
        turn_id: turn_id(turn),
        committed_at: stamp(committed_at),
        delivery_sequence,
    }
}

fn tool_observation(obs: &str, seq: u64, tool: &str, action: ToolAction) -> Observation {
    Observation::Tool(
        ToolObservation::new(
            observation_id(obs),
            sequence(seq),
            observation_id(tool),
            String::from("read"),
            action,
            None,
        )
        .expect("valid tool observation"),
    )
}

fn reasoning_delta(obs: &str, seq: u64, item: &str, delta: &str) -> Observation {
    Observation::ReasoningSummaryDelta(
        ReasoningSummaryDeltaObservation::new(
            observation_id(obs),
            sequence(seq),
            observation_id(item),
            0,
            String::from(delta),
            None,
            observation_id("provider-turn"),
        )
        .expect("valid reasoning delta"),
    )
}

fn reasoning_completed(obs: &str, seq: u64, item: &str, text: Option<&str>) -> Observation {
    Observation::ReasoningSummaryCompleted(
        ReasoningSummaryCompletedObservation::new(
            observation_id(obs),
            sequence(seq),
            observation_id(item),
            text.map(str::to_owned),
            observation_id("provider-turn"),
        )
        .expect("valid reasoning completion"),
    )
}

fn attributed_event(
    observation: Observation,
    run: &str,
    turn: &str,
    committed_at: i64,
    delivery_sequence: u64,
) -> EngineObservationEvent {
    EngineObservationEvent {
        thread_id: thread_id(),
        observation,
        attribution: Some(attribution(run, turn, committed_at, delivery_sequence)),
    }
}

fn legacy_event(observation: Observation) -> EngineObservationEvent {
    EngineObservationEvent {
        thread_id: thread_id(),
        observation,
        attribution: None,
    }
}

fn make_turn(id: &str, ordinal: u64, lifecycle: ConversationLifecycle) -> ConversationTurn {
    ConversationTurn {
        turn_id: turn_id(id),
        ordinal: TurnOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle,
        created_at: stamp(1_000),
        updated_at: stamp(5_000),
    }
}

fn make_user(id: &str, turn: &str, ordinal: u64) -> ConversationItem {
    ConversationItem::UserMessage(UserMessageItem {
        item_id: ItemId::parse(id).expect("valid item id"),
        turn_id: turn_id(turn),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Completed,
        body: MessageBody::parse(String::from("hello")).expect("valid body"),
        created_at: stamp(900),
        updated_at: stamp(950),
    })
}

fn make_assistant(id: &str, turn: &str, ordinal: u64, run: &str) -> ConversationItem {
    ConversationItem::AssistantMessage(AssistantMessageItem {
        item_id: ItemId::parse(id).expect("valid item id"),
        turn_id: turn_id(turn),
        run_id: run_id(run),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Completed,
        body: AssistantBody::parse(String::from("done")).expect("valid body"),
        phase: AssistantMessagePhase::Final,
        created_at: stamp(1_100),
        updated_at: stamp(5_000),
    })
}

fn snapshot(turns: Vec<ConversationTurn>, items: Vec<ConversationItem>) -> ConversationSnapshot {
    ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(9),
        turns,
        items,
        stamp(5_000),
    )
    .expect("valid snapshot")
}

fn controller_with_snapshot(snapshot: ConversationSnapshot) -> ConversationStateController {
    let mut controller = ConversationStateController::new(thread_id());
    let _ = controller.drain_effects();
    controller
        .on_delivery(ConversationDeliveryEvent::SnapshotReceived(snapshot))
        .expect("snapshot delivery succeeds");
    let _ = controller.drain_effects();
    controller
}

fn upsert_all(controller: &mut ConversationStateController, facts: Vec<SceneFact>) {
    for fact in facts {
        controller
            .upsert_fact(fact)
            .expect("activity fact upserts");
    }
    let _ = controller.drain_effects();
}

fn work_bodies(controller: &ConversationStateController, turn: &str) -> Vec<String> {
    let scene = controller.scene().expect("scene builds");
    let turn_scene = scene
        .turn_scene(&turn_id(turn))
        .expect("turn scene exists");
    let mut bodies = Vec::new();
    for block in turn_scene.blocks() {
        if let TurnBlock::WorkGroup(group) = block {
            for item in &group.items {
                match item {
                    artisan_frontend::conversation_scene::WorkItem::Activity { body, .. }
                    | artisan_frontend::conversation_scene::WorkItem::Reasoning { body, .. } => {
                        bodies.push(body.clone());
                    }
                    artisan_frontend::conversation_scene::WorkItem::WorkSession { title, .. } => {
                        bodies.push(title.clone());
                    }
                }
            }
        }
    }
    bodies
}

#[test]
fn attributed_tool_event_projects_stable_fact_into_scene() {
    let mut state = EngineObservationState::new(thread_id());
    let outcome = state.apply(
        7,
        &attributed_event(
            tool_observation("obs-tool-1", 1, "tool-1", ToolAction::Started),
            RUN_A,
            TURN_A,
            2_000,
            10,
        ),
    );
    assert!(matches!(outcome, ApplyOutcome::Applied { .. }));

    let snapshot = snapshot(vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)], vec![
        make_user("user_a", TURN_A, 1),
    ]);
    let projection = project_activities(&state, &snapshot);
    assert_eq!(projection.facts.len(), 1);
    assert_eq!(projection.pending, 0);
    assert_eq!(projection.rejected, 0);
    let fact = &projection.facts[0];
    assert_eq!(fact.turn_id, turn_id(TURN_A));
    assert_eq!(fact.observed_at_ms, Some(2_000));
    assert!(matches!(&fact.kind, SceneFactKind::Activity { .. }));

    let mut controller = controller_with_snapshot(snapshot);
    upsert_all(&mut controller, projection.facts);
    let bodies = work_bodies(&controller, TURN_A);
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].contains("read"), "unexpected body {}", bodies[0]);
}

#[test]
fn foreign_thread_run_and_turn_are_rejected_without_facts() {
    let mut state = EngineObservationState::new(thread_id());
    let foreign_thread = EngineObservationEvent {
        thread_id: ThreadId::parse("thread-other").expect("valid thread id"),
        observation: tool_observation("obs-foreign", 1, "tool-9", ToolAction::Started),
        attribution: Some(attribution(RUN_A, TURN_A, 2_000, 10)),
    };
    assert!(matches!(
        state.apply(1, &foreign_thread),
        ApplyOutcome::StaleThread
    ));

    // Attributed turn with no canonical turn stays pending for later replay.
    state.apply(
        2,
        &attributed_event(
            tool_observation("obs-pending", 2, "tool-9", ToolAction::Started),
            RUN_A,
            TURN_A,
            2_000,
            11,
        ),
    );
    let before_turn = snapshot(vec![], vec![]);
    let projection = project_activities(&state, &before_turn);
    assert!(projection.facts.is_empty());
    assert_eq!(projection.pending, 1);

    // The snapshot settles run_a for turn_a; an event from run_b is foreign.
    state.apply(
        3,
        &attributed_event(
            tool_observation("obs-foreign-run", 3, "tool-9", ToolAction::Started),
            RUN_B,
            TURN_A,
            2_100,
            12,
        ),
    );
    let settled = snapshot(
        vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)],
        vec![
            make_user("user_a", TURN_A, 1),
            make_assistant("assistant_a", TURN_A, 2, RUN_A),
        ],
    );
    let projection = project_activities(&state, &settled);
    assert_eq!(projection.rejected, 1);
    assert_eq!(projection.facts.len(), 1);
    assert_eq!(
        projection.facts[0].turn_id,
        turn_id(TURN_A),
        "only the attributed run_a row projects"
    );
}

#[test]
fn legacy_events_pair_but_never_project() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(
        1,
        &legacy_event(tool_observation("obs-legacy", 1, "tool-1", ToolAction::Started)),
    );
    assert!(state.tool("tool-1").is_some(), "typed payload is retained");
    let snapshot = snapshot(vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)], vec![
        make_user("user_a", TURN_A, 1),
    ]);
    let projection = project_activities(&state, &snapshot);
    assert!(projection.facts.is_empty(), "legacy rows must not fabricate facts");
}

#[test]
fn reconnect_wire_reset_applies_new_delivery_and_ignores_replay() {
    let mut state = EngineObservationState::new(thread_id());
    // First connection: wire cursor 100 carries delivery sequence 10.
    let first = state.apply(
        100,
        &attributed_event(
            tool_observation("obs-first", 1, "tool-1", ToolAction::Started),
            RUN_A,
            TURN_A,
            2_000,
            10,
        ),
    );
    assert!(matches!(first, ApplyOutcome::Applied { .. }));

    // Reconnect: the backend writer resets the wire cursor to 1 while the
    // durable delivery sequence keeps increasing. Delivery 11 must apply.
    let second = state.apply(
        1,
        &attributed_event(
            tool_observation("obs-second", 2, "tool-1", ToolAction::Progress),
            RUN_A,
            TURN_A,
            2_100,
            11,
        ),
    );
    assert!(
        matches!(second, ApplyOutcome::Applied { .. }),
        "wire-cursor reset must not drop real post-reconnect rows"
    );

    // Exact replay of delivery 10 on the new connection is ignored.
    let replay = state.apply(
        2,
        &attributed_event(
            tool_observation("obs-first", 1, "tool-1", ToolAction::Started),
            RUN_A,
            TURN_A,
            2_000,
            10,
        ),
    );
    assert!(
        matches!(replay, ApplyOutcome::Duplicate),
        "duplicate delivery sequences are ignored across connections"
    );

    let row = state
        .tool_scoped(&run_id(RUN_A), "tool-1")
        .expect("run-scoped row exists");
    assert_eq!(row.action(), ToolAction::Progress);
    assert_eq!(row.delivery_sequence(), Some(11));
}

#[test]
fn duplicate_reasoning_deltas_upsert_one_cumulative_card() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(
        1,
        &attributed_event(
            reasoning_delta("obs-rd-1", 1, "reason-1", "thinking "),
            RUN_A,
            TURN_A,
            2_000,
            10,
        ),
    );
    state.apply(
        2,
        &attributed_event(
            reasoning_delta("obs-rd-2", 2, "reason-1", "deeply."),
            RUN_A,
            TURN_A,
            2_050,
            11,
        ),
    );
    let snapshot = snapshot(vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)], vec![
        make_user("user_a", TURN_A, 1),
    ]);
    let projection = project_activities(&state, &snapshot);
    assert_eq!(projection.facts.len(), 1, "deltas must not repeat as new cards");
    assert!(
        matches!(&projection.facts[0].kind, SceneFactKind::Reasoning { body } if body == "thinking deeply."),
        "unexpected cumulative body"
    );

    let mut controller = controller_with_snapshot(snapshot);
    let fact_id = projection.facts[0].id.clone();
    let ordinal = projection.facts[0].ordinal;
    upsert_all(&mut controller, projection.facts);

    // A repeated projection of unchanged rows is effect-quiet and stable.
    let repeat = project_activities(&state, &snapshot_for_repeat());
    assert_eq!(repeat.facts.len(), 1);
    assert_eq!(repeat.facts[0].id, fact_id);
    controller
        .upsert_fact(repeat.facts[0].clone())
        .expect("identical upsert succeeds");
    assert!(
        controller.drain_effects().is_empty(),
        "identical upserts must be effect-quiet"
    );
    let scene = controller.scene().expect("scene builds");
    assert!(
        scene
            .turn_scene(&turn_id(TURN_A))
            .expect("turn scene exists")
            .blocks()
            .iter()
            .any(|block| matches!(block, TurnBlock::WorkGroup(_))),
        "the card is never removed by repeated projection"
    );
    let _ = ordinal;
}

fn snapshot_for_repeat() -> ConversationSnapshot {
    snapshot(vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)], vec![
        make_user("user_a", TURN_A, 1),
    ])
}

#[test]
fn events_before_snapshot_replay_once_canonical_turn_exists() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(
        4,
        &attributed_event(
            tool_observation("obs-early", 1, "tool-1", ToolAction::Started),
            RUN_A,
            TURN_A,
            2_000,
            10,
        ),
    );
    // No host mount and no canonical turn yet: nothing projects, rows stay.
    let empty = snapshot(vec![], vec![]);
    let early = project_activities(&state, &empty);
    assert!(early.facts.is_empty());
    assert_eq!(early.pending, 1);

    let mounted = snapshot(vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)], vec![
        make_user("user_a", TURN_A, 1),
    ]);
    let late = project_activities(&state, &mounted);
    assert_eq!(late.facts.len(), 1);
    let mut controller = controller_with_snapshot(mounted);
    upsert_all(&mut controller, late.facts);
    assert_eq!(work_bodies(&controller, TURN_A).len(), 1);
}

#[test]
fn two_runs_sharing_provider_names_project_separate_cards() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(
        1,
        &attributed_event(
            tool_observation("obs-run-a", 1, "tool-1", ToolAction::Started),
            RUN_A,
            TURN_A,
            2_000,
            10,
        ),
    );
    state.apply(
        2,
        &attributed_event(
            tool_observation("obs-run-b", 1, "tool-1", ToolAction::Completed),
            RUN_B,
            TURN_B,
            2_100,
            11,
        ),
    );
    let snapshot = snapshot(
        vec![
            make_turn(TURN_A, 0, ConversationLifecycle::Active),
            make_turn(TURN_B, 1, ConversationLifecycle::Active),
        ],
        vec![
            make_user("user_a", TURN_A, 2),
            make_user("user_b", TURN_B, 3),
        ],
    );
    let projection = project_activities(&state, &snapshot);
    assert_eq!(projection.facts.len(), 2);
    assert_ne!(projection.facts[0].id, projection.facts[1].id);

    let mut controller = controller_with_snapshot(snapshot);
    upsert_all(&mut controller, projection.facts);
    assert_eq!(work_bodies(&controller, TURN_A).len(), 1);
    assert_eq!(work_bodies(&controller, TURN_B).len(), 1);
}

#[test]
fn successive_tool_progress_updates_keep_card_and_settle_elapsed() {
    let mut state = EngineObservationState::new(thread_id());
    for (cursor, seq, delivery, action) in [
        (1_u64, 1_u64, 10_u64, ToolAction::Started),
        (2, 2, 11, ToolAction::Progress),
        (3, 3, 12, ToolAction::Completed),
    ] {
        state.apply(
            cursor,
            &attributed_event(
                tool_observation(&format!("obs-tool-{delivery}"), seq, "tool-1", action),
                RUN_A,
                TURN_A,
                2_000 + delivery as i64 * 10,
                delivery,
            ),
        );
    }
    let snapshot = snapshot(
        vec![ConversationTurn {
            turn_id: turn_id(TURN_A),
            ordinal: TurnOrdinal::new(0),
            revision: Revision::new(0),
            lifecycle: ConversationLifecycle::Completed,
            created_at: stamp(1_000),
            updated_at: stamp(5_000),
        }],
        vec![
            make_user("user_a", TURN_A, 1),
            make_assistant("assistant_a", TURN_A, 2, RUN_A),
        ],
    );
    let projection = project_activities(&state, &snapshot);
    assert_eq!(projection.facts.len(), 1);
    let fact_id = projection.facts[0].id.clone();

    let mut controller = controller_with_snapshot(snapshot);
    controller
        .register_disclosure(fact_id.clone(), true)
        .expect("disclosure registers");
    let _ = controller.drain_effects();
    upsert_all(&mut controller, projection.facts);

    let bodies = work_bodies(&controller, TURN_A);
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].contains("completed"), "unexpected body {}", bodies[0]);

    // Settled elapsed derives from canonical turn times, not the sequence.
    let scene = controller.scene().expect("scene builds");
    let status = scene
        .turn_scene(&turn_id(TURN_A))
        .expect("turn scene exists")
        .blocks()
        .iter()
        .find_map(|block| match block {
            TurnBlock::TurnStatus(status) => Some(status.narration),
            _ => None,
        })
        .expect("settled status exists");
    assert_eq!(status, SceneTurnNarration::WorkedFor { millis: 4_000 });

    // Repeating the settled projection never removes the card, resets the
    // elapsed time, or loses the disclosure registration.
    let repeat = project_activities(&state, &snapshot_for_settled_repeat());
    assert_eq!(repeat.facts.len(), 1);
    assert_eq!(repeat.facts[0].id, fact_id);
    controller
        .upsert_fact(repeat.facts[0].clone())
        .expect("settled upsert succeeds");
    assert!(
        controller.drain_effects().is_empty(),
        "settled repeats must be effect-quiet"
    );
    let scene = controller.scene().expect("scene builds");
    let status = scene
        .turn_scene(&turn_id(TURN_A))
        .expect("turn scene exists")
        .blocks()
        .iter()
        .find_map(|block| match block {
            TurnBlock::TurnStatus(status) => Some(status.narration),
            _ => None,
        })
        .expect("settled status exists");
    assert_eq!(status, SceneTurnNarration::WorkedFor { millis: 4_000 });
    assert!(
        controller
            .view()
            .disclosure_views
            .iter()
            .any(|view| view.scene_id == fact_id),
        "disclosure survives repeated upserts"
    );
    assert_eq!(work_bodies(&controller, TURN_A).len(), 1);
}

fn snapshot_for_settled_repeat() -> ConversationSnapshot {
    snapshot(
        vec![ConversationTurn {
            turn_id: turn_id(TURN_A),
            ordinal: TurnOrdinal::new(0),
            revision: Revision::new(0),
            lifecycle: ConversationLifecycle::Completed,
            created_at: stamp(1_000),
            updated_at: stamp(5_000),
        }],
        vec![
            make_user("user_a", TURN_A, 1),
            make_assistant("assistant_a", TURN_A, 2, RUN_A),
        ],
    )
}

#[test]
fn plain_reply_projects_nothing_and_stays_provider_wait() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(
        1,
        &attributed_event(
            Observation::AgentMessageCompleted(
                AgentMessageCompletedObservation::new(
                    observation_id("obs-msg"),
                    sequence(1),
                    observation_id("item-1"),
                    MessagePhase::Final,
                    String::from("a plain reply"),
                    observation_id("provider-turn"),
                )
                .expect("valid completed message"),
            ),
            RUN_A,
            TURN_A,
            2_000,
            10,
        ),
    );
    let snapshot = snapshot(vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)], vec![
        make_user("user_a", TURN_A, 1),
    ]);
    let projection = project_activities(&state, &snapshot);
    assert!(
        projection.facts.is_empty(),
        "plain replies are a no-activity negative"
    );

    let controller = controller_with_snapshot(snapshot);
    let scene = controller.scene().expect("scene builds");
    let status = scene
        .turn_scene(&turn_id(TURN_A))
        .expect("turn scene exists")
        .blocks()
        .iter()
        .find_map(|block| match block {
            TurnBlock::TurnStatus(status) => Some(status.narration),
            _ => None,
        })
        .expect("status exists");
    assert_eq!(status, SceneTurnNarration::ProviderWait);
}

#[test]
fn upsert_refuses_cross_turn_reassignment() {
    let snapshot = snapshot(vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)], vec![
        make_user("user_a", TURN_A, 1),
    ]);
    let mut controller = controller_with_snapshot(snapshot);
    let fact = SceneFact::new(
        SceneId::parse("activity-fact").expect("valid scene id"),
        turn_id(TURN_A),
        100,
        SceneFactKind::Activity {
            body: String::from("tool read started"),
        },
    )
    .expect("valid fact");
    controller.upsert_fact(fact).expect("first upsert inserts");
    let _ = controller.drain_effects();

    let moved = SceneFact::new(
        SceneId::parse("activity-fact").expect("valid scene id"),
        turn_id(TURN_B),
        101,
        SceneFactKind::Activity {
            body: String::from("tool read started"),
        },
    )
    .expect("valid fact");
    let error = controller
        .upsert_fact(moved)
        .expect_err("cross-turn reassignment is refused");
    assert!(
        matches!(
            error,
            artisan_frontend::conversation_state_machine::ConversationStateError::FactTurnMismatch { .. }
        ),
        "unexpected error {error:?}"
    );
}

#[test]
fn reasoning_completion_without_delta_still_settles_public_summary() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(
        1,
        &attributed_event(
            reasoning_completed("obs-rc", 1, "reason-9", Some("public summary")),
            RUN_A,
            TURN_A,
            2_000,
            10,
        ),
    );
    let snapshot = snapshot(vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)], vec![
        make_user("user_a", TURN_A, 1),
    ]);
    let projection = project_activities(&state, &snapshot);
    assert_eq!(projection.facts.len(), 1);
    assert!(
        matches!(&projection.facts[0].kind, SceneFactKind::Reasoning { body } if body == "public summary")
    );
}

#[test]
fn failed_terminal_projects_error_card() {
    let mut state = EngineObservationState::new(thread_id());
    state.apply(
        1,
        &attributed_event(
            Observation::TerminalActivity(
                TerminalActivityObservation::new(
                    observation_id("obs-term"),
                    sequence(1),
                    TerminalActivityInput {
                        activity_id: observation_id("activity-1"),
                        channel: None,
                        command: Some(String::from("cargo test")),
                        shell: None,
                        output: None,
                        exit_code: Some(1),
                        state: TerminalActivityState::Failed,
                    },
                )
                .expect("valid terminal observation"),
            ),
            RUN_A,
            TURN_A,
            2_000,
            10,
        ),
    );
    let snapshot = snapshot(vec![make_turn(TURN_A, 0, ConversationLifecycle::Active)], vec![
        make_user("user_a", TURN_A, 1),
    ]);
    let projection = project_activities(&state, &snapshot);
    assert_eq!(projection.facts.len(), 1);
    assert!(
        matches!(&projection.facts[0].kind, SceneFactKind::Error { .. }),
        "failed terminals surface as error cards"
    );
}
