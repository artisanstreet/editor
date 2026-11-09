use crate::{Tool, names};
use anyhow::{Context, Result, ensure};
use artisan_api::{EngineId, ProjectId, ThreadId};
use forge_runs::Hub;
use forge_store::coordination::{Content, Query as CoordinationQuery, Record, Resource};
use forge_store::{coordination::normalize_path, discovery};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn output_schema() -> Value {
    json!({"type":"object","properties":{"observed_at":{"type":"integer"},"scope":{"type":"object"},"workers":{"type":"array","items":{"type":"object"}},"next_cursor":{"type":["string","null"]},"record":{"type":["object","null"]},"records":{"type":"array","items":{"type":"object"}},"event_cursor":{"type":"integer"}}})
}

pub fn tool(
    name: &'static str,
    description: &'static str,
    properties: fn() -> Value,
    required: &'static [&'static str],
    read_only: bool,
) -> Tool {
    Tool {
        name,
        description,
        properties,
        required,
        output_schema: Some(output_schema),
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
            true,
        ),
        tool(
            "list_relevant_agents",
            "Find peer workers relevant to your task, ranked by overlapping advisory claims, declared scope, dependencies, then project activity. Use first for unexpected changes or before overlapping edits. Defaults to your project and infers resources from your published work and active claims; paths/resources can be explicit. Every result explains its relevance.",
            relevant_properties,
            &[],
            true,
        ),
        tool(
            "list_relevant_threads",
            "Find conversations relevant to your work, ranked by advisory claims, declared resources, dependencies, and project activity. Use to find who is changing related files even when their conversation title is unrelated. Returns reasons, current execution state, and coordination targets.",
            relevant_properties,
            &[],
            true,
        ),
        tool(
            "list_active_agents",
            "Find workers doing ongoing work, including peers and other threads' children. Use before overlapping edits or when unexplained changes may come from another agent. Default current project; includes running workers, user waits, and declared blocked/waiting work.",
            list_properties,
            &[],
            true,
        ),
        tool(
            "list_active_threads",
            "Find conversations with ongoing work in the current project. Use to understand concurrent activity and dependencies; execution status is separate from unread and archive state. Set include_agents=true to include child conversations.",
            list_properties,
            &[],
            true,
        ),
        tool(
            "list_agents",
            "List current and historical workers across the current project, including top-level and child agents. Use list_active_agents for ongoing work; parent explicitly restricts discovery to one parent's children.",
            list_properties,
            &[],
            true,
        ),
        tool(
            "list_threads",
            "List conversations in the current project, newest activity first, with current worker state and published work. Pagination applies after filters. Use scope=host for other projects and include_agents=true for child conversations.",
            list_properties,
            &[],
            true,
        ),
        tool(
            "get_agent",
            "Inspect one worker's execution state, project, declared worktree, objective, dependencies, and available follow-ups. Accepts the thread ID or spawned agent ID from discovery.",
            || json!({"agent":{"type":"string"}}),
            &["agent"],
            true,
        ),
        tool(
            "get_thread",
            "Inspect a conversation and its current worker. Use read_thread to retrieve messages and historical decisions.",
            || json!({"thread":{"type":"string"}}),
            &["thread"],
            true,
        ),
        tool(
            "find_threads_touching_files",
            "Find reported file activity and current work involving these paths. Use for unexpected changes, overlapping edits, or build failures from concurrent work. Historical touches are evidence, not ownership; advisory claims appear separately.",
            || json!({"paths":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":32},"project":{"type":"string"},"scope":{"type":"string","enum":["project","host"]},"limit":{"type":"integer","minimum":1,"maximum":200},"cursor":{"type":"string","description":"Opaque next_cursor from a previous page."},"match_mode":{"type":"string","enum":["exact","contains"],"default":"exact"}}),
            &["paths"],
            true,
        ),
        tool(
            "list_harnesses",
            "List harness installation, sign-in, default model, and discovery status without loading every model. Use list_models to find spawn_agent configuration choices.",
            || json!({}),
            &[],
            true,
        ),
        tool(
            "list_models",
            "Find models and their supported configuration on one harness. Returns exact effort, variant, speed and context choices for spawn_agent. Defaults to a compact page; filter by query or model ID.",
            || json!({"harness":{"type":"string","enum":["claude","codex","opencode"]},"query":{"type":"string"},"model":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":100}}),
            &["harness"],
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
    let list: List = serde_json::from_value(args)?;
    let caller_info = hub
        .store()
        .discover_workers(discovery::Query {
            target: Some(caller.to_string()),
            include_agents: true,
            limit: 1,
            ..Default::default()
        })
        .await?
        .workers
        .into_iter()
        .next()
        .context("caller not found")?;
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
        worktree: list.worktree.map(|p| normalize_path(&p)).transpose()?,
        limit: 200,
        ..Default::default()
    };
    let mut ranked = Vec::new();
    loop {
        let page = hub.store().discover_workers(query.clone()).await?;
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
            Some(cursor) => query.cursor = Some(serde_json::from_str(&cursor)?),
            None => break,
        }
    }
    ranked.sort_by(|a, b| (b.0, b.1, &b.2).cmp(&(a.0, a.1, &a.2)));
    if let Some(cursor) = list.cursor {
        let cursor: (u32, u64, ThreadId) = serde_json::from_str(&cursor)?;
        ranked.retain(|entry| (entry.0, entry.1, &entry.2) < (cursor.0, cursor.1, &cursor.2));
    }
    let limit = list.limit.unwrap_or(30).clamp(1, 200) as usize;
    let has_more = ranked.len() > limit;
    ranked.truncate(limit);
    let next_cursor = if has_more {
        ranked
            .last()
            .map(|entry| serde_json::to_string(&(entry.0, entry.1, &entry.2)))
            .transpose()?
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
                .await?
                .context("caller thread not found")?
                .project_id,
        )),
        _ => anyhow::bail!("scope must be project or host"),
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
        .await?;
    ensure!(
        page.workers.len() == 1,
        "worker not found or ambiguous; use the exact thread ID from list_agents"
    );
    Ok(page.workers[0].thread.id.clone())
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
    let id = arguments
        .get("item")
        .and_then(Value::as_str)
        .context("item required")?;
    let item = hub
        .store()
        .item(&artisan_api::ItemId::from(id))
        .await?
        .context("item not found")?;
    ensure!(
        arguments
            .get("thread")
            .and_then(Value::as_str)
            .is_none_or(|thread| thread == item.thread_id.as_str()),
        "item belongs to another thread"
    );
    let rendered = crate::transcript::render(
        hub.store(),
        std::slice::from_ref(&item),
        &crate::transcript::Options {
            whole: true,
            output: arguments
                .get("include_output")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            unbounded: true,
        },
    )
    .await?;
    let start = usize::try_from(arguments.get("offset").and_then(Value::as_u64).unwrap_or(0))?;
    ensure!(
        start <= rendered.len() && rendered.is_char_boundary(start),
        "offset must be a UTF-8 boundary from next_offset"
    );
    let mut end = start
        .saturating_add(
            arguments
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(16000)
                .clamp(1, 64000) as usize,
        )
        .min(rendered.len());
    while end > start && !rendered.is_char_boundary(end) {
        end -= 1;
    }
    if end == start && start < rendered.len() {
        end = start
            + rendered[start..]
                .chars()
                .next()
                .context("missing character")?
                .len_utf8();
    }
    Ok(Some(
        json!({"thread":item.thread_id,"item":item.id,"offset":start,"text":&rendered[start..end],"next_offset":(end<rendered.len()).then_some(end),"total_bytes":rendered.len()}),
    ))
}

async fn list_workers(
    hub: &Hub,
    caller: &ThreadId,
    name: &str,
    raw_name: &str,
    arguments: Value,
) -> Result<Option<Value>> {
    let mut list: List = serde_json::from_value(arguments)?;
    if raw_name == "agent_list" {
        list.parent = Some(caller.to_string());
        list.scope = Some("host".to_owned());
    }
    if raw_name == "thread_list" && list.scope.is_none() {
        list.scope = Some("host".to_owned());
    }
    let project = project_scope(hub, caller, list.scope.as_deref(), list.project.clone()).await?;
    let active = name.starts_with("list_active_") || list.state.as_deref() == Some("working");
    if let Some(state) = &list.state {
        ensure!(
            ["working", "any", "archived", "unread"].contains(&state.as_str()),
            "unknown legacy state filter"
        );
    }
    let archived = list
        .archived
        .unwrap_or_else(|| list.state.as_deref() == Some("archived"));
    let unread = list
        .unread
        .or_else(|| (list.state.as_deref() == Some("unread")).then_some(true));
    let worktree = list
        .worktree
        .map(|path| normalize_path(&path))
        .transpose()?;
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
            cursor: list.cursor.map(|s| serde_json::from_str(&s)).transpose()?,
            worktree: worktree.clone(),
            limit: list.limit.unwrap_or(30),
            ..Default::default()
        })
        .await?;
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
        .context("worker or thread ID is required")?;
    let target = resolve_agent(hub, target).await?;
    let page = hub
        .store()
        .discover_workers(discovery::Query {
            target: Some(target.to_string()),
            include_agents: true,
            limit: 1,
            ..Default::default()
        })
        .await?;
    Ok(Some(
        json!({"observed_at":artisan_api::now(),"worker":worker(hub,&page.workers.into_iter().next().context("worker not found")?)}),
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
    let args: Models = serde_json::from_value(arguments)?;
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
    let paths: Vec<String> =
        serde_json::from_value(args.get("paths").cloned().context("paths required")?)?;
    ensure!(
        !paths.is_empty() && paths.len() <= 32,
        "provide 1..32 file paths"
    );
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
                normalize_path(&path)
            }
        })
        .collect::<Result<Vec<_>>>()?;
    let page = hub
        .store()
        .discover_file_activity(
            project.clone(),
            paths,
            contains,
            args.get("cursor")
                .and_then(Value::as_str)
                .map(serde_json::from_str)
                .transpose()?,
            limit,
        )
        .await?;
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
        .await?;
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
    .await?;
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
    ensure!(
        resources.len() <= 256,
        "too many relevance resources; use a narrower explicit scope"
    );
    let resources = resources
        .into_iter()
        .map(Resource::normalize)
        .collect::<Result<Vec<_>>>()?;
    Ok((resources, dependencies, records))
}
