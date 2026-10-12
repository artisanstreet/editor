//! Request and response bodies. Routes:
//!
//! ```text
//! GET    /state                              State
//! GET    /events?since=N                     SSE of Envelope
//! POST   /projects                           CreateProject → Project
//! DELETE /projects/{id}
//! GET    /projects/{id}/icon                 Option<BlobRef>
//! GET    /projects/{id}/repository           Option<ProjectRepository>
//! POST   /threads                            CreateThread → Thread
//! PATCH  /threads/{id}                       UpdateThread → Thread
//! GET    /threads/{id}/items?before|after&limit   ItemsPage
//! POST   /threads/{id}/messages              SendMessage → Item
//! POST   /threads/{id}/cancel
//! POST   /items/{id}/answer                  Answer
//! PUT    /settings/{key}                     JSON value
//! GET    /drafts/{scope}   PUT /drafts/{scope}   JSON value
//! PUT    /blobs  (raw WebP bytes)            BlobRef
//! GET    /blobs/{sha256}
//! GET    /engines                            Vec<EngineStatus>
//! POST   /engines/{engine}/install | /login | /logout | /refresh
//! GET    /fs/list?path=                      DirListing
//! GET    /links/title?url=                   LinkTitle
//! GET    /skills/catalog | /prompt …         see `extensions`
//! POST   /pair                               PairRequest → PairResponse (no client key needed)
//! ```

use crate::{
    BlobHash, Decision, EngineConfig, EngineStatus, Item, ItemId, Project, ProjectId, Run, Seq,
    Thread, ThreadId,
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
    /// Runs still live, plus the latest run of every thread.
    pub runs: Vec<Run>,
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub engines: Vec<EngineStatus>,
    /// Items waiting on the user (approvals, questions), across threads.
    #[serde(default)]
    pub waiting: Vec<Item>,
    /// The account Forge runs as, shown as the Editor's profile.
    #[serde(default)]
    pub account: Option<Account>,
}

/// The operating-system user Forge runs as, on its host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// The user's login name.
    pub user: String,
    /// The host's name.
    pub host: String,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateThread {
    pub id: ThreadId,
    pub project_id: ProjectId,
    pub config: EngineConfig,
    pub title: Option<String>,
    /// Hand off from another thread: the new one starts with a summary.
    #[serde(default)]
    pub parent_thread_id: Option<ThreadId>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// More items exist past the far end of this page.
    pub more: bool,
}

/// Starts a run when the thread is idle, steers the live run otherwise.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendMessage {
    /// Client-made, so a retried send is the same message.
    pub id: ItemId,
    pub text: String,
    #[serde(default)]
    pub images: Vec<BlobHash>,
    /// Applies to the thread before the run starts.
    pub config: Option<EngineConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Answer {
    Approval {
        decision: Decision,
    },
    /// One entry per question; `None` skips it.
    Question {
        answers: Vec<Option<Vec<String>>>,
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
    pub is_dir: bool,
}

/// Asks for the title of an `http(s)` page, to show a link by name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkTitleQuery {
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkTitle {
    pub url: String,
    /// `None` when the page has no title or couldn't be fetched; the link
    /// keeps its own label.
    pub title: Option<String>,
}

/// Swaps public keys. The code (shown by `ae forge pair`) only confirms it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    pub device_name: String,
    /// The Editor's certificate (DER, base64): the one this connection
    /// presents.
    pub public_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairResponse {
    pub device_id: crate::DeviceId,
    /// Forge's public key, base64 DER SubjectPublicKeyInfo.
    pub forge_public_key: String,
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
