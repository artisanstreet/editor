//! Dependency-free workspace tab state and construction policy.
//!
//! This is the native value boundary for the workspace tab-model TypeScript
//! types, state, and internal identifier/construction helpers. It owns the
//! workspace state shape, deterministic tab identity, clean tab construction,
//! initial-state defaults, and pure view/mode transitions. Tab mutations,
//! overflow and breadcrumb derivations, persistence, UI, and host effects
//! remain outside this module.

#![allow(clippy::module_name_repetitions)]

/// The currently selected workspace surface.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum WorkspaceMode {
    /// The file editor surface.
    #[default]
    Editor,
    /// The conversation surface.
    Chat,
    /// The orchestration graph surface.
    Orchestrator,
}

/// The file identity and display metadata carried by a workspace tab.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WorkspaceFileReference {
    /// Stable file identity.
    pub id: String,
    /// File name shown in a tab or recent-file list.
    pub name: String,
    /// Language identifier used by the editor.
    pub language: String,
    /// Workspace-relative or absolute file path.
    pub path: String,
}

impl WorkspaceFileReference {
    /// Creates a complete file reference from its four TypeScript fields.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        language: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            language: language.into(),
            path: path.into(),
        }
    }
}

impl From<&WorkspaceFileReference> for WorkspaceFileReference {
    fn from(file: &WorkspaceFileReference) -> Self {
        file.clone()
    }
}

/// The lifecycle ownership of a tab in the tab strip.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TabOwnership {
    /// A transient tab that can be replaced by another preview.
    Preview,
    /// A normal opened tab.
    Open,
    /// A tab retained by an explicit pin.
    Pinned,
}

/// The kind of content represented by a tab.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum TabContent {
    /// The current file contents.
    File,
    /// A preview of one particular file change.
    DiffPreview {
        /// Stable change identity supplied by the change projection.
        change_id: String,
    },
}

/// Whether a tab has edits that have not been persisted.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TabEditState {
    /// The tab has no local edits.
    Clean,
    /// The tab has local edits at the supplied monotonically increasing
    /// revision.
    Dirty {
        /// Local edit revision used by close confirmation fencing.
        revision: u64,
    },
}

/// A badge describing a change made by an agent to a file.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AgentChangeBadge {
    /// Display name of the agent that made the change.
    pub agent_name: String,
    /// Number of added lines.
    pub added: u64,
    /// Number of removed lines.
    pub removed: u64,
}

/// One tab in the workspace tab strip.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WorkspaceTab {
    /// Deterministic identity derived from the content and file identity.
    pub id: String,
    /// Monotonic state generation assigned when the tab is created.
    pub generation: u64,
    /// File represented by the tab.
    pub file: WorkspaceFileReference,
    /// Tab ownership in the workspace tab strip.
    pub ownership: TabOwnership,
    /// File or diff content represented by the tab.
    pub content: TabContent,
    /// Local edit status of the tab.
    pub edit_state: TabEditState,
    /// Optional agent-change badge attached to the tab.
    pub agent_change: Option<AgentChangeBadge>,
}

/// Editor-specific view state.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorViewState {
    /// Vertical editor scroll position.
    pub scroll_top: f64,
    /// One-based cursor line.
    pub cursor_line: u64,
    /// One-based cursor column.
    pub cursor_column: u64,
}

impl Default for EditorViewState {
    fn default() -> Self {
        Self {
            scroll_top: 0.0,
            cursor_line: 1,
            cursor_column: 1,
        }
    }
}

/// Chat-specific view state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatViewState {
    /// Unsubmitted chat draft.
    pub draft: String,
    /// Vertical transcript scroll position.
    pub transcript_scroll_top: f64,
}

/// Orchestrator-specific view state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OrchestratorViewState {
    /// Optional graph node selected by the reader.
    pub selected_node_id: Option<String>,
    /// Graph scroll position.
    pub graph_scroll_top: f64,
}

/// One changed file and the badge displayed for its agent change.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ChangedFile {
    /// File whose change is represented.
    pub file: WorkspaceFileReference,
    /// Agent change details for the file.
    pub change: AgentChangeBadge,
}

/// Complete dependency-free workspace state.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceState {
    /// Currently selected workspace surface.
    pub mode: WorkspaceMode,
    /// Tabs in tab-strip order.
    pub tabs: Vec<WorkspaceTab>,
    /// Active tab identity, if any tab is active.
    pub active_tab_id: Option<String>,
    /// Recent files, newest first.
    pub recent_files: Vec<WorkspaceFileReference>,
    /// Files with agent changes, newest first when populated by a caller.
    pub changed_files: Vec<ChangedFile>,
    /// Editor view state retained while other modes are selected.
    pub editor: EditorViewState,
    /// Chat view state retained while other modes are selected.
    pub chat: ChatViewState,
    /// Orchestrator view state retained while other modes are selected.
    pub orchestrator: OrchestratorViewState,
    /// Generation assigned to the next newly created tab.
    pub next_tab_generation: u64,
}

impl WorkspaceState {
    /// Creates the exact initial workspace state for the supplied files.
    ///
    /// Files become clean, open file tabs in input order. Their tab
    /// generations start at zero, the first tab is active, recent files are
    /// copied in reverse input order, and the next generation equals the
    /// number of initial tabs.
    #[must_use]
    pub fn new(initial_files: impl AsRef<[WorkspaceFileReference]>) -> Self {
        let initial_files = initial_files.as_ref();
        let tabs = initial_files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                make_file_tab(
                    file,
                    TabOwnership::Open,
                    TabContent::File,
                    generation_for_index(index),
                )
            })
            .collect::<Vec<_>>();

        let active_tab_id = tabs.first().map(|tab| tab.id.clone());
        let mut recent_files = initial_files.to_vec();
        recent_files.reverse();

        Self {
            mode: WorkspaceMode::Editor,
            tabs,
            active_tab_id,
            recent_files,
            changed_files: Vec::new(),
            editor: EditorViewState::default(),
            chat: ChatViewState::default(),
            orchestrator: OrchestratorViewState::default(),
            next_tab_generation: generation_for_index(initial_files.len()),
        }
    }

    /// Switches mode without changing any tab or per-mode view state.
    #[must_use]
    pub fn switch_mode(&self, mode: WorkspaceMode) -> Self {
        let mut next = self.clone();
        next.mode = mode;
        next
    }

    /// Replaces only the editor view state.
    #[must_use]
    pub fn update_editor_view(&self, editor: EditorViewState) -> Self {
        let mut next = self.clone();
        next.editor = editor;
        next
    }

    /// Replaces only the chat view state.
    #[must_use]
    pub fn update_chat_view(&self, chat: ChatViewState) -> Self {
        let mut next = self.clone();
        next.chat = chat;
        next
    }

    /// Replaces only the orchestrator view state.
    #[must_use]
    pub fn update_orchestrator_view(&self, orchestrator: OrchestratorViewState) -> Self {
        let mut next = self.clone();
        next.orchestrator = orchestrator;
        next
    }
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self::new(Vec::<WorkspaceFileReference>::new())
    }
}

/// Creates the exact initial workspace state for the supplied files.
#[must_use]
pub fn create_workspace_state(
    initial_files: impl AsRef<[WorkspaceFileReference]>,
) -> WorkspaceState {
    WorkspaceState::new(initial_files)
}

/// Returns the deterministic identity of a file tab.
#[must_use]
pub fn file_tab_id(file_id: &str) -> String {
    format!("file:{file_id}")
}

/// Returns the deterministic JSON-tuple identity of a diff-preview tab.
///
/// This matches JSON.stringify(["diff", \`file_id\`, \`change_id\`]), including JSON
/// string escaping for identifiers containing quotes, backslashes, or control
/// characters.
#[must_use]
pub fn diff_tab_id(file_id: &str, change_id: &str) -> String {
    format!(
        "[\"diff\",\"{}\",\"{}\"]",
        escape_json_string(file_id),
        escape_json_string(change_id)
    )
}

/// Constructs a tab with the deterministic identity and clean initial state.
#[must_use]
pub fn make_file_tab(
    file: impl Into<WorkspaceFileReference>,
    ownership: TabOwnership,
    content: TabContent,
    generation: u64,
) -> WorkspaceTab {
    let file = file.into();
    let id = match &content {
        TabContent::File => file_tab_id(&file.id),
        TabContent::DiffPreview { change_id } => diff_tab_id(&file.id, change_id),
    };

    WorkspaceTab {
        id,
        generation,
        file,
        ownership,
        content,
        edit_state: TabEditState::Clean,
        agent_change: None,
    }
}

/// Switches mode as a pure whole-state transition.
#[must_use]
pub fn switch_mode(state: &WorkspaceState, mode: WorkspaceMode) -> WorkspaceState {
    state.switch_mode(mode)
}

/// Replaces only the editor view as a pure whole-state transition.
#[must_use]
pub fn update_editor_view(state: &WorkspaceState, editor: EditorViewState) -> WorkspaceState {
    state.update_editor_view(editor)
}

/// Replaces only the chat view as a pure whole-state transition.
#[must_use]
pub fn update_chat_view(state: &WorkspaceState, chat: ChatViewState) -> WorkspaceState {
    state.update_chat_view(chat)
}

/// Replaces only the orchestrator view as a pure whole-state transition.
#[must_use]
pub fn update_orchestrator_view(
    state: &WorkspaceState,
    orchestrator: OrchestratorViewState,
) -> WorkspaceState {
    state.update_orchestrator_view(orchestrator)
}

fn generation_for_index(index: usize) -> u64 {
    u64::try_from(index).expect("workspace tab generation exceeds u64")
}

fn escape_json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());

    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0C}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character <= '\u{1F}' => {
                use std::fmt::Write as _;

                write!(escaped, "\\u{:04x}", u32::from(character))
                    .expect("writing a JSON escape to a String cannot fail");
            }
            character => escaped.push(character),
        }
    }

    escaped
}
