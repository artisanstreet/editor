use crate::forge::{self, ForgeError};
use crate::server::Failure;
use crate::transcript::RenderError;
use crate::{Tool, names};
use artisan_api::{EngineId, ItemId, ProjectId, ThreadId};
use forge_runs::Hub;
use forge_store::coordination::{
    Content, CoordinationFailure, Query as CoordinationQuery, Record, Resource,
};
use forge_store::{coordination::normalize_path, discovery};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum AwarenessError {
    #[error("read the arguments")]
    Arguments(#[source] serde_json::Error),
    #[error("{field} is required")]
    Missing { field: &'static str },
    #[error("{step}")]
    Forge {
        step: &'static str,
        #[source]
        source: Box<ForgeError>,
    },
    #[error("the calling thread {thread} is not found")]
    CallerMissing { thread: ThreadId },
    #[error("scope must be project or host, not {scope}")]
    Scope { scope: String },
    #[error(
        "worker {target} not found or ambiguous ({found} matches); use the exact thread ID from list_agents"
    )]
    Worker { target: String, found: usize },
    #[error("worker {thread} disappeared while it was read")]
    Vanished { thread: ThreadId },
    #[error("item {item} not found")]
    ItemMissing { item: ItemId },
    #[error("item {item} belongs to thread {owner}, not {thread}")]
    ItemElsewhere {
        item: ItemId,
        owner: ThreadId,
        thread: String,
    },
    #[error("read item {item}")]
    Render {
        item: ItemId,
        #[source]
        source: RenderError,
    },
    #[error("offset {offset} must be a UTF-8 boundary from next_offset")]
    Offset { offset: u64 },
    #[error("read the cursor")]
    Cursor(#[source] serde_json::Error),
    #[error("write the next cursor")]
    NextCursor(#[source] serde_json::Error),
    #[error("unknown legacy state filter {state}")]
    LegacyState { state: String },
    #[error("provide 1..32 file paths, not {count}")]
    Paths { count: usize },
    #[error("normalize the path {path}")]
    Path {
        path: String,
        #[source]
        source: CoordinationFailure,
    },
    #[error("{count} relevance resources are too many; use a narrower explicit scope")]
    TooManyResources { count: usize },
    #[error("normalize a resource")]
    Resource(#[source] CoordinationFailure),
    #[error("list the coordination records")]
    Records(#[source] crate::coordination::ListRecordsError),
}

impl AwarenessError {
    pub fn failure(&self) -> Failure<'_> {
        match self {
            Self::Forge { source, .. } => source.failure(),
            Self::Path { source, .. } | Self::Resource(source) => forge::coordination(source, self),
            Self::Records(error) => error.failure(),
            Self::Arguments(_)
            | Self::Missing { .. }
            | Self::CallerMissing { .. }
            | Self::Scope { .. }
            | Self::Worker { .. }
            | Self::Vanished { .. }
            | Self::ItemMissing { .. }
            | Self::ItemElsewhere { .. }
            | Self::Render { .. }
            | Self::Offset { .. }
            | Self::Cursor(_)
            | Self::NextCursor(_)
            | Self::LegacyState { .. }
            | Self::Paths { .. }
            | Self::TooManyResources { .. } => Failure::Broken,
        }
    }
}

fn forge<E: Into<ForgeError>>(step: &'static str) -> impl FnOnce(E) -> AwarenessError {
    move |source| AwarenessError::Forge {
        step,
        source: Box::new(source.into()),
    }
}

fn normalized(path: &str) -> Result<String, AwarenessError> {
    normalize_path(path).map_err(|source| AwarenessError::Path {
        path: path.to_owned(),
        source,
    })
}

type Result<T, E = AwarenessError> = std::result::Result<T, E>;

pub fn object(required: &[&str], properties: Value) -> Value {
    let mut schema = json!({"type":"object","required":required});
    schema["properties"] = properties;
    schema
}

pub fn nullable(kind: &str) -> Value {
    json!({"type":[kind,"null"]})
}

fn project_schema() -> Value {
    object(
        &["id", "path", "name"],
        json!({"id":{"type":"string"},"path":{"type":"string"},"name":{"type":"string"}}),
    )
}

pub fn record_schema() -> Value {
    object(
        &[
            "id",
            "owner",
            "project",
            "revision",
            "updated_at",
            "content",
        ],
        json!({"id":{"type":"string"},"owner":{"type":"string"},"project":{"type":"string"},"revision":{"type":"integer","minimum":1},"updated_at":{"type":"integer"},"content":object(&["kind"], json!({"kind":{"enum":["work","claim","message","watch"]}}))}),
    )
}

pub fn optional_record_schema() -> Value {
    json!({"oneOf":[record_schema(),{"type":"null"}]})
}

fn worker_properties() -> Value {
    json!({
        "agent":{"type":"string"},
        "thread":{"type":"string"},
        "title":nullable("string"),
        "parent_thread":nullable("string"),
        "harness":{"type":"string"},
        "model":nullable("string"),
        "execution_state":{"enum":["waiting_for_user","running","idle"]},
        "latest_run":{"oneOf":[object(&["id","thread_id","engine"], json!({"id":{"type":"string"},"thread_id":{"type":"string"},"engine":{"type":"string"}})),{"type":"null"}]},
        "last_activity_at":{"type":"integer"},
        "archived":{"type":"boolean"},
        "unread":{"type":"boolean"},
        "project":project_schema(),
        "work":optional_record_schema(),
        "follow_up":object(&["inspect","read","coordinate"], json!({"inspect":{"type":"string"},"read":{"type":"string"},"coordinate":{"type":"string"}})),
    })
}

const WORKER_FIELDS: [&str; 14] = [
    "agent",
    "thread",
    "title",
    "parent_thread",
    "harness",
    "model",
    "execution_state",
    "latest_run",
    "last_activity_at",
    "archived",
    "unread",
    "project",
    "work",
    "follow_up",
];

fn worker_schema() -> Value {
    object(&WORKER_FIELDS, worker_properties())
}

fn ranked_worker_schema() -> Value {
    let mut properties = worker_properties();
    properties["relevance"] = object(
        &["score", "reasons"],
        json!({"score":{"type":"integer","minimum":1},"reasons":{"type":"array","minItems":1,"items":object(&["kind"], json!({"kind":{"enum":["overlapping_claim","work_dependency","overlapping_declared_scope","same_project"]}}))}}),
    );
    let mut required = WORKER_FIELDS.to_vec();
    required.push("relevance");
    object(&required, properties)
}

fn read_item_output() -> Value {
    object(
        &[
            "thread",
            "item",
            "offset",
            "text",
            "next_offset",
            "total_bytes",
        ],
        json!({"thread":{"type":"string"},"item":{"type":"string"},"offset":{"type":"integer","minimum":0},"text":{"type":"string"},"next_offset":nullable("integer"),"total_bytes":{"type":"integer","minimum":0}}),
    )
}

fn relevant_output() -> Value {
    object(
        &["observed_at", "scope", "ranking", "workers", "next_cursor"],
        json!({
            "observed_at":{"type":"integer"},
            "scope":object(&["host","project","resources"], json!({"host":{"const":"current_forge"},"project":nullable("string"),"resources":{"type":"array","items":{"type":"object"}}})),
            "ranking":{"type":"string"},
            "workers":{"type":"array","items":ranked_worker_schema()},
            "next_cursor":nullable("string"),
        }),
    )
}

fn list_output() -> Value {
    object(
        &["observed_at", "scope", "workers", "next_cursor"],
        json!({
            "observed_at":{"type":"integer"},
            "scope":object(&["host","project","worktree","default","worktree_source"], json!({"host":{"const":"current_forge"},"project":nullable("string"),"worktree":nullable("string"),"default":{"type":"string"},"worktree_source":{"type":"string"}})),
            "workers":{"type":"array","items":worker_schema()},
            "next_cursor":nullable("string"),
        }),
    )
}

fn get_output() -> Value {
    object(
        &["observed_at", "worker"],
        json!({"observed_at":{"type":"integer"},"worker":worker_schema()}),
    )
}

fn files_output() -> Value {
    object(
        &["scope", "activity", "next_cursor", "coverage", "follow_up"],
        json!({
            "scope":object(&["project","host"], json!({"project":nullable("string"),"host":{"const":"current_forge"}})),
            "activity":{"type":"array","items":object(&["thread","title","item","operation","path","created_at","currently_running"], json!({"thread":{"type":"string"},"title":nullable("string"),"item":{"type":"string"},"operation":{"type":"string"},"path":{"type":"string"},"created_at":{"type":"integer"},"currently_running":{"type":"boolean"}}))},
            "next_cursor":nullable("string"),
            "coverage":{"type":"string"},
            "follow_up":{"type":"string"},
        }),
    )
}

fn harnesses_output() -> Value {
    object(
        &["harnesses"],
        json!({"harnesses":{"type":"array","items":object(&["harness","installation","sign_in","default_model","discovery","model_count"], json!({"harness":{"enum":["claude","codex","opencode"]},"installation":object(&["state"], json!({"state":{"type":"string"}})),"sign_in":object(&["state"], json!({"state":{"type":"string"}})),"default_model":nullable("string"),"discovery":object(&["state"], json!({"state":{"type":"string"}})),"model_count":{"type":"integer","minimum":0}}))}}),
    )
}

fn models_output() -> Value {
    object(
        &["harness", "discovery", "models", "next_offset"],
        json!({"harness":{"enum":["claude","codex","opencode"]},"discovery":object(&["state"], json!({"state":{"type":"string"}})),"models":{"type":"array","items":object(&["id","name"], json!({"id":{"type":"string"},"name":{"type":"string"}}))},"next_offset":nullable("integer")}),
    )
}

pub fn tool(
    name: &'static str,
    description: &'static str,
    properties: fn() -> Value,
    required: &'static [&'static str],
    output: fn() -> Value,
    read_only: bool,
) -> Tool {
    Tool {
        name,
        description,
        properties,
        required,
        output_schema: Some(output),
        read_only,
        destructive: false,
    }
}

fn list_properties() -> Value {
    json!({
        "scope":{"type":"string","enum":["project","host"],"description":"Default current project on this Forge. host includes all accessible projects; resolved scope is returned."},
        "project":{"type":"string","description":"Explicit project ID; otherwise caller's project."},
        "worktree":{"type":"string","description":"Exact absolute declared worktree, falling back to project path only when work is unpublished."},
        "parent":{"type":"string","description":"Only children of this thread. Omit to discover peer workers too."},
        "include_agents":{"type":"boolean","description":"Include spawned threads; default true for agents, false for threads."},
        "archived":{"type":"boolean","description":"Default false. Independent of execution status and unread state."},
        "unread":{"type":"boolean"},
        "title":{"type":"string","maxLength":256},
        "cursor":{"type":"string","description":"Opaque next_cursor from a previous page."},
        "limit":{"type":"integer","minimum":1,"maximum":200,"default":30}
    })
}

pub fn tools() -> Vec<Tool> {
    vec![
        tool(
            "read_thread_item",
            "Read one long transcript item in bounded UTF-8 byte ranges. Use after a truncated read_thread result. Continue with next_offset until null; output can be included explicitly.",
            || json!({"item":{"type":"string"},"thread":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":64000,"default":16000},"include_output":{"type":"boolean"}}),
            &["item"],
            read_item_output,
            true,
        ),
        tool(
            "list_relevant_agents",
            "Find peer workers relevant to your task, ranked by overlapping advisory claims, declared scope, dependencies, then project activity. Use first for unexpected changes or before overlapping edits. Defaults to your project and infers resources from your published work and active claims; paths/resources can be explicit. Every result explains its relevance.",
            relevant_properties,
            &[],
            relevant_output,
            true,
        ),
        tool(
            "list_relevant_threads",
            "Find conversations relevant to your work, ranked by advisory claims, declared resources, dependencies, and project activity. Use to find who is changing related files even when their conversation title is unrelated. Returns reasons, current execution state, and coordination targets.",
            relevant_properties,
            &[],
            relevant_output,
            true,
        ),
        tool(
            "list_active_agents",
            "Find workers doing ongoing work, including peers and other threads' children. Use before overlapping edits or when unexplained changes may come from another agent. Default current project; includes running workers, user waits, and declared blocked/waiting work.",
            list_properties,
            &[],
            list_output,
            true,
        ),
        tool(
            "list_active_threads",
            "Find conversations with ongoing work in the current project. Use to understand concurrent activity and dependencies; execution status is separate from unread and archive state. Set include_agents=true to include child conversations.",
            list_properties,
            &[],
            list_output,
            true,
        ),
        tool(
            "list_agents",
            "List current and historical workers across the current project, including top-level and child agents. Use list_active_agents for ongoing work; parent explicitly restricts discovery to one parent's children.",
            list_properties,
            &[],
            list_output,
            true,
        ),
        tool(
            "list_threads",
            "List conversations in the current project, newest activity first, with current worker state and published work. Pagination applies after filters. Use scope=host for other projects and include_agents=true for child conversations.",
            list_properties,
            &[],
            list_output,
            true,
        ),
        tool(
            "get_agent",
            "Inspect one worker's execution state, project, declared worktree, objective, dependencies, and available follow-ups. Accepts the thread ID or spawned agent ID from discovery.",
            || json!({"agent":{"type":"string"}}),
            &["agent"],
            get_output,
            true,
        ),
        tool(
            "get_thread",
            "Inspect a conversation and its current worker. Use read_thread to retrieve messages and historical decisions.",
            || json!({"thread":{"type":"string"}}),
            &["thread"],
            get_output,
            true,
        ),
        tool(
            "find_threads_touching_files",
            "Find reported file activity and current work involving these paths. Use for unexpected changes, overlapping edits, or build failures from concurrent work. Historical touches are evidence, not ownership; advisory claims appear separately.",
            || json!({"paths":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":32},"project":{"type":"string"},"scope":{"type":"string","enum":["project","host"]},"limit":{"type":"integer","minimum":1,"maximum":200},"cursor":{"type":"string","description":"Opaque next_cursor from a previous page."},"match_mode":{"type":"string","enum":["exact","contains"],"default":"exact"}}),
            &["paths"],
            files_output,
            true,
        ),
        tool(
            "list_harnesses",
            "List harness installation, sign-in, default model, and discovery status without loading every model. Use list_models to find spawn_agent configuration choices.",
            || json!({}),
            &[],
            harnesses_output,
            true,
        ),
        tool(
            "list_models",
            "Find models and their supported configuration on one harness. Returns exact effort, variant, speed and context choices for spawn_agent. Defaults to a compact page; filter by query or model ID.",
            || json!({"harness":{"type":"string","enum":["claude","codex","opencode"]},"query":{"type":"string"},"model":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),
            &["harness"],
            models_output,
            true,
        ),
    ]
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct List {
    scope: Option<String>,
    project: Option<String>,
    worktree: Option<String>,
    parent: Option<String>,
    include_agents: Option<bool>,
    archived: Option<bool>,
    unread: Option<bool>,
    title: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
    state: Option<String>,
    paths: Option<Vec<String>>,
    resources: Option<Vec<Resource>>,
}

fn relevant_properties() -> Value {
    let mut properties = list_properties();
    properties["paths"] = json!({"type":"array","items":{"type":"string"},"maxItems":64,"description":"Paths inside worktree; otherwise use the caller's published work and active claims."});
    properties["resources"] = crate::coordination::resource_schema();
    properties
}

async fn relevant(hub: &Hub, caller: &ThreadId, name: &str, args: Value) -> Result<Value> {
    let list: List = serde_json::from_value(args).map_err(AwarenessError::Arguments)?;
    let caller_info = hub
        .store()
        .discover_workers(discovery::Query {
            target: Some(caller.to_string()),
            include_agents: true,
            limit: 1,
            ..Default::default()
        })
        .await
        .map_err(forge("find the calling worker"))?
        .workers
        .into_iter()
        .next()
        .ok_or_else(|| AwarenessError::CallerMissing {
            thread: caller.clone(),
        })?;
    let project = project_scope(hub, caller, list.scope.as_deref(), list.project.clone()).await?;
    let (resources, dependencies, records) = relevant_context(
        hub,
        caller,
        &list,
        &caller_info.project.path,
        project.clone(),
    )
    .await?;
    let mut matches = rank_records(records, &resources, &dependencies, caller);
    let mut query = discovery::Query {
        project: project.clone(),
        parent: list.parent.map(ThreadId::from),
        include_agents: list
            .include_agents
            .unwrap_or_else(|| name.ends_with("agents")),
        archived: Some(list.archived.unwrap_or(false)),
        unread: list.unread,
        title: list.title.unwrap_or_default(),
        worktree: list.worktree.as_deref().map(normalized).transpose()?,
        limit: 200,
        ..Default::default()
    };
    let mut ranked = Vec::new();
    loop {
        let page = hub
            .store()
            .discover_workers(query.clone())
            .await
            .map_err(forge("list the workers to rank"))?;
        for item in page.workers {
            if item.thread.id == *caller {
                continue;
            }
            let (mut score, mut reasons) = matches.remove(&item.thread.id).unwrap_or_default();
            if item.project.id == caller_info.project.id {
                score = score.max(10);
                reasons.push(json!({"kind":"same_project","project":item.project.id}));
            }
            if score > 0 {
                ranked.push((
                    score,
                    item.thread.activity_seq,
                    item.thread.id.clone(),
                    reasons,
                    item,
                ));
            }
        }
        match page.next_cursor {
            Some(cursor) => {
                query.cursor = Some(serde_json::from_str(&cursor).map_err(AwarenessError::Cursor)?);
            }
            None => break,
        }
    }
    ranked.sort_by(|a, b| (b.0, b.1, &b.2).cmp(&(a.0, a.1, &a.2)));
    if let Some(cursor) = list.cursor {
        let cursor: (u32, u64, ThreadId) =
            serde_json::from_str(&cursor).map_err(AwarenessError::Cursor)?;
        ranked.retain(|entry| (entry.0, entry.1, &entry.2) < (cursor.0, cursor.1, &cursor.2));
    }
    let limit = list.limit.unwrap_or(30).clamp(1, 200) as usize;
    let has_more = ranked.len() > limit;
    ranked.truncate(limit);
    let next_cursor = if has_more {
        ranked
            .last()
            .map(|entry| serde_json::to_string(&(entry.0, entry.1, &entry.2)))
            .transpose()
            .map_err(AwarenessError::NextCursor)?
    } else {
        None
    };
    Ok(
        json!({"observed_at":artisan_api::now(),"scope":{"host":"current_forge","project":project,"resources":resources},"ranking":"overlapping_claim, work_dependency, overlapping_declared_scope, same_project; then newest activity","workers":ranked.into_iter().map(|entry|{ let mut result=worker(hub,&entry.4); result["relevance"]=json!({"score":entry.0,"reasons":entry.3}); result }).collect::<Vec<_>>(),"next_cursor":next_cursor}),
    )
}

pub async fn project_scope(
    hub: &Hub,
    caller: &ThreadId,
    scope: Option<&str>,
    project: Option<String>,
) -> Result<Option<ProjectId>> {
    match (scope.unwrap_or("project"), project) {
        ("host", None) => Ok(None),
        ("project" | "host", Some(project)) => Ok(Some(ProjectId::from(project))),
        ("project", None) => Ok(Some(
            hub.store()
                .thread(caller)
                .await
                .map_err(forge("load the calling thread"))?
                .ok_or_else(|| AwarenessError::CallerMissing {
                    thread: caller.clone(),
                })?
                .project_id,
        )),
        (other, _) => Err(AwarenessError::Scope {
            scope: other.to_owned(),
        }),
    }
}

fn worker(hub: &Hub, worker: &discovery::Worker) -> Value {
    let state = match (worker.awaiting_user, hub.is_live(&worker.thread.id)) {
        (true, _) => "waiting_for_user",
        (_, true) => "running",
        _ => "idle",
    };
    json!({"agent":worker.thread.spawned_by.as_ref().map_or_else(||worker.thread.id.to_string(),ToString::to_string),"thread":worker.thread.id,"title":worker.thread.title,"parent_thread":worker.thread.parent_thread_id,"harness":worker.thread.config.engine,"model":worker.thread.config.model,"execution_state":state,"latest_run":worker.latest_run,"last_activity_at":worker.thread.activity_at,"archived":worker.thread.archived,"unread":worker.thread.is_unread(),"project":worker.project,"work":worker.work,"follow_up":{"inspect":"get_agent","read":"read_thread","coordinate":"send_coordination_message"}})
}

pub async fn resolve_agent(hub: &Hub, target: &str) -> Result<ThreadId> {
    let page = hub
        .store()
        .discover_workers(discovery::Query {
            target: Some(target.to_owned()),
            include_agents: true,
            limit: 2,
            ..Default::default()
        })
        .await
        .map_err(forge("find the worker"))?;
    match page.workers.as_slice() {
        [worker] => Ok(worker.thread.id.clone()),
        workers => Err(AwarenessError::Worker {
            target: target.to_owned(),
            found: workers.len(),
        }),
    }
}

pub async fn call(
    hub: &Hub,
    caller: &ThreadId,
    raw_name: &str,
    arguments: Value,
) -> Result<Option<Value>> {
    let name = names::canonical(raw_name);
    match name {
        "read_thread_item" => read_item(hub, caller, name, raw_name, arguments).await,
        "list_relevant_agents" | "list_relevant_threads" => {
            Ok(Some(relevant(hub, caller, name, arguments).await?))
        }
        "list_active_agents" | "list_active_threads" | "list_agents" | "list_threads" => {
            list_workers(hub, caller, name, raw_name, arguments).await
        }
        "get_agent" | "get_thread" => get_worker(hub, caller, name, raw_name, arguments).await,
        "list_harnesses" if raw_name != "harnesses" => Ok(Some(
            json!({"harnesses":EngineId::ALL.into_iter().map(|engine| { let status=hub.engines().status(engine); json!({"harness":engine,"installation":status.install,"sign_in":status.sign_in,"default_model":status.default_model,"discovery":status.discovery,"model_count":status.models.len()}) }).collect::<Vec<_>>()}),
        )),
        "list_models" => list_models(hub, caller, name, raw_name, arguments),
        "find_threads_touching_files" => find_files(hub, caller, name, raw_name, arguments).await,
        _ => Ok(None),
    }
}

async fn read_item(
    hub: &Hub,
    _caller: &ThreadId,
    _name: &str,
    _raw_name: &str,
    arguments: Value,
) -> Result<Option<Value>> {
    let id = ItemId::from(
        arguments
            .get("item")
            .and_then(Value::as_str)
            .ok_or(AwarenessError::Missing { field: "item" })?,
    );
    let item = hub
        .store()
        .item(&id)
        .await
        .map_err(forge("load the item"))?
        .ok_or_else(|| AwarenessError::ItemMissing { item: id.clone() })?;
    if let Some(thread) = arguments.get("thread").and_then(Value::as_str)
        && thread != item.thread_id.as_str()
    {
        return Err(AwarenessError::ItemElsewhere {
            item: id,
            owner: item.thread_id,
            thread: thread.to_owned(),
        });
    }
    let output = arguments
        .get("include_output")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let rendered = crate::transcript::render_item(hub.store(), &item, output)
        .await
        .map_err(|source| AwarenessError::Render {
            item: item.id.clone(),
            source,
        })?;
    let offset = arguments.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(16000)
        .clamp(1, 64000);
    let total = rendered.len();
    let slice = rendered
        .slice(offset, limit)
        .await
        .map_err(|error| match error {
            crate::transcript::SliceError::Offset { .. } => AwarenessError::Offset { offset },
            crate::transcript::SliceError::Render(source) => AwarenessError::Render {
                item: item.id.clone(),
                source,
            },
        })?;
    Ok(Some(
        json!({"thread":item.thread_id,"item":item.id,"offset":offset,"text":slice.text,"next_offset":(slice.end<total).then_some(slice.end),"total_bytes":total}),
    ))
}

async fn list_workers(
    hub: &Hub,
    caller: &ThreadId,
    name: &str,
    raw_name: &str,
    arguments: Value,
) -> Result<Option<Value>> {
    let mut list: List = serde_json::from_value(arguments).map_err(AwarenessError::Arguments)?;
    if raw_name == "agent_list" {
        list.parent = Some(caller.to_string());
        list.scope = Some("host".to_owned());
    }
    if raw_name == "thread_list" && list.scope.is_none() {
        list.scope = Some("host".to_owned());
    }
    let project = project_scope(hub, caller, list.scope.as_deref(), list.project.clone()).await?;
    let active = name.starts_with("list_active_") || list.state.as_deref() == Some("working");
    if let Some(state) = &list.state
        && !["working", "any", "archived", "unread"].contains(&state.as_str())
    {
        return Err(AwarenessError::LegacyState {
            state: state.clone(),
        });
    }
    let archived = list
        .archived
        .unwrap_or_else(|| list.state.as_deref() == Some("archived"));
    let unread = list
        .unread
        .or_else(|| (list.state.as_deref() == Some("unread")).then_some(true));
    let worktree = list.worktree.as_deref().map(normalized).transpose()?;
    let page = hub
        .store()
        .discover_workers(discovery::Query {
            project: project.clone(),
            parent: list.parent.map(ThreadId::from),
            include_agents: list
                .include_agents
                .unwrap_or_else(|| name.ends_with("agents")),
            active,
            live_threads: hub.active_thread_ids(),
            archived: Some(archived),
            unread,
            title: list.title.unwrap_or_default(),
            cursor: list
                .cursor
                .map(|s| serde_json::from_str(&s))
                .transpose()
                .map_err(AwarenessError::Cursor)?,
            worktree: worktree.clone(),
            limit: list.limit.unwrap_or(30),
            ..Default::default()
        })
        .await
        .map_err(forge("list the workers"))?;
    Ok(Some(
        json!({"observed_at":artisan_api::now(),"scope":{"host":"current_forge","project":project,"worktree":worktree,"default":"caller_project","worktree_source":"published_work_or_project_path"},"workers":page.workers.into_iter().map(|w|worker(hub,&w)).collect::<Vec<_>>(),"next_cursor":page.next_cursor}),
    ))
}

async fn get_worker(
    hub: &Hub,
    _caller: &ThreadId,
    name: &str,
    _raw_name: &str,
    arguments: Value,
) -> Result<Option<Value>> {
    let key = match name {
        "get_agent" => "agent",
        _ => "thread",
    };
    let target = arguments
        .get(key)
        .and_then(Value::as_str)
        .ok_or(AwarenessError::Missing { field: key })?;
    let target = resolve_agent(hub, target).await?;
    let page = hub
        .store()
        .discover_workers(discovery::Query {
            target: Some(target.to_string()),
            include_agents: true,
            limit: 1,
            ..Default::default()
        })
        .await
        .map_err(forge("load the worker"))?;
    let found = page
        .workers
        .into_iter()
        .next()
        .ok_or(AwarenessError::Vanished { thread: target })?;
    Ok(Some(
        json!({"observed_at":artisan_api::now(),"worker":worker(hub,&found)}),
    ))
}

fn list_models(
    hub: &Hub,
    _caller: &ThreadId,
    _name: &str,
    _raw_name: &str,
    arguments: Value,
) -> Result<Option<Value>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Models {
        harness: EngineId,
        query: Option<String>,
        model: Option<String>,
        offset: Option<usize>,
        limit: Option<usize>,
    }
    let args: Models = serde_json::from_value(arguments).map_err(AwarenessError::Arguments)?;
    let status = hub.engines().status(args.harness);
    let query = args.query.unwrap_or_default().to_lowercase();
    let matches: Vec<_> = status
        .models
        .iter()
        .filter(|model| {
            args.model.as_ref().is_none_or(|id| id == &model.id)
                && (query.is_empty()
                    || model.id.to_lowercase().contains(&query)
                    || model.name.to_lowercase().contains(&query))
        })
        .collect();
    let offset = args.offset.unwrap_or(0);
    let limit = args.limit.unwrap_or(20).clamp(1, 100);
    let next =
        (offset.saturating_add(limit) < matches.len()).then_some(offset.saturating_add(limit));
    Ok(Some(
        json!({"harness":args.harness,"discovery":status.discovery,"models":matches.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"next_offset":next}),
    ))
}

async fn find_files(
    hub: &Hub,
    caller: &ThreadId,
    _name: &str,
    raw_name: &str,
    arguments: Value,
) -> Result<Option<Value>> {
    let mut args = arguments;
    if let Some(path) = args.get("path").and_then(Value::as_str).map(str::to_owned) {
        args["paths"] = json!([path]);
    }
    let paths: Vec<String> = serde_json::from_value(
        args.get("paths")
            .cloned()
            .ok_or(AwarenessError::Missing { field: "paths" })?,
    )
    .map_err(AwarenessError::Arguments)?;
    if paths.is_empty() || paths.len() > 32 {
        return Err(AwarenessError::Paths { count: paths.len() });
    }
    let project = project_scope(
        hub,
        caller,
        args.get("scope").and_then(Value::as_str),
        args.get("project")
            .and_then(Value::as_str)
            .map(str::to_owned),
    )
    .await?;
    let contains = args
        .get("match_mode")
        .and_then(Value::as_str)
        .unwrap_or(match raw_name {
            "threads_touching" => "contains",
            _ => "exact",
        })
        == "contains";
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(30)
        .clamp(1, 200) as u32;
    let paths = paths
        .into_iter()
        .map(|path| {
            if contains {
                Ok(path)
            } else {
                normalized(&path)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let page = hub
        .store()
        .discover_file_activity(
            project.clone(),
            paths,
            contains,
            args.get("cursor")
                .and_then(Value::as_str)
                .map(serde_json::from_str)
                .transpose()
                .map_err(AwarenessError::Cursor)?,
            limit,
        )
        .await
        .map_err(forge("list the file activity"))?;
    let activity=page.activity.into_iter().map(|touch|json!({"thread":touch.thread,"title":touch.title,"item":touch.item,"operation":touch.operation,"path":touch.path,"created_at":touch.created_at,"currently_running":hub.is_live(&touch.thread)})).collect::<Vec<_>>();
    Ok(Some(
        json!({"scope":{"project":project,"host":"current_forge"},"activity":activity,"next_cursor":page.next_cursor,"coverage":"indexed tool-reported paths; historical activity is not ownership","follow_up":"list_resource_claims"}),
    ))
}

fn rank_records(
    records: Vec<Record>,
    resources: &[Resource],
    dependencies: &[ThreadId],
    caller: &ThreadId,
) -> BTreeMap<ThreadId, (u32, Vec<Value>)> {
    let mut matches: BTreeMap<ThreadId, (u32, Vec<Value>)> = BTreeMap::new();
    for record in records {
        let reason = match &record.content {
            Content::Claim {
                resources: other, ..
            } if record.active_claim(artisan_api::now())
                && other
                    .iter()
                    .any(|a| resources.iter().any(|b| a.overlaps(b))) =>
            {
                Some((
                    100,
                    json!({"kind":"overlapping_claim","record":record.id,"resources":other}),
                ))
            }
            Content::Work {
                dependencies: depends,
                ..
            } if dependencies.contains(&record.owner) || depends.contains(caller) => {
                Some((80, json!({"kind":"work_dependency","record":record.id})))
            }
            Content::Work {
                resources: other, ..
            } if other
                .iter()
                .any(|a| resources.iter().any(|b| a.overlaps(b))) =>
            {
                Some((
                    60,
                    json!({"kind":"overlapping_declared_scope","record":record.id}),
                ))
            }
            Content::Work { .. }
            | Content::Claim { .. }
            | Content::Message { .. }
            | Content::Watch { .. } => None,
        };
        if let Some((score, reason)) = reason {
            let entry = matches.entry(record.owner).or_default();
            entry.0 = entry.0.max(score);
            entry.1.push(reason);
        }
    }
    matches
}

async fn relevant_context(
    hub: &Hub,
    caller: &ThreadId,
    list: &List,
    project_path: &str,
    project: Option<ProjectId>,
) -> Result<(Vec<Resource>, Vec<ThreadId>, Vec<Record>)> {
    let mut resources = list.resources.clone().unwrap_or_default();
    let mut dependencies = Vec::new();
    let own_work = hub
        .store()
        .coordination_get(format!("work:{caller}"))
        .await
        .map_err(forge("load the caller's published work"))?;
    let declared_worktree = match &own_work {
        Some(Record {
            content:
                Content::Work {
                    worktree,
                    resources: declared,
                    dependencies: depends,
                    ..
                },
            ..
        }) => {
            resources.extend(declared.clone());
            dependencies = depends.clone();
            Some(worktree.clone())
        }
        _ => None,
    };
    let worktree = list
        .worktree
        .clone()
        .or(declared_worktree)
        .unwrap_or_else(|| project_path.to_owned());
    for path in list.paths.clone().unwrap_or_default() {
        resources.push(Resource::Path {
            worktree: worktree.clone(),
            path,
            recursive: false,
        });
    }
    let records = crate::coordination::all_records(
        hub,
        CoordinationQuery {
            project: project.clone(),
            ..Default::default()
        },
    )
    .await
    .map_err(AwarenessError::Records)?;
    for record in &records {
        if record.owner == *caller
            && record.active_claim(artisan_api::now())
            && let Content::Claim {
                resources: claimed, ..
            } = &record.content
        {
            resources.extend(claimed.clone());
        }
    }
    if resources.len() > 256 {
        return Err(AwarenessError::TooManyResources {
            count: resources.len(),
        });
    }
    let resources = resources
        .into_iter()
        .map(Resource::normalize)
        .collect::<Result<Vec<_>, _>>()
        .map_err(AwarenessError::Resource)?;
    Ok((resources, dependencies, records))
}

#[cfg(test)]
#[path = "../tests/mcp/awareness.rs"]
mod tests;
