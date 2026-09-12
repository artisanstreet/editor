//! Tool, file, search, and terminal activity observations.

use super::{
    OBSERVATION_COMMAND_MAX_BYTES, OBSERVATION_LABEL_MAX_BYTES, OBSERVATION_OUTPUT_MAX_BYTES,
    OBSERVATION_PATH_MAX_BYTES, OBSERVATION_QUERY_MAX_BYTES, OBSERVATION_TEXT_MAX_BYTES,
    ObservationError, ObservationId, ObservationSequence, check_count, check_nonempty_text,
    check_optional_text, check_text,
};

// ---------------------------------------------------------------------------
// Tool / file / search / terminal activity
// ---------------------------------------------------------------------------

/// Lifecycle action of one tool invocation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ToolAction {
    /// The tool started.
    Started,
    /// The tool reported progress.
    Progress,
    /// The tool completed.
    Completed,
    /// The tool failed.
    Failed,
}

impl ToolAction {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Progress => "progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// Parses a provider-disclosed tool action.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "started" => Ok(Self::Started),
            "progress" => Ok(Self::Progress),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            _ => Err(ObservationError::UnknownValue { field: "action" }),
        }
    }
}

/// One tool lifecycle event without provider-specific tool types.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ToolObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    pub(super) tool_id: ObservationId,
    pub(super) tool_name: String,
    pub(super) action: ToolAction,
    pub(super) detail: Option<String>,
}

impl ToolObservation {
    /// Creates a tool observation after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the tool name
    /// is empty or exceeds [`OBSERVATION_LABEL_MAX_BYTES`] bytes, or a present
    /// detail is empty or exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        tool_id: ObservationId,
        tool_name: String,
        action: ToolAction,
        detail: Option<String>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&tool_name, "tool_name", OBSERVATION_LABEL_MAX_BYTES)?;
        check_optional_text(detail.as_deref(), "detail", OBSERVATION_TEXT_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            tool_id,
            tool_name,
            action,
            detail,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the provider tool invocation identity.
    #[must_use]
    pub const fn tool_id(&self) -> &ObservationId {
        &self.tool_id
    }

    /// Returns the provider tool name.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// Returns the lifecycle action.
    #[must_use]
    pub const fn action(&self) -> ToolAction {
        self.action
    }

    /// Returns the optional provider detail.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

/// File action of one file observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FileAction {
    /// The file was created.
    Created,
    /// The file was modified.
    Modified,
    /// The file was deleted.
    Deleted,
    /// The file was read.
    Read,
}

impl FileAction {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Read => "read",
        }
    }

    /// Parses a provider-disclosed file action.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "created" => Ok(Self::Created),
            "modified" => Ok(Self::Modified),
            "deleted" => Ok(Self::Deleted),
            "read" => Ok(Self::Read),
            _ => Err(ObservationError::UnknownValue { field: "action" }),
        }
    }
}

/// One file mutation or inspection performed during a run.
///
/// Line counts are present only when the engine reported enough to count
/// them. Absent means uncounted, never zero.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FileObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    pub(super) path: String,
    pub(super) action: FileAction,
    pub(super) lines_added: Option<u64>,
    pub(super) lines_deleted: Option<u64>,
}

impl FileObservation {
    /// Creates a file observation after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the path is
    /// empty or exceeds [`OBSERVATION_PATH_MAX_BYTES`] bytes, or a line count
    /// leaves its finite range.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        path: String,
        action: FileAction,
        lines_added: Option<u64>,
        lines_deleted: Option<u64>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&path, "path", OBSERVATION_PATH_MAX_BYTES)?;
        check_count(lines_added, "lines_added")?;
        check_count(lines_deleted, "lines_deleted")?;
        Ok(Self {
            id,
            sequence,
            path,
            action,
            lines_added,
            lines_deleted,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the affected path description.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the file action.
    #[must_use]
    pub const fn action(&self) -> FileAction {
        self.action
    }

    /// Returns counted added lines, or [`None`] when uncounted (never zero).
    #[must_use]
    pub const fn lines_added(&self) -> Option<u64> {
        self.lines_added
    }

    /// Returns counted deleted lines, or [`None`] when uncounted (never zero).
    #[must_use]
    pub const fn lines_deleted(&self) -> Option<u64> {
        self.lines_deleted
    }
}

/// Scope of one search observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SearchScope {
    /// The search looked at workspace files.
    Workspace,
    /// The search looked at the web.
    Web,
}

impl SearchScope {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Web => "web",
        }
    }

    /// Parses a provider-disclosed search scope.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "workspace" => Ok(Self::Workspace),
            "web" => Ok(Self::Web),
            _ => Err(ObservationError::UnknownValue { field: "scope" }),
        }
    }
}

/// Lifecycle state of one search observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SearchState {
    /// The search started.
    Started,
    /// The search completed.
    Completed,
}

impl SearchState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
        }
    }

    /// Parses a provider-disclosed search state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "started" => Ok(Self::Started),
            "completed" => Ok(Self::Completed),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// One search operation performed during a run.
///
/// An absent scope stays absent: renderers apply the historical web default,
/// but persistence never imputes it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SearchObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    pub(super) query: String,
    pub(super) scope: Option<SearchScope>,
    pub(super) search_id: Option<ObservationId>,
    pub(super) state: SearchState,
    pub(super) result_count: Option<u64>,
}

impl SearchObservation {
    /// Creates a search observation after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the query is
    /// empty or exceeds [`OBSERVATION_QUERY_MAX_BYTES`] bytes, or the result
    /// count leaves its finite range.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        query: String,
        scope: Option<SearchScope>,
        search_id: Option<ObservationId>,
        state: SearchState,
        result_count: Option<u64>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&query, "query", OBSERVATION_QUERY_MAX_BYTES)?;
        check_count(result_count, "result_count")?;
        Ok(Self {
            id,
            sequence,
            query,
            scope,
            search_id,
            state,
            result_count,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the search query.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Returns where the search looked, when the provider disclosed it.
    #[must_use]
    pub const fn scope(&self) -> Option<SearchScope> {
        self.scope
    }

    /// Returns the provider search identity, when the provider supplied one.
    #[must_use]
    pub const fn search_id(&self) -> Option<&ObservationId> {
        self.search_id.as_ref()
    }

    /// Returns the search lifecycle state.
    #[must_use]
    pub const fn state(&self) -> SearchState {
        self.state
    }

    /// Returns the reported result count, when the provider supplied one.
    #[must_use]
    pub const fn result_count(&self) -> Option<u64> {
        self.result_count
    }
}

/// Output channel of one terminal activity observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TerminalChannel {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

impl TerminalChannel {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }

    /// Parses a provider-disclosed terminal channel.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "stdout" => Ok(Self::Stdout),
            "stderr" => Ok(Self::Stderr),
            _ => Err(ObservationError::UnknownValue { field: "channel" }),
        }
    }
}

/// Lifecycle state of one terminal activity observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TerminalActivityState {
    /// The command started.
    Started,
    /// The command emitted output.
    Output,
    /// The command completed.
    Completed,
    /// The command failed.
    Failed,
}

impl TerminalActivityState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Output => "output",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// Parses a provider-disclosed terminal activity state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "started" => Ok(Self::Started),
            "output" => Ok(Self::Output),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// Validated values used to construct one terminal activity observation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TerminalActivityInput {
    /// Provider activity identity.
    pub activity_id: ObservationId,
    /// Output channel, when the provider disclosed one.
    pub channel: Option<TerminalChannel>,
    /// Command text that was run, when the provider disclosed it.
    pub command: Option<String>,
    /// Interpreter executable name (`bash`, `pwsh`, `nu`), when disclosed.
    pub shell: Option<String>,
    /// Output chunk, when the provider emitted one.
    pub output: Option<String>,
    /// Process exit code, when the activity reported one.
    pub exit_code: Option<i32>,
    /// Lifecycle state.
    pub state: TerminalActivityState,
}

/// One shell or process activity row, independent from the run outcome.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TerminalActivityObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    pub(super) activity_id: ObservationId,
    pub(super) channel: Option<TerminalChannel>,
    pub(super) command: Option<String>,
    shell: Option<String>,
    pub(super) output: Option<String>,
    pub(super) exit_code: Option<i32>,
    pub(super) state: TerminalActivityState,
}

impl TerminalActivityObservation {
    /// Creates a terminal activity observation after validating identities
    /// and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, a present
    /// command, shell, or output value is empty or exceeds its ceiling.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        input: TerminalActivityInput,
    ) -> Result<Self, ObservationError> {
        check_optional_text(
            input.command.as_deref(),
            "command",
            OBSERVATION_COMMAND_MAX_BYTES,
        )?;
        check_optional_text(input.shell.as_deref(), "shell", OBSERVATION_LABEL_MAX_BYTES)?;
        if let Some(output) = input.output.as_deref() {
            check_text(output, "output", OBSERVATION_OUTPUT_MAX_BYTES)?;
        }
        Ok(Self {
            id,
            sequence,
            activity_id: input.activity_id,
            channel: input.channel,
            command: input.command,
            shell: input.shell,
            output: input.output,
            exit_code: input.exit_code,
            state: input.state,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the provider activity identity.
    #[must_use]
    pub const fn activity_id(&self) -> &ObservationId {
        &self.activity_id
    }

    /// Returns the output channel, when disclosed.
    #[must_use]
    pub const fn channel(&self) -> Option<TerminalChannel> {
        self.channel
    }

    /// Returns the command text that was run, when disclosed.
    #[must_use]
    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    /// Returns the interpreter executable name, when disclosed.
    #[must_use]
    pub fn shell(&self) -> Option<&str> {
        self.shell.as_deref()
    }

    /// Returns the output chunk, when emitted.
    #[must_use]
    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    /// Returns the process exit code, when reported.
    #[must_use]
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Returns the activity lifecycle state.
    #[must_use]
    pub const fn state(&self) -> TerminalActivityState {
        self.state
    }
}
