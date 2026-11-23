use crate::{BlobHash, ContentId, EngineId, ErrorCode, ItemId, ProjectId, ThreadId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "family", rename_all = "snake_case")]
pub enum Failure {
    #[error(transparent)]
    Thread(ThreadFailure),
    #[error(transparent)]
    Run(RunFailure),
    #[error(transparent)]
    Draft(DraftFailure),
    #[error(transparent)]
    Setting(SettingFailure),
    #[error(transparent)]
    Project(ProjectFailure),
    #[error(transparent)]
    Mcp(McpFailure),
    #[error(transparent)]
    Agent(AgentFailure),
    #[error(transparent)]
    Auth(AuthFailure),
    #[error(transparent)]
    Resource(ResourceFailure),
    #[error(transparent)]
    Extension(ExtensionFailure),
    #[error(transparent)]
    Engine(EngineFailure),
    #[error("Forge ran into a problem.")]
    Internal,
    #[error("Forge refused the call without saying why in a form this Editor reads.")]
    Legacy { code: ErrorCode },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThreadFailure {
    #[error("Thread {thread} doesn't exist on this Forge.")]
    NotFound { thread: ThreadId },
    #[error("Item {item} doesn't exist on this Forge.")]
    ItemNotFound { item: ItemId },
    #[error("Thread {thread} names parent thread {parent}, which doesn't exist.")]
    ParentNotFound { thread: ThreadId, parent: ThreadId },
    #[error("The settings of thread {thread} set a context window its engine doesn't offer.")]
    InvalidConfig { thread: ThreadId },
    #[error("The settings of thread {thread} are {bytes} bytes; they can be at most {limit}.")]
    ConfigTooLarge {
        thread: ThreadId,
        bytes: u64,
        limit: u64,
    },
    #[error("The title of thread {thread} is {bytes} bytes; it can be at most {limit}.")]
    TitleTooLong {
        thread: ThreadId,
        bytes: u64,
        limit: u64,
    },
    #[error("The search is {bytes} bytes; it can be at most {limit}.")]
    SearchTooLong { bytes: u64, limit: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunFailure {
    #[error("Thread {thread} is already working.")]
    AlreadyWorking { thread: ThreadId },
    #[error("Thread {thread} has no stopped turn to continue.")]
    NothingToContinue { thread: ThreadId },
    #[error("Thread {thread} has no goal.")]
    NoGoal { thread: ThreadId },
    #[error("The run of thread {thread} is ending; try again in a moment.")]
    Ending { thread: ThreadId },
    #[error("{engine} never started the session of thread {thread}.")]
    NoSession { thread: ThreadId, engine: EngineId },
    #[error("{engine} can't read the command sent to thread {thread}.")]
    CommandUnsupported { thread: ThreadId, engine: EngineId },
    #[error("Message id {item} was already used with different content or thread.")]
    MessageIdTaken { item: ItemId },
    #[error("The message is {bytes} bytes; it can be at most {limit}.")]
    TextTooLarge { bytes: u64, limit: u64 },
    #[error("The message has {count} images; it can have at most {limit}.")]
    TooManyImages { count: u64, limit: u64 },
    #[error("Image {blob} is {bytes} bytes; an image can be at most {limit}.")]
    ImageTooLarge {
        blob: BlobHash,
        bytes: u64,
        limit: u64,
    },
    #[error("The message's images total {bytes} bytes; they can total at most {limit}.")]
    ImagesTooLarge { bytes: u64, limit: u64 },
    #[error("Image {blob} is missing or invalid.")]
    ImageMissing { blob: BlobHash },
    #[error("The message sets a context window its engine doesn't offer.")]
    InvalidConfig,
    #[error("The message's settings are {bytes} bytes; they can be at most {limit}.")]
    ConfigTooLarge { bytes: u64, limit: u64 },
    #[error("{reason}")]
    AnswerRefused {
        item: Option<ItemId>,
        reason: AnswerRefusal,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum AnswerRefusal {
    #[error("That item isn't an approval request.")]
    NotAnApproval,
    #[error("This request doesn't offer that decision.")]
    DecisionNotOffered,
    #[error("This question isn't waiting for an answer.")]
    NotWaiting,
    #[error("The run that asked has ended.")]
    RunEnded,
    #[error("This question needs its engine, which has stopped.")]
    EngineStopped,
    #[error("This question was already answered differently.")]
    AnsweredDifferently,
    #[error("That item isn't a question.")]
    NotAQuestion,
    #[error("The answers don't match the questions: {detail}")]
    Invalid { detail: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DraftFailure {
    #[error(
        "Saving draft {scope} would make the saved drafts and settings {bytes} bytes; they can be at most {limit}."
    )]
    Full {
        scope: String,
        bytes: u64,
        limit: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingFailure {
    #[error("Setting {key} isn't one clients can change.")]
    Unknown { key: String },
    #[error("Setting {key} is saved through {route}.")]
    OwnRoute { key: String, route: String },
    #[error("Setting {key} doesn't take that value: {detail}.")]
    InvalidValue { key: String, detail: String },
    #[error(
        "Saving setting {key} would make the settings {bytes} bytes; they can be at most {limit}."
    )]
    Full { key: String, bytes: u64, limit: u64 },
    #[error("The {engine} model {model} is unavailable.")]
    ModelUnavailable { engine: EngineId, model: String },
    #[error("The settings for {sent} were sent to {engine}.")]
    ConfigEngine { engine: EngineId, sent: EngineId },
    #[error("The {engine} settings name no model.")]
    ConfigWithoutModel { engine: EngineId },
    #[error("The model customizations are invalid: {detail}")]
    InvalidCustomizations { detail: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectFailure {
    #[error("Project {project} doesn't exist on this Forge.")]
    NotFound { project: ProjectId },
    #[error("Project {project} was removed.")]
    Removed { project: ProjectId },
    #[error("{path} isn't a folder on this host.")]
    NotADirectory { path: String },
    #[error("Forge already has {limit} projects.")]
    TooMany { limit: u64 },
    #[error("The project path is {bytes} bytes; it can be at most {limit}.")]
    PathTooLong { bytes: u64, limit: u64 },
    #[error("The project name is {bytes} bytes; it can be at most {limit}.")]
    NameTooLong { bytes: u64, limit: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpFailure {
    #[error("MCP server {server} changed on another client; reload before saving.")]
    Changed { server: String },
    #[error("An MCP server needs a name.")]
    Unnamed,
    #[error("MCP server {server} needs a command or URL.")]
    TargetMissing { server: String },
    #[error("An MCP server named {server} already exists.")]
    NameTaken { server: String },
    #[error("There is no MCP server {server}.")]
    UnknownServer { server: String },
    #[error("{server} runs as a program; only HTTP servers sign in.")]
    NotHttp { server: String },
    #[error("{server} answers without signing in.")]
    Open { server: String },
    #[error("{server} doesn't offer OAuth sign-in; it needs a token in an Authorization header.")]
    NeedsToken { server: String },
    #[error("Couldn't check how {server} signs in: {detail}")]
    Probe { server: String, detail: String },
    #[error("Couldn't register Artisan to sign in to {server}: {detail}")]
    Registration { server: String, detail: String },
    #[error("Couldn't sign in to {server}: {detail}")]
    Exchange { server: String, detail: String },
    #[error("{server} was removed or changed while signing in; nothing was saved.")]
    ChangedDuringSignIn { server: String },
    #[error("{server} was signed out while signing in; nothing was saved.")]
    SignedOutDuringSignIn { server: String },
    #[error("That sign-in expired or was cancelled; start it again.")]
    SignInExpired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentFailure {
    #[error("Agent {agent} doesn't exist on this Forge.")]
    NotFound { agent: ItemId },
    #[error("Thread {thread} has no agent {agent}.")]
    NotInThread { thread: ThreadId, agent: ItemId },
    #[error("Agent thread {thread} of agent {agent} needs a parent thread.")]
    WithoutParent { thread: ThreadId, agent: ItemId },
    #[error("{agent} isn't an agent of thread {parent}.")]
    NotInParent {
        thread: ThreadId,
        parent: ThreadId,
        agent: ItemId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthFailure {
    #[error("The connection carries no client certificate.")]
    NoCertificate,
    #[error("This device isn't paired with Forge.")]
    NotPaired,
    #[error("That pairing code is wrong or expired.")]
    PairCode,
    #[error("This tunneled call isn't authorized.")]
    Tunnel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceFailure {
    #[error("Content {content} isn't stored on this Forge.")]
    ContentNotFound { content: ContentId },
    #[error("{content} isn't a content identity.")]
    InvalidContent { content: ContentId },
    #[error("A range of {limit} is outside {min}..{max}.")]
    RangeLimit { limit: u64, min: u64, max: u64 },
    #[error("Offset {offset} is past the end of {content}, which has {total}.")]
    OffsetPastEnd {
        content: ContentId,
        offset: u64,
        total: u64,
    },
    #[error("The {subject} is over its limit of {limit}.")]
    TooLarge {
        subject: Limited,
        actual: Option<u64>,
        limit: u64,
    },
    #[error("A collection record's identity is empty or longer than 4096 bytes.")]
    RecordIdentity,
    #[error("Collection {content} depends on itself.")]
    SelfDependency { content: ContentId },
    #[error("Collection dependency {content} isn't stored.")]
    MissingDependency { content: ContentId },
    #[error("The Markdown of {content} couldn't be indexed: {detail}")]
    Markdown { content: ContentId, detail: String },
    #[error("Blob {blob} isn't stored on this Forge.")]
    BlobNotFound { blob: BlobHash },
    #[error("The upload isn't a WebP image.")]
    NotWebp,
    #[error("The image is {width}x{height}; it can have at most {limit} pixels.")]
    TooManyPixels { width: u32, height: u32, limit: u64 },
    #[error("The upload was interrupted.")]
    UploadInterrupted,
    #[error("{path} doesn't exist.")]
    FileNotFound { path: String },
    #[error("{path} isn't a file URL Forge can read.")]
    InvalidUrl { path: String },
    #[error("{path} is in the WSL distribution {distribution}, which Forge doesn't run in.")]
    OtherDistribution { path: String, distribution: String },
    #[error("Forge's host has no home folder; give a full path.")]
    NoHome,
    #[error("{path} isn't a regular file of at most 128 MiB.")]
    NotAFile { path: String },
    #[error("{path} is outside the thread's folder.")]
    Outside { path: String },
    #[error("{path} is reached through a symbolic link, which Forge won't follow for the agent.")]
    Linked { path: String },
    #[error("{path} is covered by a permission rule, so Forge won't read it for the agent.")]
    Refused { path: String },
    #[error("{path} can't be listed.")]
    Unlistable { path: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum Limited {
    #[error("text")]
    Text,
    #[error("metadata field")]
    Field,
    #[error("number of field entries")]
    FieldEntries,
    #[error("item metadata")]
    Metadata,
    #[error("item data")]
    ItemData,
    #[error("collection record")]
    Record,
    #[error("collection")]
    Collection,
    #[error("answer's display text")]
    AnswerDisplay,
    #[error("source index")]
    SourceIndex,
    #[error("upload")]
    Upload,
    #[error("folder listing")]
    Listing,
    #[error("file")]
    File,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtensionFailure {
    #[error("Artisan doesn't manage skill {skill}.")]
    Unmanaged { skill: String },
    #[error("Couldn't change skill {skill}: {detail}")]
    Skill { skill: String, detail: String },
    #[error("Couldn't install {listing}: {detail}")]
    Install { listing: String, detail: String },
    #[error("Couldn't search skills.sh for {query:?}: {detail}")]
    Search { query: String, detail: String },
    #[error("The global prompt is {bytes} bytes; it can be at most {limit}.")]
    PromptTooLong { bytes: u64, limit: u64 },
    #[error("The global prompt changed to version {current} since version {base}; reload it.")]
    PromptChanged { base: u64, current: u64 },
    #[error("{engine}'s global prompt already matches.")]
    PromptNotDrifted { engine: EngineId },
    #[error("There is no prompt {candidate} to choose.")]
    PromptCandidateMissing { candidate: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineFailure {
    #[error("{engine} isn't installed.")]
    NotInstalled { engine: EngineId },
    #[error("{engine} has no build for this host.")]
    NoBuild { engine: EngineId },
    #[error("{version} isn't a version Forge can use.")]
    UnusableVersion { engine: EngineId, version: String },
    #[error("{engine} {version} is older than the oldest supported version {minimum}.")]
    VersionTooOld {
        engine: EngineId,
        version: String,
        minimum: String,
    },
    #[error("Couldn't list the {engine} versions: {detail}")]
    Versions { engine: EngineId, detail: String },
    #[error("No {engine} sign-in is in progress.")]
    NoSignIn { engine: EngineId },
    #[error("The {engine} sign-in no longer accepts input.")]
    SignInClosed { engine: EngineId },
    #[error("{engine} does not support API key sign-in here.")]
    ApiKeyUnsupported { engine: EngineId },
    #[error("Enter an API key for {engine}.")]
    ApiKeyInvalid { engine: EngineId },
    #[error("Only Claude signs in to claude.ai, not {engine}.")]
    ClaudeAiEngine { engine: EngineId },
    #[error("That browser isn't signed in to claude.ai.")]
    ClaudeAiSignedOut,
    #[error("The claude.ai session has no user agent.")]
    ClaudeAiNoUserAgent,
    #[error("claude.ai didn't take the session: {detail}")]
    ClaudeAiRefused { detail: String },
    #[error("claude.ai named no organization for the account.")]
    ClaudeAiNoOrganization,
}

impl Failure {
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Thread(failure) => failure.code(),
            Self::Run(failure) => failure.code(),
            Self::Draft(DraftFailure::Full { .. }) => ErrorCode::Conflict,
            Self::Setting(failure) => failure.code(),
            Self::Project(failure) => failure.code(),
            Self::Mcp(failure) => failure.code(),
            Self::Agent(failure) => failure.code(),
            Self::Auth(failure) => failure.code(),
            Self::Resource(failure) => failure.code(),
            Self::Extension(failure) => failure.code(),
            Self::Engine(failure) => failure.code(),
            Self::Internal => ErrorCode::Internal,
            Self::Legacy { code } => *code,
        }
    }

    #[must_use]
    pub const fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy { .. })
    }
}

impl ThreadFailure {
    const fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound { .. } | Self::ItemNotFound { .. } | Self::ParentNotFound { .. } => {
                ErrorCode::NotFound
            }
            Self::InvalidConfig { .. }
            | Self::ConfigTooLarge { .. }
            | Self::TitleTooLong { .. }
            | Self::SearchTooLong { .. } => ErrorCode::BadRequest,
        }
    }
}

impl RunFailure {
    const fn code(&self) -> ErrorCode {
        match self {
            Self::AlreadyWorking { .. }
            | Self::NothingToContinue { .. }
            | Self::NoGoal { .. }
            | Self::Ending { .. }
            | Self::NoSession { .. }
            | Self::AnswerRefused { .. } => ErrorCode::Conflict,
            Self::CommandUnsupported { .. }
            | Self::MessageIdTaken { .. }
            | Self::TextTooLarge { .. }
            | Self::TooManyImages { .. }
            | Self::ImageTooLarge { .. }
            | Self::ImagesTooLarge { .. }
            | Self::ImageMissing { .. }
            | Self::InvalidConfig
            | Self::ConfigTooLarge { .. } => ErrorCode::BadRequest,
        }
    }
}

impl SettingFailure {
    const fn code(&self) -> ErrorCode {
        match self {
            Self::Full { .. } | Self::InvalidCustomizations { .. } => ErrorCode::Conflict,
            Self::Unknown { .. }
            | Self::OwnRoute { .. }
            | Self::InvalidValue { .. }
            | Self::ModelUnavailable { .. }
            | Self::ConfigEngine { .. }
            | Self::ConfigWithoutModel { .. } => ErrorCode::BadRequest,
        }
    }
}

impl ProjectFailure {
    const fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound { .. } => ErrorCode::NotFound,
            Self::Removed { .. }
            | Self::TooMany { .. }
            | Self::PathTooLong { .. }
            | Self::NameTooLong { .. } => ErrorCode::Conflict,
            Self::NotADirectory { .. } => ErrorCode::BadRequest,
        }
    }
}

impl McpFailure {
    const fn code(&self) -> ErrorCode {
        match self {
            Self::UnknownServer { .. } => ErrorCode::NotFound,
            Self::Changed { .. }
            | Self::NotHttp { .. }
            | Self::Open { .. }
            | Self::NeedsToken { .. }
            | Self::Probe { .. }
            | Self::Registration { .. }
            | Self::Exchange { .. }
            | Self::ChangedDuringSignIn { .. }
            | Self::SignedOutDuringSignIn { .. }
            | Self::SignInExpired => ErrorCode::Conflict,
            Self::Unnamed | Self::TargetMissing { .. } | Self::NameTaken { .. } => {
                ErrorCode::BadRequest
            }
        }
    }
}

impl AgentFailure {
    const fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound { .. } | Self::NotInThread { .. } => ErrorCode::NotFound,
            Self::WithoutParent { .. } | Self::NotInParent { .. } => ErrorCode::BadRequest,
        }
    }
}

impl AuthFailure {
    const fn code(&self) -> ErrorCode {
        match self {
            Self::NoCertificate | Self::NotPaired | Self::PairCode | Self::Tunnel => {
                ErrorCode::Unauthorized
            }
        }
    }
}

impl ResourceFailure {
    const fn code(&self) -> ErrorCode {
        match self {
            Self::ContentNotFound { .. }
            | Self::BlobNotFound { .. }
            | Self::FileNotFound { .. } => ErrorCode::NotFound,
            Self::InvalidContent { .. }
            | Self::RangeLimit { .. }
            | Self::OffsetPastEnd { .. }
            | Self::TooLarge { .. }
            | Self::RecordIdentity
            | Self::SelfDependency { .. }
            | Self::MissingDependency { .. }
            | Self::Markdown { .. }
            | Self::NotWebp
            | Self::TooManyPixels { .. }
            | Self::UploadInterrupted
            | Self::InvalidUrl { .. }
            | Self::OtherDistribution { .. }
            | Self::NoHome
            | Self::NotAFile { .. }
            | Self::Outside { .. }
            | Self::Linked { .. }
            | Self::Refused { .. }
            | Self::Unlistable { .. } => ErrorCode::BadRequest,
        }
    }
}

impl ExtensionFailure {
    const fn code(&self) -> ErrorCode {
        match self {
            Self::Unmanaged { .. }
            | Self::Skill { .. }
            | Self::Install { .. }
            | Self::Search { .. }
            | Self::PromptTooLong { .. }
            | Self::PromptChanged { .. }
            | Self::PromptNotDrifted { .. }
            | Self::PromptCandidateMissing { .. } => ErrorCode::Conflict,
        }
    }
}

impl EngineFailure {
    const fn code(&self) -> ErrorCode {
        match self {
            Self::NotInstalled { .. }
            | Self::NoBuild { .. }
            | Self::UnusableVersion { .. }
            | Self::VersionTooOld { .. }
            | Self::Versions { .. }
            | Self::NoSignIn { .. }
            | Self::SignInClosed { .. }
            | Self::ApiKeyUnsupported { .. }
            | Self::ApiKeyInvalid { .. }
            | Self::ClaudeAiEngine { .. }
            | Self::ClaudeAiSignedOut
            | Self::ClaudeAiNoUserAgent
            | Self::ClaudeAiRefused { .. }
            | Self::ClaudeAiNoOrganization => ErrorCode::BadRequest,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub failure: Failure,
}

impl ApiError {
    #[must_use]
    pub fn new(failure: Failure, message: String) -> Self {
        Self {
            code: failure.code(),
            message,
            failure,
        }
    }

    #[must_use]
    pub fn legacy(code: ErrorCode, message: String) -> Self {
        Self {
            code,
            message,
            failure: Failure::Legacy { code },
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ApiError {}

#[derive(Serialize)]
struct WireOut<'a> {
    code: ErrorCode,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<&'a Failure>,
}

#[derive(Deserialize)]
struct WireIn {
    code: ErrorCode,
    message: String,
    #[serde(default)]
    failure: Option<serde_json::Value>,
}

impl Serialize for ApiError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        WireOut {
            code: self.code,
            message: &self.message,
            failure: (!self.failure.is_legacy()).then_some(&self.failure),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ApiError {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireIn::deserialize(deserializer)?;
        let legacy = Failure::Legacy { code: wire.code };
        let failure = match wire.failure.map(Failure::deserialize) {
            Some(Ok(failure)) if !failure.is_legacy() => failure,
            Some(Err(error)) => {
                tracing::warn!(
                    code = ?wire.code,
                    "read a refusal's typed failure this build doesn't know, so it stays untyped: {error}"
                );
                legacy
            }
            Some(Ok(_)) | None => legacy,
        };
        Ok(Self {
            code: wire.code,
            message: wire.message,
            failure,
        })
    }
}
