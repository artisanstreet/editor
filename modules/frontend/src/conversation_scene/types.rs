//! Scene identity, narration, disclosure, phase, and activity vocabulary.
//!
//! Extracted verbatim from `conversation_scene.rs` during the module split.

#![allow(clippy::module_name_repetitions)]

#[allow(clippy::wildcard_imports)]
use super::*;

// ---------------------------------------------------------------------------
// Scene identity
// ---------------------------------------------------------------------------

/// Validation failure for [`SceneId`].
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum SceneIdError {
    /// The supplied value was empty.
    #[error("scene id must not be empty")]
    Empty,
    /// The supplied value contained whitespace or a control character.
    #[error("scene id must not contain whitespace or control characters; found {character:?}")]
    ForbiddenCharacter { character: char },
    /// The supplied value exceeded [`SCENE_ID_MAX_BYTES`] UTF-8 bytes.
    #[error("scene id is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong { length: usize, maximum: usize },
}

fn validate_scene_id(value: &str) -> Result<(), SceneIdError> {
    if value.is_empty() {
        return Err(SceneIdError::Empty);
    }
    if let Some(character) = value
        .chars()
        .find(|character| character.is_whitespace() || character.is_control())
    {
        return Err(SceneIdError::ForbiddenCharacter { character });
    }
    let length = value.len();
    if length > SCENE_ID_MAX_BYTES {
        return Err(SceneIdError::TooLong {
            length,
            maximum: SCENE_ID_MAX_BYTES,
        });
    }
    Ok(())
}

/// Bounded validated opaque scene identity for render-only records.
///
/// Synthetic work, change, and command records use this identity until a
/// later aggregate supplies a domain identity. Real domain [`ItemId`] values
/// can be converted losslessly when they are used as scene item identities.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SceneId(String);

impl SceneId {
    /// Validates and creates a scene identity.
    ///
    /// # Errors
    ///
    /// Returns [`SceneIdError`] on empty, forbidden-character, or overlong
    /// input.
    pub fn parse(value: impl Into<String>) -> Result<Self, SceneIdError> {
        let value = value.into();
        validate_scene_id(&value)?;
        Ok(Self(value))
    }

    /// Converts a validated domain item identity without changing its text.
    #[must_use]
    pub fn from_item_id(item_id: &ItemId) -> Self {
        Self(item_id.as_str().to_owned())
    }

    /// Returns the validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<ItemId> for SceneId {
    fn from(item_id: ItemId) -> Self {
        Self::from_item_id(&item_id)
    }
}

impl std::fmt::Display for SceneId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Disclosure and narration enums (closed, not booleans)
// ---------------------------------------------------------------------------

/// Explicit disclosure value copied into the exact owning group or card.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SceneDisclosure {
    /// The disclosure is open/expanded.
    Open,
    /// The disclosure is closed/collapsed.
    Closed,
}

/// Closed per-turn narration vocabulary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TurnNarration {
    /// Quiet status (no active work).
    Quiet,
    /// Waiting for a provider to respond.
    ProviderWait,
    /// Compaction is in progress.
    Compacting,
    /// Thinking.
    Thinking,
    /// Working.
    Working,
    /// An assistant reply is streaming; quiet status can be suppressed.
    StreamingSuppression,
    /// Waiting for background agents.
    BackgroundWait,
    /// Completed work with duration in milliseconds.
    WorkedFor { millis: u64 },
    /// Completed thought with duration in milliseconds.
    ThoughtFor { millis: u64 },
    /// Turn failed.
    Failed,
    /// Turn interrupted.
    Interrupted,
    /// Turn cancelled.
    Cancelled,
}

impl TurnNarration {
    /// Whether this narration belongs to live work that may carry an
    /// active-work clock basis. `Quiet` renders no row and terminal narrations
    /// carry their own settled durations, so neither may take a live basis.
    #[must_use]
    pub const fn is_active_work(self) -> bool {
        matches!(
            self,
            Self::ProviderWait
                | Self::Compacting
                | Self::Thinking
                | Self::Working
                | Self::StreamingSuppression
                | Self::BackgroundWait
        )
    }

    pub(super) fn terminal_label(self) -> Option<WorkGroupLabel> {
        match self {
            Self::WorkedFor { millis } => Some(WorkGroupLabel::WorkedFor { millis }),
            Self::ThoughtFor { millis } => Some(WorkGroupLabel::ThoughtFor { millis }),
            _ => None,
        }
    }
}

/// Per-turn narration entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnNarrationEntry {
    /// Owning turn.
    pub turn_id: TurnId,
    /// Closed narration value.
    pub narration: TurnNarration,
    /// Authoritative active-work clock basis in Unix millis, if any.
    ///
    /// This is the turn's first active-entry event time, supplied by the
    /// caller through the turn controller. It is never sampled from a clock
    /// here, so it stays stable across rerenders, and [`ConversationScene::build`]
    /// accepts it only alongside an active-work narration.
    pub active_started_at_ms: Option<i64>,
    /// Explicit disclosure for the turn's session group, when the aggregate
    /// resolved one. The build copies this onto the session group only;
    /// legacy positional groups keep item-derived disclosure.
    pub session_disclosure: Option<SceneDisclosure>,
    /// Explicit engine display label for the turn's status row, when the
    /// aggregate resolved one from authoritative send-time metadata.
    /// The build prefers this over transition-derived labels; it carries
    /// no work, session, or lifecycle meaning.
    pub engine_label: Option<String>,
    /// Typed engine that produced the turn, when the same send-time
    /// metadata named one. Presentation policy keys off this identity,
    /// never off the display label.
    pub engine: Option<EngineId>,
}

/// Send-time engine metadata for one turn: the typed engine identity, when
/// known, plus its display label.
///
/// Dereferences to the display label so label-only readers stay textual;
/// policy decisions read [`Self::engine`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnEngineLabel {
    engine: Option<EngineId>,
    label: String,
}

impl TurnEngineLabel {
    /// Labels one engine with its provider-owned roster display name.
    #[must_use]
    pub fn for_engine(engine: EngineId) -> Self {
        Self::new(
            Some(engine),
            crate::native_profile_usage::profile_usage_display_name(engine.as_str()).to_owned(),
        )
    }

    /// Pairs an engine identity (or none) with an explicit display label.
    #[must_use]
    pub const fn new(engine: Option<EngineId>, label: String) -> Self {
        Self { engine, label }
    }

    /// Returns the typed engine identity, when known.
    #[must_use]
    pub const fn engine(&self) -> Option<EngineId> {
        self.engine
    }

    /// Returns the display label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl std::ops::Deref for TurnEngineLabel {
    type Target = str;

    fn deref(&self) -> &str {
        &self.label
    }
}

impl TurnNarrationEntry {
    /// Creates a narration entry.
    #[must_use]
    pub fn new(turn_id: TurnId, narration: TurnNarration) -> Self {
        Self {
            turn_id,
            narration,
            active_started_at_ms: None,
            session_disclosure: None,
            engine_label: None,
            engine: None,
        }
    }

    /// Attaches aggregate-validated send-time engine metadata: the display
    /// label plus the typed engine identity, when known.
    #[must_use]
    pub fn with_turn_engine(self, engine: &TurnEngineLabel) -> Self {
        Self {
            engine_label: Some(engine.label().to_owned()),
            engine: engine.engine(),
            ..self
        }
    }

    /// Attaches the authoritative active-work clock basis to this entry.
    ///
    /// [`ConversationScene::build`] validates that the narration is active
    /// work; see [`SceneBuildError::ActiveBasisWithoutActiveNarration`].
    #[must_use]
    pub fn with_active_started_at_ms(self, started_at_ms: i64) -> Self {
        Self {
            active_started_at_ms: Some(started_at_ms),
            ..self
        }
    }

    /// Attaches the aggregate-resolved session disclosure for this turn.
    ///
    /// Copied onto the session group when the build derives one; ignored
    /// otherwise. Never fabricated here.
    #[must_use]
    pub fn with_session_disclosure(self, disclosure: SceneDisclosure) -> Self {
        Self {
            session_disclosure: Some(disclosure),
            ..self
        }
    }

    /// Attaches an aggregate-validated engine display label for this turn.
    ///
    /// The caller validates through [`validate_engine_label`]; the build
    /// prefers this label over transition-derived ones without inferring
    /// anything from it.
    #[must_use]
    pub fn with_engine_label(self, label: String) -> Self {
        Self {
            engine_label: Some(label),
            ..self
        }
    }
}

/// Steering placement anchored to an exact user-message item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteeringPlacement {
    /// Steering identity.
    pub id: SceneId,
    /// Anchor must be a user-message [`ItemId`].
    pub anchor: ItemId,
    /// Renderer-safe label.
    pub label: String,
}

impl SteeringPlacement {
    /// Creates a steering placement after validating its label.
    ///
    /// # Errors
    ///
    /// Returns [`SceneBuildError`] if the label is empty or exceeds
    /// [`SCENE_MAX_STEERING_LABEL_BYTES`] UTF-8 bytes.
    pub fn new(
        id: SceneId,
        anchor: ItemId,
        label: impl Into<String>,
    ) -> Result<Self, SceneBuildError> {
        let label = label.into();
        validate_steering_label(&label)?;
        Ok(Self { id, anchor, label })
    }
}

// ---------------------------------------------------------------------------
// Input vocabulary
// ---------------------------------------------------------------------------

/// Assistant message display phase supplied by the caller, not inferred.
///
/// A 1:1 copy of the disclosed domain phase: `Unspecified` and `Commentary`
/// never collapse into a streaming marker. Whether text is still arriving is
/// carried separately by item provenance lifecycle, never guessed from text.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AssistantPhase {
    /// No phase was disclosed for this text.
    Unspecified,
    /// Progress commentary rather than the settled reply.
    Commentary,
    /// The settled reply text.
    Final,
}

/// File change status for display.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FileChangeStatus {
    /// A file was added.
    Added,
    /// A file was modified.
    Modified,
    /// A file was removed.
    Removed,
    /// A file was renamed.
    Renamed,
}

// ---------------------------------------------------------------------------
// Activity category vocabulary (ported from the reference protocol helpers)
// ---------------------------------------------------------------------------

/// The semantic bucket one activity kind falls in.
///
/// A port of the reference protocol `ConversationActivityCategory` and its
/// `GetConversationActivityCategory` classifier, covering every category the
/// native provider kinds can reach (tool names arrive open-ended, terminal
/// rows are `terminal_activity`, timeline rows are their stable tags).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ActivityCategory {
    /// App/preview inspection.
    AppInspect,
    /// Shell or process command.
    Command,
    /// Database inspection.
    Database,
    /// Change/diff review.
    Diff,
    /// File deletion.
    FileDelete,
    /// File editing.
    FileEdit,
    /// File reading.
    FileRead,
    /// File or workspace search.
    FileSearch,
    /// Git status inspection.
    GitStatus,
    /// Integration/MCP use.
    Integration,
    /// No recognised semantics; the activity's own label is the only truth.
    Other,
    /// Subagent conversation.
    Subagent,
    /// Test run.
    Test,
    /// Generic tool use.
    Tool,
    /// Type checking.
    Typecheck,
    /// Web search or fetch.
    WebSearch,
}

impl ActivityCategory {
    /// Returns the stable foreground label shared by row and header.
    ///
    /// Ported from `GetConversationActivityCategoryLabel`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::AppInspect => "App",
            Self::Command => "Command",
            Self::Database => "Database",
            Self::Diff => "Changes",
            Self::FileDelete | Self::FileEdit | Self::FileRead | Self::FileSearch => "Files",
            Self::GitStatus => "Git",
            Self::Integration => "Integrations",
            Self::Other | Self::Tool => "Tools",
            Self::Subagent => "Subagents",
            Self::Test => "Tests",
            Self::Typecheck => "Types",
            Self::WebSearch => "Web",
        }
    }

    /// Returns the lowercase grouped-chain clause for `count` members.
    ///
    /// Ported from the reference `counted` copy so a chain reads as "ran 4
    /// commands, edited 2 files" instead of a bare count.
    #[must_use]
    pub fn count_label(self, count: usize) -> String {
        let plural = |singular: &str, many: &str| {
            if count == 1 {
                singular.to_owned()
            } else {
                format!("{count} {many}")
            }
        };
        match self {
            Self::AppInspect => {
                if count == 1 {
                    "inspected the app".to_owned()
                } else {
                    format!("ran {count} app inspections")
                }
            }
            Self::Command => format!("ran {}", plural("a command", "commands")),
            Self::Database => {
                if count == 1 {
                    "inspected the database".to_owned()
                } else {
                    format!("ran {count} database inspections")
                }
            }
            Self::Diff => {
                if count == 1 {
                    "reviewed changes".to_owned()
                } else {
                    format!("reviewed {count} diffs")
                }
            }
            Self::FileDelete => format!("deleted {}", plural("a file", "files")),
            Self::FileEdit => format!("edited {}", plural("a file", "files")),
            Self::FileRead => format!("read {}", plural("a file", "files")),
            Self::FileSearch => {
                if count == 1 {
                    "searched files".to_owned()
                } else {
                    format!("searched {count} files")
                }
            }
            Self::GitStatus => {
                if count == 1 {
                    "checked Git status".to_owned()
                } else {
                    format!("ran {count} Git status checks")
                }
            }
            Self::Integration => format!("used {}", plural("an integration", "integrations")),
            Self::Other | Self::Tool => format!("used {}", plural("a tool", "tools")),
            Self::Subagent => format!("talked to {}", plural("a subagent", "subagents")),
            Self::Test => {
                if count == 1 {
                    "ran tests".to_owned()
                } else {
                    format!("ran {count} test runs")
                }
            }
            Self::Typecheck => {
                if count == 1 {
                    "checked types".to_owned()
                } else {
                    format!("ran {count} type checks")
                }
            }
            Self::WebSearch => {
                if count == 1 {
                    "searched the web".to_owned()
                } else {
                    format!("ran {count} web searches")
                }
            }
        }
    }
}

/// Classifies one activity kind exactly like the reference helper.
///
/// Order is meaningful there and here: the narrower semantics are tested
/// before the broad `tool` catch, so a kind naming both still reads as the
/// specific work it did.
#[must_use]
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "the checked value is a lowercased backend activity id, not a filesystem path; the exact suffix match is the vocabulary contract"
)]
pub fn activity_category(kind: &str) -> ActivityCategory {
    let value = kind.to_lowercase().replace(['-', '_'], ".");

    if value.contains("terminal")
        || value.contains("command")
        || value.contains("shell")
        || value.contains("bash")
        || value.contains("exec")
    {
        return ActivityCategory::Command;
    }
    if value == "file"
        || value == "read"
        || value == "read.file"
        || value.contains("file.read")
        || value.contains("workspace.read")
        || value.ends_with(".read")
    {
        return ActivityCategory::FileRead;
    }
    if value.contains("file.delete") {
        return ActivityCategory::FileDelete;
    }
    if value == "write"
        || value == "edit"
        || value == "apply"
        || value.contains("file.edit")
        || value.contains("file.write")
        || value.contains("workspace.edit")
        || value.contains("workspace.write")
        || value.contains("apply.patch")
    {
        return ActivityCategory::FileEdit;
    }
    if value.contains("workspace.search")
        || value.contains("file.list")
        || value.contains("grep")
        || value.contains("glob")
        || value.contains("find")
        || value.contains("ripgrep")
    {
        return ActivityCategory::FileSearch;
    }
    if value == "search" || value.contains("web.search") || value.contains("fetch") {
        return ActivityCategory::WebSearch;
    }
    if value.contains("test") {
        return ActivityCategory::Test;
    }
    if value.contains("typescript") || value.contains("typecheck") {
        return ActivityCategory::Typecheck;
    }
    if value.contains("git.status") {
        return ActivityCategory::GitStatus;
    }
    if value.contains("diff") {
        return ActivityCategory::Diff;
    }
    if value.contains("database") {
        return ActivityCategory::Database;
    }
    if value.contains("preview")
        || value.contains("browser")
        || value.contains("ui.inspect")
        || value.contains("accessibility")
    {
        return ActivityCategory::AppInspect;
    }
    if value.contains("subagent") || value.contains("agent.activity") {
        return ActivityCategory::Subagent;
    }
    if value.contains("mcp") || value.contains("integration") {
        return ActivityCategory::Integration;
    }
    if value == "tool" || value.contains("tool") || value.contains("plugin") {
        return ActivityCategory::Tool;
    }

    ActivityCategory::Other
}

/// The reference presentation state one lifecycle maps onto.
///
/// `interrupted` joins `cancelled`: stopped without completing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActivityPresentationState {
    /// Failed, cancelled, or interrupted work.
    Failed,
    /// Settled or unknown work; unknown never means live.
    Completed,
    /// Work still in progress.
    Active,
}

/// Maps one native lifecycle onto the reference presentation state.
///
/// Native absent lifecycle means unknown, and unknown never means live, so it
/// settles as completed rather than claiming active work.
const fn activity_state(lifecycle: Option<ConversationLifecycle>) -> ActivityPresentationState {
    match lifecycle {
        Some(
            ConversationLifecycle::Failed
            | ConversationLifecycle::Cancelled
            | ConversationLifecycle::Interrupted,
        ) => ActivityPresentationState::Failed,
        Some(
            ConversationLifecycle::Pending
            | ConversationLifecycle::Streaming
            | ConversationLifecycle::Active
            | ConversationLifecycle::Waiting,
        ) => ActivityPresentationState::Active,
        Some(ConversationLifecycle::Completed) | None => ActivityPresentationState::Completed,
    }
}

/// Maps one activity's provider kind and lifecycle onto the reference
/// fallback label, used when the activity carries no detail of its own.
///
/// A port of `GetConversationActivityPresentation` with the activity's own
/// provider label being the kind (native rows carry no second label).
/// Returns `None` only for a blank unrecognised kind, where the reference
/// would render the activity's own empty label.
#[must_use]
pub fn activity_presentation_label(
    kind: &str,
    lifecycle: Option<ConversationLifecycle>,
) -> Option<String> {
    use ActivityPresentationState::{Active, Completed, Failed};

    let category = activity_category(kind);
    let state = activity_state(lifecycle);
    match category {
        ActivityCategory::Other => {
            let label = kind.trim();
            if label.is_empty() {
                None
            } else {
                Some(kind.to_owned())
            }
        }
        ActivityCategory::Tool if kind != "Tool" && kind != "Tools" => {
            let name = kind
                .trim()
                .split(|character: char| {
                    matches!(character, '.' | '_' | '-') || character.is_whitespace()
                })
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if name.is_empty() {
                return None;
            }
            Some(match state {
                Failed => {
                    let mut characters = name.chars();
                    match characters.next() {
                        Some(first) => {
                            format!("{}{} failed", first.to_uppercase(), characters.as_str())
                        }
                        None => format!("{name} failed"),
                    }
                }
                Completed => format!("Used {name}"),
                Active => format!("Using {name}"),
            })
        }
        ActivityCategory::Subagent => Some(match state {
            Failed => "Subagent work failed".to_owned(),
            Completed => "Talked to a subagent".to_owned(),
            Active => "Talking to a subagent".to_owned(),
        }),
        _ => Some(match state {
            Failed => match category {
                ActivityCategory::Command => "Command failed",
                ActivityCategory::Test => "Tests failed",
                ActivityCategory::Typecheck => "Type check failed",
                ActivityCategory::FileRead => "File read failed",
                ActivityCategory::FileEdit => "File edit failed",
                ActivityCategory::FileDelete => "File delete failed",
                ActivityCategory::FileSearch => "File search failed",
                ActivityCategory::WebSearch => "Web search failed",
                ActivityCategory::Diff => "Change review failed",
                ActivityCategory::GitStatus => "Git status failed",
                ActivityCategory::Database => "Database inspection failed",
                ActivityCategory::AppInspect => "App inspection failed",
                ActivityCategory::Integration => "Integration failed",
                ActivityCategory::Subagent => "Subagent work failed",
                ActivityCategory::Other | ActivityCategory::Tool => "Tool failed",
            }
            .to_owned(),
            Completed => match category {
                ActivityCategory::Command => "Ran a command",
                ActivityCategory::Test => "Ran tests",
                ActivityCategory::Typecheck => "Checked types",
                ActivityCategory::FileRead => "Read a file",
                ActivityCategory::FileEdit => "Edited a file",
                ActivityCategory::FileDelete => "Deleted a file",
                ActivityCategory::FileSearch => "Searched files",
                ActivityCategory::WebSearch => "Searched the web",
                ActivityCategory::Diff => "Reviewed changes",
                ActivityCategory::GitStatus => "Checked Git status",
                ActivityCategory::Database => "Inspected the database",
                ActivityCategory::AppInspect => "Inspected the app",
                ActivityCategory::Integration => "Used an integration",
                ActivityCategory::Subagent => "Talked to a subagent",
                ActivityCategory::Other | ActivityCategory::Tool => "Used a tool",
            }
            .to_owned(),
            Active => match category {
                ActivityCategory::Command => "Running a command",
                ActivityCategory::Test => "Running tests",
                ActivityCategory::Typecheck => "Checking types",
                ActivityCategory::FileRead => "Reading a file",
                ActivityCategory::FileEdit => "Editing a file",
                ActivityCategory::FileDelete => "Deleting a file",
                ActivityCategory::FileSearch => "Searching files",
                ActivityCategory::WebSearch => "Searching the web",
                ActivityCategory::Diff => "Reviewing changes",
                ActivityCategory::GitStatus => "Checking Git status",
                ActivityCategory::Database => "Inspecting the database",
                ActivityCategory::AppInspect => "Inspecting the app",
                ActivityCategory::Integration => "Using an integration",
                ActivityCategory::Subagent => "Talking to a subagent",
                ActivityCategory::Other | ActivityCategory::Tool => "Using a tool",
            }
            .to_owned(),
        }),
    }
}
