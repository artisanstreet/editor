//! Engine-observation and transcript encode conversion.

#[allow(clippy::wildcard_imports)]
use super::*;

#[expect(
    clippy::too_many_lines,
    reason = "central observation union dispatcher; each arm maps one engine event variant"
)]
pub(crate) fn encode_engine_observation(
    mut builder: artisan_capnp::engine_observation::Builder<'_>,
    value: &Observation,
) -> Result<(), ProtocolEncodeError> {
    match value {
        Observation::AgentMessageDelta(observation) => {
            let mut encoded = builder.reborrow().init_agent_message_delta();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_item_id(observation.item_id().as_str());
            encoded.set_phase(encode_observation_message_phase(observation.phase()));
            encoded.set_delta(observation.delta());
            encoded.set_turn_id(observation.turn_id().as_str());
        }
        Observation::AgentMessageCompleted(observation) => {
            let mut encoded = builder.reborrow().init_agent_message_completed();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_item_id(observation.item_id().as_str());
            encoded.set_phase(encode_observation_message_phase(observation.phase()));
            encoded.set_message(observation.message());
            encoded.set_turn_id(observation.turn_id().as_str());
        }
        Observation::Approval(observation) => {
            let mut encoded = builder.reborrow().init_approval();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_approval_id(observation.approval_id().as_str());
            encoded.set_state(encode_observation_approval_state(observation.state()));
            encoded.set_description(observation.description());
            encode_observation_approval_request(
                encoded.reborrow().init_request(),
                observation.request(),
            );
            match observation.approved() {
                Some(decision) => {
                    encoded.reborrow().init_decision().set_decision(decision);
                }
                None => {
                    encoded.reborrow().init_decision().set_no_decision(());
                }
            }
        }
        Observation::Compaction(observation) => {
            let mut encoded = builder.reborrow().init_compaction();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_state(encode_observation_compaction_state(observation.state()));
            encoded.set_compaction_id(
                observation
                    .compaction_id()
                    .map_or("", ObservationId::as_str),
            );
            match observation.duration_ms() {
                Some(duration) => {
                    encoded
                        .reborrow()
                        .init_duration_ms()
                        .set_duration_ms(duration);
                }
                None => {
                    encoded.reborrow().init_duration_ms().set_no_duration_ms(());
                }
            }
            encoded.set_summary(observation.summary().unwrap_or(""));
        }
        Observation::File(observation) => {
            let mut encoded = builder.reborrow().init_file();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_path(observation.path());
            encoded.set_action(encode_observation_file_action(observation.action()));
            match observation.lines_added() {
                Some(count) => {
                    encoded.reborrow().init_lines_added().set_lines_added(count);
                }
                None => {
                    encoded.reborrow().init_lines_added().set_no_lines_added(());
                }
            }
            match observation.lines_deleted() {
                Some(count) => {
                    encoded
                        .reborrow()
                        .init_lines_deleted()
                        .set_lines_deleted(count);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_lines_deleted()
                        .set_no_lines_deleted(());
                }
            }
        }
        Observation::NativeAction(observation) => {
            let mut encoded = builder.reborrow().init_native_action();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_action(observation.action());
            encoded.set_detail(observation.detail().unwrap_or(""));
            encoded.set_diagnostic(observation.diagnostic());
            match observation.error_ref() {
                Some(error) => {
                    encode_observation_engine_error_ref(
                        encoded.reborrow().init_error_ref().init_error_ref(),
                        error,
                    );
                }
                None => {
                    encoded.reborrow().init_error_ref().set_no_error_ref(());
                }
            }
        }
        Observation::Plan(observation) => {
            let mut encoded = builder.reborrow().init_plan();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            let mut entries = encoded.reborrow().init_entries(list_length(
                "event.engineObservation.plan.entries",
                observation.entries().len(),
            )?);
            for (index, entry) in observation.entries().iter().enumerate() {
                let mut encoded_entry = entries
                    .reborrow()
                    .get(list_index("event.engineObservation.plan.entries", index)?);
                encoded_entry.set_id(entry.id().as_str());
                encoded_entry.set_status(encode_observation_plan_entry_status(entry.status()));
                encoded_entry.set_text(entry.text());
            }
            encoded.set_turn_id(observation.turn_id().map_or("", ObservationId::as_str));
        }
        Observation::ProcessDiagnostic(observation) => {
            let mut encoded = builder.reborrow().init_process_diagnostic();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_level(encode_observation_diagnostic_level(observation.level()));
            encoded.set_message(observation.message());
            match observation.error_ref() {
                Some(error) => {
                    encode_observation_engine_error_ref(
                        encoded.reborrow().init_error_ref().init_error_ref(),
                        error,
                    );
                }
                None => {
                    encoded.reborrow().init_error_ref().set_no_error_ref(());
                }
            }
        }
        Observation::ProtocolDiagnostic(observation) => {
            let mut encoded = builder.reborrow().init_protocol_diagnostic();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_level(encode_observation_diagnostic_level(observation.level()));
            encoded.set_message(observation.message());
        }
        Observation::Question(observation) => {
            let mut encoded = builder.reborrow().init_question();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_question_id(observation.question_id().as_str());
            encoded.set_state(encode_observation_question_state(observation.state()));
            encoded.set_text(observation.text());
            encoded.set_header(observation.header().unwrap_or(""));
            encoded.set_multi_select(observation.multi_select());
            let option_count = observation.options().map_or(0, Vec::len);
            let mut options = encoded.reborrow().init_options(list_length(
                "event.engineObservation.question.options",
                option_count,
            )?);
            if let Some(option_list) = observation.options() {
                for (index, option) in option_list.iter().enumerate() {
                    let mut encoded_option = options.reborrow().get(list_index(
                        "event.engineObservation.question.options",
                        index,
                    )?);
                    encoded_option.set_label(option.label());
                    encoded_option.set_description(option.description().unwrap_or(""));
                }
            }
            match observation.answers() {
                Some(answers) => {
                    let mut list = encoded.reborrow().init_answers().init_answers(list_length(
                        "event.engineObservation.question.answers",
                        answers.len(),
                    )?);
                    for (index, answer) in answers.iter().enumerate() {
                        list.set(
                            list_index("event.engineObservation.question.answers", index)?,
                            answer.as_str(),
                        );
                    }
                }
                None => {
                    encoded.reborrow().init_answers().set_no_answers(());
                }
            }
        }
        Observation::ReasoningSummaryCompleted(observation) => {
            let mut encoded = builder.reborrow().init_reasoning_summary_completed();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_item_id(observation.item_id().as_str());
            encoded.set_text(observation.text().unwrap_or(""));
            encoded.set_turn_id(observation.turn_id().as_str());
        }
        Observation::ReasoningSummaryDelta(observation) => {
            let mut encoded = builder.reborrow().init_reasoning_summary_delta();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_item_id(observation.item_id().as_str());
            encoded.set_summary_index(observation.summary_index());
            encoded.set_delta(observation.delta());
            match observation.thinking_tokens() {
                Some(tokens) => {
                    encoded
                        .reborrow()
                        .init_thinking_tokens()
                        .set_thinking_tokens(tokens);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_thinking_tokens()
                        .set_no_thinking_tokens(());
                }
            }
            encoded.set_turn_id(observation.turn_id().as_str());
        }
        Observation::Retry(observation) => {
            let mut encoded = builder.reborrow().init_retry();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_turn_id(observation.turn_id().as_str());
            encoded.set_attempt_state(encode_observation_retry_attempt_state(
                observation.attempt_state(),
            ));
            encoded.set_will_retry(observation.will_retry());
            encoded.set_message(observation.message());
        }
        Observation::RunState(observation) => {
            let mut encoded = builder.reborrow().init_run_state();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_state(encode_observation_run_state(observation.state()));
        }
        Observation::RunTerminal(observation) => {
            let mut encoded = builder.reborrow().init_run_terminal();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_state(encode_observation_run_terminal_state(observation.state()));
            match observation.error_ref() {
                Some(error) => {
                    encode_observation_engine_error_ref(
                        encoded.reborrow().init_error_ref().init_error_ref(),
                        error,
                    );
                }
                None => {
                    encoded.reborrow().init_error_ref().set_no_error_ref(());
                }
            }
            encoded.set_summary_title(observation.summary_title().unwrap_or(""));
        }
        Observation::Search(observation) => {
            let mut encoded = builder.reborrow().init_search();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_query(observation.query());
            match observation.scope() {
                Some(scope) => {
                    encoded
                        .reborrow()
                        .init_scope()
                        .set_scope(encode_observation_search_scope(scope));
                }
                None => {
                    encoded.reborrow().init_scope().set_no_scope(());
                }
            }
            encoded.set_search_id(observation.search_id().map_or("", ObservationId::as_str));
            encoded.set_state(encode_observation_search_state(observation.state()));
            match observation.result_count() {
                Some(count) => {
                    encoded
                        .reborrow()
                        .init_result_count()
                        .set_result_count(count);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_result_count()
                        .set_no_result_count(());
                }
            }
        }
        Observation::Subagent(observation) => {
            let mut encoded = builder.reborrow().init_subagent();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_agent_native_thread_id(observation.agent_native_thread_id().as_str());
            encoded.set_parent_native_thread_id(observation.parent_native_thread_id().as_str());
            encoded.set_state(encode_observation_subagent_state(observation.state()));
            encoded.set_activity(observation.activity().unwrap_or(""));
            encoded.set_agent_path(observation.agent_path().unwrap_or(""));
            encoded.set_turn_id(observation.turn_id().map_or("", ObservationId::as_str));
        }
        Observation::SubagentTranscript(observation) => {
            let mut encoded = builder.reborrow().init_subagent_transcript();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_agent_native_thread_id(observation.agent_native_thread_id().as_str());
            encoded.set_parent_native_thread_id(observation.parent_native_thread_id().as_str());
            encode_transcript_content(encoded.reborrow().init_content(), observation.content());
        }
        Observation::TerminalActivity(observation) => {
            let mut encoded = builder.reborrow().init_terminal_activity();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_activity_id(observation.activity_id().as_str());
            match observation.channel() {
                Some(channel) => {
                    encoded
                        .reborrow()
                        .init_channel()
                        .set_channel(encode_observation_terminal_channel(channel));
                }
                None => {
                    encoded.reborrow().init_channel().set_no_channel(());
                }
            }
            encoded.set_command(observation.command().unwrap_or(""));
            encoded.set_shell(observation.shell().unwrap_or(""));
            match observation.output() {
                Some(output) => {
                    encoded.reborrow().init_output().set_output(output);
                }
                None => {
                    encoded.reborrow().init_output().set_no_output(());
                }
            }
            match observation.exit_code() {
                Some(code) => {
                    encoded.reborrow().init_exit_code().set_exit_code(code);
                }
                None => {
                    encoded.reborrow().init_exit_code().set_no_exit_code(());
                }
            }
            encoded.set_state(encode_observation_terminal_state(observation.state()));
        }
        Observation::Tool(observation) => {
            let mut encoded = builder.reborrow().init_tool();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_tool_id(observation.tool_id().as_str());
            encoded.set_tool_name(observation.tool_name());
            encoded.set_action(encode_observation_tool_action(observation.action()));
            encoded.set_detail(observation.detail().unwrap_or(""));
        }
        Observation::TurnState(observation) => {
            let mut encoded = builder.reborrow().init_turn_state();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_turn_id(observation.turn_id().as_str());
            encoded.set_state(encode_observation_turn_state(observation.state()));
        }
        Observation::Usage(observation) => {
            let mut encoded = builder.reborrow().init_usage();
            encoded.set_id(observation.id().as_str());
            encoded.set_sequence(observation.sequence().get());
            encoded.set_basis(encode_observation_usage_basis(observation.basis()));
            match observation.input_tokens() {
                Some(tokens) => {
                    encoded
                        .reborrow()
                        .init_input_tokens()
                        .set_input_tokens(tokens);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_input_tokens()
                        .set_no_input_tokens(());
                }
            }
            match observation.cached_input_tokens() {
                Some(tokens) => {
                    encoded
                        .reborrow()
                        .init_cached_input_tokens()
                        .set_cached_input_tokens(tokens);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_cached_input_tokens()
                        .set_no_cached_input_tokens(());
                }
            }
            match observation.output_tokens() {
                Some(tokens) => {
                    encoded
                        .reborrow()
                        .init_output_tokens()
                        .set_output_tokens(tokens);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_output_tokens()
                        .set_no_output_tokens(());
                }
            }
            match observation.context_tokens() {
                Some(tokens) => {
                    encoded
                        .reborrow()
                        .init_context_tokens()
                        .set_context_tokens(tokens);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_context_tokens()
                        .set_no_context_tokens(());
                }
            }
            encoded.set_context_window_tokens(observation.context_window_tokens().unwrap_or(0));
            match observation.cost_usd() {
                Some(cost) => {
                    encoded.reborrow().init_cost().set_cost(cost);
                }
                None => {
                    encoded.reborrow().init_cost().set_no_cost(());
                }
            }
            encoded.set_provider_route_id(
                observation
                    .provider_route_id()
                    .map_or("", ObservationId::as_str),
            );
            encoded.set_turn_id(observation.turn_id().map_or("", ObservationId::as_str));
        }
    }
    Ok(())
}

pub(crate) fn encode_observation_approval_request(
    mut builder: artisan_capnp::observation_approval_request::Builder<'_>,
    value: &ApprovalRequest,
) {
    builder.set_kind(encode_observation_approval_kind(value.kind()));
    builder.set_command(value.command_text().unwrap_or(""));
    builder.set_cwd(value.cwd().unwrap_or(""));
    builder.set_reason(value.reason().unwrap_or(""));
}

pub(crate) fn encode_observation_engine_error_ref(
    mut builder: artisan_capnp::observation_engine_error_ref::Builder<'_>,
    value: &EngineErrorRef,
) {
    builder.set_artisan_code(value.artisan_code().as_str());
    builder.set_provider_code(value.provider_code().unwrap_or(""));
    builder.set_detail(value.detail().unwrap_or(""));
    builder.set_affected_model_id(value.affected_model_id().unwrap_or(""));
    builder.set_limit_id(value.limit_id().unwrap_or(""));
    builder.set_limit_label(value.limit_label().unwrap_or(""));
    match value.limit_scope() {
        Some(scope) => {
            builder
                .reborrow()
                .init_limit_scope()
                .set_limit_scope(encode_observation_limit_scope(scope));
        }
        None => {
            builder.reborrow().init_limit_scope().set_no_limit_scope(());
        }
    }
    builder.set_resets_at(value.resets_at().unwrap_or(""));
}

#[expect(
    clippy::too_many_lines,
    reason = "single transcript-content union dispatcher; each arm maps one transcript variant"
)]
pub(crate) fn encode_transcript_content(
    builder: artisan_capnp::observation_subagent_transcript_content::Builder<'_>,
    value: &TranscriptContent,
) {
    match value {
        TranscriptContent::AgentMessageDelta(content) => {
            let mut encoded = builder.init_agent_message_delta();
            encoded.set_item_id(content.item_id().as_str());
            encoded.set_phase(encode_observation_message_phase(content.phase()));
            encoded.set_delta(content.delta());
        }
        TranscriptContent::AgentMessageCompleted(content) => {
            let mut encoded = builder.init_agent_message_completed();
            encoded.set_item_id(content.item_id().as_str());
            encoded.set_phase(encode_observation_message_phase(content.phase()));
            encoded.set_message(content.message());
        }
        TranscriptContent::ReasoningSummaryDelta(content) => {
            let mut encoded = builder.init_reasoning_summary_delta();
            encoded.set_item_id(content.item_id().as_str());
            encoded.set_summary_index(content.summary_index());
            encoded.set_delta(content.delta());
        }
        TranscriptContent::ReasoningSummaryCompleted(content) => {
            let mut encoded = builder.init_reasoning_summary_completed();
            encoded.set_item_id(content.item_id().as_str());
            encoded.set_text(content.text().unwrap_or(""));
        }
        TranscriptContent::TerminalActivity(content) => {
            let mut encoded = builder.init_terminal_activity();
            encoded.set_activity_id(content.activity_id().as_str());
            match content.channel() {
                Some(channel) => {
                    encoded
                        .reborrow()
                        .init_channel()
                        .set_channel(encode_observation_terminal_channel(channel));
                }
                None => {
                    encoded.reborrow().init_channel().set_no_channel(());
                }
            }
            encoded.set_command(content.command().unwrap_or(""));
            match content.exit_code() {
                Some(code) => {
                    encoded.reborrow().init_exit_code().set_exit_code(code);
                }
                None => {
                    encoded.reborrow().init_exit_code().set_no_exit_code(());
                }
            }
            match content.output() {
                Some(output) => {
                    encoded.reborrow().init_output().set_output(output);
                }
                None => {
                    encoded.reborrow().init_output().set_no_output(());
                }
            }
            encoded.set_state(encode_observation_terminal_state(content.state()));
        }
        TranscriptContent::Tool(content) => {
            let mut encoded = builder.init_tool();
            encoded.set_tool_id(content.tool_id().as_str());
            encoded.set_tool_name(content.tool_name());
            encoded.set_action(encode_observation_tool_action(content.action()));
            encoded.set_detail(content.detail().unwrap_or(""));
        }
        TranscriptContent::File(content) => {
            let mut encoded = builder.init_file();
            encoded.set_path(content.path());
            encoded.set_action(encode_observation_file_action(content.action()));
            match content.lines_added() {
                Some(count) => {
                    encoded.reborrow().init_lines_added().set_lines_added(count);
                }
                None => {
                    encoded.reborrow().init_lines_added().set_no_lines_added(());
                }
            }
            match content.lines_deleted() {
                Some(count) => {
                    encoded
                        .reborrow()
                        .init_lines_deleted()
                        .set_lines_deleted(count);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_lines_deleted()
                        .set_no_lines_deleted(());
                }
            }
        }
        TranscriptContent::Search(content) => {
            let mut encoded = builder.init_search();
            encoded.set_query(content.query());
            match content.result_count() {
                Some(count) => {
                    encoded
                        .reborrow()
                        .init_result_count()
                        .set_result_count(count);
                }
                None => {
                    encoded
                        .reborrow()
                        .init_result_count()
                        .set_no_result_count(());
                }
            }
            match content.scope() {
                Some(scope) => {
                    encoded
                        .reborrow()
                        .init_scope()
                        .set_scope(encode_observation_search_scope(scope));
                }
                None => {
                    encoded.reborrow().init_scope().set_no_scope(());
                }
            }
            encoded.set_search_id(content.search_id().map_or("", ObservationId::as_str));
            encoded.set_state(encode_observation_search_state(content.state()));
        }
    }
}
