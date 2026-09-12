//! Canonical v1 observation checkpoint codec.

use serde::Serialize;
use serde_json::{Map, Value};
use thiserror::Error;

use artisan_domain::{
    AgentMessageCompletedObservation, AgentMessageDeltaObservation, ApprovalKind,
    ApprovalObservation, ApprovalRequest, ApprovalState, ArtisanCode, CompactionObservation,
    CompactionState, DiagnosticLevel, EngineErrorRef, EngineErrorRefInput, EngineId, FileAction,
    FileObservation, LimitScope, MessagePhase, NativeActionObservation, Observation,
    ObservationError, ObservationId, ObservationSequence, PlanEntry, PlanEntryStatus,
    PlanObservation, ProcessDiagnosticObservation, ProtocolDiagnosticObservation, QuestionInput,
    QuestionObservation, QuestionOption, QuestionState, ReasoningSummaryCompletedObservation,
    ReasoningSummaryDeltaObservation, RetryAttemptState, RetryObservation, RunState,
    RunStateObservation, RunTerminalObservation, RunTerminalState, SearchObservation, SearchScope,
    SearchState, SubagentInput, SubagentObservation, SubagentState, SubagentTranscriptObservation,
    TerminalActivityInput, TerminalActivityObservation, TerminalActivityState, TerminalChannel,
    ToolAction, ToolObservation, TranscriptAgentMessageCompleted, TranscriptAgentMessageDelta,
    TranscriptContent, TranscriptFile, TranscriptReasoningSummaryCompleted,
    TranscriptReasoningSummaryDelta, TranscriptSearch, TranscriptTerminalActivity, TranscriptTool,
    TurnState, TurnStateObservation, UsageBasis, UsageInput, UsageObservation,
};

use super::{ENGINE_CHECKPOINT_MAX_BYTES, EngineCheckpoint};
use crate::repository::run_binding::BoundRunReceipt;

mod decode;
mod encode;

pub use self::decode::{
    decode_observation_checkpoint, validate_observation_bind, validate_observation_engine,
};
pub use self::encode::{encode_observation_bytes, encode_observation_checkpoint};

/// Engine checkpoint version carrying a typed observation batch.
pub const OBSERVATION_CHECKPOINT_VERSION: i64 = 1;

/// Explicit format tag of every observation checkpoint payload.
pub const OBSERVATION_FORMAT_TAG: &str = "artisan.observation.v1";

/// Maximum observations in one committed batch.
///
/// Mirrors the conversation patch batch ceiling so one commit stays bounded
/// end to end.
pub const OBSERVATION_BATCH_MAX_OBSERVATIONS: usize = 64;

/// Typed failures of the observation checkpoint codec and bind agreement.
///
/// No variant carries observation text, identities, or provider payloads;
/// counts and lengths are bounded numbers only.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ObservationCommitError {
    /// The batch carried no observations.
    #[error("observation batch must carry at least one observation")]
    EmptyBatch,
    /// The batch exceeded its documented entry ceiling.
    #[error("observation batch has {count} observations; the maximum is {maximum}")]
    TooMany {
        /// Offending observation count.
        count: usize,
        /// The documented batch ceiling.
        maximum: usize,
    },
    /// The encoded payload exceeded the engine checkpoint byte ceiling.
    #[error("observation checkpoint is {length} bytes; the maximum is {maximum}")]
    TooLarge {
        /// Offending length in bytes.
        length: usize,
        /// The checkpoint byte ceiling.
        maximum: usize,
    },
    /// The binding version did not match the bound run.
    #[error("observation binding version does not match the bound run")]
    BindMismatch,
    /// The engine tag did not match the previously committed batch.
    #[error("observation engine tag does not match the committed batch")]
    EngineMismatch,
    /// The checkpoint version is not the observation version.
    #[error("observation checkpoint version does not match")]
    VersionMismatch,
    /// The checkpoint format tag is not the observation tag.
    #[error("observation checkpoint format tag does not match")]
    FormatMismatch,
    /// Observation sequences were not strictly increasing.
    #[error("observation sequences are not strictly increasing")]
    SequenceNotMonotonic,
    /// An observation tag is not a modeled engine observation.
    #[error("observation tag is unknown")]
    UnknownObservation,
    /// An engine tag is not a modeled engine.
    #[error("observation engine tag is unknown")]
    UnknownEngine,
    /// The checkpoint bytes are not a JSON observation envelope.
    #[error("observation checkpoint bytes are malformed")]
    Malformed,
    /// The checkpoint bytes decode but are not the canonical encoding.
    #[error("observation checkpoint bytes are not canonical")]
    NonCanonical,
    /// The batch could not be encoded.
    #[error("observation checkpoint could not be encoded")]
    Encode,
    /// The encoded payload failed checkpoint validation.
    #[error("observation checkpoint payload is invalid")]
    InvalidCheckpoint,
    /// One observation value violated its domain bounds.
    #[error(transparent)]
    InvalidObservation(#[from] ObservationError),
}

/// One decoded, engine-tagged observation batch.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedObservationBatch {
    engine: EngineId,
    binding_version: i64,
    observations: Vec<Observation>,
}

impl DecodedObservationBatch {
    /// Returns the engine tag the batch was committed under.
    #[must_use]
    pub const fn engine(&self) -> EngineId {
        self.engine
    }

    /// Returns the binding version the batch was committed under.
    #[must_use]
    pub const fn binding_version(&self) -> i64 {
        self.binding_version
    }

    /// Returns the decoded observations in durable sequence order.
    #[must_use]
    pub const fn observations(&self) -> &Vec<Observation> {
        &self.observations
    }

    /// Returns the greatest durable sequence in the batch, if any.
    #[must_use]
    pub fn max_sequence(&self) -> Option<u64> {
        self.observations
            .iter()
            .map(|observation| observation.sequence().get())
            .max()
    }
}
