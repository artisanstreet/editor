use crate::{Store, read};
use anyhow::{Context, Result, ensure};
use artisan_api::{ItemId, ProjectId, ThreadId, Timestamp, now};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, thiserror::Error)]
#[error("record {id} changed; reload it and use revision {current_revision}")]
pub struct Conflict {
    pub id: String,
    pub current_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Resource {
    Path {
        worktree: String,
        path: String,
        #[serde(default)]
        recursive: bool,
    },
    Named {
        namespace: String,
        key: String,
    },
}

pub fn normalize_path(path: &str) -> Result<String> {
    let path = path.trim().replace('\\', "/");
    ensure!(
        !path.is_empty() && path.len() <= 4096 && !path.chars().any(char::is_control),
        "path must contain 1..4096 printable bytes"
    );
    let windows = path.as_bytes().get(1) == Some(&b':') || path.starts_with("//");
    let absolute = path.starts_with('/') || (windows && path.as_bytes().get(2) == Some(&b'/'));
    ensure!(absolute, "use an absolute worktree path");
    let prefix = match (path.starts_with("//"), windows) {
        (true, _) => "//",
        (false, true) => "",
        _ => "/",
    };
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                let root_depth = if path.starts_with("//") {
                    2
                } else {
                    usize::from(windows)
                };
                ensure!(parts.len() > root_depth, "path escapes its root");
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let mut normalized = format!("{prefix}{}", parts.join("/"));
    if windows && parts.len() == 1 && path.as_bytes().get(1) == Some(&b':') {
        normalized.push('/');
    }
    Ok(if windows {
        normalized.to_lowercase()
    } else {
        normalized
    })
}

fn contains_path(parent: &str, child: &str) -> bool {
    parent == child
        || child
            .strip_prefix(parent)
            .is_some_and(|suffix| parent.ends_with('/') || suffix.starts_with('/'))
}

impl Resource {
    pub fn normalize(self) -> Result<Self> {
        match self {
            Self::Path {
                worktree,
                path,
                recursive,
            } => {
                let worktree = normalize_path(&worktree)?;
                let path = path.replace('\\', "/");
                let path = normalize_path(&if path.starts_with('/')
                    || path.as_bytes().get(1) == Some(&b':')
                {
                    path
                } else {
                    format!("{worktree}/{path}")
                })?;
                ensure!(
                    contains_path(&worktree, &path),
                    "resource path must remain inside its worktree"
                );
                Ok(Self::Path {
                    worktree,
                    path,
                    recursive,
                })
            }
            Self::Named { namespace, key } => {
                let namespace = namespace.trim().to_owned();
                let key = key.trim().to_owned();
                ensure!(
                    !namespace.is_empty()
                        && namespace.len() <= 128
                        && !key.is_empty()
                        && key.len() <= 4096,
                    "named resource needs a namespace and key"
                );
                Ok(Self::Named { namespace, key })
            }
        }
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Path {
                    worktree: left_scope,
                    path: left_key,
                    recursive: left_recursive,
                },
                Self::Path {
                    worktree: right_scope,
                    path: right_key,
                    recursive: right_recursive,
                },
            ) => {
                left_scope == right_scope
                    && (left_key == right_key
                        || (*left_recursive && contains_path(left_key, right_key))
                        || (*right_recursive && contains_path(right_key, left_key)))
            }
            (
                Self::Named {
                    namespace: left_scope,
                    key: left_key,
                },
                Self::Named {
                    namespace: right_scope,
                    key: right_key,
                },
            ) => left_scope == right_scope && left_key == right_key,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkState {
    Running,
    Waiting,
    Blocked,
    Completed,
    Failed,
    Cancelled,
}

impl WorkState {
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimState {
    Active,
    Released,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    Prepared,
    Delivered,
    Acknowledged,
    Resolved,
    Declined,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    Information,
    Boundary,
    Dependency,
    Handoff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchState {
    Pending,
    Ready,
    Delivered,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    Work {
        objective: String,
        worktree: String,
        state: WorkState,
        resources: Vec<Resource>,
        dependencies: Vec<ThreadId>,
        summary: String,
        evidence: Vec<String>,
    },
    Claim {
        resources: Vec<Resource>,
        purpose: String,
        state: ClaimState,
        expires_at: Timestamp,
    },
    Message {
        recipient: ThreadId,
        intent: Intent,
        text: String,
        state: MessageState,
        reply: Option<String>,
        delivery_item: Option<ItemId>,
        #[serde(default)]
        receipt_revision: u64,
    },
    Watch {
        target: ThreadId,
        state: WatchState,
        after_revision: u64,
        outcome: Option<String>,
    },
}

impl Content {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Work { .. } => "work",
            Self::Claim { .. } => "claim",
            Self::Message { .. } => "message",
            Self::Watch { .. } => "watch",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub owner: ThreadId,
    pub project: ProjectId,
    pub revision: u64,
    pub updated_at: Timestamp,
    pub content: Content,
}

impl Record {
    pub const fn active_claim(&self, time: Timestamp) -> bool {
        matches!(self.content, Content::Claim { state: ClaimState::Active, expires_at, .. } if expires_at > time)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cursor {
    pub updated_at: Timestamp,
    pub id: String,
}

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub project: Option<ProjectId>,
    pub kind: Option<String>,
    pub owner: Option<ThreadId>,
    pub recipient: Option<ThreadId>,
    pub cursor: Option<Cursor>,
    pub limit: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Page {
    pub records: Vec<Record>,
    pub next_cursor: Option<String>,
    pub event_cursor: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub seq: u64,
    pub record_id: String,
    pub owner: ThreadId,
    pub project: ProjectId,
    pub kind: String,
    pub revision: u64,
    pub created_at: Timestamp,
}

#[derive(Clone, Debug)]
pub enum Update {
    ReleaseClaim,
    RenewClaim {
        seconds: u32,
    },
    Acknowledge {
        state: MessageState,
        reply: Option<String>,
    },
    MessageDelivered {
        item: ItemId,
    },
    MessageFailed {
        error: String,
    },
    FinishWatch {
        state: WatchState,
        outcome: String,
    },
    PrepareWatch {
        outcome: String,
    },
    ReceiptDelivered,
}

fn get(conn: &Connection, id: &str) -> Result<Option<Record>> {
    conn.query_row(include_str!("sql/coordination/get.sql"), [id], |row| {
        row.get::<_, String>(0)
    })
    .optional()?
    .map(|body| serde_json::from_str(&body).context("decode coordination record"))
    .transpose()
}

fn save(conn: &Connection, record: &Record) -> Result<()> {
    ensure!(
        serde_json::to_vec(record)?.len() <= 192 * 1024,
        "coordination record exceeds 192 KiB; narrow its scope or evidence"
    );
    conn.execute(
        include_str!("sql/coordination/save.sql"),
        params![
            record.id,
            record.owner.as_str(),
            record.project.as_str(),
            record.content.kind(),
            record.revision,
            record.updated_at,
            serde_json::to_string(record)?
        ],
    )?;
    conn.execute(
        include_str!("sql/coordination/event.sql"),
        params![
            record.id,
            record.owner.as_str(),
            record.project.as_str(),
            record.content.kind(),
            record.revision,
            record.updated_at
        ],
    )?;
    Ok(())
}

fn validate_dependencies(
    conn: &Connection,
    caller: &ThreadId,
    dependencies: &[ThreadId],
) -> Result<()> {
    ensure!(dependencies.len() <= 64, "at most 64 work dependencies");
    let mut statement = conn.prepare(include_str!("sql/coordination/dependencies.sql"))?;
    let mut graph = BTreeMap::new();
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (owner, body) = row?;
        let record: Record = serde_json::from_str(&body)?;
        if let Content::Work { dependencies, .. } = record.content {
            graph.insert(ThreadId::from(owner), dependencies);
        }
    }
    for dependency in dependencies {
        ensure!(
            graph.contains_key(dependency),
            "dependency {dependency} must publish work first"
        );
    }
    graph.insert(caller.clone(), dependencies.to_vec());
    let mut pending = dependencies.to_vec();
    let mut visited = BTreeSet::new();
    while let Some(next) = pending.pop() {
        ensure!(&next != caller, "work dependency would create a cycle");
        if visited.insert(next.clone()) {
            pending.extend(graph.get(&next).into_iter().flatten().cloned());
        }
    }
    Ok(())
}

impl Store {
    pub async fn coordination_get(&self, id: String) -> Result<Option<Record>> {
        self.read(move |conn| get(conn, &id)).await
    }

    pub async fn coordination_list(&self, query: Query) -> Result<Page> {
        self.read(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let event_cursor =
                tx.query_row(include_str!("sql/coordination/head.sql"), [], |row| {
                    row.get(0)
                })?;
            let limit = query.limit.clamp(1, 200) as usize;
            let mut statement = tx.prepare(include_str!("sql/coordination/list.sql"))?;
            let rows = statement.query_map(
                params![
                    query.project.map(|id| id.to_string()),
                    query.kind,
                    query.owner.map(|id| id.to_string()),
                    query.recipient.map(|id| id.to_string()),
                    query.cursor.as_ref().map(|c| c.updated_at),
                    query.cursor.as_ref().map(|c| &c.id),
                    limit + 1
                ],
                |row| row.get::<_, String>(0),
            )?;
            let mut records = rows
                .map(|row| Ok(serde_json::from_str::<Record>(&row?)?))
                .collect::<Result<Vec<_>>>()?;
            let next_cursor = if records.len() > limit {
                records.truncate(limit);
                records
                    .last()
                    .map(|r| {
                        serde_json::to_string(&Cursor {
                            updated_at: r.updated_at,
                            id: r.id.clone(),
                        })
                    })
                    .transpose()?
            } else {
                None
            };
            Ok(Page {
                records,
                next_cursor,
                event_cursor,
            })
        })
        .await
    }

    #[expect(
        clippy::too_many_lines,
        reason = "validate and persist each durable coordination variant in one atomic transaction"
    )]
    pub async fn coordination_put(
        &self,
        caller: ThreadId,
        id: Option<String>,
        mut content: Content,
        expected_revision: Option<u64>,
    ) -> Result<Record> {
        self.write(move |tx| {
            let thread = read::thread(tx, &caller)?.context("calling thread no longer exists")?;
            match &mut content {
                Content::Work {
                    objective,
                    worktree,
                    resources,
                    dependencies,
                    summary,
                    evidence,
                    ..
                } => {
                    ensure!(
                        !objective.trim().is_empty()
                            && objective.len() <= 4096
                            && summary.len() <= 8192,
                        "work needs a concise objective and summary"
                    );
                    *worktree = normalize_path(worktree)?;
                    ensure!(
                        resources.len() <= 64
                            && evidence.len() <= 32
                            && evidence.iter().all(|s| s.len() <= 4096),
                        "too much work metadata"
                    );
                    *resources = resources
                        .drain(..)
                        .map(Resource::normalize)
                        .collect::<Result<_>>()?;
                    validate_dependencies(tx, &caller, dependencies)?;
                }
                Content::Claim {
                    resources,
                    purpose,
                    expires_at,
                    state,
                } => {
                    ensure!(
                        !resources.is_empty()
                            && resources.len() <= 64
                            && !purpose.trim().is_empty()
                            && purpose.len() <= 4096,
                        "claim needs resources and a concise purpose"
                    );
                    ensure!(
                        *state == ClaimState::Active
                            && *expires_at > now()
                            && *expires_at <= now() + 86_400_000,
                        "claim lifetime must be at most 24 hours"
                    );
                    *resources = resources
                        .drain(..)
                        .map(Resource::normalize)
                        .collect::<Result<_>>()?;
                }
                Content::Message {
                    recipient,
                    text,
                    state,
                    ..
                } => {
                    ensure!(
                        recipient != &caller && read::thread(tx, recipient)?.is_some(),
                        "recipient must be another existing thread"
                    );
                    ensure!(
                        !text.trim().is_empty()
                            && text.len() <= 16_384
                            && *state == MessageState::Prepared,
                        "message needs 1..16384 bytes and prepared state"
                    );
                }
                Content::Watch { target, state, .. } => {
                    ensure!(
                        target != &caller
                            && read::thread(tx, target)?.is_some()
                            && *state == WatchState::Pending,
                        "watch needs another existing thread"
                    );
                    ensure!(
                        get(tx, &format!("work:{target}"))?.is_some(),
                        "target must publish work before it can be watched"
                    );
                }
            }
            let id = match &content {
                Content::Work { .. } => format!("work:{caller}"),
                Content::Claim { .. } | Content::Message { .. } | Content::Watch { .. } => {
                    id.unwrap_or_else(|| ItemId::new().to_string())
                }
            };
            let previous = get(tx, &id)?;
            if let Some(previous) = &previous {
                ensure!(
                    previous.owner == caller && previous.content.kind() == content.kind(),
                    "coordination record belongs to another worker or kind"
                );
                if expected_revision != Some(previous.revision) {
                    return Err(Conflict {
                        id,
                        current_revision: previous.revision,
                    }
                    .into());
                }
                ensure!(
                    matches!(content, Content::Work { .. }),
                    "use the record's transition tool instead of replacing it"
                );
            } else {
                ensure!(
                    expected_revision.is_none_or(|r| r == 0),
                    "record no longer exists"
                );
            }
            let record = Record {
                id,
                owner: caller,
                project: thread.project_id,
                revision: previous.map_or(1, |r| r.revision + 1),
                updated_at: now(),
                content,
            };
            save(tx, &record)?;
            Ok(record)
        })
        .await
    }

    #[expect(
        clippy::too_many_lines,
        reason = "keep the ownership-checked coordination transition table together in one transaction"
    )]
    pub async fn coordination_update(
        &self,
        caller: ThreadId,
        id: String,
        revision: u64,
        update: Update,
    ) -> Result<Record> {
        self.write(move |tx| {
            let mut record = get(tx, &id)?.context("coordination record not found")?;
            if record.revision != revision {
                return Err(Conflict {
                    id,
                    current_revision: record.revision,
                }
                .into());
            }
            match (&mut record.content, update) {
                (Content::Claim { state, .. }, Update::ReleaseClaim) => {
                    ensure!(
                        record.owner == caller,
                        "only the claimant can release a claim"
                    );
                    *state = ClaimState::Released;
                }
                (
                    Content::Claim {
                        state: ClaimState::Active,
                        expires_at,
                        ..
                    },
                    Update::RenewClaim { seconds },
                ) => {
                    ensure!(
                        record.owner == caller && (1..=86_400).contains(&seconds),
                        "only the claimant can renew for 1..86400 seconds"
                    );
                    *expires_at = now() + i64::from(seconds) * 1000;
                }
                (
                    Content::Message {
                        recipient,
                        state,
                        reply,
                        ..
                    },
                    Update::Acknowledge {
                        state: next,
                        reply: response,
                    },
                ) => {
                    ensure!(
                        *recipient == caller
                            && matches!(
                                next,
                                MessageState::Acknowledged
                                    | MessageState::Resolved
                                    | MessageState::Declined
                            ),
                        "only the recipient can acknowledge, resolve, or decline a message"
                    );
                    ensure!(
                        !matches!(
                            state,
                            MessageState::Resolved | MessageState::Declined | MessageState::Failed
                        ),
                        "message is already settled"
                    );
                    ensure!(
                        response.as_ref().is_none_or(|text| text.len() <= 8192),
                        "reply exceeds 8192 bytes"
                    );
                    *state = next;
                    *reply = response;
                }
                (
                    Content::Message {
                        state,
                        delivery_item,
                        ..
                    },
                    Update::MessageDelivered { item },
                ) => {
                    ensure!(
                        record.owner == caller,
                        "only the sender can record delivery"
                    );
                    if *state == MessageState::Prepared {
                        *state = MessageState::Delivered;
                    }
                    *delivery_item = Some(item);
                }
                (Content::Message { state, reply, .. }, Update::MessageFailed { error }) => {
                    ensure!(
                        record.owner == caller && *state == MessageState::Prepared,
                        "message cannot be marked failed"
                    );
                    *state = MessageState::Failed;
                    *reply = Some(error);
                }
                (
                    Content::Message {
                        receipt_revision, ..
                    },
                    Update::ReceiptDelivered,
                ) => {
                    ensure!(
                        record.owner == caller,
                        "only the sender can record a receipt notification"
                    );
                    *receipt_revision = record.revision + 1;
                }
                (
                    Content::Watch {
                        state: state @ WatchState::Pending,
                        outcome,
                        ..
                    },
                    Update::PrepareWatch { outcome: result },
                ) => {
                    ensure!(
                        record.owner == caller,
                        "only the subscriber can prepare its notification"
                    );
                    *state = WatchState::Ready;
                    *outcome = Some(result);
                }
                (
                    Content::Watch { state, outcome, .. },
                    Update::FinishWatch {
                        state: next,
                        outcome: result,
                    },
                ) => {
                    ensure!(
                        record.owner == caller
                            && matches!(*state, WatchState::Pending | WatchState::Ready)
                            && !matches!(next, WatchState::Pending | WatchState::Ready),
                        "only the owner can settle a pending watch"
                    );
                    *state = next;
                    *outcome = Some(result);
                }
                _ => anyhow::bail!("transition does not apply to this coordination record"),
            }
            record.revision += 1;
            record.updated_at = now();
            save(tx, &record)?;
            Ok(record)
        })
        .await
    }

    pub async fn coordination_changes(
        &self,
        after: u64,
        project: Option<ProjectId>,
        limit: u32,
    ) -> Result<Vec<Change>> {
        self.read(move |conn| {
            let mut statement = conn.prepare(include_str!("sql/coordination/events.sql"))?;
            Ok(statement
                .query_map(
                    params![after, project.map(|id| id.to_string()), limit.clamp(1, 200)],
                    |row| {
                        Ok(Change {
                            seq: row.get(0)?,
                            record_id: row.get(1)?,
                            owner: ThreadId::from(row.get::<_, String>(2)?),
                            project: ProjectId::from(row.get::<_, String>(3)?),
                            kind: row.get(4)?,
                            revision: row.get(5)?,
                            created_at: row.get(6)?,
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    pub async fn coordination_watches(&self) -> Result<Vec<Record>> {
        self.read(move |conn| {
            let mut statement = conn.prepare(include_str!("sql/coordination/watches.sql"))?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .map(|row| Ok(serde_json::from_str(&row?)?))
                .collect()
        })
        .await
    }

    pub async fn coordination_receipts(&self) -> Result<Vec<Record>> {
        self.read(move |conn| {
            let mut statement = conn.prepare(include_str!("sql/coordination/receipts.sql"))?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .map(|row| Ok(serde_json::from_str(&row?)?))
                .collect()
        })
        .await
    }
}
