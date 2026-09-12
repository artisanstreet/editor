//! Provider-neutral engine-socket vocabulary.
//!
//! This module is the first step of the `EngineSocket` migration: it ports the
//! closed tag vocabularies of the TypeScript engine package
//! (`modules/engines/src/engine.ts`) into dependency-free Rust domain types so
//! chat-facing code can name the provider-neutral surface without importing
//! the TypeScript engine package.
//!
//! Ported vocabularies, each mirroring its TypeScript union one for one:
//!
//! - [`EngineObservationTag`]: every `_tag` of the `EngineObservation` union
//!   (`engine.ts:967`);
//! - [`EngineCommandTag`]: every `_tag` of the `EngineCommand` union
//!   (`engine.ts:1028`);
//! - [`EngineRunTerminalState`]: the five outcomes that can settle a run
//!   (`engine.ts:684`);
//! - [`EngineCapabilityName`] and [`EngineCapabilityState`]: the capability
//!   surface an adapter declares (`engine.ts:11` and `engine.ts:8`);
//! - [`EngineResumeToken`]: the native-thread continuation identity
//!   (`engine.ts:311`).
//!
//! Deliberate boundaries:
//!
//! - The [`EngineSocket`] seam and its first payload shapes exist; they are
//!   pure data and route nothing by themselves.
//! - These are closed vocabularies: every variant is valid by construction,
//!   so there is no parsing or validation here.
//! - No transport, serialization, async runtime, or I/O.
//! - Variant order mirrors the TypeScript unions so both lists read side by
//!   side during review.

use thiserror::Error;

// ---------------------------------------------------------------------------
// Engine observations
// ---------------------------------------------------------------------------

/// Every `_tag` carried by the TypeScript `EngineObservation` union
/// (`engine.ts:967`).
///
/// The union has 22 members. Each variant carries the exact tag spelling of
/// its TypeScript counterpart; the per-variant payload fields arrive with the
/// future socket packets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineObservationTag {
    /// `agent_message_completed` (`engine.ts:637`).
    AgentMessageCompleted,
    /// `agent_message_delta` (`engine.ts:623`).
    AgentMessageDelta,
    /// `approval` (`engine.ts:818`).
    Approval,
    /// `compaction` (`engine.ts:857`).
    Compaction,
    /// `file` (`engine.ts:720`).
    File,
    /// `native_action` (`engine.ts:784`).
    NativeAction,
    /// `plan` (`engine.ts:648`).
    Plan,
    /// `process_diagnostic` (`engine.ts:959`).
    ProcessDiagnostic,
    /// `protocol_diagnostic` (`engine.ts:952`).
    ProtocolDiagnostic,
    /// `question` (`engine.ts:835`).
    Question,
    /// `reasoning_summary_completed` (`engine.ts:902`).
    ReasoningSummaryCompleted,
    /// `reasoning_summary_delta` (`engine.ts:877`).
    ReasoningSummaryDelta,
    /// `retry` (`engine.ts:943`).
    Retry,
    /// `run_state` (`engine.ts:408`).
    RunState,
    /// `run_terminal` (`engine.ts:659`).
    RunTerminal,
    /// `search` (`engine.ts:737`).
    Search,
    /// `subagent` (`engine.ts:421`).
    Subagent,
    /// `subagent_transcript` (`engine.ts:517`).
    SubagentTranscript,
    /// `terminal_activity` (`engine.ts:693`).
    TerminalActivity,
    /// `tool` (`engine.ts:711`).
    Tool,
    /// `turn_state` (`engine.ts:414`).
    TurnState,
    /// `usage` (`engine.ts:910`).
    Usage,
}

// ---------------------------------------------------------------------------
// Engine commands
// ---------------------------------------------------------------------------

/// Every `_tag` carried by the TypeScript `EngineCommand` union
/// (`engine.ts:1028`).
///
/// Five commands are accepted by a live run. `start` and `resume` open a run
/// instead and stay outside this union.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineCommandTag {
    /// `cancel` (`engine.ts:1016`).
    Cancel,
    /// `close` (`engine.ts:1022`).
    Close,
    /// `respond_approval` (`engine.ts:1001`).
    RespondApproval,
    /// `respond_question` (`engine.ts:1009`).
    RespondQuestion,
    /// `steer` (`engine.ts:992`).
    Steer,
}

// ---------------------------------------------------------------------------
// Run outcome
// ---------------------------------------------------------------------------

/// Names the only outcomes that can complete an engine run
/// (`engine.ts:684`).
///
/// `interrupted` separates "something ended this run from outside" from
/// `failed`, which claims the work itself went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineRunTerminalState {
    /// The run completed.
    Completed,
    /// The run was cancelled.
    Cancelled,
    /// The work itself went wrong.
    Failed,
    /// Something ended the run from outside.
    Interrupted,
    /// The run closed and released its scoped resources.
    Closed,
}

// ---------------------------------------------------------------------------
// Capability surface
// ---------------------------------------------------------------------------

/// Names a capability declared by an engine adapter (`engine.ts:11`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineCapabilityName {
    /// `approval`.
    Approval,
    /// `auth`.
    Auth,
    /// `cancel`.
    Cancel,
    /// `close`.
    Close,
    /// `events`.
    Events,
    /// `global_guidance`.
    GlobalGuidance,
    /// `model_catalog`.
    ModelCatalog,
    /// `model_selection`.
    ModelSelection,
    /// `native_continuation`.
    NativeContinuation,
    /// `native_tools`.
    NativeTools,
    /// `probe`.
    Probe,
    /// `question`.
    Question,
    /// `raw_frames`.
    RawFrames,
    /// `resume`.
    Resume,
    /// `start`.
    Start,
    /// `steer`.
    Steer,
    /// `subagents`.
    Subagents,
}

/// States the maturity of one provider capability (`engine.ts:8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineCapabilityState {
    /// The capability is fully supported.
    Supported,
    /// The capability exists but is not stable.
    Experimental,
    /// The capability is deliberately absent.
    Unsupported,
}

// ---------------------------------------------------------------------------
// Resume identity
// ---------------------------------------------------------------------------

/// Carries provider-owned state that can reopen a run without inventing a
/// checkpoint (`engine.ts:311`).
///
/// This packet freezes the required `native_thread_id` identity. The optional
/// `opaque_checkpoint` of the TypeScript shape is not modeled yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineResumeToken {
    /// Provider-owned native thread identity.
    pub native_thread_id: String,
}

// ---------------------------------------------------------------------------
// Engine socket seam
// ---------------------------------------------------------------------------

/// Immutable identity and capability report of one engine adapter.
///
/// Mirrors the `EngineDescriptor` of `modules/engines/src/engine.ts`: the
/// exact provider identity, the human display name, the wire transport
/// spelling, and every declared capability with its maturity state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineDescriptor {
    /// Exact provider identity (`codex`, and so on).
    pub id: String,
    /// Human-readable provider name.
    pub display_name: String,
    /// Wire transport spelling (for example `stdio-jsonl`).
    pub transport: String,
    /// Every declared capability with its maturity state.
    pub capabilities: Vec<(EngineCapabilityName, EngineCapabilityState)>,
}

/// Current readiness of one engine adapter.
///
/// Mirrors the payload of the TypeScript `probe` capability: readiness, the
/// observed provider version, and the settled state of the most recent run
/// when the adapter can report one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineProbe {
    /// Whether the adapter is ready to accept a run.
    pub ready: bool,
    /// Observed provider version.
    pub version: String,
    /// Settled state of the most recent run, when known.
    pub terminal_state: Option<EngineRunTerminalState>,
}

/// Input for opening one engine run.
///
/// Mirrors the `start` and `resume` inputs of
/// `modules/engines/src/engine.ts`: the working directory, the initial
/// prompt, and the provider-owned continuation token for a resumed run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineOpenInput {
    /// Working directory the run must execute in.
    pub working_directory: String,
    /// Initial prompt text.
    pub prompt: String,
    /// Provider-owned continuation identity for a resumed run.
    pub resume: Option<EngineResumeToken>,
}

/// Handle of one open engine run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineRun {
    /// Provider-owned native thread identity.
    pub native_thread_id: EngineResumeToken,
    /// Observation tag associated with this run.
    pub observation_tag: EngineObservationTag,
}

/// Failure opening one engine run.
///
/// Skeleton adapters return [`EngineOpenError::Unimplemented`]; typed
/// transport failures arrive with the wiring packets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum EngineOpenError {
    /// The adapter has no wired open path yet.
    #[error("engine socket open is not implemented yet")]
    Unimplemented,
}

/// Result of opening one engine run.
pub type EngineOpenResult = Result<EngineRun, EngineOpenError>;

/// Provider-neutral synchronous engine socket.
///
/// One adapter per provider implements this seam on top of its verified
/// launch capability. The owner keeps serving every live path through its
/// existing executors until a later packet routes them through this trait;
/// the skeleton adapters return [`EngineOpenError::Unimplemented`] from
/// [`EngineSocket::open`] and perform no I/O anywhere.
pub trait EngineSocket: Send {
    /// Returns the immutable provider descriptor.
    #[must_use]
    fn descriptor(&self) -> EngineDescriptor;

    /// Returns the current provider readiness.
    #[must_use]
    fn probe(&self) -> EngineProbe;

    /// Opens one run and returns its provider-native handle.
    ///
    /// # Errors
    ///
    /// Returns [`EngineOpenError`] when the adapter cannot open a run. The
    /// skeleton adapters always return [`EngineOpenError::Unimplemented`]
    /// until their wiring packets land.
    fn open(&self, input: EngineOpenInput) -> EngineOpenResult;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TypeScript `_tag` spelling of one observation kind, exhaustive over the
    /// enum so a new variant cannot silently skip the vocabulary check.
    const fn observation_tag_spelling(tag: EngineObservationTag) -> &'static str {
        match tag {
            EngineObservationTag::AgentMessageCompleted => "agent_message_completed",
            EngineObservationTag::AgentMessageDelta => "agent_message_delta",
            EngineObservationTag::Approval => "approval",
            EngineObservationTag::Compaction => "compaction",
            EngineObservationTag::File => "file",
            EngineObservationTag::NativeAction => "native_action",
            EngineObservationTag::Plan => "plan",
            EngineObservationTag::ProcessDiagnostic => "process_diagnostic",
            EngineObservationTag::ProtocolDiagnostic => "protocol_diagnostic",
            EngineObservationTag::Question => "question",
            EngineObservationTag::ReasoningSummaryCompleted => "reasoning_summary_completed",
            EngineObservationTag::ReasoningSummaryDelta => "reasoning_summary_delta",
            EngineObservationTag::Retry => "retry",
            EngineObservationTag::RunState => "run_state",
            EngineObservationTag::RunTerminal => "run_terminal",
            EngineObservationTag::Search => "search",
            EngineObservationTag::Subagent => "subagent",
            EngineObservationTag::SubagentTranscript => "subagent_transcript",
            EngineObservationTag::TerminalActivity => "terminal_activity",
            EngineObservationTag::Tool => "tool",
            EngineObservationTag::TurnState => "turn_state",
            EngineObservationTag::Usage => "usage",
        }
    }

    /// TypeScript `_tag` spelling of one command kind, exhaustive over the
    /// enum.
    const fn command_tag_spelling(tag: EngineCommandTag) -> &'static str {
        match tag {
            EngineCommandTag::Cancel => "cancel",
            EngineCommandTag::Close => "close",
            EngineCommandTag::RespondApproval => "respond_approval",
            EngineCommandTag::RespondQuestion => "respond_question",
            EngineCommandTag::Steer => "steer",
        }
    }

    /// TypeScript spelling of one terminal state, exhaustive over the enum.
    const fn terminal_state_spelling(state: EngineRunTerminalState) -> &'static str {
        match state {
            EngineRunTerminalState::Completed => "completed",
            EngineRunTerminalState::Cancelled => "cancelled",
            EngineRunTerminalState::Failed => "failed",
            EngineRunTerminalState::Interrupted => "interrupted",
            EngineRunTerminalState::Closed => "closed",
        }
    }

    /// TypeScript spelling of one capability name, exhaustive over the enum.
    const fn capability_name_spelling(name: EngineCapabilityName) -> &'static str {
        match name {
            EngineCapabilityName::Approval => "approval",
            EngineCapabilityName::Auth => "auth",
            EngineCapabilityName::Cancel => "cancel",
            EngineCapabilityName::Close => "close",
            EngineCapabilityName::Events => "events",
            EngineCapabilityName::GlobalGuidance => "global_guidance",
            EngineCapabilityName::ModelCatalog => "model_catalog",
            EngineCapabilityName::ModelSelection => "model_selection",
            EngineCapabilityName::NativeContinuation => "native_continuation",
            EngineCapabilityName::NativeTools => "native_tools",
            EngineCapabilityName::Probe => "probe",
            EngineCapabilityName::Question => "question",
            EngineCapabilityName::RawFrames => "raw_frames",
            EngineCapabilityName::Resume => "resume",
            EngineCapabilityName::Start => "start",
            EngineCapabilityName::Steer => "steer",
            EngineCapabilityName::Subagents => "subagents",
        }
    }

    /// TypeScript spelling of one capability state, exhaustive over the enum.
    const fn capability_state_spelling(state: EngineCapabilityState) -> &'static str {
        match state {
            EngineCapabilityState::Supported => "supported",
            EngineCapabilityState::Experimental => "experimental",
            EngineCapabilityState::Unsupported => "unsupported",
        }
    }

    #[test]
    fn observation_tags_match_the_typescript_union() {
        // `engine.ts:967-989` — the `EngineObservation` union has 22 members.
        let tags = [
            EngineObservationTag::AgentMessageCompleted,
            EngineObservationTag::AgentMessageDelta,
            EngineObservationTag::Approval,
            EngineObservationTag::Compaction,
            EngineObservationTag::File,
            EngineObservationTag::NativeAction,
            EngineObservationTag::Plan,
            EngineObservationTag::ProcessDiagnostic,
            EngineObservationTag::ProtocolDiagnostic,
            EngineObservationTag::Question,
            EngineObservationTag::ReasoningSummaryCompleted,
            EngineObservationTag::ReasoningSummaryDelta,
            EngineObservationTag::Retry,
            EngineObservationTag::RunState,
            EngineObservationTag::RunTerminal,
            EngineObservationTag::Search,
            EngineObservationTag::Subagent,
            EngineObservationTag::SubagentTranscript,
            EngineObservationTag::TerminalActivity,
            EngineObservationTag::Tool,
            EngineObservationTag::TurnState,
            EngineObservationTag::Usage,
        ];
        let expected_spellings = [
            "agent_message_completed",
            "agent_message_delta",
            "approval",
            "compaction",
            "file",
            "native_action",
            "plan",
            "process_diagnostic",
            "protocol_diagnostic",
            "question",
            "reasoning_summary_completed",
            "reasoning_summary_delta",
            "retry",
            "run_state",
            "run_terminal",
            "search",
            "subagent",
            "subagent_transcript",
            "terminal_activity",
            "tool",
            "turn_state",
            "usage",
        ];
        assert_eq!(tags.len(), 22);
        assert_eq!(tags.map(observation_tag_spelling), expected_spellings);
    }

    #[test]
    fn command_tags_match_the_typescript_union() {
        // `engine.ts:1028-1033` — the `EngineCommand` union has 5 members.
        let tags = [
            EngineCommandTag::Cancel,
            EngineCommandTag::Close,
            EngineCommandTag::RespondApproval,
            EngineCommandTag::RespondQuestion,
            EngineCommandTag::Steer,
        ];
        let expected_spellings = [
            "cancel",
            "close",
            "respond_approval",
            "respond_question",
            "steer",
        ];
        assert_eq!(tags.len(), 5);
        assert_eq!(tags.map(command_tag_spelling), expected_spellings);
    }

    #[test]
    fn terminal_states_match_the_typescript_union() {
        // `engine.ts:684-689` — `EngineRunTerminalState` has 5 members.
        let states = [
            EngineRunTerminalState::Completed,
            EngineRunTerminalState::Cancelled,
            EngineRunTerminalState::Failed,
            EngineRunTerminalState::Interrupted,
            EngineRunTerminalState::Closed,
        ];
        let expected_spellings = ["completed", "cancelled", "failed", "interrupted", "closed"];
        assert_eq!(states.len(), 5);
        assert_eq!(states.map(terminal_state_spelling), expected_spellings);
    }

    #[test]
    fn capability_names_match_the_typescript_union() {
        // `engine.ts:11-28` — `EngineCapabilityName` has 17 members.
        let names = [
            EngineCapabilityName::Approval,
            EngineCapabilityName::Auth,
            EngineCapabilityName::Cancel,
            EngineCapabilityName::Close,
            EngineCapabilityName::Events,
            EngineCapabilityName::GlobalGuidance,
            EngineCapabilityName::ModelCatalog,
            EngineCapabilityName::ModelSelection,
            EngineCapabilityName::NativeContinuation,
            EngineCapabilityName::NativeTools,
            EngineCapabilityName::Probe,
            EngineCapabilityName::Question,
            EngineCapabilityName::RawFrames,
            EngineCapabilityName::Resume,
            EngineCapabilityName::Start,
            EngineCapabilityName::Steer,
            EngineCapabilityName::Subagents,
        ];
        let expected_spellings = [
            "approval",
            "auth",
            "cancel",
            "close",
            "events",
            "global_guidance",
            "model_catalog",
            "model_selection",
            "native_continuation",
            "native_tools",
            "probe",
            "question",
            "raw_frames",
            "resume",
            "start",
            "steer",
            "subagents",
        ];
        assert_eq!(names.len(), 17);
        assert_eq!(names.map(capability_name_spelling), expected_spellings);
    }

    #[test]
    fn capability_states_match_the_typescript_union() {
        // `engine.ts:8` — `EngineCapabilityState` has 3 members.
        let states = [
            EngineCapabilityState::Supported,
            EngineCapabilityState::Experimental,
            EngineCapabilityState::Unsupported,
        ];
        let expected_spellings = ["supported", "experimental", "unsupported"];
        assert_eq!(states.len(), 3);
        assert_eq!(states.map(capability_state_spelling), expected_spellings);
    }

    #[test]
    fn resume_token_preserves_the_native_thread_identity() {
        // `engine.ts:311-314` — the required `native_thread_id` round-trips.
        let token = EngineResumeToken {
            native_thread_id: "native-thread-1".to_owned(),
        };
        let cloned = token.clone();
        assert_eq!(cloned, token);
        assert_eq!(cloned.native_thread_id, "native-thread-1");
    }
}
