use crate::error::{StorageError, operation_error};
use crate::{Store, read};
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

type Result<T, E = CoordinationFailure> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum CoordinationFailure {
    #[error("path must contain 1..4096 printable bytes")]
    PathInvalid,
    #[error("use an absolute worktree path")]
    PathNotAbsolute,
    #[error("path escapes its root")]
    PathEscapes,
    #[error("resource path must remain inside its worktree")]
    OutsideWorktree,
    #[error("named resource needs a namespace and key")]
    NamedResourceInvalid,
    #[error("coordination record exceeds 192 KiB; narrow its scope or evidence")]
    RecordTooLarge,
    #[error("at most 64 work dependencies")]
    TooManyDependencies,
    #[error("dependency {dependency} must publish work first")]
    DependencyUnpublished { dependency: ThreadId },
    #[error("work dependency would create a cycle")]
    DependencyCycle,
    #[error("calling thread {thread} no longer exists")]
    CallerGone { thread: ThreadId },
    #[error("work needs a concise objective and summary")]
    WorkInvalid,
    #[error("too much work metadata")]
    WorkMetadataTooLarge,
    #[error("claim needs resources and a concise purpose")]
    ClaimInvalid,
    #[error("claim lifetime must be at most 24 hours")]
    ClaimLifetime,
    #[error("recipient must be another existing thread")]
    RecipientInvalid,
    #[error("message needs 1..16384 bytes and prepared state")]
    MessageInvalid,
    #[error("watch needs another existing thread")]
    WatchInvalid,
    #[error("target must publish work before it can be watched")]
    WatchTargetUnpublished,
    #[error("coordination record belongs to another worker or kind")]
    RecordOfAnother,
    #[error("use the record's transition tool instead of replacing it")]
    UseTransition,
    #[error("record no longer exists")]
    RecordGone,
    #[error("coordination record {id} not found")]
    NotFound { id: String },
    #[error(transparent)]
    Stale(Conflict),
    #[error("only the claimant can release a claim")]
    NotClaimant,
    #[error("only the claimant can renew for 1..86400 seconds")]
    RenewalInvalid,
    #[error("only the recipient can acknowledge, resolve, or decline a message")]
    NotRecipient,
    #[error("message is already settled")]
    MessageSettled,
    #[error("reply exceeds 8192 bytes")]
    ReplyTooLong,
    #[error("only the sender can record delivery")]
    NotSender,
    #[error("message cannot be marked failed")]
    CannotFail,
    #[error("only the sender can record a receipt notification")]
    NotReceiptSender,
    #[error("only the subscriber can prepare its notification")]
    NotSubscriber,
    #[error("only the owner can settle a pending watch")]
    WatchNotOwner,
    #[error("transition does not apply to this coordination record")]
    TransitionInvalid,
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<rusqlite::Error> for CoordinationFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

fn require(condition: bool, failure: CoordinationFailure) -> Result<()> {
    if condition { Ok(()) } else { Err(failure) }
}

operation_error!(CoordinationGetError(CoordinationFailure), "read coordination record {id}", id: String);
operation_error!(
    CoordinationListError(CoordinationFailure),
    "list coordination records"
);
operation_error!(CoordinationPutError(CoordinationFailure), "save a coordination record for thread {caller}", caller: ThreadId);
operation_error!(CoordinationUpdateError(CoordinationFailure), "update coordination record {id} for thread {caller}", id: String, caller: ThreadId);
operation_error!(CoordinationChangesError(StorageError), "read the coordination changes after {after}", after: u64);
operation_error!(
    CoordinationWatchesError(StorageError),
    "read the coordination watches"
);
operation_error!(
    CoordinationReceiptsError(StorageError),
    "read the coordination receipts"
);

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
    require(
        !path.is_empty() && path.len() <= 4096 && !path.chars().any(char::is_control),
        CoordinationFailure::PathInvalid,
    )?;
    let windows = path.as_bytes().get(1) == Some(&b':') || path.starts_with("//");
    let absolute = path.starts_with('/') || (windows && path.as_bytes().get(2) == Some(&b'/'));
    require(absolute, CoordinationFailure::PathNotAbsolute)?;
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
                require(parts.len() > root_depth, CoordinationFailure::PathEscapes)?;
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
                require(
                    contains_path(&worktree, &path),
                    CoordinationFailure::OutsideWorktree,
                )?;
                Ok(Self::Path {
                    worktree,
                    path,
                    recursive,
                })
            }
            Self::Named { namespace, key } => {
                let namespace = namespace.trim().to_owned();
                let key = key.trim().to_owned();
                require(
                    !namespace.is_empty()
                        && namespace.len() <= 128
                        && !key.is_empty()
                        && key.len() <= 4096,
                    CoordinationFailure::NamedResourceInvalid,
                )?;
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
    const fn kind(&self) -> &'static str {
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
    .map(|body| Ok(read::from_body(&body)?))
    .transpose()
}

fn save(conn: &Connection, record: &Record) -> Result<()> {
    require(
        read::to_body(record)?.len() <= 192 * 1024,
        CoordinationFailure::RecordTooLarge,
    )?;
    conn.execute(
        include_str!("sql/coordination/save.sql"),
        params![
            record.id,
            record.owner.as_str(),
            record.project.as_str(),
            record.content.kind(),
            record.revision,
            record.updated_at,
            read::to_body(record)?
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
    require(
        dependencies.len() <= 64,
        CoordinationFailure::TooManyDependencies,
    )?;
    let mut statement = conn.prepare(include_str!("sql/coordination/dependencies.sql"))?;
    let mut graph = BTreeMap::new();
    for row in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (owner, body) = row?;
        let record: Record = read::from_body(&body)?;
        if let Content::Work { dependencies, .. } = record.content {
            graph.insert(ThreadId::from(owner), dependencies);
        }
    }
    for dependency in dependencies {
        require(
            graph.contains_key(dependency),
            CoordinationFailure::DependencyUnpublished {
                dependency: dependency.clone(),
            },
        )?;
    }
    graph.insert(caller.clone(), dependencies.to_vec());
    let mut pending = dependencies.to_vec();
    let mut visited = BTreeSet::new();
    while let Some(next) = pending.pop() {
        require(&next != caller, CoordinationFailure::DependencyCycle)?;
        if visited.insert(next.clone()) {
            pending.extend(graph.get(&next).into_iter().flatten().cloned());
        }
    }
    Ok(())
}

impl Store {
    pub async fn coordination_get(
        &self,
        id: String,
    ) -> Result<Option<Record>, CoordinationGetError> {
        let wanted = id.clone();
        self.read(move |conn| get(conn, &id))
            .await
            .map_err(|cause| CoordinationGetError {
                id: wanted,
                cause: Box::new(cause),
            })
    }

    pub async fn coordination_list(&self, query: Query) -> Result<Page, CoordinationListError> {
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
                .map(|row| Ok(read::from_body::<Record>(&row?)?))
                .collect::<Result<Vec<_>>>()?;
            let next_cursor = if records.len() > limit {
                records.truncate(limit);
                records
                    .last()
                    .map(|r| {
                        read::to_body(&Cursor {
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
        .map_err(|cause| CoordinationListError {
            cause: Box::new(cause),
        })
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
    ) -> Result<Record, CoordinationPutError> {
        let putting = caller.clone();
        self.write_acknowledged(move |tx| {
            let thread =
                read::thread(tx, &caller)?.ok_or_else(|| CoordinationFailure::CallerGone {
                    thread: caller.clone(),
                })?;
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
                    require(
                        !objective.trim().is_empty()
                            && objective.len() <= 4096
                            && summary.len() <= 8192,
                        CoordinationFailure::WorkInvalid,
                    )?;
                    *worktree = normalize_path(worktree)?;
                    require(
                        resources.len() <= 64
                            && evidence.len() <= 32
                            && evidence.iter().all(|s| s.len() <= 4096),
                        CoordinationFailure::WorkMetadataTooLarge,
                    )?;
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
                    require(
                        !resources.is_empty()
                            && resources.len() <= 64
                            && !purpose.trim().is_empty()
                            && purpose.len() <= 4096,
                        CoordinationFailure::ClaimInvalid,
                    )?;
                    require(
                        *state == ClaimState::Active
                            && *expires_at > now()
                            && *expires_at <= now() + 86_400_000,
                        CoordinationFailure::ClaimLifetime,
                    )?;
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
                    require(
                        recipient != &caller && read::thread(tx, recipient)?.is_some(),
                        CoordinationFailure::RecipientInvalid,
                    )?;
                    require(
                        !text.trim().is_empty()
                            && text.len() <= 16_384
                            && *state == MessageState::Prepared,
                        CoordinationFailure::MessageInvalid,
                    )?;
                }
                Content::Watch { target, state, .. } => {
                    require(
                        target != &caller
                            && read::thread(tx, target)?.is_some()
                            && *state == WatchState::Pending,
                        CoordinationFailure::WatchInvalid,
                    )?;
                    require(
                        get(tx, &format!("work:{target}"))?.is_some(),
                        CoordinationFailure::WatchTargetUnpublished,
                    )?;
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
                require(
                    previous.owner == caller && previous.content.kind() == content.kind(),
                    CoordinationFailure::RecordOfAnother,
                )?;
                if expected_revision != Some(previous.revision) {
                    return Err(CoordinationFailure::Stale(Conflict {
                        id,
                        current_revision: previous.revision,
                    }));
                }
                require(
                    matches!(content, Content::Work { .. }),
                    CoordinationFailure::UseTransition,
                )?;
            } else {
                require(
                    expected_revision.is_none_or(|r| r == 0),
                    CoordinationFailure::RecordGone,
                )?;
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
        .inspect(|_| self.coordination_changed())
        .map_err(|cause| CoordinationPutError {
            caller: putting,
            cause: Box::new(cause),
        })
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
    ) -> Result<Record, CoordinationUpdateError> {
        let (updating, record_id) = (caller.clone(), id.clone());
        self.write_acknowledged(move |tx| {
            let mut record =
                get(tx, &id)?.ok_or_else(|| CoordinationFailure::NotFound { id: id.clone() })?;
            if record.revision != revision {
                return Err(CoordinationFailure::Stale(Conflict {
                    id,
                    current_revision: record.revision,
                }));
            }
            match (&mut record.content, update) {
                (Content::Claim { state, .. }, Update::ReleaseClaim) => {
                    require(record.owner == caller, CoordinationFailure::NotClaimant)?;
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
                    require(
                        record.owner == caller && (1..=86_400).contains(&seconds),
                        CoordinationFailure::RenewalInvalid,
                    )?;
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
                    require(
                        *recipient == caller
                            && matches!(
                                next,
                                MessageState::Acknowledged
                                    | MessageState::Resolved
                                    | MessageState::Declined
                            ),
                        CoordinationFailure::NotRecipient,
                    )?;
                    require(
                        !matches!(
                            state,
                            MessageState::Resolved | MessageState::Declined | MessageState::Failed
                        ),
                        CoordinationFailure::MessageSettled,
                    )?;
                    require(
                        response.as_ref().is_none_or(|text| text.len() <= 8192),
                        CoordinationFailure::ReplyTooLong,
                    )?;
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
                    require(record.owner == caller, CoordinationFailure::NotSender)?;
                    if *state == MessageState::Prepared {
                        *state = MessageState::Delivered;
                    }
                    *delivery_item = Some(item);
                }
                (Content::Message { state, reply, .. }, Update::MessageFailed { error }) => {
                    require(
                        record.owner == caller && *state == MessageState::Prepared,
                        CoordinationFailure::CannotFail,
                    )?;
                    *state = MessageState::Failed;
                    *reply = Some(error);
                }
                (
                    Content::Message {
                        receipt_revision, ..
                    },
                    Update::ReceiptDelivered,
                ) => {
                    require(
                        record.owner == caller,
                        CoordinationFailure::NotReceiptSender,
                    )?;
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
                    require(record.owner == caller, CoordinationFailure::NotSubscriber)?;
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
                    require(
                        record.owner == caller
                            && matches!(*state, WatchState::Pending | WatchState::Ready)
                            && !matches!(next, WatchState::Pending | WatchState::Ready),
                        CoordinationFailure::WatchNotOwner,
                    )?;
                    *state = next;
                    *outcome = Some(result);
                }
                _ => return Err(CoordinationFailure::TransitionInvalid),
            }
            record.revision += 1;
            record.updated_at = now();
            save(tx, &record)?;
            Ok(record)
        })
        .await
        .inspect(|_| self.coordination_changed())
        .map_err(|cause| CoordinationUpdateError {
            id: record_id,
            caller: updating,
            cause: Box::new(cause),
        })
    }

    pub async fn coordination_changes(
        &self,
        after: u64,
        project: Option<ProjectId>,
        limit: u32,
    ) -> Result<Vec<Change>, CoordinationChangesError> {
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
        .map_err(|cause| CoordinationChangesError {
            after,
            cause: Box::new(cause),
        })
    }

    pub async fn coordination_watches(&self) -> Result<Vec<Record>, CoordinationWatchesError> {
        self.read(move |conn| {
            let mut statement = conn.prepare(include_str!("sql/coordination/watches.sql"))?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .map(|row| read::from_body(&row?))
                .collect()
        })
        .await
        .map_err(|cause| CoordinationWatchesError {
            cause: Box::new(cause),
        })
    }

    pub async fn coordination_receipts(
        &self,
        deferred: Vec<String>,
    ) -> Result<Vec<Record>, CoordinationReceiptsError> {
        self.read(move |conn| {
            let mut statement = conn.prepare(include_str!("sql/coordination/receipts.sql"))?;
            statement
                .query_map([read::to_body(&deferred)?], |row| row.get::<_, String>(0))?
                .map(|row| read::from_body(&row?))
                .collect()
        })
        .await
        .map_err(|cause| CoordinationReceiptsError {
            cause: Box::new(cause),
        })
    }
}
