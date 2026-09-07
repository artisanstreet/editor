//! Pure presentation projection for the thread environment card.
//!
//! This is the dependency-free Rust counterpart of
//! `routes/components/thread-environment-card.svelte`. The caller supplies
//! already-decoded snapshots; this module only selects and copies the values
//! that the card presents. It does not read browser state, refresh a
//! controller, connect a machine, assign a URL, or render a widget.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

const DETACHED_HEAD_LABEL: &str = "Detached HEAD";
const FALLBACK_HOME_LABEL: &str = "This computer";
const FALLBACK_HOME_DETAIL: &str = "Return to this desktop's Forge";
const MACHINE_LABEL_FALLBACK: &str = "Not connected";
const RETURNING_DETAIL: &str = "Returning…";
const STARTING_DETAIL: &str = "Starting…";
const WSL_MACHINE_LABEL: &str = "This computer on WSL2";
const NO_BRANCH_LABEL: &str = "No branch";

/// The minimal host identity read by the environment card.
#[must_use = "retain the host identity snapshot"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HostIdentitySnapshot {
    /// The raw hostname used when no machine snapshot has supplied a label.
    pub hostname: String,
}

impl HostIdentitySnapshot {
    /// Creates an identity without trimming or otherwise changing its
    /// hostname.
    #[must_use = "retain the host identity snapshot"]
    pub fn new(hostname: impl Into<String>) -> Self {
        Self {
            hostname: hostname.into(),
        }
    }
}

/// Distinguishes WSL rows from every other machine row.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum HostMachineKind {
    /// The connected Forge's local host.
    Local,
    /// A WSL2 peer that can host another Forge.
    Wsl,
    /// A future or otherwise non-WSL machine kind retained verbatim.
    Other(String),
}

impl HostMachineKind {
    /// Classifies an exact machine-kind spelling without normalization.
    #[must_use = "retain the machine kind"]
    pub fn from_raw(raw: &str) -> Self {
        match raw {
            "local" => Self::Local,
            "wsl" => Self::Wsl,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Returns the exact protocol spelling represented by this kind.
    #[must_use]
    pub fn as_raw(&self) -> &str {
        match self {
            Self::Local => "local",
            Self::Wsl => "wsl",
            Self::Other(raw) => raw,
        }
    }

    /// Returns whether the kind receives WSL switching presentation.
    #[must_use]
    pub const fn is_wsl(&self) -> bool {
        matches!(self, Self::Wsl)
    }
}

impl From<&str> for HostMachineKind {
    fn from(raw: &str) -> Self {
        Self::from_raw(raw)
    }
}

impl From<String> for HostMachineKind {
    fn from(raw: String) -> Self {
        match raw.as_str() {
            "local" => Self::Local,
            "wsl" => Self::Wsl,
            _ => Self::Other(raw),
        }
    }
}

/// One machine row supplied by the host-machines snapshot.
#[must_use = "retain the machine snapshot"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HostMachineSnapshot {
    /// Optional secondary text, such as a hostname or distribution name.
    pub detail: Option<String>,
    /// Stable machine identifier used for switching and error matching.
    pub id: String,
    /// Machine kind; only [`HostMachineKind::Wsl`] is switchable here.
    pub kind: HostMachineKind,
    /// Primary machine label, retained exactly.
    pub label: String,
}

impl HostMachineSnapshot {
    /// Creates a machine with no secondary detail.
    #[must_use = "retain the machine snapshot"]
    pub fn new(id: impl Into<String>, kind: HostMachineKind, label: impl Into<String>) -> Self {
        Self {
            detail: None,
            id: id.into(),
            kind,
            label: label.into(),
        }
    }

    /// Adds secondary detail without normalizing it.
    #[must_use = "retain the machine snapshot"]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Ordered host-machines snapshot consumed by the card.
#[must_use = "retain the host-machines snapshot"]
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct HostMachinesSnapshot {
    /// Machines in their source order; the first machine is current.
    pub machines: Vec<HostMachineSnapshot>,
}

impl HostMachinesSnapshot {
    /// Creates a snapshot while retaining machine order and duplicates.
    #[must_use = "retain the host-machines snapshot"]
    pub fn new(machines: Vec<HostMachineSnapshot>) -> Self {
        Self { machines }
    }
}

/// The remembered desktop host offered by a WSL-hosted Forge.
#[must_use = "retain the remembered home host"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HomeHostMemory {
    /// Secondary text remembered for the desktop host.
    pub detail: Option<String>,
    /// Primary label remembered for the desktop host.
    pub label: String,
}

impl HomeHostMemory {
    /// Creates remembered home data without secondary detail.
    #[must_use = "retain the remembered home host"]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            detail: None,
            label: label.into(),
        }
    }

    /// Adds remembered secondary detail without changing its text.
    #[must_use = "retain the remembered home host"]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// The repository-state values relevant to environment presentation.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum RepositoryState {
    /// A Git repository snapshot whose branch and remotes are eligible.
    Repository,
    /// A known non-repository snapshot.
    NotRepository,
    /// A future or malformed state retained verbatim and treated as
    /// non-repository by the projection.
    Other(String),
}

impl RepositoryState {
    /// Classifies an exact repository-state spelling.
    #[must_use = "retain the repository state"]
    pub fn from_raw(raw: &str) -> Self {
        match raw {
            "repository" => Self::Repository,
            "not_repository" => Self::NotRepository,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Returns the exact raw spelling represented by this state.
    #[must_use]
    pub fn as_raw(&self) -> &str {
        match self {
            Self::Repository => "repository",
            Self::NotRepository => "not_repository",
            Self::Other(raw) => raw,
        }
    }

    /// Returns whether repository branch and remote data are eligible.
    #[must_use]
    pub const fn is_repository(&self) -> bool {
        matches!(self, Self::Repository)
    }
}

impl From<&str> for RepositoryState {
    fn from(raw: &str) -> Self {
        Self::from_raw(raw)
    }
}

impl From<String> for RepositoryState {
    fn from(raw: String) -> Self {
        match raw.as_str() {
            "repository" => Self::Repository,
            "not_repository" => Self::NotRepository,
            _ => Self::Other(raw),
        }
    }
}

/// The remote fields used by the environment card.
#[must_use = "retain the repository remote"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RepositoryRemote {
    /// Configured Git remote name.
    pub name: String,
    /// Optional browser-facing URL, retained exactly when supplied.
    pub web_url: Option<String>,
}

impl RepositoryRemote {
    /// Creates a remote with no browser URL.
    #[must_use = "retain the repository remote"]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            web_url: None,
        }
    }

    /// Adds a browser URL without parsing or normalizing it.
    #[must_use = "retain the repository remote"]
    pub fn with_web_url(mut self, web_url: impl Into<String>) -> Self {
        self.web_url = Some(web_url.into());
        self
    }
}

/// The repository fields consumed by the environment card.
#[must_use = "retain the repository snapshot"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProjectRepository {
    /// Repository state controlling branch and remote eligibility.
    pub state: RepositoryState,
    /// Repository branch, if the decoded snapshot carries one.
    pub branch: Option<GitBranchState>,
    /// Name used to select the default remote.
    pub default_remote: Option<String>,
    /// Remotes in source order, including duplicate names if supplied.
    pub remotes: Vec<RepositoryRemote>,
}

impl ProjectRepository {
    /// Creates a repository input from the fields read by the card.
    #[must_use = "retain the repository snapshot"]
    pub fn new(
        state: RepositoryState,
        branch: Option<GitBranchState>,
        default_remote: Option<String>,
        remotes: Vec<RepositoryRemote>,
    ) -> Self {
        Self {
            state,
            branch,
            default_remote,
            remotes,
        }
    }
}

/// The branch states needed by branch labels and branch-choice deduplication.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum GitBranchState {
    /// An attached branch with its exact name.
    Attached {
        /// The exact attached branch name.
        name: String,
    },
    /// A detached `HEAD`.
    Detached,
    /// An unborn branch with its exact name.
    Unborn {
        /// The exact unborn branch name.
        name: String,
    },
}

impl GitBranchState {
    /// Creates an attached branch without changing its name.
    #[must_use = "retain the branch state"]
    pub fn attached(name: impl Into<String>) -> Self {
        Self::Attached { name: name.into() }
    }

    /// Creates an unborn branch without changing its name.
    #[must_use = "retain the branch state"]
    pub fn unborn(name: impl Into<String>) -> Self {
        Self::Unborn { name: name.into() }
    }

    /// Creates a detached branch state.
    #[must_use = "retain the branch state"]
    pub const fn detached() -> Self {
        Self::Detached
    }

    /// Creates an attached branch from the generic named-branch vocabulary.
    #[must_use = "retain the branch state"]
    pub fn named(name: impl Into<String>) -> Self {
        Self::attached(name)
    }

    /// Returns the exact branch name for a named branch.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Attached { name } | Self::Unborn { name } => Some(name),
            Self::Detached => None,
        }
    }
}

/// A content-free Git worktree input.
#[must_use = "retain the worktree snapshot"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GitWorktree {
    /// The exact path shown as the worktree's secondary text.
    pub path: String,
    /// Whether this entry is the current worktree candidate.
    pub is_current: bool,
    /// The worktree branch, when present.
    pub branch: Option<GitBranchState>,
}

impl GitWorktree {
    /// Creates a worktree while retaining path, currentness, and branch.
    #[must_use = "retain the worktree snapshot"]
    pub fn new(path: impl Into<String>, is_current: bool, branch: Option<GitBranchState>) -> Self {
        Self {
            path: path.into(),
            is_current,
            branch,
        }
    }
}

/// The optional workspace projection read by the environment card.
#[must_use = "retain the workspace snapshot"]
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct GitWorkspace {
    /// Workspace branch, which takes precedence over repository branch when
    /// present.
    pub branch: Option<GitBranchState>,
    /// Aggregate workspace change counts, when supplied.
    pub aggregate: Option<GitDiffSummary>,
    /// Worktrees in their source order.
    pub worktrees: Vec<GitWorktree>,
}

impl GitWorkspace {
    /// Creates a workspace from the exact fields consumed by the card.
    #[must_use = "retain the workspace snapshot"]
    pub fn new(
        branch: Option<GitBranchState>,
        worktrees: Vec<GitWorktree>,
        aggregate: Option<GitDiffSummary>,
    ) -> Self {
        Self {
            branch,
            aggregate,
            worktrees,
        }
    }
}

/// Aggregate line-change counts shown in the Changes row.
#[must_use = "retain the change summary"]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GitDiffSummary {
    /// Number of added lines.
    pub lines_added: u64,
    /// Number of deleted lines.
    pub lines_deleted: u64,
}

impl GitDiffSummary {
    /// Creates an aggregate summary without clamping its counts.
    #[must_use = "retain the change summary"]
    pub const fn new(lines_added: u64, lines_deleted: u64) -> Self {
        Self {
            lines_added,
            lines_deleted,
        }
    }
}

/// A per-machine switch failure supplied by the caller-owned controller.
#[must_use = "retain the machine switch error"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MachineSwitchError {
    /// Machine identifier to which the error belongs.
    pub id: String,
    /// Exact user-facing error text.
    pub message: String,
}

impl MachineSwitchError {
    /// Creates an error without changing either identifier or message.
    #[must_use = "retain the machine switch error"]
    pub fn new(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            message: message.into(),
        }
    }
}

/// All already-decoded facts needed for one environment-card projection.
///
/// `machines` and `workspace` intentionally remain optional. An absent
/// machine snapshot behaves like an empty machine list, while an absent
/// workspace differs from a present workspace with zero worktrees because
/// only the former falls back to `project_root_path` in the worktree menu.
#[must_use = "retain the environment projection input"]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThreadEnvironmentInput {
    /// Whether the renderer is the desktop surface eligible for a home row.
    pub desktop: bool,
    /// Raw host identity used for the machine-label fallback.
    pub identity: Option<HostIdentitySnapshot>,
    /// Host-machines snapshot, if it has been observed.
    pub machines: Option<HostMachinesSnapshot>,
    /// Remembered desktop host, if session storage supplied one.
    pub home_host: Option<HomeHostMemory>,
    /// Project repository snapshot, if one was observed.
    pub repository: Option<ProjectRepository>,
    /// Thread workspace snapshot, if one was observed.
    pub workspace: Option<GitWorkspace>,
    /// Project-root fallback for current and listed worktree paths.
    pub project_root_path: Option<String>,
    /// Identifier currently being switched to, including `home`.
    pub switching: Option<String>,
    /// The most recent machine-specific switch error.
    pub switch_error: Option<MachineSwitchError>,
}

/// One ordered machine row after switch-state precedence is applied.
#[must_use = "retain the machine row projection"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MachineRowProjection {
    /// Stable machine identifier.
    pub id: String,
    /// Machine kind copied from the input.
    pub kind: HostMachineKind,
    /// Exact primary machine label.
    pub label: String,
    /// Resolved secondary detail, if the row has one.
    pub detail: Option<String>,
    /// Whether this row is disabled by an in-progress switch.
    pub disabled: bool,
    /// Whether the row is informational rather than a WSL switch action.
    pub informational: bool,
}

/// The desktop-home row after home and switch-state precedence is applied.
#[must_use = "retain the home row projection"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HomeRowProjection {
    /// The synthetic home-row identifier used by the switching state.
    pub id: String,
    /// Exact primary home label.
    pub label: String,
    /// Resolved secondary detail shown by the row.
    pub detail: Option<String>,
    /// Whether this row is disabled by an in-progress switch.
    pub disabled: bool,
}

/// A branch choice with its rendered label retained beside the branch value.
#[must_use = "retain the branch choice projection"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BranchChoiceProjection {
    /// The rendered branch label used as the JavaScript `Map` key.
    pub label: String,
    /// The later value retained for this label.
    pub branch: GitBranchState,
}

/// One worktree path and its display label.
#[must_use = "retain the worktree choice projection"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WorktreeChoiceProjection {
    /// The exact worktree path.
    pub path: String,
    /// The final non-empty slash-separated path segment, or the path itself.
    pub label: String,
}

/// The complete owned projection consumed by a renderer adapter.
#[must_use = "retain the environment-card projection"]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThreadEnvironmentProjection {
    /// Machine rows in snapshot order; home is kept separately.
    pub machine_rows: Vec<MachineRowProjection>,
    /// The first input machine, when one exists.
    pub current_machine: Option<HostMachineSnapshot>,
    /// Machine label shown in the card trigger.
    pub machine_label: String,
    /// Optional desktop-home row.
    pub home_row: Option<HomeRowProjection>,
    /// Whether the machine trigger needs a menu.
    pub machine_menu_required: bool,
    /// The selected branch value after workspace/repository fallback.
    pub current_branch: Option<GitBranchState>,
    /// Rendered label for the selected branch.
    pub current_branch_label: Option<String>,
    /// Ordered, deduplicated branch choices.
    pub branch_choices: Vec<BranchChoiceProjection>,
    /// The first current worktree entry, if any.
    pub current_worktree: Option<GitWorktree>,
    /// Current worktree path, falling back to project root.
    pub current_worktree_path: Option<String>,
    /// Rendered label for the current worktree path.
    pub current_worktree_label: Option<String>,
    /// Worktree paths in source order, preserving empty paths.
    pub worktree_paths: Vec<String>,
    /// Worktree paths paired with their rendered labels.
    pub worktree_choices: Vec<WorktreeChoiceProjection>,
    /// Workspace aggregate changes only.
    pub change_summary: Option<GitDiffSummary>,
    /// First matching default remote for a repository state.
    pub default_remote: Option<RepositoryRemote>,
}

/// Returns the exact branch label used by the legacy card.
///
/// `None` is represented as `No branch`; detached branches are represented as
/// `Detached HEAD`; attached and unborn branches retain their exact names.
#[must_use]
pub fn branch_label(branch: Option<&GitBranchState>) -> String {
    match branch {
        None => NO_BRANCH_LABEL.to_owned(),
        Some(GitBranchState::Detached) => DETACHED_HEAD_LABEL.to_owned(),
        Some(GitBranchState::Attached { name } | GitBranchState::Unborn { name }) => name.clone(),
    }
}

/// Returns the final non-empty segment after splitting on `/` or `\\`.
///
/// Separators are treated as a run because filtering empty segments produces
/// the same result as the legacy `/[\\/]+/u` split. An empty or
/// all-separator path therefore falls back to its original text.
#[must_use]
pub fn worktree_label(path: &str) -> String {
    path.split(['/', '\\'])
        .rfind(|segment| !segment.is_empty())
        .map_or_else(|| path.to_owned(), str::to_owned)
}

/// Projects one environment-card input into owned renderer-facing data.
///
/// The projection preserves source order and exact text. Machine rows use
/// switch-id precedence over matching errors, branch choices use ordered
/// replacement semantics equivalent to JavaScript `Map`, and workspace
/// presence controls whether project-root fallback paths are emitted.
#[must_use = "retain the environment-card projection"]
pub fn present_thread_environment(input: &ThreadEnvironmentInput) -> ThreadEnvironmentProjection {
    let machines = input
        .machines
        .as_ref()
        .map_or(&[][..], |snapshot| snapshot.machines.as_slice());
    let current_machine = machines.first().cloned();
    let machine_label = current_machine
        .as_ref()
        .map(|machine| machine.label.clone())
        .or_else(|| {
            input
                .identity
                .as_ref()
                .map(|identity| identity.hostname.clone())
        })
        .unwrap_or_else(|| MACHINE_LABEL_FALLBACK.to_owned());
    let home_row = build_home_row(input, current_machine.as_ref());
    let machine_menu_required = machines.len() > 1 || home_row.is_some();
    let machine_rows = machines
        .iter()
        .map(|machine| project_machine_row(machine, input))
        .collect();

    let current_branch = input
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.branch.clone())
        .or_else(|| repository_branch(input.repository.as_ref()));
    let current_branch_label = current_branch
        .as_ref()
        .map(|branch| branch_label(Some(branch)));
    let branch_choices = branch_choices(input.workspace.as_ref(), current_branch.as_ref());

    let current_worktree = input
        .workspace
        .as_ref()
        .and_then(|workspace| {
            workspace
                .worktrees
                .iter()
                .find(|worktree| worktree.is_current)
        })
        .cloned();
    let current_worktree_path = current_worktree
        .as_ref()
        .map(|worktree| worktree.path.clone())
        .or_else(|| input.project_root_path.clone());
    let current_worktree_label = current_worktree_path.as_deref().map(worktree_label);
    let worktree_paths =
        worktree_paths(input.workspace.as_ref(), input.project_root_path.as_deref());
    let worktree_choices = worktree_paths
        .iter()
        .map(|path| WorktreeChoiceProjection {
            label: worktree_label(path),
            path: path.clone(),
        })
        .collect();
    let change_summary = input
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.aggregate);
    let default_remote = default_remote(input.repository.as_ref());

    ThreadEnvironmentProjection {
        machine_rows,
        current_machine,
        machine_label,
        home_row,
        machine_menu_required,
        current_branch,
        current_branch_label,
        branch_choices,
        current_worktree,
        current_worktree_path,
        current_worktree_label,
        worktree_paths,
        worktree_choices,
        change_summary,
        default_remote,
    }
}

/// Alias using the vocabulary used by projection callers.
#[must_use = "retain the environment-card projection"]
pub fn project_thread_environment(input: &ThreadEnvironmentInput) -> ThreadEnvironmentProjection {
    present_thread_environment(input)
}

fn build_home_row(
    input: &ThreadEnvironmentInput,
    current_machine: Option<&HostMachineSnapshot>,
) -> Option<HomeRowProjection> {
    if !input.desktop
        || current_machine.map(|machine| machine.label.as_str()) != Some(WSL_MACHINE_LABEL)
    {
        return None;
    }

    let (label, remembered_detail) = input.home_host.as_ref().map_or_else(
        || (FALLBACK_HOME_LABEL.to_owned(), None),
        |home| (home.label.clone(), home.detail.clone()),
    );
    let detail = if input.switching.as_deref() == Some("home") {
        Some(RETURNING_DETAIL.to_owned())
    } else {
        Some(remembered_detail.unwrap_or_else(|| FALLBACK_HOME_DETAIL.to_owned()))
    };

    Some(HomeRowProjection {
        detail,
        disabled: input.switching.is_some(),
        id: String::from("home"),
        label,
    })
}

fn project_machine_row(
    machine: &HostMachineSnapshot,
    input: &ThreadEnvironmentInput,
) -> MachineRowProjection {
    let detail = if machine.kind.is_wsl() {
        if input.switching.as_deref() == Some(machine.id.as_str()) {
            Some(STARTING_DETAIL.to_owned())
        } else if input
            .switch_error
            .as_ref()
            .is_some_and(|error| error.id == machine.id)
        {
            input
                .switch_error
                .as_ref()
                .map(|error| error.message.clone())
        } else {
            machine.detail.clone()
        }
    } else {
        machine.detail.clone()
    };

    MachineRowProjection {
        detail,
        disabled: input.switching.is_some(),
        id: machine.id.clone(),
        informational: !machine.kind.is_wsl(),
        kind: machine.kind.clone(),
        label: machine.label.clone(),
    }
}

fn repository_branch(repository: Option<&ProjectRepository>) -> Option<GitBranchState> {
    repository
        .filter(|repository| repository.state.is_repository())
        .and_then(|repository| repository.branch.clone())
}

fn default_remote(repository: Option<&ProjectRepository>) -> Option<RepositoryRemote> {
    let repository = repository.filter(|repository| repository.state.is_repository())?;
    let default_name = repository.default_remote.as_deref()?;
    repository
        .remotes
        .iter()
        .find(|remote| remote.name == default_name)
        .cloned()
}

fn branch_choices(
    workspace: Option<&GitWorkspace>,
    current_branch: Option<&GitBranchState>,
) -> Vec<BranchChoiceProjection> {
    let mut choices = Vec::new();
    if let Some(workspace) = workspace {
        for branch in workspace
            .worktrees
            .iter()
            .filter_map(|worktree| worktree.branch.as_ref())
        {
            insert_branch_choice(&mut choices, branch);
        }
    }
    if let Some(branch) = current_branch {
        insert_branch_choice(&mut choices, branch);
    }
    choices
}

fn insert_branch_choice(choices: &mut Vec<BranchChoiceProjection>, branch: &GitBranchState) {
    let label = branch_label(Some(branch));
    if let Some(index) = choices.iter().position(|choice| choice.label == label) {
        choices[index] = BranchChoiceProjection {
            branch: branch.clone(),
            label,
        };
    } else {
        choices.push(BranchChoiceProjection {
            branch: branch.clone(),
            label,
        });
    }
}

fn worktree_paths(
    workspace: Option<&GitWorkspace>,
    project_root_path: Option<&str>,
) -> Vec<String> {
    workspace.map_or_else(
        || project_root_path.map_or_else(Vec::new, |path| vec![path.to_owned()]),
        |workspace| {
            workspace
                .worktrees
                .iter()
                .map(|worktree| worktree.path.clone())
                .collect()
        },
    )
}
