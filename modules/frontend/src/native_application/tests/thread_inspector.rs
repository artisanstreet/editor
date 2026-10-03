//! The thread inspector's live feed: the open thread's project, its
//! project's branch, and its latest plan reach the mounted screen as the
//! application learns them, and a thread switch never carries the previous
//! thread's facts over.

use super::*;
use crate::thread_agents::{ThreadAgentEntry, ThreadAgentState};
use crate::thread_environment_presentation::present_thread_environment;
use crate::thread_panel_policy::ChecklistEntryState;
use crate::thread_screen::{
    THREAD_SCREEN_AGENTS_SELECTOR, THREAD_SCREEN_PROJECT_ROW_SELECTOR, ThreadChecklistEntry,
};
use artisan_domain::{
    EngineObservationAttribution, EngineObservationEvent, Observation, ObservationSequence,
    PlanEntry, PlanEntryStatus, PlanObservation, ToolAction, ToolObservation,
};

/// The inspector facts the mounted thread screen retains.
#[derive(Debug, PartialEq)]
struct InspectorFacts {
    project: Option<String>,
    branch: Option<String>,
    checklist: Vec<ThreadChecklistEntry>,
}

fn inspector_facts(
    view: &gpui::Entity<NativeApplication>,
    cx: &mut gpui::VisualTestContext,
) -> InspectorFacts {
    cx.update(|_, app| {
        let screen = view
            .read(app)
            .thread_screen
            .clone()
            .expect("thread screen mounted");
        let screen = screen.read(app);
        InspectorFacts {
            project: screen.project_label().map(str::to_owned),
            branch: present_thread_environment(screen.environment()).current_branch_label,
            checklist: screen.checklist().to_vec(),
        }
    })
}

fn checklist_entry(id: &str, state: ChecklistEntryState, text: &str) -> ThreadChecklistEntry {
    ThreadChecklistEntry {
        id: id.to_owned(),
        state,
        text: text.to_owned(),
    }
}

/// One attributed plan update for `thread_id` at `delivery_sequence`.
fn plan_event(
    thread_id: &ThreadId,
    plan_id: &str,
    delivery_sequence: u64,
    entries: &[(&str, PlanEntryStatus, &str)],
) -> NativeTransportEvent {
    let entries = entries
        .iter()
        .map(|(id, status, text)| {
            PlanEntry::new(
                ObservationId::parse(*id).expect("plan entry id"),
                *status,
                (*text).to_owned(),
            )
            .expect("plan entry")
        })
        .collect();
    let plan = PlanObservation::new(
        ObservationId::parse(plan_id).expect("plan id"),
        ObservationSequence::new(delivery_sequence).expect("sequence"),
        entries,
        None,
    )
    .expect("plan");
    NativeTransportEvent::EngineObservation(artisan_protocol::ServerEvent {
        cursor: artisan_protocol::EventCursor::new(delivery_sequence).expect("cursor"),
        event: artisan_domain::Event::EngineObservation(EngineObservationEvent {
            thread_id: thread_id.clone(),
            observation: Observation::Plan(plan),
            attribution: Some(EngineObservationAttribution {
                run_id: RunId::parse("run-plan").expect("run"),
                turn_id: TurnId::parse("turn-plan").expect("turn"),
                committed_at: UnixMillis::from_millis(10),
                delivery_sequence,
            }),
        }),
    })
}

/// Opens `thread_id` in the selected project with its transcript and
/// observation history delivered, so the screen presents its inspector.
fn open_thread(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    thread_id: &ThreadId,
) {
    let (sink, _commands) = command_sink([]);
    install_ready_message_surface(application, cx, thread_id.clone(), "", sink);
    application.handle_service_event(
        NativeTransportEvent::Snapshot(snapshot_for(thread_id, 1)),
        cx,
    );
    application.handle_service_event(history_current_event(thread_id), cx);
}

/// Mounts the thread `message-project`/`thread-a` in a window wide enough to
/// seat the inspector, with the project listed as `Varde` and its repository
/// read in flight.
fn mount_open_thread(
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<NativeApplication>,
    &mut gpui::VisualTestContext,
) {
    let (view, cx) = cx.add_window_view(test_application);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_project_option(application, "Varde");
            open_thread(
                application,
                cx,
                &ThreadId::parse("thread-a").expect("thread"),
            );
            application.request_project_repository(cx);
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1800.0), gpui::px(900.0)));
    cx.run_until_parked();
    (view, cx)
}

fn message_project() -> ProjectId {
    ProjectId::parse("message-project").expect("project")
}

/// The Project row names the open thread's project the way the titlebar
/// does, and Branch follows the project's repository reply the moment it
/// arrives, after the screen has mounted.
#[gpui::test]
fn inspector_names_the_open_threads_project_and_branch(cx: &mut TestAppContext) {
    let (view, cx) = mount_open_thread(cx);
    assert_eq!(
        inspector_facts(&view, cx),
        InspectorFacts {
            project: Some(String::from("Varde")),
            branch: None,
            checklist: Vec::new(),
        },
        "before the repository reply the project folder names the row"
    );
    assert!(
        cx.debug_bounds(THREAD_SCREEN_PROJECT_ROW_SELECTOR)
            .is_some(),
        "the Project row paints"
    );

    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let repository =
                artisan_protocol::ProjectRepository::Repository(protocol_repository_snapshot());
            application.handle_project_repository(&message_project(), Some(&repository), cx);
        });
    });
    cx.run_until_parked();
    let facts = inspector_facts(&view, cx);
    assert_eq!(facts.project.as_deref(), Some("artisanstreet/varde"));
    assert_eq!(facts.branch.as_deref(), Some("main"));

    // A root Git does not track keeps the folder name and shows no branch.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_project_repository(
                &message_project(),
                Some(&artisan_protocol::ProjectRepository::NotRepository),
                cx,
            );
        });
    });
    cx.run_until_parked();
    let facts = inspector_facts(&view, cx);
    assert_eq!(facts.project.as_deref(), Some("Varde"));
    assert_eq!(facts.branch, None);
}

/// The Checklist is the thread's latest plan: each live update replaces it
/// with its states mapped through the checklist policy, and a late replay of
/// an older plan cannot displace the newer one.
#[gpui::test]
fn inspector_checklist_follows_plan_updates(cx: &mut TestAppContext) {
    let (view, cx) = mount_open_thread(cx);
    let thread_a = ThreadId::parse("thread-a").expect("thread");
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                plan_event(
                    &thread_a,
                    "plan-1",
                    1,
                    &[
                        ("step-1", PlanEntryStatus::InProgress, "Read the panel"),
                        ("step-2", PlanEntryStatus::Pending, "Wire the feed"),
                    ],
                ),
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert_eq!(
        inspector_facts(&view, cx).checklist,
        vec![
            checklist_entry("step-1", ChecklistEntryState::Active, "Read the panel"),
            checklist_entry("step-2", ChecklistEntryState::Pending, "Wire the feed"),
        ]
    );

    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                plan_event(
                    &thread_a,
                    "plan-3",
                    3,
                    &[
                        ("step-1", PlanEntryStatus::Completed, "Read the panel"),
                        ("step-2", PlanEntryStatus::InProgress, "Wire the feed"),
                    ],
                ),
                cx,
            );
            application.handle_service_event(
                plan_event(
                    &thread_a,
                    "plan-2",
                    2,
                    &[("step-1", PlanEntryStatus::Pending, "Stale plan")],
                ),
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert_eq!(
        inspector_facts(&view, cx).checklist,
        vec![
            checklist_entry("step-1", ChecklistEntryState::Completed, "Read the panel"),
            checklist_entry("step-2", ChecklistEntryState::Active, "Wire the feed"),
        ]
    );
}

/// Switching threads never shows the previous thread's facts: a sibling
/// thread keeps its (shared) project and branch but not the other thread's
/// checklist, and a thread of another project shows that project with no
/// branch until its own repository reply arrives.
#[gpui::test]
fn inspector_facts_reset_on_thread_switch(cx: &mut TestAppContext) {
    let (view, cx) = mount_open_thread(cx);
    let thread_a = ThreadId::parse("thread-a").expect("thread");
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let repository =
                artisan_protocol::ProjectRepository::Repository(protocol_repository_snapshot());
            application.handle_project_repository(&message_project(), Some(&repository), cx);
            application.handle_service_event(
                plan_event(
                    &thread_a,
                    "plan-1",
                    1,
                    &[("step-1", PlanEntryStatus::InProgress, "Thread A's plan")],
                ),
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert_eq!(inspector_facts(&view, cx).checklist.len(), 1);

    // A sibling thread of the same project: the retained observations still
    // belong to thread A, so the fence must keep its plan off thread B.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            open_thread(
                application,
                cx,
                &ThreadId::parse("thread-b").expect("thread"),
            );
        });
    });
    cx.run_until_parked();
    assert_eq!(
        inspector_facts(&view, cx),
        InspectorFacts {
            project: Some(String::from("artisanstreet/varde")),
            branch: Some(String::from("main")),
            checklist: Vec::new(),
        }
    );

    // A thread of another project: its own name, no branch yet, no plan.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let other = ProjectId::parse("other-project").expect("project");
            application.project_options.push(ProjectOption {
                id: other.clone(),
                name: SharedString::from("Other"),
            });
            application.selected_project = Some(other);
            application.request_project_repository(cx);
            open_thread(
                application,
                cx,
                &ThreadId::parse("thread-c").expect("thread"),
            );
        });
    });
    cx.run_until_parked();
    assert_eq!(
        inspector_facts(&view, cx),
        InspectorFacts {
            project: Some(String::from("Other")),
            branch: None,
            checklist: Vec::new(),
        }
    );
}

/// The Machine row names the connected host exactly as the sidebar's profile
/// footer does (the host's registered name), not the computer's hostname.
#[gpui::test]
fn inspector_names_the_machine_like_the_profile_footer(cx: &mut TestAppContext) {
    let (view, cx) = mount_open_thread(cx);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.profile_hostname = Some(String::from("DESKTOP-96USC6J"));
            application.machine_label = String::from("Ubuntu");
            cx.notify();
        });
    });
    cx.run_until_parked();
    let machine = cx.update(|_, app| {
        let screen = view
            .read(app)
            .thread_screen
            .clone()
            .expect("thread screen mounted");
        present_thread_environment(screen.read(app).environment()).machine_label
    });
    assert_eq!(machine, "Ubuntu");
}

/// One attributed tool report for `thread_id` from `run` at
/// `delivery_sequence`.
fn tool_event(
    thread_id: &ThreadId,
    run: &str,
    delivery_sequence: u64,
    tool_id: &str,
    kind: &str,
    action: ToolAction,
    detail: Option<&str>,
) -> NativeTransportEvent {
    let tool = ToolObservation::new(
        ObservationId::parse(format!("{run}:{tool_id}:{delivery_sequence}")).expect("id"),
        ObservationSequence::new(delivery_sequence).expect("sequence"),
        ObservationId::parse(tool_id).expect("tool id"),
        kind.to_owned(),
        action,
        detail.map(str::to_owned),
    )
    .expect("tool");
    NativeTransportEvent::EngineObservation(artisan_protocol::ServerEvent {
        cursor: artisan_protocol::EventCursor::new(delivery_sequence).expect("cursor"),
        event: artisan_domain::Event::EngineObservation(EngineObservationEvent {
            thread_id: thread_id.clone(),
            observation: Observation::Tool(tool),
            attribution: Some(EngineObservationAttribution {
                run_id: RunId::parse(run).expect("run"),
                turn_id: TurnId::parse("turn-agents").expect("turn"),
                committed_at: UnixMillis::from_millis(10),
                delivery_sequence,
            }),
        }),
    })
}

fn inspector_agents(
    view: &gpui::Entity<NativeApplication>,
    cx: &mut gpui::VisualTestContext,
) -> Vec<ThreadAgentEntry> {
    cx.update(|_, app| {
        let screen = view
            .read(app)
            .thread_screen
            .clone()
            .expect("thread screen mounted");
        screen.read(app).agents().to_vec()
    })
}

fn agent(id: &str, name: &str, state: ThreadAgentState) -> ThreadAgentEntry {
    ThreadAgentEntry {
        id: id.to_owned(),
        name: name.to_owned(),
        state,
    }
}

/// The Agents section lists the subagents the thread's current run started,
/// named by their task and settled as their tool reports settle. Other tools
/// never appear, a settled run lists none, and a new run starts over.
#[gpui::test]
fn inspector_agents_follow_the_current_runs_subagents(cx: &mut TestAppContext) {
    let (view, cx) = mount_open_thread(cx);
    let thread_a = ThreadId::parse("thread-a").expect("thread");
    assert!(inspector_agents(&view, cx).is_empty());
    assert!(cx.debug_bounds(THREAD_SCREEN_AGENTS_SELECTOR).is_none());

    let send = |cx: &mut gpui::VisualTestContext, events: Vec<NativeTransportEvent>| {
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                for event in events {
                    application.handle_service_event(event, cx);
                }
            });
        });
        cx.run_until_parked();
    };
    let live = |cx: &mut gpui::VisualTestContext, run: Option<&str>| {
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                match run {
                    Some(run) => application.run_controls.set_live_for_test(
                        thread_a.clone(),
                        artisan_domain::RunId::parse(run).expect("run"),
                    ),
                    None => application.run_controls.clear_transient_observation(),
                }
                cx.notify();
            });
        });
        cx.run_until_parked();
    };
    live(cx, Some("run-1"));
    send(
        cx,
        vec![
            tool_event(
                &thread_a,
                "run-1",
                1,
                "tool-read",
                "read",
                ToolAction::Started,
                Some("src/lib.rs"),
            ),
            tool_event(
                &thread_a,
                "run-1",
                2,
                "tool-agent-a",
                "subagent",
                ToolAction::Started,
                Some("Build flat right sidebar panel"),
            ),
            tool_event(
                &thread_a,
                "run-1",
                3,
                "tool-agent-b",
                "subagent",
                ToolAction::Started,
                None,
            ),
        ],
    );
    assert_eq!(
        inspector_agents(&view, cx),
        vec![
            agent(
                "tool-agent-a",
                "Build flat right sidebar panel",
                ThreadAgentState::Working
            ),
            agent("tool-agent-b", "Subagent", ThreadAgentState::Working),
        ]
    );
    assert!(
        cx.debug_bounds(THREAD_SCREEN_AGENTS_SELECTOR).is_some(),
        "the Agents section paints"
    );

    send(
        cx,
        vec![
            tool_event(
                &thread_a,
                "run-1",
                4,
                "tool-agent-a",
                "subagent",
                ToolAction::Completed,
                Some("Build flat right sidebar panel"),
            ),
            tool_event(
                &thread_a,
                "run-1",
                5,
                "tool-agent-b",
                "subagent",
                ToolAction::Failed,
                None,
            ),
        ],
    );
    assert_eq!(
        inspector_agents(&view, cx)
            .iter()
            .map(|agent| agent.state)
            .collect::<Vec<_>>(),
        vec![ThreadAgentState::Completed, ThreadAgentState::Failed]
    );

    // Once the run settles its agents are done: the section clears.
    live(cx, None);
    assert!(inspector_agents(&view, cx).is_empty());
    assert!(cx.debug_bounds(THREAD_SCREEN_AGENTS_SELECTOR).is_none());

    // A new run without subagents lists none.
    live(cx, Some("run-2"));
    send(
        cx,
        vec![tool_event(
            &thread_a,
            "run-2",
            6,
            "tool-grep",
            "grep",
            ToolAction::Started,
            Some("needle"),
        )],
    );
    assert!(inspector_agents(&view, cx).is_empty());
    assert!(cx.debug_bounds(THREAD_SCREEN_AGENTS_SELECTOR).is_none());
}

/// A thread whose activity fills the scene's fact window keeps taking its
/// own transcript: each new message makes the oldest activity slide out
/// instead of being refused, so the thread never stops updating.
#[gpui::test]
fn a_thread_with_a_full_activity_window_keeps_receiving_messages(cx: &mut TestAppContext) {
    let (view, cx) = mount_open_thread(cx);
    let thread_a = ThreadId::parse("thread-a").expect("thread");
    let state = |cx: &mut gpui::VisualTestContext| {
        cx.update(|_, app| {
            let application = view.read(app);
            let host = application.conversation_host.clone().expect("host");
            let host = host.read(app);
            (
                matches!(application.state, NativeViewState::Failure(_)),
                host.derived_fact_ids().len(),
                host.controller_view().delivery.cursor,
                host.controller_scene().is_ok(),
            )
        })
    };
    let message = |from: u64, item: &str, ordinal: u64, body: &str| {
        NativeTransportEvent::PatchBatch(echo_batch(
            &thread_a,
            from,
            item,
            None,
            "turn-agents",
            0,
            ordinal,
            body,
        ))
    };
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(message(1, "item-user", 1, "hello"), cx);
            // More tool reports than the scene has room for.
            for index in 0..600_u64 {
                application.handle_service_event(
                    tool_event(
                        &thread_a,
                        "run-a",
                        index + 1,
                        &format!("tool-{index}"),
                        "bash",
                        ToolAction::Completed,
                        Some("ls"),
                    ),
                    cx,
                );
            }
        });
    });
    let window = crate::conversation_scene::SCENE_MAX_ITEMS - 1;
    assert_eq!(
        state(cx),
        (false, window, Some(ConversationCursor::new(3)), true),
        "the newest activity fills the room the one message leaves"
    );

    // Two more messages arrive with no observation between them.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(message(3, "item-user-2", 2, "second"), cx);
            application.handle_service_event(message(5, "item-user-3", 3, "third"), cx);
        });
    });
    assert_eq!(
        state(cx),
        (false, window - 2, Some(ConversationCursor::new(7)), true),
        "each message is accepted and the oldest activity makes way"
    );

    // Live work keeps landing in the narrower window.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                tool_event(
                    &thread_a,
                    "run-a",
                    601,
                    "tool-600",
                    "bash",
                    ToolAction::Completed,
                    Some("ls"),
                ),
                cx,
            );
        });
    });
    assert_eq!(
        state(cx),
        (false, window - 2, Some(ConversationCursor::new(7)), true)
    );
}
