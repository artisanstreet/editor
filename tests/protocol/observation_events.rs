//! Finite engine-observation delivery over protocol events (S1b).
//!
//! Every observation row below is an already-validated, sanitized S1a domain
//! value published as `Event::EngineObservation`. These tests prove the
//! owned codec carries all twenty-two observation kinds field-for-field
//! (twenty-four rows, counting approval/question requested and resolved
//! states separately), preserves the approval/question request identities
//! the later A-approve packet answers, rejects malformed and unknown input
//! with typed errors, and still decodes the frozen v1 event bytes: the
//! schema change is purely additive, so raw v1-shaped frames (built here
//! with the generated bindings, never setting the new member) must decode
//! exactly as before.

use std::error::Error;

use artisan_domain::{
    AgentMessageCompletedObservation, AgentMessageDeltaObservation, ApprovalObservation,
    ApprovalRequest, ArtisanCode, CompactionObservation, CompactionState, DiagnosticLevel,
    DisplayName, EngineErrorRef, EngineErrorRefInput, EngineObservationEvent, Event, FileAction,
    FileObservation, FirstMessageQueued, LimitScope, MessageBody, MessageId, MessagePhase,
    NativeActionObservation, Observation, ObservationId, ObservationSequence, PlanEntry,
    PlanEntryStatus, PlanObservation, ProcessDiagnosticObservation, ProjectAttached, ProjectId,
    ProjectSummary, ProtocolDiagnosticObservation, QuestionInput, QuestionObservation,
    QuestionOption, QueuedMessage, ReasoningSummaryCompletedObservation,
    ReasoningSummaryDeltaObservation, RequestId, RetryAttemptState, RetryObservation, RootPath,
    RunState, RunStateObservation, RunTerminalObservation, RunTerminalState, SearchObservation,
    SearchScope, SearchState, SubagentInput, SubagentObservation, SubagentState,
    SubagentTranscriptObservation, TerminalActivityInput, TerminalActivityObservation,
    TerminalActivityState, TerminalChannel, ThreadCreated, ThreadId, ThreadSummary, ThreadTitle,
    ToolAction, ToolObservation, TranscriptContent, TranscriptTool, TurnState,
    TurnStateObservation, UnixMillis, UsageBasis, UsageInput, UsageObservation,
};
use artisan_protocol::artisan_capnp::{
    ObservationTerminalState as WireTerminalState, ObservationToolAction as WireToolAction,
    envelope,
};
use artisan_protocol::{
    EventCursor, FrameId, ProtocolDecodeError, ProtocolVersion, ServerEvent, WireEnvelope,
    WireEnvelopeBody, decode_envelope, encode_envelope,
};
use capnp::message::{Builder, HeapAllocator};
use capnp::serialize;

// ---------------------------------------------------------------------------
// Validated fixtures
// ---------------------------------------------------------------------------

fn observation_id(value: &str) -> ObservationId {
    ObservationId::parse(value).expect("fixture observation id is valid")
}

fn sequence(value: u64) -> ObservationSequence {
    ObservationSequence::new(value).expect("fixture sequence is valid")
}

fn thread_id() -> ThreadId {
    ThreadId::parse("thread-obs").expect("fixture thread id is valid")
}

fn envelope(frame: &str, body: WireEnvelopeBody) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame).expect("fixture frame id is valid"),
        sent_at: UnixMillis::from_millis(-4_000),
        body,
    }
}

fn observation_envelope(frame: &str, cursor: u64, observation: Observation) -> WireEnvelope {
    envelope(
        frame,
        WireEnvelopeBody::Event(ServerEvent {
            cursor: EventCursor::new(cursor).expect("fixture event cursor is positive"),
            event: Event::EngineObservation(EngineObservationEvent {
                thread_id: thread_id(),
                observation,
                attribution: None,
            }),
        }),
    )
}

fn attributed_observation_envelope(
    frame: &str,
    cursor: u64,
    observation: Observation,
    run_id: &str,
    turn_id: &str,
    committed_at: i64,
    delivery_sequence: u64,
) -> WireEnvelope {
    envelope(
        frame,
        WireEnvelopeBody::Event(ServerEvent {
            cursor: EventCursor::new(cursor).expect("fixture event cursor is positive"),
            event: Event::EngineObservation(EngineObservationEvent {
                thread_id: thread_id(),
                observation,
                attribution: Some(artisan_domain::EngineObservationAttribution {
                    run_id: artisan_domain::RunId::parse(run_id)
                        .expect("fixture run id is valid"),
                    turn_id: artisan_domain::TurnId::parse(turn_id)
                        .expect("fixture turn id is valid"),
                    committed_at: UnixMillis::from_millis(committed_at),
                    delivery_sequence,
                }),
            }),
        }),
    )
}

fn assert_roundtrip(value: &WireEnvelope) -> Result<(), Box<dyn Error>> {
    let encoded = encode_envelope(value)?;
    let decoded = decode_envelope(&encoded)?;
    assert!(
        decoded == *value,
        "owned envelope must survive field-for-field"
    );
    Ok(())
}

fn agent_message_delta(sequence_value: u64) -> Observation {
    Observation::AgentMessageDelta(
        AgentMessageDeltaObservation::new(
            observation_id("obs-message-delta"),
            sequence(sequence_value),
            observation_id("item-1"),
            MessagePhase::Commentary,
            String::from("streamed fragment"),
            observation_id("turn-1"),
        )
        .expect("fixture message delta is valid"),
    )
}

fn agent_message_completed(sequence_value: u64) -> Observation {
    Observation::AgentMessageCompleted(
        AgentMessageCompletedObservation::new(
            observation_id("obs-message-completed"),
            sequence(sequence_value),
            observation_id("item-1"),
            MessagePhase::Final,
            String::from("settled reply"),
            observation_id("turn-1"),
        )
        .expect("fixture completed message is valid"),
    )
}

fn reasoning_summary_delta(sequence_value: u64) -> Observation {
    Observation::ReasoningSummaryDelta(
        ReasoningSummaryDeltaObservation::new(
            observation_id("obs-reasoning-delta"),
            sequence(sequence_value),
            observation_id("item-2"),
            3,
            String::from("summary fragment"),
            Some(128),
            observation_id("turn-1"),
        )
        .expect("fixture reasoning delta is valid"),
    )
}

fn reasoning_summary_completed(sequence_value: u64) -> Observation {
    Observation::ReasoningSummaryCompleted(
        ReasoningSummaryCompletedObservation::new(
            observation_id("obs-reasoning-completed"),
            sequence(sequence_value),
            observation_id("item-2"),
            Some(String::from("public summary")),
            observation_id("turn-1"),
        )
        .expect("fixture settled reasoning is valid"),
    )
}

fn tool(sequence_value: u64) -> Observation {
    Observation::Tool(
        ToolObservation::new(
            observation_id("obs-tool"),
            sequence(sequence_value),
            observation_id("tool-1"),
            String::from("read"),
            ToolAction::Completed,
            Some(String::from("read 42 lines")),
        )
        .expect("fixture tool row is valid"),
    )
}

fn file(sequence_value: u64) -> Observation {
    Observation::File(
        FileObservation::new(
            observation_id("obs-file"),
            sequence(sequence_value),
            String::from("src/main.rs"),
            FileAction::Modified,
            Some(10),
            Some(2),
        )
        .expect("fixture file row is valid"),
    )
}

fn search(sequence_value: u64) -> Observation {
    Observation::Search(
        SearchObservation::new(
            observation_id("obs-search"),
            sequence(sequence_value),
            String::from("observation delivery"),
            Some(SearchScope::Workspace),
            Some(observation_id("search-1")),
            SearchState::Completed,
            Some(7),
        )
        .expect("fixture search row is valid"),
    )
}

fn terminal_activity(sequence_value: u64) -> Observation {
    Observation::TerminalActivity(
        TerminalActivityObservation::new(
            observation_id("obs-terminal"),
            sequence(sequence_value),
            TerminalActivityInput {
                activity_id: observation_id("activity-1"),
                channel: Some(TerminalChannel::Stdout),
                command: Some(String::from("cargo test")),
                shell: Some(String::from("pwsh")),
                output: Some(String::from("test result: ok")),
                exit_code: Some(0),
                state: TerminalActivityState::Completed,
            },
        )
        .expect("fixture terminal row is valid"),
    )
}

fn approval_requested(sequence_value: u64) -> Observation {
    Observation::Approval(
        ApprovalObservation::requested(
            observation_id("obs-approval-requested"),
            sequence(sequence_value),
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

fn approval_resolved(sequence_value: u64) -> Observation {
    Observation::Approval(
        ApprovalObservation::resolved(
            observation_id("obs-approval-resolved"),
            sequence(sequence_value),
            observation_id("approval-1"),
            String::from("Run the test suite?"),
            ApprovalRequest::command(
                String::from("cargo test"),
                Some(String::from("C:/repos/demo")),
                Some(String::from("verify before landing")),
            )
            .expect("fixture approval request is valid"),
            true,
        )
        .expect("fixture resolved approval is valid"),
    )
}

fn question_requested(sequence_value: u64) -> Observation {
    Observation::Question(
        QuestionObservation::requested(
            observation_id("obs-question-requested"),
            sequence(sequence_value),
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

fn question_resolved(sequence_value: u64) -> Observation {
    Observation::Question(
        QuestionObservation::resolved(
            observation_id("obs-question-resolved"),
            sequence(sequence_value),
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

fn plan(sequence_value: u64) -> Observation {
    Observation::Plan(
        PlanObservation::new(
            observation_id("obs-plan"),
            sequence(sequence_value),
            vec![
                PlanEntry::new(
                    observation_id("plan-entry-1"),
                    PlanEntryStatus::Completed,
                    String::from("Define the vocabulary"),
                )
                .expect("fixture plan entry is valid"),
                PlanEntry::new(
                    observation_id("plan-entry-2"),
                    PlanEntryStatus::InProgress,
                    String::from("Deliver the rows"),
                )
                .expect("fixture plan entry is valid"),
            ],
            Some(observation_id("turn-1")),
        )
        .expect("fixture plan is valid"),
    )
}

fn compaction(sequence_value: u64) -> Observation {
    Observation::Compaction(
        CompactionObservation::new(
            observation_id("obs-compaction"),
            sequence(sequence_value),
            CompactionState::Completed,
            Some(observation_id("compaction-1")),
            Some(900),
            Some(String::from("compacted 128k tokens")),
        )
        .expect("fixture compaction is valid"),
    )
}

fn retry(sequence_value: u64) -> Observation {
    Observation::Retry(
        RetryObservation::new(
            observation_id("obs-retry"),
            sequence(sequence_value),
            observation_id("turn-1"),
            RetryAttemptState::Retrying,
            true,
            String::from("provider rate limit, backing off"),
        )
        .expect("fixture retry is valid"),
    )
}

fn run_state(sequence_value: u64) -> Observation {
    Observation::RunState(RunStateObservation::new(
        observation_id("obs-run-state"),
        sequence(sequence_value),
        RunState::Running,
    ))
}

fn turn_state(sequence_value: u64) -> Observation {
    Observation::TurnState(TurnStateObservation::new(
        observation_id("obs-turn-state"),
        sequence(sequence_value),
        observation_id("turn-1"),
        TurnState::Started,
    ))
}

fn subagent(sequence_value: u64) -> Observation {
    Observation::Subagent(
        SubagentObservation::new(
            observation_id("obs-subagent"),
            sequence(sequence_value),
            SubagentInput {
                agent_native_thread_id: observation_id("agent-child"),
                parent_native_thread_id: observation_id("agent-parent"),
                state: SubagentState::Running,
                activity: Some(String::from("exploring")),
                agent_path: Some(String::from("agents/explorer")),
                turn_id: Some(observation_id("turn-1")),
            },
        )
        .expect("fixture subagent row is valid"),
    )
}

fn subagent_transcript(sequence_value: u64) -> Observation {
    Observation::SubagentTranscript(SubagentTranscriptObservation::new(
        observation_id("obs-subagent-transcript"),
        sequence(sequence_value),
        observation_id("agent-child"),
        observation_id("agent-parent"),
        TranscriptContent::Tool(
            TranscriptTool::new(
                observation_id("tool-child-1"),
                String::from("grep"),
                ToolAction::Completed,
                Some(String::from("3 matches")),
            )
            .expect("fixture transcript tool is valid"),
        ),
    ))
}

fn usage(sequence_value: u64) -> Observation {
    Observation::Usage(
        UsageObservation::new(
            observation_id("obs-usage"),
            sequence(sequence_value),
            UsageInput {
                basis: UsageBasis::Cumulative,
                input_tokens: Some(1_000),
                cached_input_tokens: Some(200),
                output_tokens: Some(150),
                context_tokens: Some(1_350),
                context_window_tokens: Some(200_000),
                cost_usd: Some(0.5),
                provider_route_id: Some(observation_id("route-1")),
                turn_id: Some(observation_id("turn-1")),
            },
        )
        .expect("fixture usage is valid"),
    )
}

fn error_ref() -> EngineErrorRef {
    EngineErrorRef::new(EngineErrorRefInput {
        artisan_code: ArtisanCode::parse("AE-PROVIDER-RATE-LIMITED")
            .expect("fixture artisan code is valid"),
        provider_code: Some(String::from("rate_limit_exceeded")),
        detail: Some(String::from("slow down")),
        affected_model_id: Some(String::from("model-1")),
        limit_id: Some(String::from("bucket-7")),
        limit_label: Some(String::from("requests per minute")),
        limit_scope: Some(LimitScope::Shared),
        resets_at: Some(String::from("2026-09-08T00:00:00Z")),
    })
    .expect("fixture error reference is valid")
}

fn native_action(sequence_value: u64) -> Observation {
    Observation::NativeAction(
        NativeActionObservation::new(
            observation_id("obs-native-action"),
            sequence(sequence_value),
            String::from("provider_web_search"),
            Some(String::from("searched the web")),
            false,
            Some(error_ref()),
        )
        .expect("fixture native action is valid"),
    )
}

fn process_diagnostic(sequence_value: u64) -> Observation {
    Observation::ProcessDiagnostic(
        ProcessDiagnosticObservation::new(
            observation_id("obs-process-diagnostic"),
            sequence(sequence_value),
            DiagnosticLevel::Warning,
            String::from("engine process restarted"),
            Some(error_ref()),
        )
        .expect("fixture process diagnostic is valid"),
    )
}

fn protocol_diagnostic(sequence_value: u64) -> Observation {
    Observation::ProtocolDiagnostic(
        ProtocolDiagnosticObservation::new(
            observation_id("obs-protocol-diagnostic"),
            sequence(sequence_value),
            DiagnosticLevel::Info,
            String::from("replayed one batch"),
        )
        .expect("fixture protocol diagnostic is valid"),
    )
}

fn run_terminal(sequence_value: u64) -> Observation {
    Observation::RunTerminal(
        RunTerminalObservation::new(
            observation_id("obs-run-terminal"),
            sequence(sequence_value),
            RunTerminalState::Completed,
            None,
            Some(String::from("Session title")),
        )
        .expect("fixture run terminal is valid"),
    )
}

// ---------------------------------------------------------------------------
// Round-trips
// ---------------------------------------------------------------------------

#[test]
fn every_engine_observation_row_roundtrips() -> Result<(), Box<dyn Error>> {
    let rows: Vec<Observation> = vec![
        agent_message_delta(1),
        agent_message_completed(2),
        reasoning_summary_delta(3),
        reasoning_summary_completed(4),
        tool(5),
        file(6),
        search(7),
        terminal_activity(8),
        approval_requested(9),
        approval_resolved(10),
        question_requested(11),
        question_resolved(12),
        plan(13),
        compaction(14),
        retry(15),
        run_state(16),
        turn_state(17),
        subagent(18),
        subagent_transcript(19),
        usage(20),
        native_action(21),
        process_diagnostic(22),
        protocol_diagnostic(23),
        run_terminal(24),
    ];
    assert_eq!(rows.len(), 24, "every observation row needs a round-trip");
    for (index, observation) in rows.into_iter().enumerate() {
        let cursor = u64::try_from(index + 1).expect("fixture cursor fits") + 100;
        assert_roundtrip(&observation_envelope(
            &format!("server-event-observation-{cursor}"),
            cursor,
            observation,
        ))?;
    }
    Ok(())
}

#[test]
fn approval_and_question_request_ids_survive_the_wire() -> Result<(), Box<dyn Error>> {
    for (frame, observation) in [
        ("server-event-approval-requested", approval_requested(9)),
        ("server-event-approval-resolved", approval_resolved(10)),
        ("server-event-question-requested", question_requested(11)),
        ("server-event-question-resolved", question_resolved(12)),
    ] {
        let value = observation_envelope(frame, 7, observation);
        let decoded = decode_envelope(&encode_envelope(&value)?)?;
        let WireEnvelopeBody::Event(event) = decoded.body else {
            panic!("an observation frame must decode as an event");
        };
        let Event::EngineObservation(delivered) = event.event else {
            panic!("an observation frame must decode as an engine observation");
        };
        assert_eq!(delivered.thread_id, thread_id());
        match delivered.observation {
            Observation::Approval(approval) => {
                assert_eq!(approval.approval_id().as_str(), "approval-1");
                assert_eq!(approval.description(), "Run the test suite?");
                assert_eq!(
                    approval.request().command_text(),
                    Some("cargo test"),
                    "the command under review must survive for A-approve"
                );
            }
            Observation::Question(question) => {
                assert_eq!(question.question_id().as_str(), "question-1");
                assert_eq!(question.text(), "Which runtime?");
            }
            other => panic!("unexpected observation survived: {}", other.tag()),
        }
    }

    let resolved_approval =
        observation_envelope("server-event-approval-yes", 8, approval_resolved(10));
    let WireEnvelopeBody::Event(event) =
        decode_envelope(&encode_envelope(&resolved_approval)?)?.body
    else {
        panic!("a resolved approval must decode as an event");
    };
    let Event::EngineObservation(delivered) = event.event else {
        panic!("a resolved approval must decode as an engine observation");
    };
    let Observation::Approval(approval) = delivered.observation else {
        panic!("a resolved approval must stay an approval");
    };
    assert_eq!(approval.approved(), Some(true));

    let resolved_question =
        observation_envelope("server-event-question-yes", 9, question_resolved(12));
    let WireEnvelopeBody::Event(event) =
        decode_envelope(&encode_envelope(&resolved_question)?)?.body
    else {
        panic!("a resolved question must decode as an event");
    };
    let Event::EngineObservation(delivered) = event.event else {
        panic!("a resolved question must decode as an engine observation");
    };
    let Observation::Question(question) = delivered.observation else {
        panic!("a resolved question must stay a question");
    };
    assert_eq!(
        question.answers(),
        Some(&vec![String::from("tokio")]),
        "answers must survive for A-approve settlement"
    );
    Ok(())
}

#[test]
fn attributed_observation_roundtrips_with_thread_scoped_cursor() -> Result<(), Box<dyn Error>> {
    let value = attributed_observation_envelope(
        "server-event-observation-attributed",
        41,
        tool(5),
        "run-attributed-1",
        "turn-attributed-1",
        6_001,
        17,
    );
    let decoded = decode_envelope(&encode_envelope(&value)?)?;
    let WireEnvelopeBody::Event(event) = &decoded.body else {
        panic!("an attributed observation frame must decode as an event");
    };
    let Event::EngineObservation(delivered) = &event.event else {
        panic!("an attributed observation frame must decode as an engine observation");
    };
    let attribution = delivered
        .attribution
        .as_ref()
        .expect("attribution must survive the wire");
    assert_eq!(attribution.run_id.as_str(), "run-attributed-1");
    assert_eq!(attribution.turn_id.as_str(), "turn-attributed-1");
    assert_eq!(attribution.committed_at, UnixMillis::from_millis(6_001));
    assert_eq!(attribution.delivery_sequence, 17);
    assert!(
        decoded == value,
        "attributed envelope must survive field-for-field"
    );
    Ok(())
}

#[test]
fn absent_attribution_decodes_as_none_for_old_frames() -> Result<(), Box<dyn Error>> {
    // Pre-attribution v1 bytes never set the union, so they must decode as
    // `None` rather than a defaulted struct.
    let value = observation_envelope("server-event-observation-legacy", 42, tool(5));
    let decoded = decode_envelope(&encode_envelope(&value)?)?;
    let WireEnvelopeBody::Event(event) = decoded.body else {
        panic!("a legacy observation frame must decode as an event");
    };
    let Event::EngineObservation(delivered) = event.event else {
        panic!("a legacy observation frame must decode as an engine observation");
    };
    assert!(
        delivered.attribution.is_none(),
        "absent attribution must decode as None"
    );
    Ok(())
}

#[test]
fn malformed_attribution_is_rejected() {
    for (frame, build) in [
        (
            "raw-attribution-empty-run",
            |attribution: artisan_protocol::artisan_capnp::engine_observation_attribution::Builder<'_>| {
                let mut owned = attribution;
                owned.set_run_id("");
                owned.set_turn_id("turn-1");
                owned.set_committed_at_millis(6_001);
                owned.set_delivery_sequence(1);
            },
        ),
        (
            "raw-attribution-zero-sequence",
            |attribution: artisan_protocol::artisan_capnp::engine_observation_attribution::Builder<'_>| {
                let mut owned = attribution;
                owned.set_run_id("run-1");
                owned.set_turn_id("turn-1");
                owned.set_committed_at_millis(6_001);
                owned.set_delivery_sequence(0);
            },
        ),
        (
            "raw-attribution-nonpositive-time",
            |attribution: artisan_protocol::artisan_capnp::engine_observation_attribution::Builder<'_>| {
                let mut owned = attribution;
                owned.set_run_id("run-1");
                owned.set_turn_id("turn-1");
                owned.set_committed_at_millis(0);
                owned.set_delivery_sequence(1);
            },
        ),
    ] as [(&str, fn(artisan_protocol::artisan_capnp::engine_observation_attribution::Builder<'_>)); 3]
    {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(1);
        root.set_message_id(frame);
        let mut event = root.reborrow().init_body().init_event();
        event.set_cursor(43);
        let mut observation = event.reborrow().init_engine_observation();
        observation.set_thread_id("thread-obs");
        let mut tool = observation.reborrow().init_observation().init_tool();
        tool.set_id("obs-tool");
        tool.set_sequence(5);
        tool.set_tool_id("tool-1");
        tool.set_tool_name("read");
        tool.set_action(artisan_protocol::artisan_capnp::ObservationToolAction::Completed);
        tool.set_detail("read 42 lines");
        build(observation.init_attribution().init_attribution());
        let encoded = serialize::write_message_to_words(&message);
        assert!(
            decode_envelope(&encoded).is_err(),
            "malformed attribution must be rejected for {frame}"
        );
    }
}

// ---------------------------------------------------------------------------
// Malformed and unknown input
// ---------------------------------------------------------------------------

fn raw_envelope() -> Builder<HeapAllocator> {
    Builder::new(HeapAllocator::new())
}

fn raw_observation_event(
    frame: &str,
    build: impl FnOnce(artisan_protocol::artisan_capnp::engine_observation::Builder<'_>),
) -> Vec<u8> {
    let mut message = raw_envelope();
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id(frame);
    let mut event = root.reborrow().init_body().init_event();
    event.set_cursor(11);
    let mut observation = event.reborrow().init_engine_observation();
    observation.set_thread_id("thread-obs");
    build(observation.init_observation());
    serialize::write_message_to_words(&message)
}

fn raw_tool_event(frame: &str, action: WireToolAction) -> Vec<u8> {
    raw_observation_event(frame, |observation| {
        let mut tool = observation.init_tool();
        tool.set_id("obs-tool");
        tool.set_sequence(5);
        tool.set_tool_id("tool-1");
        tool.set_tool_name("read");
        tool.set_action(action);
        tool.set_detail("read 42 lines");
    })
}

fn raw_file_event(frame: &str) -> Vec<u8> {
    raw_observation_event(frame, |observation| {
        let mut file = observation.init_file();
        file.set_id("obs-file");
        file.set_sequence(6);
        file.set_path("src/main.rs");
        file.set_action(artisan_protocol::artisan_capnp::ObservationFileAction::Modified);
        file.reborrow().init_lines_added().set_no_lines_added(());
        file.reborrow()
            .init_lines_deleted()
            .set_no_lines_deleted(());
    })
}

#[test]
fn unknown_observation_union_discriminant_is_rejected() {
    let tool = raw_tool_event("raw-union-tool", WireToolAction::Started);
    let file = raw_file_event("raw-union-file");
    let mut found = false;
    for index in 0..tool.len().min(file.len()) {
        if tool[index] == file[index] {
            continue;
        }
        let mut corrupted = tool.clone();
        corrupted[index] = 255;
        if matches!(
            decode_envelope(&corrupted),
            Err(ProtocolDecodeError::UnknownDiscriminant { value: 255 })
        ) {
            found = true;
            break;
        }
    }
    assert!(
        found,
        "no differing byte produced UnknownDiscriminant {{ value: 255 }}"
    );
}

#[test]
fn unknown_observation_enum_ordinal_is_rejected() {
    let started = raw_tool_event("raw-enum", WireToolAction::Started);
    let completed = raw_tool_event("raw-enum", WireToolAction::Completed);
    let differing: Vec<usize> = started
        .iter()
        .zip(completed.iter())
        .enumerate()
        .filter_map(|(index, (left, right))| (left != right).then_some(index))
        .collect();
    assert_eq!(differing.len(), 1, "only the action ordinal should differ");
    let mut malformed = started;
    malformed[differing[0]] = 255;
    assert!(matches!(
        decode_envelope(&malformed),
        Err(ProtocolDecodeError::UnknownDiscriminant { value: 255 })
    ));
}

#[test]
fn unknown_terminal_state_ordinal_is_rejected() {
    let started = raw_observation_event("raw-terminal", |observation| {
        let mut activity = observation.init_terminal_activity();
        activity.set_id("obs-terminal");
        activity.set_sequence(8);
        activity.set_activity_id("activity-1");
        activity.reborrow().init_channel().set_no_channel(());
        activity.reborrow().init_output().set_no_output(());
        activity.reborrow().init_exit_code().set_no_exit_code(());
        activity.set_state(WireTerminalState::Started);
    });
    let completed = raw_observation_event("raw-terminal", |observation| {
        let mut activity = observation.init_terminal_activity();
        activity.set_id("obs-terminal");
        activity.set_sequence(8);
        activity.set_activity_id("activity-1");
        activity.reborrow().init_channel().set_no_channel(());
        activity.reborrow().init_output().set_no_output(());
        activity.reborrow().init_exit_code().set_no_exit_code(());
        activity.set_state(WireTerminalState::Completed);
    });
    let differing: Vec<usize> = started
        .iter()
        .zip(completed.iter())
        .enumerate()
        .filter_map(|(index, (left, right))| (left != right).then_some(index))
        .collect();
    assert_eq!(differing.len(), 1, "only the state ordinal should differ");
    let mut malformed = started;
    malformed[differing[0]] = 255;
    assert!(matches!(
        decode_envelope(&malformed),
        Err(ProtocolDecodeError::UnknownDiscriminant { value: 255 })
    ));
}

#[test]
fn empty_required_observation_text_is_rejected() {
    let malformed = raw_observation_event("raw-empty-tool-name", |observation| {
        let mut tool = observation.init_tool();
        tool.set_id("obs-tool");
        tool.set_sequence(5);
        tool.set_tool_id("tool-1");
        tool.set_tool_name("");
        tool.set_action(WireToolAction::Started);
    });
    assert!(matches!(
        decode_envelope(&malformed),
        Err(ProtocolDecodeError::Observation { .. })
    ));
}

#[test]
fn empty_message_delta_is_rejected() {
    let malformed = raw_observation_event("raw-empty-delta", |observation| {
        let mut delta = observation.init_agent_message_delta();
        delta.set_id("obs-message-delta");
        delta.set_sequence(1);
        delta.set_item_id("item-1");
        delta.set_phase(artisan_protocol::artisan_capnp::ObservationMessagePhase::Commentary);
        delta.set_delta("");
        delta.set_turn_id("turn-1");
    });
    assert!(matches!(
        decode_envelope(&malformed),
        Err(ProtocolDecodeError::Observation { .. })
    ));
}

#[test]
fn resolved_approval_without_a_decision_is_rejected() {
    let malformed = raw_observation_event("raw-approval-no-decision", |observation| {
        let mut approval = observation.init_approval();
        approval.set_id("obs-approval");
        approval.set_sequence(10);
        approval.set_approval_id("approval-1");
        approval.set_state(artisan_protocol::artisan_capnp::ObservationApprovalState::Resolved);
        approval.set_description("Run the test suite?");
        let mut request = approval.reborrow().init_request();
        request.set_kind(artisan_protocol::artisan_capnp::ObservationApprovalKind::Action);
        request.set_reason("verify before landing");
        approval.init_decision().set_no_decision(());
    });
    assert!(matches!(
        decode_envelope(&malformed),
        Err(ProtocolDecodeError::Observation { .. })
    ));
}

#[test]
fn requested_question_with_answers_is_rejected() {
    let malformed = raw_observation_event("raw-question-early-answers", |observation| {
        let mut question = observation.init_question();
        question.set_id("obs-question");
        question.set_sequence(11);
        question.set_question_id("question-1");
        question.set_state(artisan_protocol::artisan_capnp::ObservationQuestionState::Requested);
        question.set_text("Which runtime?");
        let mut answers = question.reborrow().init_answers().init_answers(1);
        answers.set(0_u32, "tokio");
    });
    assert!(matches!(
        decode_envelope(&malformed),
        Err(ProtocolDecodeError::Observation { .. })
    ));
}

#[test]
fn zero_event_cursor_is_rejected() {
    let mut message = raw_envelope();
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("raw-zero-cursor");
    let mut event = root.reborrow().init_body().init_event();
    event.set_cursor(0);
    let mut project = event.init_project_attached();
    project.set_project_id("project-1");
    let encoded = serialize::write_message_to_words(&message);
    assert!(matches!(
        decode_envelope(&encoded),
        Err(ProtocolDecodeError::ProtocolValue { .. })
    ));
}

#[test]
fn trailing_bytes_after_an_observation_are_rejected() {
    let mut encoded = encode_envelope(&observation_envelope("frame-trailing", 31, tool(5)))
        .expect("fixture observation must encode");
    encoded.push(0xFF);
    assert!(matches!(
        decode_envelope(&encoded),
        Err(ProtocolDecodeError::TrailingBytes { .. })
    ));
}

// ---------------------------------------------------------------------------
// v1 backward decoding: the change is purely additive, so frames shaped by
// the old contract (built here without ever setting the new member) decode
// exactly as before through the new codec. These raw frames are the frozen
// v1 bytes in executable form.
// ---------------------------------------------------------------------------

fn project_fixture() -> ProjectSummary {
    ProjectSummary {
        project_id: ProjectId::parse("project-1").expect("fixture project id is valid"),
        display_name: DisplayName::parse("Artisan Editor").expect("fixture name is valid"),
        root_path: RootPath::parse(r"C:\source\artisan-editor").expect("fixture path is valid"),
        attached_at: UnixMillis::from_millis(7),
    }
}

fn thread_fixture() -> ThreadSummary {
    ThreadSummary {
        thread_id: ThreadId::parse("thread-1").expect("fixture thread id is valid"),
        project_id: ProjectId::parse("project-1").expect("fixture project id is valid"),
        title: ThreadTitle::parse("New thread").expect("fixture title is valid"),
        created_at: UnixMillis::from_millis(7),
        updated_at: UnixMillis::from_millis(9),
    }
}

#[test]
fn v1_event_bytes_decode_unchanged() -> Result<(), Box<dyn Error>> {
    let attached = {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(1);
        root.set_message_id("v1-project-attached");
        let mut event = root.reborrow().init_body().init_event();
        event.set_cursor(1);
        let mut project = event.init_project_attached();
        project.set_project_id("project-1");
        project.set_display_name("Artisan Editor");
        project.set_root_path(r"C:\source\artisan-editor");
        project.set_attached_at_millis(7);
        serialize::write_message_to_words(&message)
    };
    let decoded = decode_envelope(&attached)?;
    let WireEnvelopeBody::Event(event) = decoded.body else {
        panic!("v1 projectAttached must still decode as an event");
    };
    assert_eq!(event.cursor.get(), 1);
    assert_eq!(
        event.event,
        Event::ProjectAttached(ProjectAttached {
            project: project_fixture()
        })
    );

    let created = {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(1);
        root.set_message_id("v1-thread-created");
        let mut event = root.reborrow().init_body().init_event();
        event.set_cursor(2);
        let mut thread = event.init_thread_created();
        thread.set_thread_id("thread-1");
        thread.set_project_id("project-1");
        thread.set_title("New thread");
        thread.set_created_at_millis(7);
        thread.set_updated_at_millis(9);
        serialize::write_message_to_words(&message)
    };
    let decoded = decode_envelope(&created)?;
    let WireEnvelopeBody::Event(event) = decoded.body else {
        panic!("v1 threadCreated must still decode as an event");
    };
    assert_eq!(event.cursor.get(), 2);
    assert_eq!(
        event.event,
        Event::ThreadCreated(ThreadCreated {
            thread: thread_fixture()
        })
    );

    let queued = {
        let mut message = raw_envelope();
        let mut root = message.init_root::<envelope::Builder>();
        root.set_protocol_version(1);
        root.set_message_id("v1-first-message-queued");
        let mut event = root.reborrow().init_body().init_event();
        event.set_cursor(3);
        let mut queued = event.init_first_message_queued();
        queued.set_request_id("request-message");
        queued.set_message_id("message-1");
        queued.set_thread_id("thread-1");
        queued.set_body("The event retains this complete body.");
        serialize::write_message_to_words(&message)
    };
    let decoded = decode_envelope(&queued)?;
    let WireEnvelopeBody::Event(event) = decoded.body else {
        panic!("v1 firstMessageQueued must still decode as an event");
    };
    assert_eq!(event.cursor.get(), 3);
    assert_eq!(
        event.event,
        Event::FirstMessageQueued(FirstMessageQueued {
            message: QueuedMessage {
                request_id: RequestId::parse("request-message")?,
                message_id: MessageId::parse("message-1")?,
                thread_id: ThreadId::parse("thread-1")?,
                body: MessageBody::parse("The event retains this complete body.")?,
            },
        })
    );
    Ok(())
}
