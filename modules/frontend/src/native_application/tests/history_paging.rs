//! On-demand history of the open thread: older turns are read as the reader
//! nears the start of the loaded ones, and a settled turn's work rows are
//! read when its section opens.

use super::*;
use crate::conversation_observation_projection::held_back_fact_id;
use crate::conversation_scene::{SceneDisclosure, session_anchor_id};
use crate::conversation_state_machine::ConversationStateEvent;
use crate::conversation_view_machine::DisclosureEvent;
use artisan_domain::{
    AssistantBody, AssistantMessageItem, AssistantMessagePhase, ConversationHistoryPage,
    ConversationHistoryPart, EngineObservationAttribution, EngineObservationEvent,
    HeldBackTurnWork, HeldBackWork, Observation, ObservationSequence, ToolAction, ToolObservation,
};

fn thread_a() -> ThreadId {
    ThreadId::parse("thread-a").expect("thread")
}

fn turn_id(value: &str) -> TurnId {
    TurnId::parse(value).expect("turn")
}

fn turn(id: &str, ordinal: u64, lifecycle: ConversationLifecycle) -> ConversationTurn {
    ConversationTurn {
        turn_id: turn_id(id),
        ordinal: TurnOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle,
        created_at: UnixMillis::EPOCH,
        updated_at: UnixMillis::from_millis(10),
    }
}

fn user(id: &str, turn: &str, ordinal: u64) -> ConversationItem {
    ConversationItem::UserMessage(UserMessageItem {
        item_id: ItemId::parse(id).expect("item"),
        turn_id: turn_id(turn),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Completed,
        body: MessageBody::parse(format!("question of {turn}")).expect("user body"),
        source_message_id: None,
        created_at: UnixMillis::EPOCH,
        updated_at: UnixMillis::from_millis(10),
    })
}

fn reply(id: &str, turn: &str, ordinal: u64, run: &str) -> ConversationItem {
    ConversationItem::AssistantMessage(AssistantMessageItem {
        item_id: ItemId::parse(id).expect("item"),
        turn_id: turn_id(turn),
        run_id: RunId::parse(run).expect("run"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Completed,
        body: AssistantBody::parse(format!("answer of {turn}")).expect("assistant body"),
        phase: AssistantMessagePhase::Final,
        created_at: UnixMillis::from_millis(5),
        updated_at: UnixMillis::from_millis(10),
    })
}

fn snapshot(turns: Vec<ConversationTurn>, items: Vec<ConversationItem>) -> ConversationSnapshot {
    ConversationSnapshot::new(
        thread_a(),
        ConversationCursor::new(7),
        turns,
        items,
        UnixMillis::from_millis(10),
    )
    .expect("snapshot")
}

fn held_back(turn: &str, run: &str, rows: u32, first: u64) -> HeldBackTurnWork {
    HeldBackTurnWork {
        turn_id: turn_id(turn),
        run_id: RunId::parse(run).expect("run"),
        row_count: rows,
        first_committed_at: UnixMillis::from_millis(3),
        first_delivery_sequence: first,
    }
}

fn tool_row(turn: &str, run: &str, tool: &str, delivery_sequence: u64) -> EngineObservationEvent {
    EngineObservationEvent {
        thread_id: thread_a(),
        observation: Observation::Tool(
            ToolObservation::new(
                ObservationId::parse(format!("{run}:{tool}")).expect("id"),
                ObservationSequence::new(delivery_sequence).expect("sequence"),
                ObservationId::parse(tool).expect("tool id"),
                "read".to_owned(),
                ToolAction::Completed,
                Some(format!("{tool} detail")),
            )
            .expect("tool"),
        ),
        attribution: Some(EngineObservationAttribution {
            run_id: RunId::parse(run).expect("run"),
            turn_id: turn_id(turn),
            committed_at: UnixMillis::from_millis(3),
            delivery_sequence,
        }),
    }
}

/// What the test reads back after each step.
#[derive(Debug, PartialEq)]
struct Loaded {
    failed: bool,
    turns: Vec<String>,
    facts: Vec<String>,
    cursor: Option<ConversationCursor>,
}

fn loaded(view: &gpui::Entity<NativeApplication>, cx: &mut gpui::VisualTestContext) -> Loaded {
    cx.update(|_, app| {
        let application = view.read(app);
        let host = application.conversation_host.clone().expect("host");
        let host = host.read(app);
        let mut facts: Vec<String> = host
            .derived_fact_ids()
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect();
        facts.sort();
        Loaded {
            failed: matches!(application.state, NativeViewState::Failure(_)),
            turns: host
                .canonical_snapshot()
                .expect("snapshot")
                .turns()
                .iter()
                .map(|turn| turn.turn_id.as_str().to_owned())
                .collect(),
            facts,
            cursor: host.controller_view().delivery.cursor,
        }
    })
}

fn history_reads(
    commands: &Rc<RefCell<Vec<NativeTransportCommand>>>,
) -> Vec<ConversationHistoryPart> {
    commands
        .borrow()
        .iter()
        .filter_map(|command| match command {
            NativeTransportCommand::ReadConversationHistory { thread_id, part } => {
                assert_eq!(thread_id, &thread_a());
                Some(part.clone())
            }
            _ => None,
        })
        .collect()
}

/// The reads of one turn's work rows. The mounted surface is short, so it
/// also asks for older turns on its own; those reads are not counted here.
fn turn_work_reads(
    commands: &Rc<RefCell<Vec<NativeTransportCommand>>>,
) -> Vec<ConversationHistoryPart> {
    history_reads(commands)
        .into_iter()
        .filter(|part| matches!(part, ConversationHistoryPart::TurnWork { .. }))
        .collect()
}

/// Opens `thread-a` on its two newest turns: `turn-c` settled with five work
/// rows held back on the Forge, `turn-d` still running.
fn open_windowed_thread(
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<NativeApplication>,
    &mut gpui::VisualTestContext,
    Rc<RefCell<Vec<NativeTransportCommand>>>,
) {
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_project_option(application, "Varde");
            install_ready_message_surface(application, cx, thread_a(), "", sink);
            application.handle_service_event(
                NativeTransportEvent::Snapshot(snapshot(
                    vec![
                        turn("turn-c", 20, ConversationLifecycle::Completed),
                        turn("turn-d", 30, ConversationLifecycle::Active),
                    ],
                    vec![
                        user("user-c", "turn-c", 21),
                        reply("reply-c", "turn-c", 22, "run-c"),
                        user("user-d", "turn-d", 31),
                    ],
                )),
                cx,
            );
            application.handle_service_event(
                NativeTransportEvent::HostState(
                    crate::native_transport_service::HostStateEvent::HeldBackWork(HeldBackWork {
                        thread_id: thread_a(),
                        turns: vec![held_back("turn-c", "run-c", 5, 40)],
                    }),
                ),
                cx,
            );
            application.handle_service_event(history_current_event(&thread_a()), cx);
        });
    });
    cx.run_until_parked();
    (view, cx, commands)
}

/// A settled turn's work stays on the Forge until its section opens: one
/// stand-in row keeps the section there, opening it reads the rows page by
/// page, and the stand-in goes once they are all here.
#[gpui::test]
fn a_settled_turns_work_is_read_when_its_section_opens(cx: &mut TestAppContext) {
    let (view, cx, commands) = open_windowed_thread(cx);
    let stand_in = held_back_fact_id(&turn_id("turn-c"))
        .expect("stand-in id")
        .as_str()
        .to_owned();
    let state = loaded(&view, cx);
    assert!(!state.failed);
    assert_eq!(state.turns, ["turn-c", "turn-d"]);
    assert_eq!(state.facts, [stand_in.clone()]);
    // The section is there, closed, and nothing was read for it.
    cx.update(|_, app| {
        let application = view.read(app);
        let host = application.conversation_host.clone().expect("host");
        assert_eq!(
            host.read(app).session_disclosure(&turn_id("turn-c")),
            Some(SceneDisclosure::Closed)
        );
    });
    assert!(turn_work_reads(&commands).is_empty());

    // Opening the section asks for the turn's work rows, once.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let host = application.conversation_host.clone().expect("host");
            host.update(cx, |host, host_cx| {
                host.dispatch(
                    ConversationStateEvent::Disclosure {
                        scene_id: session_anchor_id(&turn_id("turn-c")).expect("anchor"),
                        event: DisclosureEvent::UserOpen,
                    },
                    host_cx,
                )
            })
            .expect("section opens");
            application.pump_host_boundary(&host, cx);
            application.request_open_turn_work(cx);
            application.request_open_turn_work(cx);
        });
    });
    assert_eq!(
        turn_work_reads(&commands),
        [ConversationHistoryPart::TurnWork {
            turn_id: turn_id("turn-c"),
            after_sequence: 0,
        }]
    );

    // The first page says more remain: the next is asked for, and the
    // stand-in stays beside the rows that arrived.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                NativeTransportEvent::ConversationHistory {
                    part: ConversationHistoryPart::TurnWork {
                        turn_id: turn_id("turn-c"),
                        after_sequence: 0,
                    },
                    page: Box::new(ConversationHistoryPage {
                        thread_id: thread_a(),
                        snapshot: None,
                        observations: vec![
                            tool_row("turn-c", "run-c", "tool-1", 40),
                            tool_row("turn-c", "run-c", "tool-2", 41),
                        ],
                        held_back: Vec::new(),
                        next_after_sequence: Some(41),
                    }),
                },
                cx,
            );
        });
    });
    assert_eq!(
        turn_work_reads(&commands).last(),
        Some(&ConversationHistoryPart::TurnWork {
            turn_id: turn_id("turn-c"),
            after_sequence: 41,
        })
    );
    let state = loaded(&view, cx);
    assert!(!state.failed);
    assert_eq!(
        state.facts,
        [
            stand_in.clone(),
            "tool-run-c-tool-1".to_owned(),
            "tool-run-c-tool-2".to_owned(),
        ]
    );

    // The last page completes the turn: only its rows remain, and nothing
    // more is asked for however often the section is looked at.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                NativeTransportEvent::ConversationHistory {
                    part: ConversationHistoryPart::TurnWork {
                        turn_id: turn_id("turn-c"),
                        after_sequence: 41,
                    },
                    page: Box::new(ConversationHistoryPage {
                        thread_id: thread_a(),
                        snapshot: None,
                        observations: vec![tool_row("turn-c", "run-c", "tool-3", 42)],
                        held_back: Vec::new(),
                        next_after_sequence: None,
                    }),
                },
                cx,
            );
            application.request_open_turn_work(cx);
        });
    });
    let state = loaded(&view, cx);
    assert!(!state.failed);
    assert_eq!(
        state.facts,
        [
            "tool-run-c-tool-1".to_owned(),
            "tool-run-c-tool-2".to_owned(),
            "tool-run-c-tool-3".to_owned(),
        ]
    );
    assert_eq!(turn_work_reads(&commands).len(), 2);
    cx.update(|_, app| {
        let application = view.read(app);
        let host = application.conversation_host.clone().expect("host");
        assert!(host.read(app).controller_scene().is_ok());
    });
}

/// Older turns arrive as the reader nears the start of the loaded ones: the
/// page joins in front of the window, the cursor stays where delivery left
/// it, and nothing is asked for once the thread's first turn is loaded.
#[gpui::test]
fn older_turns_are_read_as_the_reader_nears_the_start(cx: &mut TestAppContext) {
    let (view, cx, commands) = open_windowed_thread(cx);
    let before = loaded(&view, cx);

    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.note_reader_near_start(cx);
            // One page in flight is not asked for twice.
            application.note_reader_near_start(cx);
        });
    });
    assert_eq!(
        history_reads(&commands),
        [ConversationHistoryPart::EarlierTurns {
            before_turn_ordinal: TurnOrdinal::new(20),
            minimum_turn_ordinal: None,
            maximum_turn_count: artisan_domain::QueryTurnCount::new(u64::from(
                crate::native_transport_service::HISTORY_PAGE_TURNS
            ))
            .expect("page size"),
        }]
    );

    let page = |application: &mut NativeApplication, cx: &mut Context<NativeApplication>| {
        application.handle_service_event(
            NativeTransportEvent::ConversationHistory {
                part: history_reads(&commands)[0].clone(),
                page: Box::new(ConversationHistoryPage {
                    thread_id: thread_a(),
                    snapshot: Some(snapshot(
                        vec![
                            turn("turn-a", 0, ConversationLifecycle::Completed),
                            turn("turn-b", 10, ConversationLifecycle::Completed),
                        ],
                        vec![
                            user("user-a", "turn-a", 1),
                            reply("reply-a", "turn-a", 2, "run-a"),
                            user("user-b", "turn-b", 11),
                            reply("reply-b", "turn-b", 12, "run-b"),
                        ],
                    )),
                    observations: Vec::new(),
                    held_back: vec![held_back("turn-b", "run-b", 2, 12)],
                    next_after_sequence: None,
                }),
            },
            cx,
        );
    };
    cx.update(|_, app| view.update(app, |application, cx| page(application, cx)));
    let state = loaded(&view, cx);
    assert!(!state.failed);
    assert_eq!(state.turns, ["turn-a", "turn-b", "turn-c", "turn-d"]);
    assert_eq!(
        state.cursor, before.cursor,
        "a page is history, not delivery"
    );
    assert_eq!(
        state.facts,
        [
            held_back_fact_id(&turn_id("turn-b"))
                .expect("stand-in id")
                .as_str()
                .to_owned(),
            held_back_fact_id(&turn_id("turn-c"))
                .expect("stand-in id")
                .as_str()
                .to_owned(),
        ]
    );
    cx.update(|_, app| {
        let application = view.read(app);
        let host = application.conversation_host.clone().expect("host");
        let scene = host.read(app).controller_scene().expect("scene builds");
        assert_eq!(scene.turn_scenes().len(), 4);
    });

    // The same page arriving again changes nothing, and the thread's first
    // turn is loaded, so nothing further is asked for.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            page(application, cx);
            application.note_reader_near_start(cx);
        });
    });
    assert_eq!(loaded(&view, cx), state);
    assert_eq!(history_reads(&commands).len(), 1);

    // Delivery continues from the same cursor.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                NativeTransportEvent::PatchBatch(echo_batch(
                    &thread_a(),
                    7,
                    "user-e",
                    None,
                    "turn-e",
                    40,
                    41,
                    "next question",
                )),
                cx,
            );
        });
    });
    let state = loaded(&view, cx);
    assert!(!state.failed);
    assert_eq!(state.turns.last().map(String::as_str), Some("turn-e"));
    assert!(state.cursor > before.cursor);
}

/// The turn navigator lists the questions of turns that are not loaded, and
/// jumping to one reads every turn from its own up to the loaded ones.
#[gpui::test]
fn the_navigator_lists_unloaded_turns_and_a_jump_reads_them(cx: &mut TestAppContext) {
    let (view, cx, commands) = open_windowed_thread(cx);
    let labels = |cx: &mut gpui::VisualTestContext| {
        cx.update(|_, app| {
            let application = view.read(app);
            let host = application.conversation_host.clone().expect("host");
            let surface = host.read(app).surface().clone();
            surface.read(app).navigator_marker_labels()
        })
    };
    let marker = |item: &str, ordinal: u64, label: &str| artisan_domain::EarlierTurnMarker {
        item_id: ItemId::parse(item).expect("item"),
        turn_ordinal: TurnOrdinal::new(ordinal),
        label: label.to_owned(),
    };
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                NativeTransportEvent::HostState(
                    crate::native_transport_service::HostStateEvent::EarlierTurnMarkers(
                        artisan_domain::EarlierTurnMarkers {
                            thread_id: thread_a(),
                            markers: vec![
                                marker("user-a", 0, "question of turn-a"),
                                marker("user-b", 10, "question of turn-b"),
                            ],
                        },
                    ),
                ),
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert_eq!(
        labels(cx),
        [
            ("question of turn-a".to_owned(), false),
            ("question of turn-b".to_owned(), false),
            ("question of turn-c".to_owned(), true),
            ("question of turn-d".to_owned(), true),
        ]
    );

    // A page above is already in flight; the jump waits for it and then
    // reads down to the turn it wants.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.note_reader_near_start(cx)
        });
    });
    let first_read = history_reads(&commands);
    assert_eq!(first_read.len(), 1);
    let target = crate::conversation_surface::ConversationSurfaceTarget::Item(
        ItemId::parse("user-a").expect("item"),
    );
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            assert!(application.jump_to_earlier_turn(&target, cx));
            // Only turn-b arrives with the page in flight.
            application.handle_service_event(
                NativeTransportEvent::ConversationHistory {
                    part: first_read[0].clone(),
                    page: Box::new(ConversationHistoryPage {
                        thread_id: thread_a(),
                        snapshot: Some(snapshot(
                            vec![turn("turn-b", 10, ConversationLifecycle::Completed)],
                            vec![
                                user("user-b", "turn-b", 11),
                                reply("reply-b", "turn-b", 12, "run-b"),
                            ],
                        )),
                        observations: Vec::new(),
                        held_back: Vec::new(),
                        next_after_sequence: None,
                    }),
                },
                cx,
            );
        });
    });
    let reads = history_reads(&commands);
    assert_eq!(
        reads.last(),
        Some(&ConversationHistoryPart::EarlierTurns {
            before_turn_ordinal: TurnOrdinal::new(10),
            minimum_turn_ordinal: Some(TurnOrdinal::new(0)),
            maximum_turn_count: artisan_domain::QueryTurnCount::new(u64::from(
                artisan_domain::CONVERSATION_QUERY_MAX_TURNS
            ))
            .expect("turn count"),
        })
    );
    // The loaded turn left the unloaded list.
    assert_eq!(
        labels(cx),
        [
            ("question of turn-a".to_owned(), false),
            ("question of turn-b".to_owned(), true),
            ("question of turn-c".to_owned(), true),
            ("question of turn-d".to_owned(), true),
        ]
    );

    // The wanted turn arrives: everything is loaded and the jump is done.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                NativeTransportEvent::ConversationHistory {
                    part: reads.last().expect("jump read").clone(),
                    page: Box::new(ConversationHistoryPage {
                        thread_id: thread_a(),
                        snapshot: Some(snapshot(
                            vec![turn("turn-a", 0, ConversationLifecycle::Completed)],
                            vec![
                                user("user-a", "turn-a", 1),
                                reply("reply-a", "turn-a", 2, "run-a"),
                            ],
                        )),
                        observations: Vec::new(),
                        held_back: Vec::new(),
                        next_after_sequence: None,
                    }),
                },
                cx,
            );
        });
    });
    cx.run_until_parked();
    let state = loaded(&view, cx);
    assert!(!state.failed);
    assert_eq!(state.turns, ["turn-a", "turn-b", "turn-c", "turn-d"]);
    assert!(labels(cx).iter().all(|(_, turn_loaded)| *turn_loaded));
    assert_eq!(history_reads(&commands).len(), 2);
    // A loaded message is an ordinary scroll target again.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            assert!(!application.jump_to_earlier_turn(&target, cx));
        });
    });
}
