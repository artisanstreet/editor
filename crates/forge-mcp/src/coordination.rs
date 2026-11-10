use crate::{Tool, awareness};
use anyhow::{Context, Result, ensure};
use artisan_api::{ThreadId, now};
use forge_runs::Hub;
use forge_store::coordination::{
    ClaimState, Content, Intent, MessageState, Query, Record, Resource, Update, WatchState,
    WorkState,
};
use serde_json::{Value, json};
use std::time::Duration;

pub fn resource_schema() -> Value {
    json!({"type":"array","maxItems":64,"items":{"oneOf":[{"type":"object","properties":{"kind":{"const":"path"},"worktree":{"type":"string","description":"Absolute checkout root on this Forge host."},"path":{"type":"string","description":"Absolute path or path relative to that worktree."},"recursive":{"type":"boolean","default":false}},"required":["kind","worktree","path"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"named"},"namespace":{"type":"string","description":"For example build, deployment, browser, or port."},"key":{"type":"string","description":"Stable shared resource identity on this Forge."}},"required":["kind","namespace","key"],"additionalProperties":false}]}})
}

fn record_properties() -> Value {
    json!({"id":{"type":"string"},"revision":{"type":"integer","minimum":1}})
}
fn list_properties() -> Value {
    json!({"scope":{"type":"string","enum":["project","host"]},"project":{"type":"string"},"owner":{"type":"string"},"cursor":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200,"default":30}})
}

#[expect(
    clippy::too_many_lines,
    reason = "declarative coordination tool catalog"
)]
pub fn tools() -> Vec<Tool> {
    vec![
        awareness::tool(
            "publish_work",
            "Publish this worker's objective, exact worktree, intended resources, dependencies, state, validation evidence, and handoff summary. Use at task start and when scope or readiness changes. Advisory only; updates require the revision from get_work.",
            || json!({"objective":{"type":"string","maxLength":4096},"worktree":{"type":"string"},"state":{"type":"string","enum":["running","waiting","blocked","completed","failed","cancelled"]},"resources":resource_schema(),"dependencies":{"type":"array","items":{"type":"string"},"maxItems":64,"description":"Other thread IDs with published work; cycles are rejected."},"summary":{"type":"string","maxLength":8192},"evidence":{"type":"array","items":{"type":"string"},"maxItems":32},"revision":{"type":"integer","minimum":0}}),
            &["objective", "worktree", "state"],
            false,
        ),
        awareness::tool(
            "get_work",
            "Inspect a worker's published scope, dependency readiness, validation evidence, and current revision. Missing published work is reported explicitly. Defaults to this thread.",
            || json!({"thread":{"type":"string"}}),
            &[],
            true,
        ),
        awareness::tool(
            "claim_resources",
            "Declare advisory ownership of files, directories, builds, deployments, browser sessions, or other resources. Use before overlapping work. Overlaps are returned for negotiation; claims never block edits or reject competing declarations. Default lifetime is one hour.",
            || json!({"resources":resource_schema(),"purpose":{"type":"string","maxLength":4096},"lifetime_seconds":{"type":"integer","minimum":1,"maximum":86400,"default":3600}}),
            &["resources", "purpose"],
            false,
        ),
        awareness::tool(
            "list_resource_claims",
            "Inspect advisory claims in the current project, including released and expired claims with explicit active flags. Filter resources to find overlaps; use scope=host to include other projects. Claims are coordination metadata, never filesystem locks.",
            || {
                let mut p = list_properties();
                p["resources"] = resource_schema();
                p
            },
            &[],
            true,
        ),
        awareness::tool(
            "renew_resource_claim",
            "Renew your advisory claim using its current revision. This updates metadata only and does not grant filesystem permissions.",
            || {
                let mut p = record_properties();
                p["lifetime_seconds"] = json!({"type":"integer","minimum":1,"maximum":86400});
                p
            },
            &["id", "revision", "lifetime_seconds"],
            false,
        ),
        awareness::tool(
            "release_resources",
            "Release your advisory resource claim after work or an agreed handoff. Only the claimant may release it; other workers negotiate through request_resource_handoff.",
            record_properties,
            &["id", "revision"],
            false,
        ),
        awareness::tool(
            "request_resource_handoff",
            "Ask an advisory claimant to release or sequence work on their resources. Delivers an acknowledged coordination request to its owner; it never transfers or revokes a claim automatically.",
            || json!({"claim":{"type":"string"},"text":{"type":"string","maxLength":16384}}),
            &["claim", "text"],
            false,
        ),
        awareness::tool(
            "send_coordination_message",
            "Send a peer worker a boundary, dependency, handoff, or information request and track its acknowledgment separately from delivery. Starts or steers the recipient's thread. Use id to retry an existing prepared request without duplicating delivery.",
            || json!({"recipient":{"type":"string"},"intent":{"type":"string","enum":["information","boundary","dependency","handoff"]},"text":{"type":"string","maxLength":16384},"id":{"type":"string","description":"Retry this existing request; omit to create one."}}),
            &[],
            false,
        ),
        awareness::tool(
            "list_coordination_messages",
            "Read incoming or sent coordination requests and their delivery, acknowledgment, and resolution states. Defaults to this worker's inbox; reading does not acknowledge an agreement.",
            || json!({"direction":{"type":"string","enum":["incoming","sent"],"default":"incoming"},"cursor":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200}}),
            &[],
            true,
        ),
        awareness::tool(
            "acknowledge_coordination_message",
            "Acknowledge, resolve, or decline a coordination request addressed to this worker. Records your reply and notifies its sender. Acknowledgment accepts the communication; use the reply to state the exact agreement.",
            || {
                let mut p = record_properties();
                p["state"] = json!({"type":"string","enum":["acknowledged","resolved","declined"]});
                p["reply"] = json!({"type":"string","maxLength":8192});
                p
            },
            &["id", "revision", "state"],
            false,
        ),
        awareness::tool(
            "get_coordination_record",
            "Read a claim, work declaration, coordination request, or watch by stable ID, including its revision for safe updates.",
            || json!({"id":{"type":"string"}}),
            &["id"],
            true,
        ),
        awareness::tool(
            "watch_work",
            "Subscribe this thread to another worker's terminal work result. Completion, failure, or cancellation sends a notification even after this turn ends or Forge restarts. Published work is required; after_revision avoids stale completions.",
            || json!({"thread":{"type":"string"},"after_revision":{"type":"integer","minimum":0}}),
            &["thread"],
            false,
        ),
        awareness::tool(
            "cancel_work_watch",
            "Cancel your pending dependency notification without changing the watched worker's work.",
            record_properties,
            &["id", "revision"],
            false,
        ),
        awareness::tool(
            "wait_for_work",
            "Wait briefly for published work to complete, fail, or be cancelled. Returns current readiness on timeout; use watch_work for a durable background notification while doing independent work.",
            || json!({"thread":{"type":"string"},"after_revision":{"type":"integer","minimum":0},"timeout_seconds":{"type":"integer","minimum":0,"maximum":30,"default":20}}),
            &["thread"],
            true,
        ),
        awareness::tool(
            "list_coordination_events",
            "Read durable coordination changes after an event cursor. Use event_cursor from list_resource_claims or list_coordination_messages to avoid missing changes between observation and waiting.",
            || json!({"after":{"type":"integer","minimum":0},"scope":{"type":"string","enum":["project","host"]},"project":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200}}),
            &["after"],
            true,
        ),
    ]
}

pub async fn all_records(hub: &Hub, mut query: Query) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    loop {
        query.limit = 200;
        let page = hub.store().coordination_list(query.clone()).await?;
        records.extend(page.records);
        match page.next_cursor {
            Some(cursor) => query.cursor = Some(serde_json::from_str(&cursor)?),
            None => break,
        }
    }
    Ok(records)
}

fn string(args: &Value, key: &str) -> Result<String> {
    Ok(args
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{key} is required"))?
        .to_owned())
}

fn revision(args: &Value) -> Result<u64> {
    args.get("revision")
        .and_then(Value::as_u64)
        .context("current record revision is required")
}

async fn send(
    hub: &Hub,
    caller: &ThreadId,
    recipient: ThreadId,
    intent: Intent,
    text: String,
) -> Result<Record> {
    let record = hub
        .store()
        .coordination_put(
            caller.clone(),
            None,
            Content::Message {
                recipient,
                intent,
                text,
                state: MessageState::Prepared,
                reply: None,
                delivery_item: None,
                receipt_revision: 0,
            },
            None,
        )
        .await?;
    hub.deliver_coordination_message(caller, record.id).await
}

pub async fn call(hub: &Hub, caller: &ThreadId, name: &str, args: Value) -> Result<Option<Value>> {
    let value = match name {
        "publish_work" => publish_work(hub, caller, name, args).await?,
        "get_work" => get_work(hub, caller, name, args).await?,
        "claim_resources" => claim_resources(hub, caller, name, args).await?,
        "list_resource_claims" | "list_coordination_messages" => {
            list_records(hub, caller, name, args).await?
        }
        "get_coordination_record" => get_record(hub, caller, name, args).await?,
        "renew_resource_claim" | "release_resources" | "cancel_work_watch" => {
            transition(hub, caller, name, args).await?
        }
        "request_resource_handoff" => request_handoff(hub, caller, name, args).await?,
        "send_coordination_message" => send_message(hub, caller, name, args).await?,
        "acknowledge_coordination_message" => acknowledge(hub, caller, name, args).await?,
        "watch_work" => watch(hub, caller, name, args).await?,
        "wait_for_work" => wait(hub, caller, name, args).await?,
        "list_coordination_events" => events(hub, caller, name, args).await?,
        _ => return Ok(None),
    };
    Ok(Some(value))
}

async fn publish_work(hub: &Hub, caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let store = hub.store();
    let value = {
        let content = Content::Work {
            objective: string(&args, "objective")?,
            worktree: string(&args, "worktree")?,
            state: serde_json::from_value(args.get("state").cloned().context("state required")?)?,
            resources: serde_json::from_value(
                args.get("resources").cloned().unwrap_or_else(|| json!([])),
            )?,
            dependencies: serde_json::from_value(
                args.get("dependencies")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )?,
            summary: args
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            evidence: serde_json::from_value(
                args.get("evidence").cloned().unwrap_or_else(|| json!([])),
            )?,
        };
        json!({"record":store.coordination_put(caller.clone(),None,content,args.get("revision").and_then(Value::as_u64)).await?})
    };
    Ok(value)
}

async fn get_work(hub: &Hub, caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let store = hub.store();
    let value = {
        let thread = args
            .get("thread")
            .and_then(Value::as_str)
            .map_or_else(|| caller.clone(), ThreadId::from);
        let record = store.coordination_get(format!("work:{thread}")).await?;
        let mut dependencies = Vec::new();
        if let Some(Record {
            content:
                Content::Work {
                    dependencies: targets,
                    ..
                },
            ..
        }) = &record
        {
            for target in targets {
                let work = store.coordination_get(format!("work:{target}")).await?;
                let ready = work.as_ref().is_some_and(|r| {
                    matches!(
                        r.content,
                        Content::Work {
                            state: WorkState::Completed,
                            ..
                        }
                    )
                });
                dependencies.push(json!({"thread":target,"ready":ready,"revision":work.as_ref().map(|record|record.revision),"state":work.as_ref().and_then(|record|match record.content { Content::Work { state,.. }=>Some(state),Content::Claim { .. } | Content::Message { .. } | Content::Watch { .. }=>None })}));
            }
        }
        json!({"thread":thread,"published":record.is_some(),"record":record,"dependencies":dependencies})
    };
    Ok(value)
}

async fn claim_resources(hub: &Hub, caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let store = hub.store();
    let value = {
        let seconds = args
            .get("lifetime_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(3600);
        ensure!(
            (1..=86400).contains(&seconds),
            "claim lifetime must be 1..86400 seconds"
        );
        let resources: Vec<Resource> = serde_json::from_value(
            args.get("resources")
                .cloned()
                .context("resources required")?,
        )?;
        let record = store
            .coordination_put(
                caller.clone(),
                None,
                Content::Claim {
                    resources,
                    purpose: string(&args, "purpose")?,
                    state: ClaimState::Active,
                    expires_at: now() + i64::try_from(seconds)? * 1000,
                },
                None,
            )
            .await?;
        let Content::Claim { resources, .. } = &record.content else {
            anyhow::bail!("store returned a non-claim record");
        };
        let mut conflicts:Vec<_>=all_records(hub,Query { kind:Some("claim".to_owned()),..Default::default() }).await?.into_iter().filter(|other| other.owner!=*caller && other.active_claim(now()) && matches!(&other.content,Content::Claim { resources:others,.. } if others.iter().any(|a|resources.iter().any(|b|a.overlaps(b))))).collect();
        let overlaps_truncated = conflicts.len() > 50;
        conflicts.truncate(50);
        json!({"advisory":true,"record":record,"overlaps_truncated":overlaps_truncated,"overlaps":conflicts,"next_action":"coordinate overlapping work; claims never block filesystem writes"})
    };
    Ok(value)
}

async fn list_records(hub: &Hub, caller: &ThreadId, name: &str, args: Value) -> Result<Value> {
    let store = hub.store();
    let value = {
        let incoming = name == "list_coordination_messages"
            && args
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or("incoming")
                == "incoming";
        let project = match name {
            "list_coordination_messages" => None,
            _ => {
                awareness::project_scope(
                    hub,
                    caller,
                    args.get("scope").and_then(Value::as_str),
                    args.get("project")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                )
                .await?
            }
        };
        let query = Query {
            project: project.clone(),
            kind: Some(
                match name {
                    "list_resource_claims" => "claim",
                    _ => "message",
                }
                .to_owned(),
            ),
            owner: match name {
                "list_coordination_messages" if !incoming => Some(caller.clone()),
                _ => args
                    .get("owner")
                    .and_then(Value::as_str)
                    .map(ThreadId::from),
            },
            recipient: incoming.then(|| caller.clone()),
            cursor: args
                .get("cursor")
                .and_then(Value::as_str)
                .map(serde_json::from_str)
                .transpose()?,
            limit: args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(30)
                .clamp(1, 200) as u32,
        };
        let resources: Vec<Resource> =
            serde_json::from_value(args.get("resources").cloned().unwrap_or_else(|| json!([])))?;
        let resources = resources
            .into_iter()
            .map(Resource::normalize)
            .collect::<Result<Vec<_>>>()?;
        if resources.is_empty() {
            let page = store.coordination_list(query).await?;
            json!({"advisory":true,"scope":{"project":project,"host":"current_forge"},"records":page.records.into_iter().map(|record|json!({"active":record.active_claim(now()),"record":record})).collect::<Vec<_>>(),"next_cursor":page.next_cursor,"event_cursor":page.event_cursor})
        } else {
            let limit = query.limit as usize;
            let event_cursor = store
                .coordination_list(Query {
                    limit: 1,
                    ..query.clone()
                })
                .await?
                .event_cursor;
            let mut records=all_records(hub,query).await?.into_iter().filter(|r|matches!(&r.content,Content::Claim { resources:others,.. } if others.iter().any(|a|resources.iter().any(|b|a.overlaps(b))))).collect::<Vec<_>>();
            let has_more = records.len() > limit;
            records.truncate(limit);
            let next_cursor = if has_more {
                records
                    .last()
                    .map(|record| {
                        serde_json::to_string(&forge_store::coordination::Cursor {
                            updated_at: record.updated_at,
                            id: record.id.clone(),
                        })
                    })
                    .transpose()?
            } else {
                None
            };
            json!({"advisory":true,"scope":{"project":project,"host":"current_forge"},"event_cursor":event_cursor,"records":records.into_iter().map(|record|json!({"active":record.active_claim(now()),"record":record})).collect::<Vec<_>>(),"next_cursor":next_cursor})
        }
    };
    Ok(value)
}

async fn get_record(hub: &Hub, _caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let store = hub.store();
    let value = { json!({"record":store.coordination_get(string(&args,"id")?).await?}) };
    Ok(value)
}

async fn transition(hub: &Hub, caller: &ThreadId, name: &str, args: Value) -> Result<Value> {
    let store = hub.store();
    let value = {
        let update = match name {
            "release_resources" => Update::ReleaseClaim,
            "cancel_work_watch" => Update::FinishWatch {
                state: WatchState::Cancelled,
                outcome: "cancelled by subscriber".to_owned(),
            },
            _ => Update::RenewClaim {
                seconds: u32::try_from(
                    args.get("lifetime_seconds")
                        .and_then(Value::as_u64)
                        .context("lifetime_seconds required")?,
                )?,
            },
        };
        json!({"record":store.coordination_update(caller.clone(),string(&args,"id")?,revision(&args)?,update).await?})
    };
    Ok(value)
}

async fn request_handoff(hub: &Hub, caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let store = hub.store();
    let value = {
        let claim = store
            .coordination_get(string(&args, "claim")?)
            .await?
            .context("claim not found")?;
        ensure!(
            matches!(claim.content, Content::Claim { .. }),
            "record is not a claim"
        );
        json!({"record":send(hub,caller,claim.owner,Intent::Handoff,format!("Request handoff for advisory claim {} revision {}.\n\n{}",claim.id,claim.revision,string(&args,"text")?)).await?})
    };
    Ok(value)
}

async fn send_message(hub: &Hub, caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let value = {
        let record = match args.get("id").and_then(Value::as_str) {
            Some(id) => {
                hub.deliver_coordination_message(caller, id.to_owned())
                    .await?
            }
            None => {
                send(
                    hub,
                    caller,
                    awareness::resolve_agent(hub, &string(&args, "recipient")?).await?,
                    serde_json::from_value(
                        args.get("intent")
                            .cloned()
                            .unwrap_or_else(|| json!("information")),
                    )?,
                    string(&args, "text")?,
                )
                .await?
            }
        };
        json!({"record":record,"delivery_is_not_agreement":true})
    };
    Ok(value)
}

async fn acknowledge(hub: &Hub, caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let value = {
        json!({"record":hub.acknowledge_coordination(caller,string(&args,"id")?,revision(&args)?,serde_json::from_value(args.get("state").cloned().context("state required")?)?,args.get("reply").and_then(Value::as_str).map(str::to_owned)).await?})
    };
    Ok(value)
}

async fn watch(hub: &Hub, caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let store = hub.store();
    let value = {
        json!({"record":store.coordination_put(caller.clone(),None,Content::Watch { target:ThreadId::from(string(&args,"thread")?),state:WatchState::Pending,after_revision:args.get("after_revision").and_then(Value::as_u64).unwrap_or(0),outcome:None },None).await?})
    };
    Ok(value)
}

async fn wait(hub: &Hub, _caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let store = hub.store();
    let value = {
        let target = ThreadId::from(string(&args, "thread")?);
        let after = args
            .get("after_revision")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let timeout = args
            .get("timeout_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(20)
            .min(30);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
        loop {
            let work = store
                .coordination_get(format!("work:{target}"))
                .await?
                .context("target must publish work first")?;
            let terminal = matches!(&work.content,Content::Work { state,.. } if state.terminal())
                && work.revision > after;
            if terminal || tokio::time::Instant::now() >= deadline {
                break json!({"terminal":terminal,"record":work,"timed_out":!terminal});
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    };
    Ok(value)
}

async fn events(hub: &Hub, caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let store = hub.store();
    let value = {
        let after = args
            .get("after")
            .and_then(Value::as_u64)
            .context("event cursor after is required")?;
        let project = awareness::project_scope(
            hub,
            caller,
            args.get("scope").and_then(Value::as_str),
            args.get("project")
                .and_then(Value::as_str)
                .map(str::to_owned),
        )
        .await?;
        let changes = store
            .coordination_changes(
                after,
                project,
                args.get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(100)
                    .clamp(1, 200) as u32,
            )
            .await?;
        json!({"next_cursor":changes.last().map_or(after,|change|change.seq),"changes":changes})
    };
    Ok(value)
}
