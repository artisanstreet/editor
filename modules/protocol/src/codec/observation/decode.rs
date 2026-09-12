//! Engine-observation and transcript decode conversion.

#[allow(clippy::wildcard_imports)]
use super::*;

#[expect(
    clippy::too_many_lines,
    reason = "central observation union dispatcher mirroring `encode_engine_observation`"
)]
pub(crate) fn decode_engine_observation(
    value: artisan_capnp::engine_observation::Reader<'_>,
) -> Result<Observation, ProtocolDecodeError> {
    match value.which()? {
        artisan_capnp::engine_observation::Which::AgentMessageDelta(observation) => {
            let observation = observation?;
            Ok(Observation::AgentMessageDelta(
                AgentMessageDeltaObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.agentMessageDelta.id",
                        )?,
                        "event.engineObservation.agentMessageDelta.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    parse_observation_id(
                        read_text(
                            observation.get_item_id(),
                            "event.engineObservation.agentMessageDelta.itemId",
                        )?,
                        "event.engineObservation.agentMessageDelta.itemId",
                    )?,
                    decode_observation_message_phase(observation.get_phase()?),
                    read_text(
                        observation.get_delta(),
                        "event.engineObservation.agentMessageDelta.delta",
                    )?,
                    parse_observation_id(
                        read_text(
                            observation.get_turn_id(),
                            "event.engineObservation.agentMessageDelta.turnId",
                        )?,
                        "event.engineObservation.agentMessageDelta.turnId",
                    )?,
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::AgentMessageCompleted(observation) => {
            let observation = observation?;
            Ok(Observation::AgentMessageCompleted(
                AgentMessageCompletedObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.agentMessageCompleted.id",
                        )?,
                        "event.engineObservation.agentMessageCompleted.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    parse_observation_id(
                        read_text(
                            observation.get_item_id(),
                            "event.engineObservation.agentMessageCompleted.itemId",
                        )?,
                        "event.engineObservation.agentMessageCompleted.itemId",
                    )?,
                    decode_observation_message_phase(observation.get_phase()?),
                    read_text(
                        observation.get_message(),
                        "event.engineObservation.agentMessageCompleted.message",
                    )?,
                    parse_observation_id(
                        read_text(
                            observation.get_turn_id(),
                            "event.engineObservation.agentMessageCompleted.turnId",
                        )?,
                        "event.engineObservation.agentMessageCompleted.turnId",
                    )?,
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::Approval(observation) => {
            let observation = observation?;
            let id = parse_observation_id(
                read_text(observation.get_id(), "event.engineObservation.approval.id")?,
                "event.engineObservation.approval.id",
            )?;
            let sequence = parse_observation_sequence(observation.get_sequence())?;
            let approval_id = parse_observation_id(
                read_text(
                    observation.get_approval_id(),
                    "event.engineObservation.approval.approvalId",
                )?,
                "event.engineObservation.approval.approvalId",
            )?;
            let state = decode_observation_approval_state(observation.get_state()?);
            let description = read_text(
                observation.get_description(),
                "event.engineObservation.approval.description",
            )?;
            let request = decode_observation_approval_request(observation.get_request()?)?;
            let decision = match observation.get_decision().which()? {
                artisan_capnp::observation_approval::decision::Which::NoDecision(()) => None,
                artisan_capnp::observation_approval::decision::Which::Decision(decision) => {
                    Some(decision)
                }
            };
            match (state, decision) {
                (ApprovalState::Requested, None) => {
                    Ok(Observation::Approval(ApprovalObservation::requested(
                        id,
                        sequence,
                        approval_id,
                        description,
                        request,
                    )?))
                }
                (ApprovalState::Resolved, Some(approved)) => {
                    Ok(Observation::Approval(ApprovalObservation::resolved(
                        id,
                        sequence,
                        approval_id,
                        description,
                        request,
                        approved,
                    )?))
                }
                (ApprovalState::Requested, Some(_)) => {
                    Err(ObservationError::UnexpectedField { field: "decision" }.into())
                }
                (ApprovalState::Resolved, None) => {
                    Err(ObservationError::MissingField { field: "decision" }.into())
                }
            }
        }
        artisan_capnp::engine_observation::Which::Compaction(observation) => {
            let observation = observation?;
            Ok(Observation::Compaction(CompactionObservation::new(
                parse_observation_id(
                    read_text(
                        observation.get_id(),
                        "event.engineObservation.compaction.id",
                    )?,
                    "event.engineObservation.compaction.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                decode_observation_compaction_state(observation.get_state()?),
                parse_optional_observation_id(
                    read_text(
                        observation.get_compaction_id(),
                        "event.engineObservation.compaction.compactionId",
                    )?,
                    "event.engineObservation.compaction.compactionId",
                )?,
                match observation.get_duration_ms().which()? {
                    artisan_capnp::observation_compaction::duration_ms::Which::NoDurationMs(()) => {
                        None
                    }
                    artisan_capnp::observation_compaction::duration_ms::Which::DurationMs(
                        duration,
                    ) => Some(duration),
                },
                absent_if_empty(read_text(
                    observation.get_summary(),
                    "event.engineObservation.compaction.summary",
                )?),
            )?))
        }
        artisan_capnp::engine_observation::Which::File(observation) => {
            let observation = observation?;
            Ok(Observation::File(FileObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.file.id")?,
                    "event.engineObservation.file.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                read_text(observation.get_path(), "event.engineObservation.file.path")?,
                decode_observation_file_action(observation.get_action()?),
                match observation.get_lines_added().which()? {
                    artisan_capnp::observation_file::lines_added::Which::NoLinesAdded(()) => None,
                    artisan_capnp::observation_file::lines_added::Which::LinesAdded(count) => {
                        Some(count)
                    }
                },
                match observation.get_lines_deleted().which()? {
                    artisan_capnp::observation_file::lines_deleted::Which::NoLinesDeleted(()) => {
                        None
                    }
                    artisan_capnp::observation_file::lines_deleted::Which::LinesDeleted(count) => {
                        Some(count)
                    }
                },
            )?))
        }
        artisan_capnp::engine_observation::Which::NativeAction(observation) => {
            let observation = observation?;
            Ok(Observation::NativeAction(NativeActionObservation::new(
                parse_observation_id(
                    read_text(
                        observation.get_id(),
                        "event.engineObservation.nativeAction.id",
                    )?,
                    "event.engineObservation.nativeAction.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                read_text(
                    observation.get_action(),
                    "event.engineObservation.nativeAction.action",
                )?,
                absent_if_empty(read_text(
                    observation.get_detail(),
                    "event.engineObservation.nativeAction.detail",
                )?),
                observation.get_diagnostic(),
                match observation.get_error_ref().which()? {
                    artisan_capnp::observation_native_action::error_ref::Which::NoErrorRef(()) => {
                        None
                    }
                    artisan_capnp::observation_native_action::error_ref::Which::ErrorRef(error) => {
                        Some(decode_observation_engine_error_ref(
                            error?,
                            "event.engineObservation.nativeAction.errorRef",
                        )?)
                    }
                },
            )?))
        }
        artisan_capnp::engine_observation::Which::Plan(observation) => {
            let observation = observation?;
            let encoded_entries = observation.get_entries()?;
            let entry_count = encoded_entries.len() as usize;
            if entry_count > OBSERVATION_PLAN_MAX_ENTRIES {
                return Err(ProtocolDecodeError::Observation {
                    source: ObservationError::TooMany {
                        field: "entries",
                        count: entry_count,
                        maximum: OBSERVATION_PLAN_MAX_ENTRIES,
                    },
                });
            }
            let mut entries = Vec::with_capacity(entry_count);
            for encoded_entry in encoded_entries {
                entries.push(PlanEntry::new(
                    parse_observation_id(
                        read_text(
                            encoded_entry.get_id(),
                            "event.engineObservation.plan.entries.id",
                        )?,
                        "event.engineObservation.plan.entries.id",
                    )?,
                    decode_observation_plan_entry_status(encoded_entry.get_status()?),
                    read_text(
                        encoded_entry.get_text(),
                        "event.engineObservation.plan.entries.text",
                    )?,
                )?);
            }
            Ok(Observation::Plan(PlanObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.plan.id")?,
                    "event.engineObservation.plan.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                entries,
                parse_optional_observation_id(
                    read_text(
                        observation.get_turn_id(),
                        "event.engineObservation.plan.turnId",
                    )?,
                    "event.engineObservation.plan.turnId",
                )?,
            )?))
        }
        artisan_capnp::engine_observation::Which::ProcessDiagnostic(observation) => {
            let observation = observation?;
            Ok(Observation::ProcessDiagnostic(
                ProcessDiagnosticObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.processDiagnostic.id",
                        )?,
                        "event.engineObservation.processDiagnostic.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    decode_observation_diagnostic_level(observation.get_level()?),
                    read_text(
                        observation.get_message(),
                        "event.engineObservation.processDiagnostic.message",
                    )?,
                    match observation.get_error_ref().which()? {
                        artisan_capnp::observation_process_diagnostic::error_ref::Which::NoErrorRef(
                            (),
                        ) => None,
                        artisan_capnp::observation_process_diagnostic::error_ref::Which::ErrorRef(
                            error,
                        ) => Some(decode_observation_engine_error_ref(
                            error?,
                            "event.engineObservation.processDiagnostic.errorRef",
                        )?),
                    },
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::ProtocolDiagnostic(observation) => {
            let observation = observation?;
            Ok(Observation::ProtocolDiagnostic(
                ProtocolDiagnosticObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.protocolDiagnostic.id",
                        )?,
                        "event.engineObservation.protocolDiagnostic.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    decode_observation_diagnostic_level(observation.get_level()?),
                    read_text(
                        observation.get_message(),
                        "event.engineObservation.protocolDiagnostic.message",
                    )?,
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::Question(observation) => {
            let observation = observation?;
            let id = parse_observation_id(
                read_text(observation.get_id(), "event.engineObservation.question.id")?,
                "event.engineObservation.question.id",
            )?;
            let sequence = parse_observation_sequence(observation.get_sequence())?;
            let question_id = parse_observation_id(
                read_text(
                    observation.get_question_id(),
                    "event.engineObservation.question.questionId",
                )?,
                "event.engineObservation.question.questionId",
            )?;
            let state = decode_observation_question_state(observation.get_state()?);
            let text = read_text(
                observation.get_text(),
                "event.engineObservation.question.text",
            )?;
            let header = absent_if_empty(read_text(
                observation.get_header(),
                "event.engineObservation.question.header",
            )?);
            let multi_select = observation.get_multi_select();
            let encoded_options = observation.get_options()?;
            let option_count = encoded_options.len() as usize;
            if option_count > OBSERVATION_QUESTION_MAX_OPTIONS {
                return Err(ProtocolDecodeError::Observation {
                    source: ObservationError::TooMany {
                        field: "options",
                        count: option_count,
                        maximum: OBSERVATION_QUESTION_MAX_OPTIONS,
                    },
                });
            }
            let options = if option_count == 0 {
                None
            } else {
                let mut options = Vec::with_capacity(option_count);
                for encoded_option in encoded_options {
                    options.push(QuestionOption::new(
                        read_text(
                            encoded_option.get_label(),
                            "event.engineObservation.question.options.label",
                        )?,
                        absent_if_empty(read_text(
                            encoded_option.get_description(),
                            "event.engineObservation.question.options.description",
                        )?),
                    )?);
                }
                Some(options)
            };
            let answers = match observation.get_answers().which()? {
                artisan_capnp::observation_question::answers::Which::NoAnswers(()) => None,
                artisan_capnp::observation_question::answers::Which::Answers(encoded) => {
                    let encoded = encoded?;
                    let answer_count = encoded.len() as usize;
                    if answer_count > OBSERVATION_ANSWERS_MAX {
                        return Err(ProtocolDecodeError::Observation {
                            source: ObservationError::TooMany {
                                field: "answers",
                                count: answer_count,
                                maximum: OBSERVATION_ANSWERS_MAX,
                            },
                        });
                    }
                    let mut answers = Vec::with_capacity(answer_count);
                    for answer in encoded {
                        answers.push(read_text(
                            answer,
                            "event.engineObservation.question.answers",
                        )?);
                    }
                    Some(answers)
                }
            };
            let input = QuestionInput {
                question_id,
                text,
                header,
                multi_select,
                options,
            };
            match (state, answers) {
                (QuestionState::Requested, None) => Ok(Observation::Question(
                    QuestionObservation::requested(id, sequence, input)?,
                )),
                (QuestionState::Resolved, Some(answers)) => Ok(Observation::Question(
                    QuestionObservation::resolved(id, sequence, input, answers)?,
                )),
                (QuestionState::Requested, Some(_)) => {
                    Err(ObservationError::UnexpectedField { field: "answers" }.into())
                }
                (QuestionState::Resolved, None) => {
                    Err(ObservationError::MissingField { field: "answers" }.into())
                }
            }
        }
        artisan_capnp::engine_observation::Which::ReasoningSummaryCompleted(observation) => {
            let observation = observation?;
            Ok(Observation::ReasoningSummaryCompleted(
                ReasoningSummaryCompletedObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.reasoningSummaryCompleted.id",
                        )?,
                        "event.engineObservation.reasoningSummaryCompleted.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    parse_observation_id(
                        read_text(
                            observation.get_item_id(),
                            "event.engineObservation.reasoningSummaryCompleted.itemId",
                        )?,
                        "event.engineObservation.reasoningSummaryCompleted.itemId",
                    )?,
                    absent_if_empty(read_text(
                        observation.get_text(),
                        "event.engineObservation.reasoningSummaryCompleted.text",
                    )?),
                    parse_observation_id(
                        read_text(
                            observation.get_turn_id(),
                            "event.engineObservation.reasoningSummaryCompleted.turnId",
                        )?,
                        "event.engineObservation.reasoningSummaryCompleted.turnId",
                    )?,
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::ReasoningSummaryDelta(observation) => {
            let observation = observation?;
            Ok(Observation::ReasoningSummaryDelta(
                ReasoningSummaryDeltaObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.reasoningSummaryDelta.id",
                        )?,
                        "event.engineObservation.reasoningSummaryDelta.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    parse_observation_id(
                        read_text(
                            observation.get_item_id(),
                            "event.engineObservation.reasoningSummaryDelta.itemId",
                        )?,
                        "event.engineObservation.reasoningSummaryDelta.itemId",
                    )?,
                    observation.get_summary_index(),
                    read_text(
                        observation.get_delta(),
                        "event.engineObservation.reasoningSummaryDelta.delta",
                    )?,
                    match observation.get_thinking_tokens().which()? {
                        artisan_capnp::observation_reasoning_summary_delta::thinking_tokens::Which::NoThinkingTokens(
                            (),
                        ) => None,
                        artisan_capnp::observation_reasoning_summary_delta::thinking_tokens::Which::ThinkingTokens(
                            tokens,
                        ) => Some(tokens),
                    },
                    parse_observation_id(
                        read_text(
                            observation.get_turn_id(),
                            "event.engineObservation.reasoningSummaryDelta.turnId",
                        )?,
                        "event.engineObservation.reasoningSummaryDelta.turnId",
                    )?,
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::Retry(observation) => {
            let observation = observation?;
            Ok(Observation::Retry(RetryObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.retry.id")?,
                    "event.engineObservation.retry.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                parse_observation_id(
                    read_text(
                        observation.get_turn_id(),
                        "event.engineObservation.retry.turnId",
                    )?,
                    "event.engineObservation.retry.turnId",
                )?,
                decode_observation_retry_attempt_state(observation.get_attempt_state()?),
                observation.get_will_retry(),
                read_text(
                    observation.get_message(),
                    "event.engineObservation.retry.message",
                )?,
            )?))
        }
        artisan_capnp::engine_observation::Which::RunState(observation) => {
            let observation = observation?;
            Ok(Observation::RunState(RunStateObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.runState.id")?,
                    "event.engineObservation.runState.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                decode_observation_run_state(observation.get_state()?),
            )))
        }
        artisan_capnp::engine_observation::Which::RunTerminal(observation) => {
            let observation = observation?;
            Ok(Observation::RunTerminal(RunTerminalObservation::new(
                parse_observation_id(
                    read_text(
                        observation.get_id(),
                        "event.engineObservation.runTerminal.id",
                    )?,
                    "event.engineObservation.runTerminal.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                decode_observation_run_terminal_state(observation.get_state()?),
                match observation.get_error_ref().which()? {
                    artisan_capnp::observation_run_terminal::error_ref::Which::NoErrorRef(()) => {
                        None
                    }
                    artisan_capnp::observation_run_terminal::error_ref::Which::ErrorRef(error) => {
                        Some(decode_observation_engine_error_ref(
                            error?,
                            "event.engineObservation.runTerminal.errorRef",
                        )?)
                    }
                },
                absent_if_empty(read_text(
                    observation.get_summary_title(),
                    "event.engineObservation.runTerminal.summaryTitle",
                )?),
            )?))
        }
        artisan_capnp::engine_observation::Which::Search(observation) => {
            let observation = observation?;
            Ok(Observation::Search(SearchObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.search.id")?,
                    "event.engineObservation.search.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                read_text(
                    observation.get_query(),
                    "event.engineObservation.search.query",
                )?,
                match observation.get_scope().which()? {
                    artisan_capnp::observation_search::scope::Which::NoScope(()) => None,
                    artisan_capnp::observation_search::scope::Which::Scope(scope) => {
                        Some(decode_observation_search_scope(scope?))
                    }
                },
                parse_optional_observation_id(
                    read_text(
                        observation.get_search_id(),
                        "event.engineObservation.search.searchId",
                    )?,
                    "event.engineObservation.search.searchId",
                )?,
                decode_observation_search_state(observation.get_state()?),
                match observation.get_result_count().which()? {
                    artisan_capnp::observation_search::result_count::Which::NoResultCount(()) => {
                        None
                    }
                    artisan_capnp::observation_search::result_count::Which::ResultCount(count) => {
                        Some(count)
                    }
                },
            )?))
        }
        artisan_capnp::engine_observation::Which::Subagent(observation) => {
            let observation = observation?;
            Ok(Observation::Subagent(SubagentObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.subagent.id")?,
                    "event.engineObservation.subagent.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                SubagentInput {
                    agent_native_thread_id: parse_observation_id(
                        read_text(
                            observation.get_agent_native_thread_id(),
                            "event.engineObservation.subagent.agentNativeThreadId",
                        )?,
                        "event.engineObservation.subagent.agentNativeThreadId",
                    )?,
                    parent_native_thread_id: parse_observation_id(
                        read_text(
                            observation.get_parent_native_thread_id(),
                            "event.engineObservation.subagent.parentNativeThreadId",
                        )?,
                        "event.engineObservation.subagent.parentNativeThreadId",
                    )?,
                    state: decode_observation_subagent_state(observation.get_state()?),
                    activity: absent_if_empty(read_text(
                        observation.get_activity(),
                        "event.engineObservation.subagent.activity",
                    )?),
                    agent_path: absent_if_empty(read_text(
                        observation.get_agent_path(),
                        "event.engineObservation.subagent.agentPath",
                    )?),
                    turn_id: parse_optional_observation_id(
                        read_text(
                            observation.get_turn_id(),
                            "event.engineObservation.subagent.turnId",
                        )?,
                        "event.engineObservation.subagent.turnId",
                    )?,
                },
            )?))
        }
        artisan_capnp::engine_observation::Which::SubagentTranscript(observation) => {
            let observation = observation?;
            Ok(Observation::SubagentTranscript(
                SubagentTranscriptObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.subagentTranscript.id",
                        )?,
                        "event.engineObservation.subagentTranscript.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    parse_observation_id(
                        read_text(
                            observation.get_agent_native_thread_id(),
                            "event.engineObservation.subagentTranscript.agentNativeThreadId",
                        )?,
                        "event.engineObservation.subagentTranscript.agentNativeThreadId",
                    )?,
                    parse_observation_id(
                        read_text(
                            observation.get_parent_native_thread_id(),
                            "event.engineObservation.subagentTranscript.parentNativeThreadId",
                        )?,
                        "event.engineObservation.subagentTranscript.parentNativeThreadId",
                    )?,
                    decode_transcript_content(observation.get_content()?)?,
                ),
            ))
        }
        artisan_capnp::engine_observation::Which::TerminalActivity(observation) => {
            let observation = observation?;
            Ok(Observation::TerminalActivity(
                TerminalActivityObservation::new(
                    parse_observation_id(
                        read_text(
                            observation.get_id(),
                            "event.engineObservation.terminalActivity.id",
                        )?,
                        "event.engineObservation.terminalActivity.id",
                    )?,
                    parse_observation_sequence(observation.get_sequence())?,
                    TerminalActivityInput {
                        activity_id: parse_observation_id(
                            read_text(
                                observation.get_activity_id(),
                                "event.engineObservation.terminalActivity.activityId",
                            )?,
                            "event.engineObservation.terminalActivity.activityId",
                        )?,
                        channel: match observation.get_channel().which()? {
                            artisan_capnp::observation_terminal_activity::channel::Which::NoChannel(
                                (),
                            ) => None,
                            artisan_capnp::observation_terminal_activity::channel::Which::Channel(
                                channel,
                            ) => Some(decode_observation_terminal_channel(channel?)),
                        },
                        command: absent_if_empty(read_text(
                            observation.get_command(),
                            "event.engineObservation.terminalActivity.command",
                        )?),
                        shell: absent_if_empty(read_text(
                            observation.get_shell(),
                            "event.engineObservation.terminalActivity.shell",
                        )?),
                        output: match observation.get_output().which()? {
                            artisan_capnp::observation_terminal_activity::output::Which::NoOutput(
                                (),
                            ) => None,
                            artisan_capnp::observation_terminal_activity::output::Which::Output(
                                output,
                            ) => Some(read_text(
                                output,
                                "event.engineObservation.terminalActivity.output",
                            )?),
                        },
                        exit_code: match observation.get_exit_code().which()? {
                            artisan_capnp::observation_terminal_activity::exit_code::Which::NoExitCode(
                                (),
                            ) => None,
                            artisan_capnp::observation_terminal_activity::exit_code::Which::ExitCode(
                                code,
                            ) => Some(code),
                        },
                        state: decode_observation_terminal_state(observation.get_state()?),
                    },
                )?,
            ))
        }
        artisan_capnp::engine_observation::Which::Tool(observation) => {
            let observation = observation?;
            Ok(Observation::Tool(ToolObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.tool.id")?,
                    "event.engineObservation.tool.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                parse_observation_id(
                    read_text(
                        observation.get_tool_id(),
                        "event.engineObservation.tool.toolId",
                    )?,
                    "event.engineObservation.tool.toolId",
                )?,
                read_text(
                    observation.get_tool_name(),
                    "event.engineObservation.tool.toolName",
                )?,
                decode_observation_tool_action(observation.get_action()?),
                absent_if_empty(read_text(
                    observation.get_detail(),
                    "event.engineObservation.tool.detail",
                )?),
            )?))
        }
        artisan_capnp::engine_observation::Which::TurnState(observation) => {
            let observation = observation?;
            Ok(Observation::TurnState(TurnStateObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.turnState.id")?,
                    "event.engineObservation.turnState.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                parse_observation_id(
                    read_text(
                        observation.get_turn_id(),
                        "event.engineObservation.turnState.turnId",
                    )?,
                    "event.engineObservation.turnState.turnId",
                )?,
                decode_observation_turn_state(observation.get_state()?),
            )))
        }
        artisan_capnp::engine_observation::Which::Usage(observation) => {
            let observation = observation?;
            Ok(Observation::Usage(UsageObservation::new(
                parse_observation_id(
                    read_text(observation.get_id(), "event.engineObservation.usage.id")?,
                    "event.engineObservation.usage.id",
                )?,
                parse_observation_sequence(observation.get_sequence())?,
                UsageInput {
                    basis: decode_observation_usage_basis(observation.get_basis()?),
                    input_tokens: match observation.get_input_tokens().which()? {
                        artisan_capnp::observation_usage::input_tokens::Which::NoInputTokens(()) => {
                            None
                        }
                        artisan_capnp::observation_usage::input_tokens::Which::InputTokens(
                            tokens,
                        ) => Some(tokens),
                    },
                    cached_input_tokens: match observation.get_cached_input_tokens().which()? {
                        artisan_capnp::observation_usage::cached_input_tokens::Which::NoCachedInputTokens(
                            (),
                        ) => None,
                        artisan_capnp::observation_usage::cached_input_tokens::Which::CachedInputTokens(
                            tokens,
                        ) => Some(tokens),
                    },
                    output_tokens: match observation.get_output_tokens().which()? {
                        artisan_capnp::observation_usage::output_tokens::Which::NoOutputTokens(
                            (),
                        ) => None,
                        artisan_capnp::observation_usage::output_tokens::Which::OutputTokens(
                            tokens,
                        ) => Some(tokens),
                    },
                    context_tokens: match observation.get_context_tokens().which()? {
                        artisan_capnp::observation_usage::context_tokens::Which::NoContextTokens(
                            (),
                        ) => None,
                        artisan_capnp::observation_usage::context_tokens::Which::ContextTokens(
                            tokens,
                        ) => Some(tokens),
                    },
                    context_window_tokens: {
                        let window = observation.get_context_window_tokens();
                        if window == 0 { None } else { Some(window) }
                    },
                    cost_usd: match observation.get_cost().which()? {
                        artisan_capnp::observation_usage::cost::Which::NoCost(()) => None,
                        artisan_capnp::observation_usage::cost::Which::Cost(cost) => Some(cost),
                    },
                    provider_route_id: parse_optional_observation_id(
                        read_text(
                            observation.get_provider_route_id(),
                            "event.engineObservation.usage.providerRouteId",
                        )?,
                        "event.engineObservation.usage.providerRouteId",
                    )?,
                    turn_id: parse_optional_observation_id(
                        read_text(
                            observation.get_turn_id(),
                            "event.engineObservation.usage.turnId",
                        )?,
                        "event.engineObservation.usage.turnId",
                    )?,
                },
            )?))
        }
    }
}

pub(crate) fn parse_optional_observation_id(
    value: String,
    field: &'static str,
) -> Result<Option<ObservationId>, ProtocolDecodeError> {
    match absent_if_empty(value) {
        None => Ok(None),
        Some(text) => parse_observation_id(text, field).map(Some),
    }
}

pub(crate) fn decode_observation_approval_request(
    value: artisan_capnp::observation_approval_request::Reader<'_>,
) -> Result<ApprovalRequest, ProtocolDecodeError> {
    let kind = decode_observation_approval_kind(value.get_kind()?);
    let command = absent_if_empty(read_text(
        value.get_command(),
        "event.engineObservation.approval.request.command",
    )?);
    let cwd = absent_if_empty(read_text(
        value.get_cwd(),
        "event.engineObservation.approval.request.cwd",
    )?);
    let reason = absent_if_empty(read_text(
        value.get_reason(),
        "event.engineObservation.approval.request.reason",
    )?);
    match kind {
        ApprovalKind::Command => {
            let Some(command) = command else {
                return Err(ObservationError::MissingField { field: "command" }.into());
            };
            Ok(ApprovalRequest::command(command, cwd, reason)?)
        }
        ApprovalKind::FileChange => {
            if command.is_some() {
                return Err(ObservationError::UnexpectedField { field: "command" }.into());
            }
            if cwd.is_some() {
                return Err(ObservationError::UnexpectedField { field: "cwd" }.into());
            }
            Ok(ApprovalRequest::file_change(reason)?)
        }
        ApprovalKind::Action => {
            if command.is_some() {
                return Err(ObservationError::UnexpectedField { field: "command" }.into());
            }
            if cwd.is_some() {
                return Err(ObservationError::UnexpectedField { field: "cwd" }.into());
            }
            Ok(ApprovalRequest::action(reason)?)
        }
    }
}

pub(crate) fn decode_observation_engine_error_ref(
    value: artisan_capnp::observation_engine_error_ref::Reader<'_>,
    field: &'static str,
) -> Result<EngineErrorRef, ProtocolDecodeError> {
    // Sub-field failures report the enclosing error-reference label: every
    // label below stays a static string so no provider text ever enters an
    // error value.
    Ok(EngineErrorRef::new(EngineErrorRefInput {
        artisan_code: ArtisanCode::parse(read_text(value.get_artisan_code(), field)?)
            .map_err(ProtocolDecodeError::from)?,
        provider_code: absent_if_empty(read_text(value.get_provider_code(), field)?),
        detail: absent_if_empty(read_text(value.get_detail(), field)?),
        affected_model_id: absent_if_empty(read_text(value.get_affected_model_id(), field)?),
        limit_id: absent_if_empty(read_text(value.get_limit_id(), field)?),
        limit_label: absent_if_empty(read_text(value.get_limit_label(), field)?),
        limit_scope: match value.get_limit_scope().which()? {
            artisan_capnp::observation_engine_error_ref::limit_scope::Which::NoLimitScope(()) => {
                None
            }
            artisan_capnp::observation_engine_error_ref::limit_scope::Which::LimitScope(scope) => {
                Some(decode_observation_limit_scope(scope?))
            }
        },
        resets_at: absent_if_empty(read_text(value.get_resets_at(), field)?),
    })?)
}

#[expect(
    clippy::too_many_lines,
    reason = "single transcript-content union dispatcher mirroring `encode_transcript_content`"
)]
pub(crate) fn decode_transcript_content(
    value: artisan_capnp::observation_subagent_transcript_content::Reader<'_>,
) -> Result<TranscriptContent, ProtocolDecodeError> {
    match value.which()? {
        artisan_capnp::observation_subagent_transcript_content::Which::AgentMessageDelta(
            content,
        ) => {
            let content = content?;
            Ok(TranscriptContent::AgentMessageDelta(
                TranscriptAgentMessageDelta::new(
                    parse_observation_id(
                        read_text(
                            content.get_item_id(),
                            "event.engineObservation.subagentTranscript.content.agentMessageDelta.itemId",
                        )?,
                        "event.engineObservation.subagentTranscript.content.agentMessageDelta.itemId",
                    )?,
                    decode_observation_message_phase(content.get_phase()?),
                    read_text(
                        content.get_delta(),
                        "event.engineObservation.subagentTranscript.content.agentMessageDelta.delta",
                    )?,
                )?,
            ))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::AgentMessageCompleted(
            content,
        ) => {
            let content = content?;
            Ok(TranscriptContent::AgentMessageCompleted(
                TranscriptAgentMessageCompleted::new(
                    parse_observation_id(
                        read_text(
                            content.get_item_id(),
                            "event.engineObservation.subagentTranscript.content.agentMessageCompleted.itemId",
                        )?,
                        "event.engineObservation.subagentTranscript.content.agentMessageCompleted.itemId",
                    )?,
                    decode_observation_message_phase(content.get_phase()?),
                    read_text(
                        content.get_message(),
                        "event.engineObservation.subagentTranscript.content.agentMessageCompleted.message",
                    )?,
                )?,
            ))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::ReasoningSummaryDelta(
            content,
        ) => {
            let content = content?;
            Ok(TranscriptContent::ReasoningSummaryDelta(
                TranscriptReasoningSummaryDelta::new(
                    parse_observation_id(
                        read_text(
                            content.get_item_id(),
                            "event.engineObservation.subagentTranscript.content.reasoningSummaryDelta.itemId",
                        )?,
                        "event.engineObservation.subagentTranscript.content.reasoningSummaryDelta.itemId",
                    )?,
                    content.get_summary_index(),
                    read_text(
                        content.get_delta(),
                        "event.engineObservation.subagentTranscript.content.reasoningSummaryDelta.delta",
                    )?,
                )?,
            ))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::ReasoningSummaryCompleted(
            content,
        ) => {
            let content = content?;
            Ok(TranscriptContent::ReasoningSummaryCompleted(
                TranscriptReasoningSummaryCompleted::new(
                    parse_observation_id(
                        read_text(
                            content.get_item_id(),
                            "event.engineObservation.subagentTranscript.content.reasoningSummaryCompleted.itemId",
                        )?,
                        "event.engineObservation.subagentTranscript.content.reasoningSummaryCompleted.itemId",
                    )?,
                    absent_if_empty(read_text(
                        content.get_text(),
                        "event.engineObservation.subagentTranscript.content.reasoningSummaryCompleted.text",
                    )?),
                )?,
            ))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::TerminalActivity(
            content,
        ) => {
            let content = content?;
            Ok(TranscriptContent::TerminalActivity(
                TranscriptTerminalActivity::new(
                    parse_observation_id(
                        read_text(
                            content.get_activity_id(),
                            "event.engineObservation.subagentTranscript.content.terminalActivity.activityId",
                        )?,
                        "event.engineObservation.subagentTranscript.content.terminalActivity.activityId",
                    )?,
                    match content.get_channel().which()? {
                        artisan_capnp::observation_transcript_terminal_activity::channel::Which::NoChannel(
                            (),
                        ) => None,
                        artisan_capnp::observation_transcript_terminal_activity::channel::Which::Channel(
                            channel,
                        ) => Some(decode_observation_terminal_channel(channel?)),
                    },
                    absent_if_empty(read_text(
                        content.get_command(),
                        "event.engineObservation.subagentTranscript.content.terminalActivity.command",
                    )?),
                    match content.get_exit_code().which()? {
                        artisan_capnp::observation_transcript_terminal_activity::exit_code::Which::NoExitCode(
                            (),
                        ) => None,
                        artisan_capnp::observation_transcript_terminal_activity::exit_code::Which::ExitCode(
                            code,
                        ) => Some(code),
                    },
                    match content.get_output().which()? {
                        artisan_capnp::observation_transcript_terminal_activity::output::Which::NoOutput(
                            (),
                        ) => None,
                        artisan_capnp::observation_transcript_terminal_activity::output::Which::Output(
                            output,
                        ) => Some(read_text(
                            output,
                            "event.engineObservation.subagentTranscript.content.terminalActivity.output",
                        )?),
                    },
                    decode_observation_terminal_state(content.get_state()?),
                )?,
            ))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::Tool(content) => {
            let content = content?;
            Ok(TranscriptContent::Tool(TranscriptTool::new(
                parse_observation_id(
                    read_text(
                        content.get_tool_id(),
                        "event.engineObservation.subagentTranscript.content.tool.toolId",
                    )?,
                    "event.engineObservation.subagentTranscript.content.tool.toolId",
                )?,
                read_text(
                    content.get_tool_name(),
                    "event.engineObservation.subagentTranscript.content.tool.toolName",
                )?,
                decode_observation_tool_action(content.get_action()?),
                absent_if_empty(read_text(
                    content.get_detail(),
                    "event.engineObservation.subagentTranscript.content.tool.detail",
                )?),
            )?))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::File(content) => {
            let content = content?;
            Ok(TranscriptContent::File(TranscriptFile::new(
                read_text(
                    content.get_path(),
                    "event.engineObservation.subagentTranscript.content.file.path",
                )?,
                decode_observation_file_action(content.get_action()?),
                match content.get_lines_added().which()? {
                    artisan_capnp::observation_transcript_file::lines_added::Which::NoLinesAdded(
                        (),
                    ) => None,
                    artisan_capnp::observation_transcript_file::lines_added::Which::LinesAdded(
                        count,
                    ) => Some(count),
                },
                match content.get_lines_deleted().which()? {
                    artisan_capnp::observation_transcript_file::lines_deleted::Which::NoLinesDeleted(
                        (),
                    ) => None,
                    artisan_capnp::observation_transcript_file::lines_deleted::Which::LinesDeleted(
                        count,
                    ) => Some(count),
                },
            )?))
        }
        artisan_capnp::observation_subagent_transcript_content::Which::Search(content) => {
            let content = content?;
            Ok(TranscriptContent::Search(TranscriptSearch::new(
                read_text(
                    content.get_query(),
                    "event.engineObservation.subagentTranscript.content.search.query",
                )?,
                match content.get_result_count().which()? {
                    artisan_capnp::observation_transcript_search::result_count::Which::NoResultCount(
                        (),
                    ) => None,
                    artisan_capnp::observation_transcript_search::result_count::Which::ResultCount(
                        count,
                    ) => Some(count),
                },
                match content.get_scope().which()? {
                    artisan_capnp::observation_transcript_search::scope::Which::NoScope(()) => None,
                    artisan_capnp::observation_transcript_search::scope::Which::Scope(scope) => {
                        Some(decode_observation_search_scope(scope?))
                    }
                },
                parse_optional_observation_id(
                    read_text(
                        content.get_search_id(),
                        "event.engineObservation.subagentTranscript.content.search.searchId",
                    )?,
                    "event.engineObservation.subagentTranscript.content.search.searchId",
                )?,
                decode_observation_search_state(content.get_state()?),
            )?))
        }
    }
}
