//! Observation identifiers and every enum wire conversion shared by encode and decode.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn parse_observation_id(
    value: String,
    field: &'static str,
) -> Result<ObservationId, ProtocolDecodeError> {
    ObservationId::parse(value).map_err(|source| ProtocolDecodeError::Identifier { field, source })
}

pub(crate) fn parse_observation_sequence(
    value: u64,
) -> Result<ObservationSequence, ProtocolDecodeError> {
    ObservationSequence::new(value).map_err(ProtocolDecodeError::from)
}

/// Maps an empty wire string to an absent optional value.
///
/// Every optional text below rejects empty content at the domain boundary,
/// so empty decodes as absent without loss. Required texts never pass
/// through here: they go to the constructors, which reject emptiness.
pub(crate) fn absent_if_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

pub(crate) const fn encode_observation_message_phase(
    value: MessagePhase,
) -> artisan_capnp::ObservationMessagePhase {
    match value {
        MessagePhase::Commentary => artisan_capnp::ObservationMessagePhase::Commentary,
        MessagePhase::Final => artisan_capnp::ObservationMessagePhase::Final,
        MessagePhase::Unspecified => artisan_capnp::ObservationMessagePhase::Unspecified,
    }
}

pub(crate) const fn decode_observation_message_phase(
    value: artisan_capnp::ObservationMessagePhase,
) -> MessagePhase {
    match value {
        artisan_capnp::ObservationMessagePhase::Commentary => MessagePhase::Commentary,
        artisan_capnp::ObservationMessagePhase::Final => MessagePhase::Final,
        artisan_capnp::ObservationMessagePhase::Unspecified => MessagePhase::Unspecified,
    }
}

pub(crate) const fn encode_observation_tool_action(
    value: ToolAction,
) -> artisan_capnp::ObservationToolAction {
    match value {
        ToolAction::Started => artisan_capnp::ObservationToolAction::Started,
        ToolAction::Progress => artisan_capnp::ObservationToolAction::Progress,
        ToolAction::Completed => artisan_capnp::ObservationToolAction::Completed,
        ToolAction::Failed => artisan_capnp::ObservationToolAction::Failed,
    }
}

pub(crate) const fn decode_observation_tool_action(
    value: artisan_capnp::ObservationToolAction,
) -> ToolAction {
    match value {
        artisan_capnp::ObservationToolAction::Started => ToolAction::Started,
        artisan_capnp::ObservationToolAction::Progress => ToolAction::Progress,
        artisan_capnp::ObservationToolAction::Completed => ToolAction::Completed,
        artisan_capnp::ObservationToolAction::Failed => ToolAction::Failed,
    }
}

pub(crate) const fn encode_observation_file_action(
    value: FileAction,
) -> artisan_capnp::ObservationFileAction {
    match value {
        FileAction::Created => artisan_capnp::ObservationFileAction::Created,
        FileAction::Modified => artisan_capnp::ObservationFileAction::Modified,
        FileAction::Deleted => artisan_capnp::ObservationFileAction::Deleted,
        FileAction::Read => artisan_capnp::ObservationFileAction::Read,
    }
}

pub(crate) const fn decode_observation_file_action(
    value: artisan_capnp::ObservationFileAction,
) -> FileAction {
    match value {
        artisan_capnp::ObservationFileAction::Created => FileAction::Created,
        artisan_capnp::ObservationFileAction::Modified => FileAction::Modified,
        artisan_capnp::ObservationFileAction::Deleted => FileAction::Deleted,
        artisan_capnp::ObservationFileAction::Read => FileAction::Read,
    }
}

pub(crate) const fn encode_observation_search_scope(
    value: SearchScope,
) -> artisan_capnp::ObservationSearchScope {
    match value {
        SearchScope::Workspace => artisan_capnp::ObservationSearchScope::Workspace,
        SearchScope::Web => artisan_capnp::ObservationSearchScope::Web,
    }
}

pub(crate) const fn decode_observation_search_scope(
    value: artisan_capnp::ObservationSearchScope,
) -> SearchScope {
    match value {
        artisan_capnp::ObservationSearchScope::Workspace => SearchScope::Workspace,
        artisan_capnp::ObservationSearchScope::Web => SearchScope::Web,
    }
}

pub(crate) const fn encode_observation_search_state(
    value: SearchState,
) -> artisan_capnp::ObservationSearchState {
    match value {
        SearchState::Started => artisan_capnp::ObservationSearchState::Started,
        SearchState::Completed => artisan_capnp::ObservationSearchState::Completed,
    }
}

pub(crate) const fn decode_observation_search_state(
    value: artisan_capnp::ObservationSearchState,
) -> SearchState {
    match value {
        artisan_capnp::ObservationSearchState::Started => SearchState::Started,
        artisan_capnp::ObservationSearchState::Completed => SearchState::Completed,
    }
}

pub(crate) const fn encode_observation_terminal_channel(
    value: TerminalChannel,
) -> artisan_capnp::ObservationTerminalChannel {
    match value {
        TerminalChannel::Stdout => artisan_capnp::ObservationTerminalChannel::Stdout,
        TerminalChannel::Stderr => artisan_capnp::ObservationTerminalChannel::Stderr,
    }
}

pub(crate) const fn decode_observation_terminal_channel(
    value: artisan_capnp::ObservationTerminalChannel,
) -> TerminalChannel {
    match value {
        artisan_capnp::ObservationTerminalChannel::Stdout => TerminalChannel::Stdout,
        artisan_capnp::ObservationTerminalChannel::Stderr => TerminalChannel::Stderr,
    }
}

pub(crate) const fn encode_observation_terminal_state(
    value: TerminalActivityState,
) -> artisan_capnp::ObservationTerminalState {
    match value {
        TerminalActivityState::Started => artisan_capnp::ObservationTerminalState::Started,
        TerminalActivityState::Output => artisan_capnp::ObservationTerminalState::Output,
        TerminalActivityState::Completed => artisan_capnp::ObservationTerminalState::Completed,
        TerminalActivityState::Failed => artisan_capnp::ObservationTerminalState::Failed,
    }
}

pub(crate) const fn decode_observation_terminal_state(
    value: artisan_capnp::ObservationTerminalState,
) -> TerminalActivityState {
    match value {
        artisan_capnp::ObservationTerminalState::Started => TerminalActivityState::Started,
        artisan_capnp::ObservationTerminalState::Output => TerminalActivityState::Output,
        artisan_capnp::ObservationTerminalState::Completed => TerminalActivityState::Completed,
        artisan_capnp::ObservationTerminalState::Failed => TerminalActivityState::Failed,
    }
}

pub(crate) const fn encode_observation_approval_state(
    value: ApprovalState,
) -> artisan_capnp::ObservationApprovalState {
    match value {
        ApprovalState::Requested => artisan_capnp::ObservationApprovalState::Requested,
        ApprovalState::Resolved => artisan_capnp::ObservationApprovalState::Resolved,
    }
}

pub(crate) const fn decode_observation_approval_state(
    value: artisan_capnp::ObservationApprovalState,
) -> ApprovalState {
    match value {
        artisan_capnp::ObservationApprovalState::Requested => ApprovalState::Requested,
        artisan_capnp::ObservationApprovalState::Resolved => ApprovalState::Resolved,
    }
}

pub(crate) const fn encode_observation_approval_kind(
    value: ApprovalKind,
) -> artisan_capnp::ObservationApprovalKind {
    match value {
        ApprovalKind::Command => artisan_capnp::ObservationApprovalKind::Command,
        ApprovalKind::FileChange => artisan_capnp::ObservationApprovalKind::FileChange,
        ApprovalKind::Action => artisan_capnp::ObservationApprovalKind::Action,
    }
}

pub(crate) const fn decode_observation_approval_kind(
    value: artisan_capnp::ObservationApprovalKind,
) -> ApprovalKind {
    match value {
        artisan_capnp::ObservationApprovalKind::Command => ApprovalKind::Command,
        artisan_capnp::ObservationApprovalKind::FileChange => ApprovalKind::FileChange,
        artisan_capnp::ObservationApprovalKind::Action => ApprovalKind::Action,
    }
}

pub(crate) const fn encode_observation_question_state(
    value: QuestionState,
) -> artisan_capnp::ObservationQuestionState {
    match value {
        QuestionState::Requested => artisan_capnp::ObservationQuestionState::Requested,
        QuestionState::Resolved => artisan_capnp::ObservationQuestionState::Resolved,
    }
}

pub(crate) const fn decode_observation_question_state(
    value: artisan_capnp::ObservationQuestionState,
) -> QuestionState {
    match value {
        artisan_capnp::ObservationQuestionState::Requested => QuestionState::Requested,
        artisan_capnp::ObservationQuestionState::Resolved => QuestionState::Resolved,
    }
}

pub(crate) const fn encode_observation_plan_entry_status(
    value: PlanEntryStatus,
) -> artisan_capnp::ObservationPlanEntryStatus {
    match value {
        PlanEntryStatus::Pending => artisan_capnp::ObservationPlanEntryStatus::Pending,
        PlanEntryStatus::InProgress => artisan_capnp::ObservationPlanEntryStatus::InProgress,
        PlanEntryStatus::Completed => artisan_capnp::ObservationPlanEntryStatus::Completed,
    }
}

pub(crate) const fn decode_observation_plan_entry_status(
    value: artisan_capnp::ObservationPlanEntryStatus,
) -> PlanEntryStatus {
    match value {
        artisan_capnp::ObservationPlanEntryStatus::Pending => PlanEntryStatus::Pending,
        artisan_capnp::ObservationPlanEntryStatus::InProgress => PlanEntryStatus::InProgress,
        artisan_capnp::ObservationPlanEntryStatus::Completed => PlanEntryStatus::Completed,
    }
}

pub(crate) const fn encode_observation_compaction_state(
    value: CompactionState,
) -> artisan_capnp::ObservationCompactionState {
    match value {
        CompactionState::Started => artisan_capnp::ObservationCompactionState::Started,
        CompactionState::Completed => artisan_capnp::ObservationCompactionState::Completed,
    }
}

pub(crate) const fn decode_observation_compaction_state(
    value: artisan_capnp::ObservationCompactionState,
) -> CompactionState {
    match value {
        artisan_capnp::ObservationCompactionState::Started => CompactionState::Started,
        artisan_capnp::ObservationCompactionState::Completed => CompactionState::Completed,
    }
}

pub(crate) const fn encode_observation_retry_attempt_state(
    value: RetryAttemptState,
) -> artisan_capnp::ObservationRetryAttemptState {
    match value {
        RetryAttemptState::Retrying => artisan_capnp::ObservationRetryAttemptState::Retrying,
        RetryAttemptState::Terminal => artisan_capnp::ObservationRetryAttemptState::Terminal,
    }
}

pub(crate) const fn decode_observation_retry_attempt_state(
    value: artisan_capnp::ObservationRetryAttemptState,
) -> RetryAttemptState {
    match value {
        artisan_capnp::ObservationRetryAttemptState::Retrying => RetryAttemptState::Retrying,
        artisan_capnp::ObservationRetryAttemptState::Terminal => RetryAttemptState::Terminal,
    }
}

pub(crate) const fn encode_observation_run_state(
    value: RunState,
) -> artisan_capnp::ObservationRunState {
    match value {
        RunState::Opening => artisan_capnp::ObservationRunState::Opening,
        RunState::Running => artisan_capnp::ObservationRunState::Running,
        RunState::Waiting => artisan_capnp::ObservationRunState::Waiting,
    }
}

pub(crate) const fn decode_observation_run_state(
    value: artisan_capnp::ObservationRunState,
) -> RunState {
    match value {
        artisan_capnp::ObservationRunState::Opening => RunState::Opening,
        artisan_capnp::ObservationRunState::Running => RunState::Running,
        artisan_capnp::ObservationRunState::Waiting => RunState::Waiting,
    }
}

pub(crate) const fn encode_observation_turn_state(
    value: TurnState,
) -> artisan_capnp::ObservationTurnState {
    match value {
        TurnState::Started => artisan_capnp::ObservationTurnState::Started,
        TurnState::Waiting => artisan_capnp::ObservationTurnState::Waiting,
        TurnState::Completed => artisan_capnp::ObservationTurnState::Completed,
        TurnState::Cancelled => artisan_capnp::ObservationTurnState::Cancelled,
        TurnState::Failed => artisan_capnp::ObservationTurnState::Failed,
    }
}

pub(crate) const fn decode_observation_turn_state(
    value: artisan_capnp::ObservationTurnState,
) -> TurnState {
    match value {
        artisan_capnp::ObservationTurnState::Started => TurnState::Started,
        artisan_capnp::ObservationTurnState::Waiting => TurnState::Waiting,
        artisan_capnp::ObservationTurnState::Completed => TurnState::Completed,
        artisan_capnp::ObservationTurnState::Cancelled => TurnState::Cancelled,
        artisan_capnp::ObservationTurnState::Failed => TurnState::Failed,
    }
}

pub(crate) const fn encode_observation_subagent_state(
    value: SubagentState,
) -> artisan_capnp::ObservationSubagentState {
    match value {
        SubagentState::Discovered => artisan_capnp::ObservationSubagentState::Discovered,
        SubagentState::Running => artisan_capnp::ObservationSubagentState::Running,
        SubagentState::Waiting => artisan_capnp::ObservationSubagentState::Waiting,
        SubagentState::Completed => artisan_capnp::ObservationSubagentState::Completed,
        SubagentState::Failed => artisan_capnp::ObservationSubagentState::Failed,
        SubagentState::Interrupted => artisan_capnp::ObservationSubagentState::Interrupted,
    }
}

pub(crate) const fn decode_observation_subagent_state(
    value: artisan_capnp::ObservationSubagentState,
) -> SubagentState {
    match value {
        artisan_capnp::ObservationSubagentState::Discovered => SubagentState::Discovered,
        artisan_capnp::ObservationSubagentState::Running => SubagentState::Running,
        artisan_capnp::ObservationSubagentState::Waiting => SubagentState::Waiting,
        artisan_capnp::ObservationSubagentState::Completed => SubagentState::Completed,
        artisan_capnp::ObservationSubagentState::Failed => SubagentState::Failed,
        artisan_capnp::ObservationSubagentState::Interrupted => SubagentState::Interrupted,
    }
}

pub(crate) const fn encode_observation_usage_basis(
    value: UsageBasis,
) -> artisan_capnp::ObservationUsageBasis {
    match value {
        UsageBasis::Delta => artisan_capnp::ObservationUsageBasis::Delta,
        UsageBasis::Cumulative => artisan_capnp::ObservationUsageBasis::Cumulative,
        UsageBasis::Unknown => artisan_capnp::ObservationUsageBasis::Unknown,
    }
}

pub(crate) const fn decode_observation_usage_basis(
    value: artisan_capnp::ObservationUsageBasis,
) -> UsageBasis {
    match value {
        artisan_capnp::ObservationUsageBasis::Delta => UsageBasis::Delta,
        artisan_capnp::ObservationUsageBasis::Cumulative => UsageBasis::Cumulative,
        artisan_capnp::ObservationUsageBasis::Unknown => UsageBasis::Unknown,
    }
}

pub(crate) const fn encode_observation_diagnostic_level(
    value: DiagnosticLevel,
) -> artisan_capnp::ObservationDiagnosticLevel {
    match value {
        DiagnosticLevel::Info => artisan_capnp::ObservationDiagnosticLevel::Info,
        DiagnosticLevel::Warning => artisan_capnp::ObservationDiagnosticLevel::Warning,
        DiagnosticLevel::Error => artisan_capnp::ObservationDiagnosticLevel::Error,
    }
}

pub(crate) const fn decode_observation_diagnostic_level(
    value: artisan_capnp::ObservationDiagnosticLevel,
) -> DiagnosticLevel {
    match value {
        artisan_capnp::ObservationDiagnosticLevel::Info => DiagnosticLevel::Info,
        artisan_capnp::ObservationDiagnosticLevel::Warning => DiagnosticLevel::Warning,
        artisan_capnp::ObservationDiagnosticLevel::Error => DiagnosticLevel::Error,
    }
}

pub(crate) const fn encode_observation_run_terminal_state(
    value: RunTerminalState,
) -> artisan_capnp::ObservationRunTerminalState {
    match value {
        RunTerminalState::Completed => artisan_capnp::ObservationRunTerminalState::Completed,
        RunTerminalState::Cancelled => artisan_capnp::ObservationRunTerminalState::Cancelled,
        RunTerminalState::Failed => artisan_capnp::ObservationRunTerminalState::Failed,
        RunTerminalState::Interrupted => artisan_capnp::ObservationRunTerminalState::Interrupted,
        RunTerminalState::Closed => artisan_capnp::ObservationRunTerminalState::Closed,
    }
}

pub(crate) const fn decode_observation_run_terminal_state(
    value: artisan_capnp::ObservationRunTerminalState,
) -> RunTerminalState {
    match value {
        artisan_capnp::ObservationRunTerminalState::Completed => RunTerminalState::Completed,
        artisan_capnp::ObservationRunTerminalState::Cancelled => RunTerminalState::Cancelled,
        artisan_capnp::ObservationRunTerminalState::Failed => RunTerminalState::Failed,
        artisan_capnp::ObservationRunTerminalState::Interrupted => RunTerminalState::Interrupted,
        artisan_capnp::ObservationRunTerminalState::Closed => RunTerminalState::Closed,
    }
}

pub(crate) const fn encode_observation_limit_scope(
    value: LimitScope,
) -> artisan_capnp::ObservationLimitScope {
    match value {
        LimitScope::Shared => artisan_capnp::ObservationLimitScope::Shared,
        LimitScope::Model => artisan_capnp::ObservationLimitScope::Model,
        LimitScope::Unknown => artisan_capnp::ObservationLimitScope::Unknown,
    }
}

pub(crate) const fn decode_observation_limit_scope(
    value: artisan_capnp::ObservationLimitScope,
) -> LimitScope {
    match value {
        artisan_capnp::ObservationLimitScope::Shared => LimitScope::Shared,
        artisan_capnp::ObservationLimitScope::Model => LimitScope::Model,
        artisan_capnp::ObservationLimitScope::Unknown => LimitScope::Unknown,
    }
}
