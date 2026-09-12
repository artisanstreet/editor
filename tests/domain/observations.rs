//! Shared observation vocabulary coverage: validated construction and bounds
//! for every observation variant, typed rejection of unknown provider values,
//! approval/question state rules, reasoning settle without deltas, subagent
//! transcript projection, usage basis with the context-gauge rule, and
//! `AE-*` error references.

use artisan_domain::{
    AgentMessageCompletedObservation, AgentMessageDeltaObservation, ApprovalObservation,
    ApprovalRequest, ApprovalState, ArtisanCode, CompactionObservation, CompactionState,
    DiagnosticLevel, EngineErrorRef, EngineErrorRefInput, FileAction, FileObservation,
    IdentifierError, LimitScope, MessagePhase, NativeActionObservation, OBSERVATION_ANSWERS_MAX,
    OBSERVATION_DELTA_MAX_BYTES, OBSERVATION_ID_MAX_BYTES, OBSERVATION_MESSAGE_MAX_BYTES,
    OBSERVATION_PLAN_MAX_ENTRIES, OBSERVATION_QUESTION_MAX_OPTIONS, Observation, ObservationError,
    ObservationId, ObservationSequence, PlanEntry, PlanEntryStatus, PlanObservation,
    ProcessDiagnosticObservation, ProtocolDiagnosticObservation, QuestionInput,
    QuestionObservation, QuestionOption, QuestionState, ReasoningSummaryCompletedObservation,
    ReasoningSummaryDeltaObservation, RetryAttemptState, RetryObservation, RunState,
    RunStateObservation, RunTerminalObservation, RunTerminalState, SearchObservation, SearchScope,
    SearchState, SubagentInput, SubagentObservation, SubagentState, TerminalActivityInput,
    TerminalActivityObservation, TerminalActivityState, TerminalChannel, ToolAction,
    ToolObservation, TranscriptContent, TurnState, TurnStateObservation, UsageBasis, UsageInput,
    UsageObservation,
};

fn oid(value: &str) -> ObservationId {
    ObservationId::parse(value.to_owned()).expect("fixture identity is valid")
}

fn seq(value: u64) -> ObservationSequence {
    ObservationSequence::new(value).expect("fixture sequence is valid")
}

fn delta_observation(sequence: u64) -> Observation {
    Observation::AgentMessageDelta(
        AgentMessageDeltaObservation::new(
            oid("obs-delta-1"),
            seq(sequence),
            oid("item-1"),
            MessagePhase::Commentary,
            "partial text".to_owned(),
            oid("turn-1"),
        )
        .expect("fixture delta is valid"),
    )
}

fn error_ref() -> EngineErrorRef {
    EngineErrorRef::new(EngineErrorRefInput {
        artisan_code: ArtisanCode::parse("AE-PROVIDER-206".to_owned())
            .expect("fixture artisan code is valid"),
        provider_code: Some("quota_exhausted".to_owned()),
        detail: Some("model allowance depleted".to_owned()),
        affected_model_id: Some("model-1".to_owned()),
        limit_id: Some("weekly".to_owned()),
        limit_label: Some("Weekly allowance".to_owned()),
        limit_scope: Some(LimitScope::Model),
        resets_at: Some("2026-09-06T00:00:00Z".to_owned()),
    })
    .expect("fixture error reference is valid")
}

#[test]
fn message_phases_preserve_provider_disclosure_verbatim() {
    assert_eq!(
        MessagePhase::parse("commentary"),
        Ok(MessagePhase::Commentary)
    );
    assert_eq!(MessagePhase::parse("final"), Ok(MessagePhase::Final));
    assert_eq!(
        MessagePhase::parse("unspecified"),
        Ok(MessagePhase::Unspecified)
    );
    assert_eq!(
        MessagePhase::parse(" COMMENTARY "),
        Err(ObservationError::UnknownValue { field: "phase" })
    );
    assert_eq!(
        MessagePhase::parse(""),
        Err(ObservationError::UnknownValue { field: "phase" })
    );
    assert_eq!(MessagePhase::Commentary.as_str(), "commentary");
    assert_eq!(MessagePhase::Final.as_str(), "final");
    assert_eq!(MessagePhase::Unspecified.as_str(), "unspecified");
}

#[test]
fn agent_message_delta_bounds() {
    for phase in [
        MessagePhase::Commentary,
        MessagePhase::Final,
        MessagePhase::Unspecified,
    ] {
        let value = AgentMessageDeltaObservation::new(
            oid("obs-1"),
            seq(1),
            oid("item-1"),
            phase,
            "delta".to_owned(),
            oid("turn-1"),
        )
        .expect("phase is preserved verbatim");
        assert_eq!(value.phase(), phase);
        assert_eq!(value.delta(), "delta");
    }

    assert_eq!(
        AgentMessageDeltaObservation::new(
            oid("obs-1"),
            seq(1),
            oid("item-1"),
            MessagePhase::Commentary,
            String::new(),
            oid("turn-1"),
        )
        .expect_err("empty deltas carry no information"),
        ObservationError::Empty { field: "delta" }
    );
    let oversize = "x".repeat(OBSERVATION_DELTA_MAX_BYTES + 1);
    assert_eq!(
        AgentMessageDeltaObservation::new(
            oid("obs-1"),
            seq(1),
            oid("item-1"),
            MessagePhase::Commentary,
            oversize.clone(),
            oid("turn-1"),
        )
        .expect_err("deltas stay within the fragment ceiling"),
        ObservationError::TooLong {
            field: "delta",
            length: oversize.len(),
            maximum: OBSERVATION_DELTA_MAX_BYTES,
        }
    );
}

#[test]
fn agent_message_completed_allows_empty_but_bounds_length() {
    let empty = AgentMessageCompletedObservation::new(
        oid("obs-1"),
        seq(1),
        oid("item-1"),
        MessagePhase::Final,
        String::new(),
        oid("turn-1"),
    )
    .expect("empty settled bodies are valid");
    assert_eq!(empty.message(), "");

    let oversize = "x".repeat(OBSERVATION_MESSAGE_MAX_BYTES + 1);
    assert_eq!(
        AgentMessageCompletedObservation::new(
            oid("obs-1"),
            seq(1),
            oid("item-1"),
            MessagePhase::Final,
            oversize.clone(),
            oid("turn-1"),
        )
        .expect_err("completed bodies stay within the body ceiling"),
        ObservationError::TooLong {
            field: "message",
            length: oversize.len(),
            maximum: OBSERVATION_MESSAGE_MAX_BYTES,
        }
    );
}

#[test]
fn reasoning_summary_delta_bounds() {
    let value = ReasoningSummaryDeltaObservation::new(
        oid("obs-1"),
        seq(1),
        oid("item-1"),
        3,
        "thinking aloud".to_owned(),
        Some(128),
        oid("turn-1"),
    )
    .expect("fixture reasoning delta is valid");
    assert_eq!(value.summary_index(), 3);
    assert_eq!(value.thinking_tokens(), Some(128));

    assert_eq!(
        ReasoningSummaryDeltaObservation::new(
            oid("obs-1"),
            seq(1),
            oid("item-1"),
            u64::MAX,
            "thinking aloud".to_owned(),
            None,
            oid("turn-1"),
        )
        .expect_err("summary indexes stay within the SQLite range"),
        ObservationError::OutOfRange {
            field: "summary_index"
        }
    );
}

#[test]
fn reasoning_completed_without_delta_settles() {
    let settled = ReasoningSummaryCompletedObservation::new(
        oid("obs-1"),
        seq(7),
        oid("item-1"),
        None,
        oid("turn-1"),
    )
    .expect("complete-without-delta settles the phase");
    assert_eq!(settled.text(), None);

    let replaced = ReasoningSummaryCompletedObservation::new(
        oid("obs-2"),
        seq(8),
        oid("item-1"),
        Some("authoritative summary".to_owned()),
        oid("turn-1"),
    )
    .expect("supplied text replaces streamed deltas");
    assert_eq!(replaced.text(), Some("authoritative summary"));
}

#[test]
fn tool_file_search_terminal_rows_validate() {
    let tool = ToolObservation::new(
        oid("obs-1"),
        seq(1),
        oid("tool-1"),
        "bash".to_owned(),
        ToolAction::Started,
        Some("running tests".to_owned()),
    )
    .expect("fixture tool row is valid");
    assert_eq!(tool.action(), ToolAction::Started);
    assert_eq!(
        ToolAction::parse("bogus"),
        Err(ObservationError::UnknownValue { field: "action" })
    );

    let file = FileObservation::new(
        oid("obs-2"),
        seq(2),
        "src/main.rs".to_owned(),
        FileAction::Modified,
        Some(12),
        None,
    )
    .expect("fixture file row is valid");
    assert_eq!(file.lines_added(), Some(12));
    assert_eq!(
        file.lines_deleted(),
        None,
        "absent counts stay uncounted, never zero"
    );
    assert_eq!(
        FileObservation::new(
            oid("obs-2"),
            seq(2),
            "src/main.rs".to_owned(),
            FileAction::Read,
            Some(u64::MAX),
            None,
        )
        .expect_err("counts stay within the SQLite range"),
        ObservationError::OutOfRange {
            field: "lines_added"
        }
    );
    assert_eq!(
        FileAction::parse("renamed"),
        Err(ObservationError::UnknownValue { field: "action" })
    );

    let search = SearchObservation::new(
        oid("obs-3"),
        seq(3),
        "observation codec".to_owned(),
        None,
        Some(oid("search-1")),
        SearchState::Completed,
        Some(4),
    )
    .expect("fixture search row is valid");
    assert_eq!(
        search.scope(),
        None,
        "absent scopes stay absent instead of being imputed"
    );
    assert_eq!(
        SearchObservation::new(
            oid("obs-3"),
            seq(3),
            String::new(),
            Some(SearchScope::Workspace),
            None,
            SearchState::Started,
            None,
        )
        .expect_err("queries are non-empty"),
        ObservationError::Empty { field: "query" }
    );
    assert_eq!(
        SearchScope::parse("everywhere"),
        Err(ObservationError::UnknownValue { field: "scope" })
    );
    assert_eq!(
        SearchState::parse("paused"),
        Err(ObservationError::UnknownValue { field: "state" })
    );

    terminal_activity_rows_validate();
}

fn terminal_activity_rows_validate() {
    let terminal = TerminalActivityObservation::new(
        oid("obs-4"),
        seq(4),
        TerminalActivityInput {
            activity_id: oid("activity-1"),
            channel: Some(TerminalChannel::Stdout),
            command: Some("cargo test".to_owned()),
            shell: Some("bash".to_owned()),
            output: Some("ok".to_owned()),
            exit_code: Some(0),
            state: TerminalActivityState::Completed,
        },
    )
    .expect("fixture terminal row is valid");
    assert_eq!(terminal.exit_code(), Some(0));
    assert_eq!(
        TerminalChannel::parse("stdin"),
        Err(ObservationError::UnknownValue { field: "channel" })
    );
    assert_eq!(
        TerminalActivityState::parse("stalled"),
        Err(ObservationError::UnknownValue { field: "state" })
    );
}

#[test]
fn approval_requested_has_no_decision_and_resolved_carries_one() {
    let request = ApprovalRequest::command(
        "rm -rf /tmp/work".to_owned(),
        Some("/tmp/work".to_owned()),
        Some("cleanup".to_owned()),
    )
    .expect("fixture command request is valid");
    let requested = ApprovalObservation::requested(
        oid("obs-1"),
        seq(1),
        oid("approval-1"),
        "Remove the scratch directory?".to_owned(),
        request,
    )
    .expect("fixture approval request is valid");
    assert_eq!(requested.state(), ApprovalState::Requested);
    assert_eq!(requested.approved(), None);

    let request = ApprovalRequest::file_change(Some("apply edits".to_owned()))
        .expect("fixture file-change request is valid");
    let resolved = ApprovalObservation::resolved(
        oid("obs-2"),
        seq(2),
        oid("approval-1"),
        "Apply the edits?".to_owned(),
        request,
        true,
    )
    .expect("fixture approval resolution is valid");
    assert_eq!(resolved.state(), ApprovalState::Resolved);
    assert_eq!(resolved.approved(), Some(true));

    assert_eq!(
        ApprovalRequest::command(String::new(), None, None)
            .expect_err("commands under review are non-empty"),
        ObservationError::Empty { field: "command" }
    );
    assert_eq!(
        ApprovalState::parse("pending"),
        Err(ObservationError::UnknownValue { field: "state" })
    );
}

#[test]
fn question_requested_and_resolved_follow_s0_shapes() {
    let input = QuestionInput {
        question_id: oid("question-1"),
        text: "Which runtime should run first?".to_owned(),
        header: Some("Runtime".to_owned()),
        multi_select: false,
        options: Some(vec![
            QuestionOption::new(
                "OpenCode".to_owned(),
                Some("Use the certified runtime".to_owned()),
            )
            .expect("fixture option is valid"),
            QuestionOption::new("Codex".to_owned(), None).expect("fixture option is valid"),
        ]),
    };
    let requested = QuestionObservation::requested(oid("obs-1"), seq(1), input.clone())
        .expect("fixture question request is valid");
    assert_eq!(requested.state(), QuestionState::Requested);
    assert_eq!(requested.answers(), None);
    assert_eq!(requested.options().expect("options are present").len(), 2);

    let free_form = QuestionInput {
        question_id: oid("question-2"),
        text: "Describe the failure.".to_owned(),
        header: None,
        multi_select: false,
        options: None,
    };
    let resolved = QuestionObservation::resolved(
        oid("obs-2"),
        seq(2),
        free_form,
        vec!["it timed out".to_owned()],
    )
    .expect("free-form questions resolve with typed text");
    assert_eq!(
        resolved.answers().expect("answers are present"),
        &vec!["it timed out".to_owned()]
    );

    let many: Vec<QuestionOption> = (0..=OBSERVATION_QUESTION_MAX_OPTIONS)
        .map(|index| QuestionOption::new(format!("option-{index}"), None).expect("option is valid"))
        .collect();
    assert_eq!(
        QuestionObservation::requested(
            oid("obs-3"),
            seq(3),
            QuestionInput {
                question_id: oid("question-3"),
                text: "Pick one.".to_owned(),
                header: None,
                multi_select: true,
                options: Some(many),
            },
        )
        .expect_err("options stay bounded"),
        ObservationError::TooMany {
            field: "options",
            count: OBSERVATION_QUESTION_MAX_OPTIONS + 1,
            maximum: OBSERVATION_QUESTION_MAX_OPTIONS,
        }
    );
    let answers = vec!["x".to_owned(); OBSERVATION_ANSWERS_MAX + 1];
    assert_eq!(
        QuestionObservation::resolved(
            oid("obs-4"),
            seq(4),
            QuestionInput {
                question_id: oid("question-4"),
                text: "Pick any.".to_owned(),
                header: None,
                multi_select: true,
                options: None,
            },
            answers.clone(),
        )
        .expect_err("answers stay bounded"),
        ObservationError::TooMany {
            field: "answers",
            count: answers.len(),
            maximum: OBSERVATION_ANSWERS_MAX,
        }
    );
    assert_eq!(
        QuestionState::parse("answered"),
        Err(ObservationError::UnknownValue { field: "state" })
    );
}

#[test]
fn plan_compaction_retry_run_and_turn_states_validate() {
    let entries = vec![
        PlanEntry::new(
            oid("step-1"),
            PlanEntryStatus::Completed,
            "Inventory".to_owned(),
        )
        .expect("fixture entry is valid"),
        PlanEntry::new(
            oid("step-2"),
            PlanEntryStatus::InProgress,
            "Implement".to_owned(),
        )
        .expect("fixture entry is valid"),
    ];
    let plan = PlanObservation::new(oid("obs-1"), seq(1), entries, Some(oid("turn-1")))
        .expect("fixture plan is valid");
    assert_eq!(plan.entries().len(), 2);
    assert_eq!(
        PlanObservation::new(oid("obs-1"), seq(1), Vec::new(), None)
            .expect_err("plans carry at least one entry"),
        ObservationError::Empty { field: "entries" }
    );
    let crowded: Vec<PlanEntry> = (0..=OBSERVATION_PLAN_MAX_ENTRIES)
        .map(|index| {
            PlanEntry::new(
                oid(&format!("step-{index}")),
                PlanEntryStatus::Pending,
                "work".to_owned(),
            )
            .expect("entry is valid")
        })
        .collect();
    assert_eq!(
        PlanObservation::new(oid("obs-1"), seq(1), crowded.clone(), None)
            .expect_err("plans stay bounded"),
        ObservationError::TooMany {
            field: "entries",
            count: crowded.len(),
            maximum: OBSERVATION_PLAN_MAX_ENTRIES,
        }
    );
    assert_eq!(
        PlanEntryStatus::parse("done"),
        Err(ObservationError::UnknownValue { field: "status" })
    );

    let compaction = CompactionObservation::new(
        oid("obs-2"),
        seq(2),
        CompactionState::Completed,
        Some(oid("compaction-1")),
        Some(1_250),
        Some("summarized".to_owned()),
    )
    .expect("fixture compaction is valid");
    assert_eq!(compaction.duration_ms(), Some(1_250));
    assert_eq!(
        CompactionState::parse("compacting"),
        Err(ObservationError::UnknownValue { field: "state" })
    );

    let retry = RetryObservation::new(
        oid("obs-3"),
        seq(3),
        oid("turn-1"),
        RetryAttemptState::Retrying,
        true,
        "upstream hiccup".to_owned(),
    )
    .expect("fixture retry is valid");
    assert!(retry.will_retry());
    assert_eq!(
        RetryAttemptState::parse("gave_up"),
        Err(ObservationError::UnknownValue {
            field: "attempt_state"
        })
    );

    assert_eq!(
        RunState::parse("paused"),
        Err(ObservationError::UnknownValue { field: "state" })
    );
    assert_eq!(
        TurnState::parse("paused"),
        Err(ObservationError::UnknownValue { field: "state" })
    );
    let run_state = RunStateObservation::new(oid("obs-4"), seq(4), RunState::Waiting);
    assert_eq!(run_state.state(), RunState::Waiting);
    let turn_state =
        TurnStateObservation::new(oid("obs-5"), seq(5), oid("turn-1"), TurnState::Started);
    assert_eq!(turn_state.state(), TurnState::Started);
}

#[test]
fn subagent_lifecycle_and_transcript_projection() {
    let lifecycle = SubagentObservation::new(
        oid("obs-1"),
        seq(1),
        SubagentInput {
            agent_native_thread_id: oid("child-1"),
            parent_native_thread_id: oid("parent-1"),
            state: SubagentState::Running,
            activity: Some("researching".to_owned()),
            agent_path: None,
            turn_id: None,
        },
    )
    .expect("fixture subagent row is valid");
    assert_eq!(lifecycle.state(), SubagentState::Running);
    assert_eq!(
        SubagentState::parse("sleeping"),
        Err(ObservationError::UnknownValue { field: "state" })
    );

    let projected =
        TranscriptContent::project(&delta_observation(9)).expect("deltas project into transcripts");
    assert_eq!(projected.tag(), "agent_message_delta");

    for observation in [
        Observation::Approval(
            ApprovalObservation::requested(
                oid("obs-a"),
                seq(10),
                oid("approval-1"),
                "Approve?".to_owned(),
                ApprovalRequest::action(None).expect("action request is valid"),
            )
            .expect("approval is valid"),
        ),
        Observation::Question(
            QuestionObservation::requested(
                oid("obs-q"),
                seq(11),
                QuestionInput {
                    question_id: oid("question-1"),
                    text: "Which one?".to_owned(),
                    header: None,
                    multi_select: false,
                    options: None,
                },
            )
            .expect("question is valid"),
        ),
        Observation::RunTerminal(
            RunTerminalObservation::new(
                oid("obs-t"),
                seq(12),
                RunTerminalState::Completed,
                None,
                None,
            )
            .expect("terminal outcome is valid"),
        ),
        Observation::Usage(
            UsageObservation::new(
                oid("obs-u"),
                seq(13),
                UsageInput {
                    basis: UsageBasis::Delta,
                    input_tokens: Some(10),
                    cached_input_tokens: None,
                    output_tokens: Some(4),
                    context_tokens: None,
                    context_window_tokens: None,
                    cost_usd: None,
                    provider_route_id: None,
                    turn_id: None,
                },
            )
            .expect("usage is valid"),
        ),
    ] {
        assert_eq!(
            TranscriptContent::project(&observation).expect_err("only renderer-safe kinds project"),
            ObservationError::NotProjectable {
                kind: observation.tag()
            }
        );
        assert!(!observation.is_child());
    }

    let child = Observation::Subagent(lifecycle);
    assert!(child.is_child());
    assert_eq!(
        TranscriptContent::project(&child).expect_err("lifecycle rows never project"),
        ObservationError::NotProjectable { kind: "subagent" }
    );
    assert!(!delta_observation(9).is_child());
}

#[test]
fn usage_basis_is_preserved_and_context_stays_a_gauge() {
    for basis in [
        UsageBasis::Delta,
        UsageBasis::Cumulative,
        UsageBasis::Unknown,
    ] {
        let value = UsageObservation::new(
            oid("obs-1"),
            seq(1),
            UsageInput {
                basis,
                input_tokens: Some(0),
                cached_input_tokens: None,
                output_tokens: Some(4),
                context_tokens: Some(9_000),
                context_window_tokens: Some(200_000),
                cost_usd: Some(0.02),
                provider_route_id: Some(oid("route-1")),
                turn_id: Some(oid("turn-1")),
            },
        )
        .expect("fixture usage is valid");
        assert_eq!(value.basis(), basis);
        assert_eq!(
            value.input_tokens(),
            Some(0),
            "zero stays measured, never absent"
        );
        assert_eq!(
            value.context_tokens(),
            Some(9_000),
            "the gauge value is preserved verbatim; consumers replace, never sum"
        );
    }
    assert_eq!(
        UsageBasis::parse("running_total"),
        Err(ObservationError::UnknownValue { field: "basis" })
    );
    assert_eq!(
        UsageObservation::new(
            oid("obs-1"),
            seq(1),
            UsageInput {
                basis: UsageBasis::Cumulative,
                input_tokens: None,
                cached_input_tokens: None,
                output_tokens: None,
                context_tokens: None,
                context_window_tokens: Some(0),
                cost_usd: None,
                provider_route_id: None,
                turn_id: None,
            },
        )
        .expect_err("a zero context window is meaningless"),
        ObservationError::OutOfRange {
            field: "context_window_tokens"
        }
    );
    assert_eq!(
        UsageObservation::new(
            oid("obs-1"),
            seq(1),
            UsageInput {
                basis: UsageBasis::Delta,
                input_tokens: Some(u64::MAX),
                cached_input_tokens: None,
                output_tokens: None,
                context_tokens: None,
                context_window_tokens: None,
                cost_usd: None,
                provider_route_id: None,
                turn_id: None,
            },
        )
        .expect_err("token counts stay within the SQLite range"),
        ObservationError::OutOfRange {
            field: "input_tokens"
        }
    );
    for bad_cost in [f64::NAN, f64::INFINITY, -0.5] {
        assert_eq!(
            UsageObservation::new(
                oid("obs-1"),
                seq(1),
                UsageInput {
                    basis: UsageBasis::Delta,
                    input_tokens: None,
                    cached_input_tokens: None,
                    output_tokens: None,
                    context_tokens: None,
                    context_window_tokens: None,
                    cost_usd: Some(bad_cost),
                    provider_route_id: None,
                    turn_id: None,
                },
            )
            .expect_err("costs are finite and non-negative"),
            ObservationError::OutOfRange { field: "cost_usd" }
        );
    }
}

#[test]
fn native_action_diagnostic_flag_and_diagnostics_validate() {
    let drift = NativeActionObservation::new(
        oid("obs-1"),
        seq(1),
        "provider.frame.unknown".to_owned(),
        None,
        true,
        None,
    )
    .expect("diagnostic drift rows are valid");
    assert!(drift.diagnostic());
    assert_eq!(drift.error_ref(), None);

    let classified = NativeActionObservation::new(
        oid("obs-2"),
        seq(2),
        "provider.deploy".to_owned(),
        Some("rolled out".to_owned()),
        false,
        Some(error_ref()),
    )
    .expect("classified actions carry their error reference");
    assert!(!classified.diagnostic());
    assert_eq!(
        classified
            .error_ref()
            .expect("error reference is present")
            .artisan_code()
            .as_str(),
        "AE-PROVIDER-206"
    );
    assert_eq!(
        NativeActionObservation::new(oid("obs-2"), seq(2), String::new(), None, false, None,)
            .expect_err("action names are non-empty"),
        ObservationError::Empty { field: "action" }
    );

    let process = ProcessDiagnosticObservation::new(
        oid("obs-3"),
        seq(3),
        DiagnosticLevel::Error,
        "child exited".to_owned(),
        Some(error_ref()),
    )
    .expect("fixture process diagnostic is valid");
    assert_eq!(process.level(), DiagnosticLevel::Error);

    let protocol = ProtocolDiagnosticObservation::new(
        oid("obs-4"),
        seq(4),
        DiagnosticLevel::Warning,
        "unknown frame ignored".to_owned(),
    )
    .expect("fixture protocol diagnostic is valid");
    assert_eq!(protocol.level(), DiagnosticLevel::Warning);
    assert_eq!(
        DiagnosticLevel::parse("fatal"),
        Err(ObservationError::UnknownValue { field: "level" })
    );
}

#[test]
fn error_references_require_ae_codes_and_keep_provider_evidence() {
    let code = ArtisanCode::parse("AE-PROVIDER-206".to_owned()).expect("code is valid");
    assert_eq!(code.as_str(), "AE-PROVIDER-206");
    for bad in ["PROVIDER-206", "ae-provider-206", "AE-", "AE-has space", ""] {
        assert_eq!(
            ArtisanCode::parse(bad.to_owned()).expect_err("codes are AE-* shaped"),
            ObservationError::UnknownValue {
                field: "artisan_code"
            }
        );
    }

    let value = error_ref();
    assert_eq!(value.provider_code(), Some("quota_exhausted"));
    assert_eq!(value.limit_scope(), Some(LimitScope::Model));
    assert_eq!(value.resets_at(), Some("2026-09-06T00:00:00Z"));
    assert_eq!(
        LimitScope::parse("per_user"),
        Err(ObservationError::UnknownValue {
            field: "limit_scope"
        })
    );
    assert_eq!(
        EngineErrorRef::new(EngineErrorRefInput {
            artisan_code: ArtisanCode::parse("AE-PROVIDER-206".to_owned()).expect("code is valid"),
            provider_code: None,
            detail: None,
            affected_model_id: None,
            limit_id: None,
            limit_label: None,
            limit_scope: None,
            resets_at: Some("not-a-timestamp\n".to_owned()),
        })
        .expect_err("timestamps carry no whitespace"),
        ObservationError::Identifier(IdentifierError::ForbiddenCharacter { character: '\n' })
    );
}

#[test]
fn run_terminal_states_include_closed_without_collapsing_interruption() {
    for state in [
        RunTerminalState::Completed,
        RunTerminalState::Cancelled,
        RunTerminalState::Failed,
        RunTerminalState::Interrupted,
        RunTerminalState::Closed,
    ] {
        let value = RunTerminalObservation::new(
            oid("obs-1"),
            seq(1),
            state,
            None,
            Some("Session title".to_owned()),
        )
        .expect("terminal state is valid");
        assert_eq!(value.state(), state);
    }
    assert_eq!(
        RunTerminalState::parse("done"),
        Err(ObservationError::UnknownValue { field: "state" })
    );
}

#[test]
fn observation_sequences_and_identities_stay_bounded() {
    assert_eq!(
        ObservationSequence::new(u64::MAX).expect_err("sequences fit SQLite"),
        ObservationError::OutOfRange { field: "sequence" }
    );
    assert_eq!(
        ObservationId::parse(String::new()).expect_err("identities are non-empty"),
        IdentifierError::Empty
    );
    assert_eq!(
        ObservationId::parse("has space").expect_err("identities carry no whitespace"),
        IdentifierError::ForbiddenCharacter { character: ' ' }
    );
    let oversize = "x".repeat(OBSERVATION_ID_MAX_BYTES + 1);
    assert_eq!(
        ObservationId::parse(oversize.clone()).expect_err("identities stay bounded"),
        IdentifierError::TooLong {
            length: oversize.len(),
            maximum: OBSERVATION_ID_MAX_BYTES,
        }
    );
}

#[test]
fn observation_tags_match_the_typescript_union() {
    let cases = [
        (
            delta_observation(1),
            "agent_message_delta",
            "obs-delta-1",
            1,
        ),
        (
            Observation::TurnState(TurnStateObservation::new(
                oid("obs"),
                seq(2),
                oid("turn-1"),
                TurnState::Completed,
            )),
            "turn_state",
            "obs",
            2,
        ),
        (
            Observation::RunState(RunStateObservation::new(
                oid("obs"),
                seq(3),
                RunState::Running,
            )),
            "run_state",
            "obs",
            3,
        ),
    ];
    for (observation, tag, id, sequence) in cases {
        assert_eq!(observation.tag(), tag);
        assert_eq!(observation.observation_id().as_str(), id);
        assert_eq!(observation.sequence().get(), sequence);
        assert!(!observation.is_child());
    }
}
