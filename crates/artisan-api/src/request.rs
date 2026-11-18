use crate::{
    BlobHash, Decision, EngineConfig, EngineStatus, Item, ItemId, Project, ProjectId, Run, RunId,
    Seq, Thread, ThreadId,
};
use serde::{Deserialize, Serialize};

/// Everything a client needs on connect, besides item windows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub head: Seq,
    pub forge_version: String,
    pub protocol: u32,
    pub projects: Vec<Project>,
    pub threads: Vec<Thread>,
    #[serde(default)]
    pub threads_next: Option<ThreadCursor>,
    /// Runs still live, plus the latest run of every thread.
    pub runs: Vec<Run>,
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub engines: Vec<EngineStatus>,
    /// Pending interaction identities; bodies are fetched only for an opened thread.
    #[serde(default)]
    pub waiting: Vec<PendingItem>,
    /// The account Forge runs as, shown as the Editor's profile.
    #[serde(default)]
    pub account: Option<Account>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingItem {
    pub id: ItemId,
    pub thread_id: ThreadId,
}

/// The operating-system user Forge runs as, on its host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// The user's login name.
    pub user: String,
    /// The host's name.
    pub host: String,
    /// The WSL distribution Forge runs in, when it runs in one: lets a
    /// Windows Editor start that distribution when Forge is unreachable.
    #[serde(default)]
    pub wsl_distro: Option<String>,
}

/// What git says about a project's folder, read when asked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRepository {
    /// The checked-out branch; `None` on a detached head.
    pub branch: Option<String>,
    /// The default remote's host: `github`, `gitlab`, `bitbucket`,
    /// `codeberg`, `gitea`, `sourcehut`, `azure` or `other`.
    pub host: Option<String>,
    /// The default remote's browser page, when it has one.
    pub web_url: Option<String>,
    /// The linked worktree the folder is in, by its folder name; `None` in
    /// the main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateProject {
    pub id: ProjectId,
    pub path: String,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateThread {
    pub id: ThreadId,
    pub project_id: ProjectId,
    pub config: EngineConfig,
    pub title: Option<String>,
    /// Hand off from another thread: the new one starts with a summary.
    /// With `spawned_by`, the parent of an agent instead, which starts with
    /// only its prompt.
    #[serde(default)]
    pub parent_thread_id: Option<ThreadId>,
    /// The `Agent` item in the parent thread this thread runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<ItemId>,
}

/// Opens or closes one of an engine's model groups in the picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelGroup {
    pub name: String,
    pub open: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateThread {
    pub title: Option<String>,
    pub archived: Option<bool>,
    pub config: Option<EngineConfig>,
    pub read_seq: Option<Seq>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemsQuery {
    pub before: Option<Seq>,
    pub after: Option<Seq>,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemsPage {
    /// Ordered by `position`.
    pub items: Vec<Item>,
    /// Canonical metadata for the runs represented by this page.
    #[serde(default)]
    pub runs: Vec<Run>,
    /// More items exist past the far end of this page.
    pub more: bool,
    /// On a transcript page, the work between its rows that it did not
    /// ship, by position.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub work: Vec<WorkSpan>,
}

/// One run's unbroken stretch of work, summarised.
///
/// These are root items that fold into work (see
/// [`crate::Item::folds_into_work`]): a closed work section is drawn from
/// this alone, an open one loads its items through `/work`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkSpan {
    pub run_id: Option<RunId>,
    /// Positions of its oldest and newest item.
    pub first: Seq,
    pub last: Seq,
    pub count: u32,
    /// It holds a tool call, agent or plan, not only commentary.
    pub worked: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ThreadWindow {
    #[default]
    Latest,
    Before(ThreadCursor),
    After(ThreadCursor),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ThreadsQueryWire", into = "ThreadsQueryWire")]
pub struct ThreadsQuery {
    pub archived: bool,
    pub window: ThreadWindow,
    pub limit: Option<u32>,
    pub query: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ThreadsQueryWire {
    #[serde(default)]
    archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before_activity: Option<Seq>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    before_id: Option<ThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after_activity: Option<Seq>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after_id: Option<ThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    query: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ThreadsQueryError {
    #[error("a thread cursor needs both its activity and its id")]
    IncompleteCursor,
    #[error("a thread page is either before or after its cursor, not both")]
    BothDirections,
}

fn thread_cursor(
    activity: Option<Seq>,
    id: Option<ThreadId>,
) -> Result<Option<ThreadCursor>, ThreadsQueryError> {
    match (activity, id) {
        (None, None) => Ok(None),
        (Some(activity_seq), Some(id)) => Ok(Some(ThreadCursor { activity_seq, id })),
        (Some(_), None) | (None, Some(_)) => Err(ThreadsQueryError::IncompleteCursor),
    }
}

impl TryFrom<ThreadsQueryWire> for ThreadsQuery {
    type Error = ThreadsQueryError;

    fn try_from(wire: ThreadsQueryWire) -> Result<Self, Self::Error> {
        let before = thread_cursor(wire.before_activity, wire.before_id)?;
        let after = thread_cursor(wire.after_activity, wire.after_id)?;
        let window = match (before, after) {
            (None, None) => ThreadWindow::Latest,
            (Some(cursor), None) => ThreadWindow::Before(cursor),
            (None, Some(cursor)) => ThreadWindow::After(cursor),
            (Some(_), Some(_)) => return Err(ThreadsQueryError::BothDirections),
        };
        Ok(Self {
            archived: wire.archived,
            window,
            limit: wire.limit,
            query: wire.query,
        })
    }
}

impl From<ThreadsQuery> for ThreadsQueryWire {
    fn from(query: ThreadsQuery) -> Self {
        let (before, after) = match query.window {
            ThreadWindow::Latest => (None, None),
            ThreadWindow::Before(cursor) => (Some(cursor), None),
            ThreadWindow::After(cursor) => (None, Some(cursor)),
        };
        Self {
            archived: query.archived,
            before_activity: before.as_ref().map(|cursor| cursor.activity_seq),
            before_id: before.map(|cursor| cursor.id),
            after_activity: after.as_ref().map(|cursor| cursor.activity_seq),
            after_id: after.map(|cursor| cursor.id),
            limit: query.limit,
            query: query.query,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadCursor {
    pub activity_seq: Seq,
    pub id: ThreadId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreadsPage {
    /// Newest first, whichever way the page was asked for.
    pub threads: Vec<Thread>,
    pub runs: Vec<Run>,
    pub next: Option<ThreadCursor>,
    /// Asked with `after_*`: threads newer than this page exist.
    #[serde(default)]
    pub newer: bool,
}

/// Starts a run when the thread is idle, steers the live run otherwise.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessage {
    /// Client-made, so a retried send is the same message.
    pub id: ItemId,
    pub text: String,
    #[serde(default)]
    pub images: Vec<BlobHash>,
    /// Applies to the thread before the run starts.
    pub config: Option<EngineConfig>,
    /// Clear this draft revision in the same transaction as accepting input.
    #[serde(default)]
    pub draft: Option<DraftRevision>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftSnapshot {
    pub revision: u64,
    pub body: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteDraft {
    pub expected_revision: u64,
    pub body: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftRevision {
    pub scope: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftWriteResult {
    pub saved: bool,
    pub current: DraftSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    Approval {
        decision: Decision,
    },
    /// One entry per question; `None` skips it.
    Question {
        answers: Vec<Option<crate::QuestionAnswer>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    pub hash: BlobHash,
    pub bytes: u64,
}

/// A skill installed on Forge (`<data>/extensions/skills/<folder>/SKILL.md`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The folder on Forge's host.
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirListing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<DirEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    /// Opaque path on Forge's host. Clients must not construct host paths.
    pub path: String,
    pub is_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettleThread {
    pub observed_seq: crate::Seq,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectorSummary {
    #[serde(default)]
    pub goal: Option<ThreadGoal>,
    #[serde(default)]
    pub questions: Vec<ThreadQuestionnaire>,
    pub revision: crate::Seq,
    pub terminals: Vec<InspectorTerminal>,
    pub agents: Vec<InspectorAgent>,
    #[serde(default)]
    pub workflows: Vec<InspectorWorkflow>,
    pub plan: crate::Collection<crate::PlanStep>,
    /// Totals of edit operations, not a net working-tree diff.
    pub edits: Option<EditTotals>,
    /// The user's messages in the thread, in order: the map the navigator
    /// draws, whatever part of the thread is loaded.
    #[serde(default)]
    pub outline: Vec<OutlineEntry>,
}

/// One of the user's messages, as the navigator lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutlineEntry {
    pub id: ItemId,
    pub position: crate::Seq,
    /// Its first line, bounded.
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadGoal {
    pub objective: crate::TextContent,
    /// Bounded source-derived label for compact controls; full objective remains available.
    pub summary: String,
    pub state: ThreadGoalState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadGoalState {
    Active,
    Paused,
    /// The engine stopped on something only the user can resolve.
    Blocked,
    /// The engine's usage limit stopped it.
    UsageLimited,
    /// The goal's token budget ran out.
    BudgetLimited,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadQuestionnaire {
    pub id: ItemId,
    pub questions: crate::Collection<crate::Question>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectorTerminal {
    pub id: ItemId,
    pub command: crate::TextContent,
    pub cwd: Option<String>,
    pub status: crate::Status,
    /// Nothing waits on it: the engine started it in the background, or it
    /// outlived its run.
    #[serde(default)]
    pub background: bool,
    /// A watcher, reporting events rather than ending with a result.
    #[serde(default)]
    pub watch: bool,
}

/// A workflow the engine runs in the background (Claude's `Workflow`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectorWorkflow {
    pub id: ItemId,
    /// Its name, when the engine gives one.
    pub name: Option<String>,
    pub status: crate::Status,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectorAgent {
    pub id: ItemId,
    pub label: String,
    pub description: crate::TextContent,
    pub status: crate::Status,
    /// The harness it runs on: its own for a Forge-started agent, else the
    /// thread's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<crate::EngineId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// How it runs: its own thread's settings for a Forge-started agent, else
    /// the thread's with the agent's model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<crate::EngineConfig>,
    /// The thread a Forge-started agent runs in; opening it opens that thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadId>,
    /// Nothing waits on it: started in the background, or it outlived its run.
    #[serde(default)]
    pub background: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditTotals {
    pub added: u64,
    pub removed: u64,
}

/// Authorizes the TLS client certificate using a one-time pairing code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    pub device_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairResponse {
    pub device_id: crate::DeviceId,
}

/// The one wire error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    BadRequest,
    Conflict,
    Unauthorized,
    VersionMismatch,
    EngineUnavailable,
    Internal,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for ApiError {}
