//! Canonical stored projections, their serialize structs, and encode entry points.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Encodes one bounded observation batch to its canonical bytes.
///
/// This is the byte-level form of [`encode_observation_checkpoint`]: same
/// validation and the same canonical layout, without the checkpoint wrapper.
/// Tests and tooling use it to verify canonical bytes; production commits wrap
/// it in an [`EngineCheckpoint`] so checkpoint bytes stay redacted.
///
/// # Errors
///
/// Returns [`ObservationCommitError`] for an empty or oversized batch,
/// non-positive binding versions, non-monotonic sequences, oversized payloads,
/// or domain bound violations.
pub fn encode_observation_bytes(
    engine: EngineId,
    binding_version: i64,
    base_sequence: Option<u64>,
    observations: &[Observation],
) -> Result<Vec<u8>, ObservationCommitError> {
    if binding_version <= 0 {
        return Err(ObservationCommitError::InvalidObservation(
            ObservationError::OutOfRange {
                field: "binding_version",
            },
        ));
    }
    if observations.is_empty() {
        return Err(ObservationCommitError::EmptyBatch);
    }
    if observations.len() > OBSERVATION_BATCH_MAX_OBSERVATIONS {
        return Err(ObservationCommitError::TooMany {
            count: observations.len(),
            maximum: OBSERVATION_BATCH_MAX_OBSERVATIONS,
        });
    }
    let mut previous = base_sequence;
    for observation in observations {
        let sequence = observation.sequence().get();
        if previous.is_some_and(|bound| sequence <= bound) {
            return Err(ObservationCommitError::SequenceNotMonotonic);
        }
        previous = Some(sequence);
    }
    let encoded = encode_bytes(engine, binding_version, observations)?;
    if encoded.len() > ENGINE_CHECKPOINT_MAX_BYTES {
        return Err(ObservationCommitError::TooLarge {
            length: encoded.len(),
            maximum: ENGINE_CHECKPOINT_MAX_BYTES,
        });
    }
    Ok(encoded)
}

/// Packs one bounded observation batch into an engine checkpoint.
///
/// Sequences must be strictly increasing and strictly greater than
/// `base_sequence` (the previously committed maximum, [`None`] for the first
/// batch of a run). Bind agreement is checked separately with
/// [`validate_observation_bind`]; the checkpoint embeds the engine tag and
/// binding version so durable history stays attributable.
///
/// # Errors
///
/// Returns [`ObservationCommitError`] for an empty or oversized batch,
/// non-positive binding versions, non-monotonic sequences, oversized payloads,
/// or domain bound violations. No SQL is opened here; commit the returned
/// checkpoint through [`Repository::commit_run_batch`].
pub fn encode_observation_checkpoint(
    engine: EngineId,
    binding_version: i64,
    base_sequence: Option<u64>,
    observations: &[Observation],
) -> Result<EngineCheckpoint, ObservationCommitError> {
    let encoded = encode_observation_bytes(engine, binding_version, base_sequence, observations)?;
    EngineCheckpoint::new(OBSERVATION_CHECKPOINT_VERSION, encoded)
        .map_err(|_| ObservationCommitError::InvalidCheckpoint)
}

pub(super) fn encode_bytes(
    engine: EngineId,
    binding_version: i64,
    observations: &[Observation],
) -> Result<Vec<u8>, ObservationCommitError> {
    let mut stored = Vec::with_capacity(observations.len());
    for observation in observations {
        stored.push(stored_observation(observation));
    }
    let batch = StoredBatch {
        format: OBSERVATION_FORMAT_TAG,
        version: OBSERVATION_CHECKPOINT_VERSION,
        engine: engine.as_str(),
        binding_version,
        observations: stored,
    };
    serde_json::to_vec(&batch).map_err(|_| ObservationCommitError::Encode)
}

// ---------------------------------------------------------------------------
// Canonical encoding structs (field order is the persisted contract)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct StoredBatch<'a> {
    format: &'static str,
    version: i64,
    engine: &'static str,
    binding_version: i64,
    observations: Vec<StoredObservation<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum StoredObservation<'a> {
    AgentMessageDelta(StoredAgentMessageDelta<'a>),
    AgentMessageCompleted(StoredAgentMessageCompleted<'a>),
    Approval(StoredApproval<'a>),
    Compaction(StoredCompaction<'a>),
    File(StoredFile<'a>),
    NativeAction(StoredNativeAction<'a>),
    Plan(StoredPlan<'a>),
    ProcessDiagnostic(StoredProcessDiagnostic<'a>),
    ProtocolDiagnostic(StoredProtocolDiagnostic<'a>),
    Question(StoredQuestion<'a>),
    ReasoningSummaryCompleted(StoredReasoningSummaryCompleted<'a>),
    ReasoningSummaryDelta(StoredReasoningSummaryDelta<'a>),
    Retry(StoredRetry<'a>),
    RunState(StoredRunState<'a>),
    RunTerminal(StoredRunTerminal<'a>),
    Search(StoredSearch<'a>),
    Subagent(StoredSubagent<'a>),
    SubagentTranscript(StoredSubagentTranscript<'a>),
    TerminalActivity(StoredTerminalActivity<'a>),
    Tool(StoredTool<'a>),
    TurnState(StoredTurnState<'a>),
    Usage(StoredUsage<'a>),
}

#[derive(Serialize)]
struct StoredAgentMessageDelta<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    item_id: &'a str,
    phase: &'static str,
    delta: &'a str,
    turn_id: &'a str,
}

#[derive(Serialize)]
struct StoredAgentMessageCompleted<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    item_id: &'a str,
    phase: &'static str,
    message: &'a str,
    turn_id: &'a str,
}

#[derive(Serialize)]
struct StoredReasoningSummaryDelta<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    item_id: &'a str,
    summary_index: u64,
    delta: &'a str,
    thinking_tokens: Option<u64>,
    turn_id: &'a str,
}

#[derive(Serialize)]
struct StoredReasoningSummaryCompleted<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    item_id: &'a str,
    text: Option<&'a str>,
    turn_id: &'a str,
}

#[derive(Serialize)]
struct StoredTool<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    tool_id: &'a str,
    tool_name: &'a str,
    action: &'static str,
    detail: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredFile<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    path: &'a str,
    action: &'static str,
    lines_added: Option<u64>,
    lines_deleted: Option<u64>,
}

#[derive(Serialize)]
struct StoredSearch<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    query: &'a str,
    scope: Option<&'static str>,
    search_id: Option<&'a str>,
    state: &'static str,
    result_count: Option<u64>,
}

#[derive(Serialize)]
struct StoredTerminalActivity<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    activity_id: &'a str,
    channel: Option<&'static str>,
    command: Option<&'a str>,
    shell: Option<&'a str>,
    output: Option<&'a str>,
    exit_code: Option<i32>,
    state: &'static str,
}

#[derive(Serialize)]
struct StoredApprovalRequest<'a> {
    kind: &'static str,
    command: Option<&'a str>,
    cwd: Option<&'a str>,
    reason: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredApproval<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    approval_id: &'a str,
    state: &'static str,
    description: &'a str,
    request: StoredApprovalRequest<'a>,
    approved: Option<bool>,
}

#[derive(Serialize)]
struct StoredQuestionOption<'a> {
    label: &'a str,
    description: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredQuestion<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    question_id: &'a str,
    state: &'static str,
    text: &'a str,
    header: Option<&'a str>,
    multi_select: bool,
    options: Option<Vec<StoredQuestionOption<'a>>>,
    answers: Option<&'a Vec<String>>,
}

#[derive(Serialize)]
struct StoredPlanEntry<'a> {
    id: &'a str,
    status: &'static str,
    text: &'a str,
}

#[derive(Serialize)]
struct StoredPlan<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    entries: Vec<StoredPlanEntry<'a>>,
    turn_id: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredCompaction<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    state: &'static str,
    compaction_id: Option<&'a str>,
    duration_ms: Option<u64>,
    summary: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredRetry<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    turn_id: &'a str,
    attempt_state: &'static str,
    will_retry: bool,
    message: &'a str,
}

#[derive(Serialize)]
struct StoredRunState<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    state: &'static str,
}

#[derive(Serialize)]
struct StoredTurnState<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    turn_id: &'a str,
    state: &'static str,
}

#[derive(Serialize)]
struct StoredSubagent<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    agent_native_thread_id: &'a str,
    parent_native_thread_id: &'a str,
    state: &'static str,
    activity: Option<&'a str>,
    agent_path: Option<&'a str>,
    turn_id: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum StoredTranscriptContent<'a> {
    AgentMessageDelta(StoredTranscriptAgentMessageDelta<'a>),
    AgentMessageCompleted(StoredTranscriptAgentMessageCompleted<'a>),
    ReasoningSummaryDelta(StoredTranscriptReasoningSummaryDelta<'a>),
    ReasoningSummaryCompleted(StoredTranscriptReasoningSummaryCompleted<'a>),
    TerminalActivity(StoredTranscriptTerminalActivity<'a>),
    Tool(StoredTranscriptTool<'a>),
    File(StoredTranscriptFile<'a>),
    Search(StoredTranscriptSearch<'a>),
}

#[derive(Serialize)]
struct StoredTranscriptAgentMessageDelta<'a> {
    tag: &'static str,
    item_id: &'a str,
    phase: &'static str,
    delta: &'a str,
}

#[derive(Serialize)]
struct StoredTranscriptAgentMessageCompleted<'a> {
    tag: &'static str,
    item_id: &'a str,
    phase: &'static str,
    message: &'a str,
}

#[derive(Serialize)]
struct StoredTranscriptReasoningSummaryDelta<'a> {
    tag: &'static str,
    item_id: &'a str,
    summary_index: u64,
    delta: &'a str,
}

#[derive(Serialize)]
struct StoredTranscriptReasoningSummaryCompleted<'a> {
    tag: &'static str,
    item_id: &'a str,
    text: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredTranscriptTerminalActivity<'a> {
    tag: &'static str,
    activity_id: &'a str,
    channel: Option<&'static str>,
    command: Option<&'a str>,
    exit_code: Option<i32>,
    output: Option<&'a str>,
    state: &'static str,
}

#[derive(Serialize)]
struct StoredTranscriptTool<'a> {
    tag: &'static str,
    tool_id: &'a str,
    tool_name: &'a str,
    action: &'static str,
    detail: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredTranscriptFile<'a> {
    tag: &'static str,
    path: &'a str,
    action: &'static str,
    lines_added: Option<u64>,
    lines_deleted: Option<u64>,
}

#[derive(Serialize)]
struct StoredTranscriptSearch<'a> {
    tag: &'static str,
    query: &'a str,
    result_count: Option<u64>,
    scope: Option<&'static str>,
    search_id: Option<&'a str>,
    state: &'static str,
}

#[derive(Serialize)]
struct StoredSubagentTranscript<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    agent_native_thread_id: &'a str,
    parent_native_thread_id: &'a str,
    content: StoredTranscriptContent<'a>,
}

#[derive(Serialize)]
struct StoredUsage<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    basis: &'static str,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    context_tokens: Option<u64>,
    context_window_tokens: Option<u64>,
    cost_usd: Option<f64>,
    provider_route_id: Option<&'a str>,
    turn_id: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredErrorRef<'a> {
    artisan_code: &'a str,
    provider_code: Option<&'a str>,
    detail: Option<&'a str>,
    affected_model_id: Option<&'a str>,
    limit_id: Option<&'a str>,
    limit_label: Option<&'a str>,
    limit_scope: Option<&'static str>,
    resets_at: Option<&'a str>,
}

#[derive(Serialize)]
struct StoredNativeAction<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    action: &'a str,
    detail: Option<&'a str>,
    diagnostic: bool,
    error_ref: Option<StoredErrorRef<'a>>,
}

#[derive(Serialize)]
struct StoredProcessDiagnostic<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    level: &'static str,
    message: &'a str,
    error_ref: Option<StoredErrorRef<'a>>,
}

#[derive(Serialize)]
struct StoredProtocolDiagnostic<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    level: &'static str,
    message: &'a str,
}

#[derive(Serialize)]
struct StoredRunTerminal<'a> {
    tag: &'static str,
    id: &'a str,
    sequence: u64,
    state: &'static str,
    error_ref: Option<StoredErrorRef<'a>>,
    summary_title: Option<&'a str>,
}

fn stored_error_ref(value: &EngineErrorRef) -> StoredErrorRef<'_> {
    StoredErrorRef {
        artisan_code: value.artisan_code().as_str(),
        provider_code: value.provider_code(),
        detail: value.detail(),
        affected_model_id: value.affected_model_id(),
        limit_id: value.limit_id(),
        limit_label: value.limit_label(),
        limit_scope: value.limit_scope().map(LimitScope::as_str),
        resets_at: value.resets_at(),
    }
}

fn stored_transcript_content(value: &TranscriptContent) -> StoredTranscriptContent<'_> {
    match value {
        TranscriptContent::AgentMessageDelta(content) => {
            StoredTranscriptContent::AgentMessageDelta(StoredTranscriptAgentMessageDelta {
                tag: value.tag(),
                item_id: content.item_id().as_str(),
                phase: content.phase().as_str(),
                delta: content.delta(),
            })
        }
        TranscriptContent::AgentMessageCompleted(content) => {
            StoredTranscriptContent::AgentMessageCompleted(StoredTranscriptAgentMessageCompleted {
                tag: value.tag(),
                item_id: content.item_id().as_str(),
                phase: content.phase().as_str(),
                message: content.message(),
            })
        }
        TranscriptContent::ReasoningSummaryDelta(content) => {
            StoredTranscriptContent::ReasoningSummaryDelta(StoredTranscriptReasoningSummaryDelta {
                tag: value.tag(),
                item_id: content.item_id().as_str(),
                summary_index: content.summary_index(),
                delta: content.delta(),
            })
        }
        TranscriptContent::ReasoningSummaryCompleted(content) => {
            StoredTranscriptContent::ReasoningSummaryCompleted(
                StoredTranscriptReasoningSummaryCompleted {
                    tag: value.tag(),
                    item_id: content.item_id().as_str(),
                    text: content.text(),
                },
            )
        }
        TranscriptContent::TerminalActivity(content) => {
            StoredTranscriptContent::TerminalActivity(StoredTranscriptTerminalActivity {
                tag: value.tag(),
                activity_id: content.activity_id().as_str(),
                channel: content.channel().map(TerminalChannel::as_str),
                command: content.command(),
                exit_code: content.exit_code(),
                output: content.output(),
                state: content.state().as_str(),
            })
        }
        TranscriptContent::Tool(content) => StoredTranscriptContent::Tool(StoredTranscriptTool {
            tag: value.tag(),
            tool_id: content.tool_id().as_str(),
            tool_name: content.tool_name(),
            action: content.action().as_str(),
            detail: content.detail(),
        }),
        TranscriptContent::File(content) => StoredTranscriptContent::File(StoredTranscriptFile {
            tag: value.tag(),
            path: content.path(),
            action: content.action().as_str(),
            lines_added: content.lines_added(),
            lines_deleted: content.lines_deleted(),
        }),
        TranscriptContent::Search(content) => {
            StoredTranscriptContent::Search(StoredTranscriptSearch {
                tag: value.tag(),
                query: content.query(),
                result_count: content.result_count(),
                scope: content.scope().map(SearchScope::as_str),
                search_id: content.search_id().map(ObservationId::as_str),
                state: content.state().as_str(),
            })
        }
    }
}

#[allow(clippy::too_many_lines)]
fn stored_observation(observation: &Observation) -> StoredObservation<'_> {
    match observation {
        Observation::AgentMessageDelta(value) => {
            StoredObservation::AgentMessageDelta(StoredAgentMessageDelta {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                item_id: value.item_id().as_str(),
                phase: value.phase().as_str(),
                delta: value.delta(),
                turn_id: value.turn_id().as_str(),
            })
        }
        Observation::AgentMessageCompleted(value) => {
            StoredObservation::AgentMessageCompleted(StoredAgentMessageCompleted {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                item_id: value.item_id().as_str(),
                phase: value.phase().as_str(),
                message: value.message(),
                turn_id: value.turn_id().as_str(),
            })
        }
        Observation::Approval(value) => StoredObservation::Approval(StoredApproval {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            approval_id: value.approval_id().as_str(),
            state: value.state().as_str(),
            description: value.description(),
            request: StoredApprovalRequest {
                kind: value.request().kind().as_str(),
                command: value.request().command_text(),
                cwd: value.request().cwd(),
                reason: value.request().reason(),
            },
            approved: value.approved(),
        }),
        Observation::Compaction(value) => StoredObservation::Compaction(StoredCompaction {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            state: value.state().as_str(),
            compaction_id: value.compaction_id().map(ObservationId::as_str),
            duration_ms: value.duration_ms(),
            summary: value.summary(),
        }),
        Observation::File(value) => StoredObservation::File(StoredFile {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            path: value.path(),
            action: value.action().as_str(),
            lines_added: value.lines_added(),
            lines_deleted: value.lines_deleted(),
        }),
        Observation::NativeAction(value) => StoredObservation::NativeAction(StoredNativeAction {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            action: value.action(),
            detail: value.detail(),
            diagnostic: value.diagnostic(),
            error_ref: value.error_ref().map(stored_error_ref),
        }),
        Observation::Plan(value) => {
            let mut entries = Vec::with_capacity(value.entries().len());
            for entry in value.entries() {
                entries.push(StoredPlanEntry {
                    id: entry.id().as_str(),
                    status: entry.status().as_str(),
                    text: entry.text(),
                });
            }
            StoredObservation::Plan(StoredPlan {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                entries,
                turn_id: value.turn_id().map(ObservationId::as_str),
            })
        }
        Observation::ProcessDiagnostic(value) => {
            StoredObservation::ProcessDiagnostic(StoredProcessDiagnostic {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                level: value.level().as_str(),
                message: value.message(),
                error_ref: value.error_ref().map(stored_error_ref),
            })
        }
        Observation::ProtocolDiagnostic(value) => {
            StoredObservation::ProtocolDiagnostic(StoredProtocolDiagnostic {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                level: value.level().as_str(),
                message: value.message(),
            })
        }
        Observation::Question(value) => {
            let options = value.options().map(|options| {
                let mut stored = Vec::with_capacity(options.len());
                for option in options {
                    stored.push(StoredQuestionOption {
                        label: option.label(),
                        description: option.description(),
                    });
                }
                stored
            });
            StoredObservation::Question(StoredQuestion {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                question_id: value.question_id().as_str(),
                state: value.state().as_str(),
                text: value.text(),
                header: value.header(),
                multi_select: value.multi_select(),
                options,
                answers: value.answers(),
            })
        }
        Observation::ReasoningSummaryCompleted(value) => {
            StoredObservation::ReasoningSummaryCompleted(StoredReasoningSummaryCompleted {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                item_id: value.item_id().as_str(),
                text: value.text(),
                turn_id: value.turn_id().as_str(),
            })
        }
        Observation::ReasoningSummaryDelta(value) => {
            StoredObservation::ReasoningSummaryDelta(StoredReasoningSummaryDelta {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                item_id: value.item_id().as_str(),
                summary_index: value.summary_index(),
                delta: value.delta(),
                thinking_tokens: value.thinking_tokens(),
                turn_id: value.turn_id().as_str(),
            })
        }
        Observation::Retry(value) => StoredObservation::Retry(StoredRetry {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            turn_id: value.turn_id().as_str(),
            attempt_state: value.attempt_state().as_str(),
            will_retry: value.will_retry(),
            message: value.message(),
        }),
        Observation::RunState(value) => StoredObservation::RunState(StoredRunState {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            state: value.state().as_str(),
        }),
        Observation::RunTerminal(value) => StoredObservation::RunTerminal(StoredRunTerminal {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            state: value.state().as_str(),
            error_ref: value.error_ref().map(stored_error_ref),
            summary_title: value.summary_title(),
        }),
        Observation::Search(value) => StoredObservation::Search(StoredSearch {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            query: value.query(),
            scope: value.scope().map(SearchScope::as_str),
            search_id: value.search_id().map(ObservationId::as_str),
            state: value.state().as_str(),
            result_count: value.result_count(),
        }),
        Observation::Subagent(value) => StoredObservation::Subagent(StoredSubagent {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            agent_native_thread_id: value.agent_native_thread_id().as_str(),
            parent_native_thread_id: value.parent_native_thread_id().as_str(),
            state: value.state().as_str(),
            activity: value.activity(),
            agent_path: value.agent_path(),
            turn_id: value.turn_id().map(ObservationId::as_str),
        }),
        Observation::SubagentTranscript(value) => {
            StoredObservation::SubagentTranscript(StoredSubagentTranscript {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                agent_native_thread_id: value.agent_native_thread_id().as_str(),
                parent_native_thread_id: value.parent_native_thread_id().as_str(),
                content: stored_transcript_content(value.content()),
            })
        }
        Observation::TerminalActivity(value) => {
            StoredObservation::TerminalActivity(StoredTerminalActivity {
                tag: observation.tag(),
                id: value.id().as_str(),
                sequence: value.sequence().get(),
                activity_id: value.activity_id().as_str(),
                channel: value.channel().map(TerminalChannel::as_str),
                command: value.command(),
                shell: value.shell(),
                output: value.output(),
                exit_code: value.exit_code(),
                state: value.state().as_str(),
            })
        }
        Observation::Tool(value) => StoredObservation::Tool(StoredTool {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            tool_id: value.tool_id().as_str(),
            tool_name: value.tool_name(),
            action: value.action().as_str(),
            detail: value.detail(),
        }),
        Observation::TurnState(value) => StoredObservation::TurnState(StoredTurnState {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            turn_id: value.turn_id().as_str(),
            state: value.state().as_str(),
        }),
        Observation::Usage(value) => StoredObservation::Usage(StoredUsage {
            tag: observation.tag(),
            id: value.id().as_str(),
            sequence: value.sequence().get(),
            basis: value.basis().as_str(),
            input_tokens: value.input_tokens(),
            cached_input_tokens: value.cached_input_tokens(),
            output_tokens: value.output_tokens(),
            context_tokens: value.context_tokens(),
            context_window_tokens: value.context_window_tokens(),
            cost_usd: value.cost_usd(),
            provider_route_id: value.provider_route_id().map(ObservationId::as_str),
            turn_id: value.turn_id().map(ObservationId::as_str),
        }),
    }
}
