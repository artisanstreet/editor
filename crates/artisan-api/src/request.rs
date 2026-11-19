use crate::{
    BlobHash, Decision, EngineConfig, EngineStatus, Item, ItemId, Project, ProjectId, Run, RunId,
    Seq, Thread, ThreadId,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub head: Seq,
    pub forge_version: String,
    pub protocol: u32,
    pub projects: Vec<Project>,
    pub threads: Vec<Thread>,
    #[serde(default)]
    pub threads_next: Option<ThreadCursor>,
    pub runs: Vec<Run>,
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub engines: Vec<EngineStatus>,
    #[serde(default)]
    pub waiting: Vec<PendingItem>,
    #[serde(default)]
    pub account: Option<Account>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingItem {
    pub id: ItemId,
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub user: String,
    pub host: String,
    #[serde(default)]
    pub wsl_distro: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRepository {
    pub branch: Option<String>,
    pub host: Option<String>,
    pub web_url: Option<String>,
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
    #[serde(default)]
    pub parent_thread_id: Option<ThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<ItemId>,
}

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
    pub items: Vec<Item>,
    #[serde(default)]
    pub runs: Vec<Run>,
    pub more: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub work: Vec<WorkSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkSpan {
    pub run_id: Option<RunId>,
    pub first: Seq,
    pub last: Seq,
    pub count: u32,
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
    pub threads: Vec<Thread>,
    pub runs: Vec<Run>,
    pub next: Option<ThreadCursor>,
    #[serde(default)]
    pub newer: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessage {
    pub id: ItemId,
    pub text: String,
    #[serde(default)]
    pub images: Vec<BlobHash>,
    pub config: Option<EngineConfig>,
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
    Question {
        answers: Vec<Option<crate::QuestionAnswer>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    pub hash: BlobHash,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
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
    pub edits: Option<EditTotals>,
    #[serde(default)]
    pub outline: Vec<OutlineEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutlineEntry {
    pub id: ItemId,
    pub position: crate::Seq,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadGoal {
    pub objective: crate::TextContent,
    pub summary: String,
    pub state: ThreadGoalState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadGoalState {
    Active,
    Paused,
    Blocked,
    UsageLimited,
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
    #[serde(default)]
    pub background: bool,
    #[serde(default)]
    pub watch: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectorWorkflow {
    pub id: ItemId,
    pub name: Option<String>,
    pub status: crate::Status,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectorAgent {
    pub id: ItemId,
    pub label: String,
    pub description: crate::TextContent,
    pub status: crate::Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<crate::EngineId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<crate::EngineConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadId>,
    #[serde(default)]
    pub background: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditTotals {
    pub added: u64,
    pub removed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    pub device_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairResponse {
    pub device_id: crate::DeviceId,
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
