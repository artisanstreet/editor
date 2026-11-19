mod agent_names;
#[cfg(feature = "test-support")]
pub mod clock;
mod content;
mod engine;
mod event;
mod extensions;
mod failure;
mod ids;
mod item;
mod model;
mod reassembly;
mod request;
mod search;
mod secret;
mod settings;
mod source_index;
mod thread;
#[cfg(feature = "tls")]
pub mod tls;
pub mod transport;
mod tunnel;

pub use agent_names::{AGENT_NAMES, choose_agent_name, fallback_agent_name};
pub use content::{
    CONTENT_RANGE_BYTES, ContentId, ContentQuery, ContentRange, ContentRef, INLINE_TEXT_BYTES,
    TextContent,
};
pub use engine::{
    ContextWindow, EffortNote, EngineConfig, EngineId, EngineStatus, HARNESS_CONFIGS_KEY,
    InstallState, Integrity, LAST_PROJECT_KEY, MODEL_CONFIGS_KEY, MODEL_GROUPS_KEY, Model,
    Permissions, PublishedVersion, SignInState, SlashCommand, Speed, SpendLine, UsageCadence,
    UsageLimits, UsageSpend, UsageWindow, Variant, model_config_key, saved_model_config,
};
pub use event::{Envelope, Event, LiveCoverage, LiveTarget};
pub use extensions::{
    CatalogSkill, EngineCommand, GlobalPrompt, InstallListing, Listing, MCP_AUTH_KEY,
    McpAuthStatus, McpMutation, McpServer, McpSignInFinish, McpSignInStart, McpSignInStarted,
    McpSignOut, PromptCandidate, PromptEngineState, PromptEngineStatus, ResolvePrompt, SavePrompt,
    SkillAction, SkillEngineState, SkillEngineStatus, SkillOrigin,
};
pub use failure::{
    AgentFailure, AnswerRefusal, ApiError, AuthFailure, DraftFailure, EngineFailure,
    ExtensionFailure, Failure, Limited, McpFailure, ProjectFailure, ResourceFailure, RunFailure,
    SettingFailure, ThreadFailure,
};
pub use ids::{BlobHash, DeviceId, ItemId, ProjectId, RunId, Seq, ThreadId};
pub use item::{
    AgentAction, Approval, ApprovalRequest, Decision, ErrorKind, FileChange, FileOp, Item,
    ItemKind, MessageDirection, ModelSwitch, NamedModel, NoticeKind, Output, Phase, PlanStep,
    Question, QuestionAnswer, QuestionOption, SearchMode, Sender, Status, StepStatus, ToolKind,
    UnsandboxedReason,
};
pub use model::{
    Capability, ChoiceSemantic, ConfigBinding, CustomizationScope, DiscoveryState, FieldSource,
    HIDE_BAD_MODEL_OPTIONS_KEY, MODEL_CUSTOMIZATIONS_KEY, ModelAxis, ModelChoice, ModelControl,
    ModelCustomization, ModelCustomizationSet, ModelCustomizationsError, ModelEvidence,
    ModelIdentity, ModelSpecification, OptionOrigin, ResolutionChange, ResolvedModelConfig,
    SpeedLevel, ThinkingLevel, model_customizations,
};
pub use reassembly::{Reassembly, ReassemblyError};
pub use request::{
    Account, Answer, BlobRef, CreateProject, CreateThread, DatabaseRollback, DirEntry, DirListing,
    DraftRevision, DraftSnapshot, DraftWriteResult, EditTotals, ErrorCode, InspectorAgent,
    InspectorSummary, InspectorTerminal, InspectorWorkflow, ItemsPage, ItemsQuery, ModelGroup,
    OutlineEntry, PairRequest, PairResponse, PendingItem, ProjectRepository, SendMessage,
    SettleThread, Skill, State, ThreadCursor, ThreadGoal, ThreadGoalState, ThreadQuestionnaire,
    ThreadWindow, ThreadsPage, ThreadsQuery, ThreadsQueryError, UpdateThread, WorkSpan, WriteDraft,
};
pub use search::{TextHit, TextSearchQuery};
pub use secret::Secret;
pub use settings::{
    EditorCall, EditorReply, SETTINGS, SHOW_ALL_MODELS_KEY, SettingDecl, SettingKind, SettingOwner,
    SettingValueError, THREAD_RETENTION_DAYS_KEY, setting_decl,
};
pub use source_index::{
    SOURCE_INDEX_PAGE_LEAVES, SOURCE_INDEX_VERSION, SourceIndexFormat, SourceIndexHeader,
    SourceIndexQuery, SourceIndexRange, SourceLeafDescriptor,
};
pub use thread::{
    Continuation, Project, Run, RunAlreadyFinished, RunOutcome, RunState, RunStatus, RunWireError,
    Thread, Usage,
};
pub use transport::{
    CLAUDE_AI, CLAUDE_AI_SESSION_PREFIX, CLAUDE_CODE, ClaudeAiSession, ClaudeAiState,
    TRANSPORT_KEY, transport_of, transport_of_session,
};
pub use tunnel::{
    BYTES_ENVELOPE_KEY, BodyDecodeError, METHOD_SUBSCRIBE, METHOD_UNSUBSCRIBE,
    REASSEMBLY_MAX_BYTES_PER_CONN, STREAM_EVENT_KEY, StreamEvent, StreamStart, TRANSFER_MAX_BYTES,
    TUNNEL_BODY_MAX_BYTES, TUNNEL_PLAINTEXT_BUDGET, TunnelBytes, TunnelChunk, TunnelRequest,
    TunnelResponse, chunk_at, chunk_bytes, chunk_count, request_body_bytes,
};

pub type Timestamp = i64;

#[must_use]
pub fn now() -> Timestamp {
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    #[cfg(feature = "test-support")]
    let wall = wall.saturating_add(clock::offset());
    wall
}

pub const PROTOCOL_VERSION: u32 = 7;

mod collection;
pub use collection::{
    COLLECTION_RANGE_BYTES, COLLECTION_RANGE_ENTRIES, Collection, CollectionQuery, CollectionRange,
    CollectionRef,
};
