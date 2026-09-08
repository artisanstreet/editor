//! Pairing of engine subscription events into frontend presentation state.
//!
//! These tests run under the existing cargo frontend `[[test]]` harness, the
//! same harness the current frontend unit tests use: plain `#[test]`
//! functions against the production `artisan_frontend` and `artisan_domain`
//! APIs, with no Bazel-only or display dependencies.

use artisan_domain::{
    AgentMessageCompletedObservation, AgentMessageDeltaObservation, ApprovalObservation,
    ApprovalRequest, EngineObservationEvent, Event, FileAction, FileObservation, MessagePhase,
    Observation, ObservationId, ObservationSequence, ProcessDiagnosticObservation,
    ProtocolDiagnosticObservation, QuestionInput, QuestionObservation, QuestionOption,
    ReasoningSummaryCompletedObservation, ReasoningSummaryDeltaObservation, RetryAttemptState,
    RetryObservation, RunState, RunStateObservation, RunTerminalObservation, RunTerminalState,
    SearchObservation, SearchScope, SearchState, SubagentInput, SubagentObservation, SubagentState,
    SubagentTranscriptObservation, TerminalActivityInput, TerminalActivityObservation,
    TerminalActivityState, ThreadId, ToolAction, ToolObservation, TranscriptContent,
    TranscriptTool, TurnState, TurnStateObservation, UnixMillis, UsageBasis, UsageInput,
    UsageObservation,
};
use artisan_frontend::engine_observation_state::{
    ApplyOutcome, EngineObservationState, TimelineRow,
};
use artisan_frontend::native_transport_service::{UniDelivery, validate_uni_envelope};
use artisan_protocol::{
    EventCursor, FrameId, ProtocolVersion, ServerEvent, WireEnvelope, WireEnvelopeBody,
};

fn observation_id(value: &str) -> ObservationId {
    ObservationId::parse(value).expect("fixture observation id is valid")
}

fn sequence(value: u64) -> ObservationSequence {
    ObservationSequence::new(value).expect("fixture sequence is valid")
}

fn thread_id() -> ThreadId {
    ThreadId::parse("thread-obs").expect("fixture thread id is valid")
}

fn other_thread_id() -> ThreadId {
    ThreadId::parse("thread-other").expect("fixture thread id is valid")
}

fn state() -> EngineObservationState {
    EngineObservationState::new(thread_id())
}

fn event(observation: Observation) -> EngineObservationEvent {
    EngineObservationEvent {
        thread_id: thread_id(),
        observation,
    }
}

fn message_delta(id: &str, item: &str, phase: MessagePhase, delta: &str) -> Observation {
    Observation::AgentMessageDelta(
        AgentMessageDeltaObservation::new(
            observation_id(id),
            sequence(1),
            observation_id(item),
            phase,
            String::from(delta),
            observation_id("turn-1"),
        )
        .expect("fixture message delta is valid"),
    )
}

fn message_completed(item: &str, phase: MessagePhase, message: &str) -> Observation {
    Observation::AgentMessageCompleted(
        AgentMessageCompletedObservation::new(
            observation_id("obs-message-completed"),
            sequence(2),
            observation_id(item),
            phase,
            String::from(message),
            observation_id("turn-1"),
        )
        .expect("fixture completed message is valid"),
    )
}

fn approval_requested() -> Observation {
    Observation::Approval(
        ApprovalObservation::requested(
            observation_id("obs-approval-requested"),
            sequence(9),
            observation_id("approval-1"),
            String::from("Run the test suite?"),
            ApprovalRequest::command(
                String::from("cargo test"),
                Some(String::from("C:/repos/demo")),
                Some(String::from("verify before landing")),
            )
            .expect("fixture approval request is valid"),
        )
        .expect("fixture requested approval is valid"),
    )
}

fn approval_resolved(approved: bool) -> Observation {
    Observation::Approval(
        ApprovalObservation::resolved(
            observation_id("obs-approval-resolved"),
            sequence(10),
            observation_id("approval-1"),
            String::from("Run the test suite?"),
            ApprovalRequest::command(
                String::from("cargo test"),
                Some(String::from("C:/repos/demo")),
                Some(String::from("verify before landing")),
            )
            .expect("fixture approval request is valid"),
            approved,
        )
        .expect("fixture resolved approval is valid"),
    )
}

fn question_requested() -> Observation {
    Observation::Question(
        QuestionObservation::requested(
            observation_id("obs-question-requested"),
            sequence(11),
            QuestionInput {
                question_id: observation_id("question-1"),
                text: String::from("Which runtime?"),
                header: Some(String::from("Runtime")),
                multi_select: false,
                options: Some(vec![
                    QuestionOption::new(String::from("tokio"), None)
                        .expect("fixture option is valid"),
                    QuestionOption::new(
                        String::from("async-std"),
                        Some(String::from("alternative runtime")),
                    )
                    .expect("fixture option is valid"),
                ]),
            },
        )
        .expect("fixture requested question is valid"),
    )
}

fn question_resolved() -> Observation {
    Observation::Question(
        QuestionObservation::resolved(
            observation_id("obs-question-resolved"),
            sequence(12),
            QuestionInput {
                question_id: observation_id("question-1"),
                text: String::from("Which runtime?"),
                header: Some(String::from("Runtime")),
                multi_select: false,
                options: Some(vec![
                    QuestionOption::new(String::from("tokio"), None)
                        .expect("fixture option is valid"),
                ]),
            },
            vec![String::from("tokio")],
        )
        .expect("fixture resolved question is valid"),
    )
}

fn usage_report(
    id: &str,
    basis: UsageBasis,
    input: u64,
    output: u64,
    context: u64,
    cost: f64,
) -> Observation {
    Observation::Usage(
        UsageObservation::new(
            observation_id(id),
            sequence(20),
            UsageInput {
                basis,
                input_tokens: Some(input),
                cached_input_tokens: None,
                output_tokens: Some(output),
                context_tokens: Some(context),
                context_window_tokens: Some(200_000),
                cost_usd: Some(cost),
                provider_route_id: None,
                turn_id: None,
            },
        )
        .expect("fixture usage is valid"),
    )
}

#[test]
fn message_deltas_accumulate_and_completion_settles() {
    let mut presentation = state();
    let first = presentation.apply(
        1,
        &event(message_delta(
            "obs-delta-1",
            "item-1",
            MessagePhase::Commentary,
            "hel",
        )),
    );
    assert!(matches!(
        first,
        ApplyOutcome::Applied {
            tag: "agent_message_delta",
            settled_in_place: false
        }
    ));
    let second = presentation.apply(
        2,
        &event(message_delta(
            "obs-delta-2",
            "item-1",
            MessagePhase::Commentary,
            "lo",
        )),
    );
    assert!(matches!(second, ApplyOutcome::Applied { .. }));
    let row = presentation.message("item-1").expect("delta row pairs");
    assert_eq!(row.text(), "hello");
    assert_eq!(row.phase(), MessagePhase::Commentary);
    assert!(!row.completed());

    let settled = presentation.apply(
        3,
        &event(message_completed(
            "item-1",
            MessagePhase::Final,
            "settled reply",
        )),
    );
    assert!(matches!(
        settled,
        ApplyOutcome::Applied {
            tag: "agent_message_completed",
            settled_in_place: true
        }
    ));
    let row = presentation.message("item-1").expect("completed row pairs");
    assert_eq!(row.text(), "settled reply");
    assert_eq!(row.phase(), MessagePhase::Final);
    assert!(row.completed());
    assert_eq!(presentation.messages_in_order().len(), 1);
}

#[test]
fn reasoning_deltas_accumulate_and_empty_completion_keeps_text() {
    let mut presentation = state();
    let delta = Observation::ReasoningSummaryDelta(
        ReasoningSummaryDeltaObservation::new(
            observation_id("obs-reasoning-delta"),
            sequence(3),
            observation_id("item-2"),
            3,
            String::from("summary "),
            None,
            observation_id("turn-1"),
        )
        .expect("fixture reasoning delta is valid"),
    );
    presentation.apply(1, &event(delta));
    let completed = Observation::ReasoningSummaryCompleted(
        ReasoningSummaryCompletedObservation::new(
            observation_id("obs-reasoning-completed"),
            sequence(4),
            observation_id("item-2"),
            None,
            observation_id("turn-1"),
        )
        .expect("fixture settled reasoning is valid"),
    );
    let outcome = presentation.apply(2, &event(completed));
    assert!(matches!(
        outcome,
        ApplyOutcome::Applied {
            tag: "reasoning_summary_completed",
            settled_in_place: true
        }
    ));
    let row = presentation
        .reasoning("item-2")
        .expect("reasoning row pairs");
    assert_eq!(row.text(), "summary ");
    assert!(row.settled());
}

#[test]
fn tool_file_search_and_terminal_rows_pair() {
    let mut presentation = state();
    let started = Observation::Tool(
        ToolObservation::new(
            observation_id("obs-tool-started"),
            sequence(5),
            observation_id("tool-1"),
            String::from("read"),
            ToolAction::Started,
            None,
        )
        .expect("fixture tool row is valid"),
    );
    presentation.apply(1, &event(started));
    let completed = Observation::Tool(
        ToolObservation::new(
            observation_id("obs-tool-completed"),
            sequence(6),
            observation_id("tool-1"),
            String::from("read"),
            ToolAction::Completed,
            Some(String::from("read 42 lines")),
        )
        .expect("fixture tool row is valid"),
    );
    presentation.apply(2, &event(completed));
    let row = presentation.tool("tool-1").expect("tool row pairs");
    assert_eq!(row.action(), ToolAction::Completed);
    assert_eq!(row.detail(), Some("read 42 lines"));

    let file = Observation::File(
        FileObservation::new(
            observation_id("obs-file"),
            sequence(7),
            String::from("src/main.rs"),
            FileAction::Modified,
            Some(10),
            Some(2),
        )
        .expect("fixture file row is valid"),
    );
    presentation.apply(3, &event(file));
    let search = Observation::Search(
        SearchObservation::new(
            observation_id("obs-search"),
            sequence(8),
            String::from("observation delivery"),
            Some(SearchScope::Workspace),
            Some(observation_id("search-1")),
            SearchState::Completed,
            Some(7),
        )
        .expect("fixture search row is valid"),
    );
    presentation.apply(4, &event(search));

    let terminal = Observation::TerminalActivity(
        TerminalActivityObservation::new(
            observation_id("obs-terminal"),
            sequence(9),
            TerminalActivityInput {
                activity_id: observation_id("activity-1"),
                channel: None,
                command: Some(String::from("cargo test")),
                shell: Some(String::from("pwsh")),
                output: Some(String::from("ok")),
                exit_code: Some(0),
                state: TerminalActivityState::Completed,
            },
        )
        .expect("fixture terminal row is valid"),
    );
    presentation.apply(5, &event(terminal));
    let row = presentation
        .terminal("activity-1")
        .expect("terminal row pairs");
    assert_eq!(row.command(), Some("cargo test"));
    assert_eq!(row.output(), "ok");
    assert_eq!(row.exit_code(), Some(0));

    let tags = presentation
        .timeline()
        .iter()
        .map(TimelineRow::tag)
        .collect::<Vec<&str>>();
    assert!(tags.contains(&"file"));
    assert!(tags.contains(&"search"));
}

#[test]
fn approval_requested_resolves_in_place_by_request_id() {
    let mut presentation = state();
    presentation.apply(9, &event(approval_requested()));
    let requested = presentation
        .approval("approval-1")
        .expect("requested row pairs");
    assert!(requested.is_requested());
    assert_eq!(requested.approved(), None);
    assert_eq!(presentation.approvals_in_order().len(), 1);

    let outcome = presentation.apply(10, &event(approval_resolved(true)));
    assert!(matches!(
        outcome,
        ApplyOutcome::Applied {
            tag: "approval",
            settled_in_place: true
        }
    ));
    assert_eq!(presentation.approvals_in_order().len(), 1);
    let resolved = presentation
        .approval("approval-1")
        .expect("resolved row pairs");
    assert!(!resolved.is_requested());
    assert_eq!(resolved.approved(), Some(true));
    assert_eq!(resolved.approval_id(), "approval-1");

    let rendered = presentation
        .approval_presentation("approval-1")
        .expect("resolved approval renders");
    assert_eq!(rendered.title, "Command approved");
    assert_eq!(rendered.approve_label, "Run command");
    assert_eq!(rendered.command.as_deref(), Some("cargo test"));
}

#[test]
fn approval_order_is_stable_across_resolutions() {
    let mut presentation = state();
    presentation.apply(9, &event(approval_requested()));
    let second = Observation::Approval(
        ApprovalObservation::requested(
            observation_id("obs-approval-second"),
            sequence(11),
            observation_id("approval-2"),
            String::from("Apply the edits?"),
            ApprovalRequest::file_change(Some(String::from("generated fixes")))
                .expect("fixture approval request is valid"),
        )
        .expect("fixture requested approval is valid"),
    );
    presentation.apply(11, &event(second));
    presentation.apply(12, &event(approval_resolved(false)));

    let order = presentation
        .approvals_in_order()
        .iter()
        .map(|row| row.approval_id().to_owned())
        .collect::<Vec<String>>();
    assert_eq!(
        order,
        vec![String::from("approval-1"), String::from("approval-2")]
    );
    assert_eq!(
        presentation.approval("approval-1").expect("row").approved(),
        Some(false)
    );
    let rendered = presentation
        .approval_presentation("approval-1")
        .expect("denied approval renders");
    assert_eq!(rendered.title, "Command denied");
}

#[test]
fn question_requested_resolves_in_place_by_request_id() {
    let mut presentation = state();
    presentation.apply(11, &event(question_requested()));
    let requested = presentation
        .question("question-1")
        .expect("requested row pairs");
    assert!(requested.is_requested());
    assert_eq!(requested.text(), "Which runtime?");
    assert_eq!(requested.header(), Some("Runtime"));
    assert!(!requested.multi_select());
    assert_eq!(requested.options().map(|options| options.len()), Some(2));

    let outcome = presentation.apply(12, &event(question_resolved()));
    assert!(matches!(
        outcome,
        ApplyOutcome::Applied {
            tag: "question",
            settled_in_place: true
        }
    ));
    assert_eq!(presentation.questions_in_order().len(), 1);
    let resolved = presentation
        .question("question-1")
        .expect("resolved row pairs");
    assert!(!resolved.is_requested());
    let answers = resolved
        .answers()
        .expect("resolved question carries answers");
    assert_eq!(answers, &vec![String::from("tokio")]);
}

#[test]
fn usage_reports_fold_by_basis_and_gauge_replaces() {
    let mut presentation = state();
    presentation.apply(
        1,
        &event(usage_report(
            "obs-usage-1",
            UsageBasis::Delta,
            1_000,
            150,
            1_350,
            0.5,
        )),
    );
    presentation.apply(
        2,
        &event(usage_report(
            "obs-usage-2",
            UsageBasis::Delta,
            500,
            100,
            1_400,
            0.25,
        )),
    );
    let totals = presentation.usage();
    assert_eq!(totals.reports(), 2);
    assert_eq!(totals.input_tokens(), 1_500);
    assert_eq!(totals.output_tokens(), 250);
    assert_eq!(totals.context_tokens(), Some(1_400));
    assert!((totals.cost_usd() - 0.75).abs() < 0.001);

    presentation.apply(
        3,
        &event(usage_report(
            "obs-usage-3",
            UsageBasis::Cumulative,
            2_000,
            300,
            1_500,
            1.0,
        )),
    );
    let totals = presentation.usage();
    assert_eq!(totals.basis(), UsageBasis::Cumulative);
    assert_eq!(totals.input_tokens(), 2_000);
    assert_eq!(totals.output_tokens(), 300);
    assert_eq!(totals.context_tokens(), Some(1_500));
    assert!((totals.cost_usd() - 1.0).abs() < 0.001);
}

#[test]
fn reconnect_replay_applies_in_cursor_order_with_dedup() {
    let mut presentation = state();
    let batch = vec![
        (3_u64, event(approval_resolved(true))),
        (1_u64, event(approval_requested())),
        (
            2_u64,
            event(Observation::Tool(
                ToolObservation::new(
                    observation_id("obs-tool-replay"),
                    sequence(5),
                    observation_id("tool-9"),
                    String::from("grep"),
                    ToolAction::Completed,
                    None,
                )
                .expect("fixture tool row is valid"),
            )),
        ),
    ];
    let summary = presentation.apply_replay(batch);
    assert_eq!(summary.applied, 3);
    assert_eq!(summary.duplicates, 0);
    assert_eq!(summary.stale, 0);
    assert_eq!(presentation.last_cursor(), Some(3));
    let resolved = presentation.approval("approval-1").expect("replay settles");
    assert_eq!(resolved.approved(), Some(true));

    let replay = vec![
        (1_u64, event(approval_requested())),
        (2_u64, event(approval_requested())),
        (3_u64, event(approval_resolved(true))),
        (
            4_u64,
            EngineObservationEvent {
                thread_id: other_thread_id(),
                observation: approval_requested(),
            },
        ),
        (0_u64, event(approval_requested())),
    ];
    let summary = presentation.apply_replay(replay);
    assert_eq!(summary.applied, 0);
    assert_eq!(summary.duplicates, 4);
    assert_eq!(summary.stale, 1);
}

#[test]
fn stale_threads_and_zero_cursors_change_nothing() {
    let mut presentation = state();
    let foreign = EngineObservationEvent {
        thread_id: other_thread_id(),
        observation: approval_requested(),
    };
    assert!(matches!(
        presentation.apply(1, &foreign),
        ApplyOutcome::StaleThread
    ));
    assert!(presentation.is_empty());
    assert_eq!(presentation.last_cursor(), None);

    assert!(matches!(
        presentation.apply(0, &event(approval_requested())),
        ApplyOutcome::Duplicate
    ));
    assert!(presentation.is_empty());
}

#[test]
fn unknown_arms_degrade_to_diagnostic_rows() {
    let mut presentation = state();
    let outcome = presentation.record_unknown(
        &thread_id(),
        1,
        "future_observation",
        String::from("a newer engine reported future_observation"),
    );
    assert!(matches!(
        outcome,
        ApplyOutcome::Applied {
            tag: "unknown",
            settled_in_place: false
        }
    ));
    let rows = presentation.timeline();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tag(), "future_observation");
    assert!(rows[0].summary().contains("future_observation"));

    assert!(matches!(
        presentation.record_unknown(
            &other_thread_id(),
            2,
            "future_observation",
            String::from("x")
        ),
        ApplyOutcome::StaleThread
    ));
    assert!(matches!(
        presentation.record_unknown(&thread_id(), 1, "future_observation", String::from("x")),
        ApplyOutcome::Duplicate
    ));
    assert_eq!(presentation.timeline().len(), 1);
}

#[test]
fn approval_kinds_map_to_exact_presentations() {
    let mut presentation = state();
    presentation.apply(9, &event(approval_requested()));
    let second = Observation::Approval(
        ApprovalObservation::requested(
            observation_id("obs-approval-file"),
            sequence(13),
            observation_id("approval-file"),
            String::from("Apply the edits?"),
            ApprovalRequest::file_change(Some(String::from("generated fixes")))
                .expect("fixture approval request is valid"),
        )
        .expect("fixture requested approval is valid"),
    );
    presentation.apply(13, &event(second));
    let third = Observation::Approval(
        ApprovalObservation::requested(
            observation_id("obs-approval-action"),
            sequence(14),
            observation_id("approval-action"),
            String::from("Connect the provider?"),
            ApprovalRequest::action(Some(String::from("connect")))
                .expect("fixture approval request is valid"),
        )
        .expect("fixture requested approval is valid"),
    );
    presentation.apply(14, &event(third));

    let command = presentation
        .approval_presentation("approval-1")
        .expect("command approval renders");
    assert_eq!(command.approve_label, "Run command");
    assert_eq!(command.title, "Run this command?");
    assert_eq!(command.icon_name(), "terminal-2");
    assert_eq!(
        command.description.as_deref(),
        Some("verify before landing")
    );

    let file_change = presentation
        .approval_presentation("approval-file")
        .expect("file-change approval renders");
    assert_eq!(file_change.approve_label, "Apply changes");
    assert_eq!(file_change.title, "Apply these changes?");
    assert_eq!(file_change.icon_name(), "file-diff");

    let action = presentation
        .approval_presentation("approval-action")
        .expect("action approval renders");
    assert_eq!(action.approve_label, "Approve");
    assert_eq!(action.title, "Allow this action?");
    assert_eq!(action.icon_name(), "file-diff");
    assert!(
        presentation
            .approval_presentation("approval-missing")
            .is_none()
    );
}

#[test]
fn turn_run_and_terminal_states_pair() {
    let mut presentation = state();
    let turn_started = Observation::TurnState(TurnStateObservation::new(
        observation_id("obs-turn-started"),
        sequence(16),
        observation_id("turn-1"),
        TurnState::Started,
    ));
    presentation.apply(1, &event(turn_started));
    let turn_completed = Observation::TurnState(TurnStateObservation::new(
        observation_id("obs-turn-completed"),
        sequence(17),
        observation_id("turn-1"),
        TurnState::Completed,
    ));
    presentation.apply(2, &event(turn_completed));
    assert_eq!(
        presentation.turn_state("turn-1"),
        Some(TurnState::Completed)
    );

    let run_state = Observation::RunState(RunStateObservation::new(
        observation_id("obs-run-state"),
        sequence(18),
        RunState::Waiting,
    ));
    presentation.apply(3, &event(run_state));
    assert_eq!(presentation.run_state(), Some(RunState::Waiting));

    let run_terminal = Observation::RunTerminal(
        RunTerminalObservation::new(
            observation_id("obs-run-terminal"),
            sequence(19),
            RunTerminalState::Completed,
            None,
            Some(String::from("Session title")),
        )
        .expect("fixture run terminal is valid"),
    );
    presentation.apply(4, &event(run_terminal));
    let terminal = presentation.run_terminal().expect("terminal outcome pairs");
    assert_eq!(terminal.state, RunTerminalState::Completed);
    assert!(terminal.has_summary_title);

    let tags = presentation
        .timeline()
        .iter()
        .map(TimelineRow::tag)
        .collect::<Vec<&str>>();
    for expected in ["turn_state", "run_state", "run_terminal"] {
        assert!(tags.contains(&expected), "missing timeline tag {expected}");
    }
}

#[test]
fn subagent_retry_and_diagnostic_rows_pair() {
    let mut presentation = state();
    let subagent = Observation::Subagent(
        SubagentObservation::new(
            observation_id("obs-subagent"),
            sequence(21),
            SubagentInput {
                agent_native_thread_id: observation_id("agent-child"),
                parent_native_thread_id: observation_id("agent-parent"),
                state: SubagentState::Running,
                activity: Some(String::from("exploring")),
                agent_path: None,
                turn_id: None,
            },
        )
        .expect("fixture subagent row is valid"),
    );
    presentation.apply(1, &event(subagent));
    let transcript = Observation::SubagentTranscript(SubagentTranscriptObservation::new(
        observation_id("obs-subagent-transcript"),
        sequence(22),
        observation_id("agent-child"),
        observation_id("agent-parent"),
        TranscriptContent::Tool(
            TranscriptTool::new(
                observation_id("tool-child-1"),
                String::from("grep"),
                ToolAction::Completed,
                None,
            )
            .expect("fixture transcript tool is valid"),
        ),
    ));
    presentation.apply(2, &event(transcript));

    let diagnostic = Observation::ProcessDiagnostic(
        ProcessDiagnosticObservation::new(
            observation_id("obs-process-diagnostic"),
            sequence(23),
            artisan_domain::DiagnosticLevel::Warning,
            String::from("engine process restarted"),
            None,
        )
        .expect("fixture process diagnostic is valid"),
    );
    presentation.apply(3, &event(diagnostic));
    let protocol_diagnostic = Observation::ProtocolDiagnostic(
        ProtocolDiagnosticObservation::new(
            observation_id("obs-protocol-diagnostic"),
            sequence(24),
            artisan_domain::DiagnosticLevel::Info,
            String::from("replayed one batch"),
        )
        .expect("fixture protocol diagnostic is valid"),
    );
    presentation.apply(4, &event(protocol_diagnostic));

    let retry = Observation::Retry(
        RetryObservation::new(
            observation_id("obs-retry"),
            sequence(25),
            observation_id("turn-1"),
            RetryAttemptState::Retrying,
            true,
            String::from("backing off"),
        )
        .expect("fixture retry is valid"),
    );
    presentation.apply(5, &event(retry));

    let tags = presentation
        .timeline()
        .iter()
        .map(TimelineRow::tag)
        .collect::<Vec<&str>>();
    for expected in [
        "subagent",
        "subagent_transcript",
        "process_diagnostic",
        "protocol_diagnostic",
        "retry",
    ] {
        assert!(tags.contains(&expected), "missing timeline tag {expected}");
    }
    assert!(!presentation.is_empty());
}

#[test]
fn uni_delivery_accepts_engine_observation_events() {
    let observation_envelope = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("frame-observation").expect("fixture frame id is valid"),
        sent_at: UnixMillis::EPOCH,
        body: WireEnvelopeBody::Event(ServerEvent {
            cursor: EventCursor::new(7).expect("fixture event cursor is positive"),
            event: Event::EngineObservation(EngineObservationEvent {
                thread_id: thread_id(),
                observation: approval_requested(),
            }),
        }),
    };
    let decoded = validate_uni_envelope(&observation_envelope, ProtocolVersion::V1)
        .expect("observation events ride the uni delivery stream");
    assert!(matches!(decoded, UniDelivery::Observation(_)));
    let UniDelivery::Observation(delivered) = decoded else {
        panic!("an observation frame must decode as an observation");
    };
    assert_eq!(delivered.cursor.get(), 7);
    let Event::EngineObservation(paired) = delivered.event else {
        panic!("an observation frame must carry an engine observation");
    };
    assert_eq!(paired.thread_id, thread_id());

    let response_envelope = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("frame-response").expect("fixture frame id is valid"),
        sent_at: UnixMillis::EPOCH,
        body: WireEnvelopeBody::Response(artisan_protocol::ServerResponse {
            request_id: artisan_domain::RequestId::parse("request-r").expect("fixture request"),
            payload: artisan_protocol::ResponsePayload::ProjectListing(
                artisan_domain::ProjectListing::new(Vec::new()).expect("fixture listing"),
            ),
        }),
    };
    assert!(validate_uni_envelope(&response_envelope, ProtocolVersion::V1).is_err());
}
