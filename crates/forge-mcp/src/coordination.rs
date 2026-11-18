use crate::awareness::{AwarenessError, nullable, object, optional_record_schema, record_schema};
use crate::forge::{self, ForgeError};
use crate::server::Failure;
use crate::{Tool, awareness};
use artisan_api::{ThreadId, now};
use forge_runs::Hub;
use forge_store::coordination::{
    ClaimState, Content, CoordinationFailure, Intent, MessageState, Query, Record, Resource,
    Update, WatchState, WorkState,
};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ListRecordsError {
    #[error("list a page of coordination records")]
    Forge(#[source] forge_store::coordination::CoordinationListError),
    #[error("read the next page's cursor")]
    Cursor(#[source] serde_json::Error),
}

impl ListRecordsError {
    pub(crate) fn failure(&self) -> Failure<'_> {
        match self {
            Self::Forge(source) => forge::coordination(&source.cause, self),
            Self::Cursor(_) => Failure::Broken,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoordinationError {
    #[error("read {field}")]
    Arguments {
        field: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("{field} is required")]
    Missing { field: &'static str },
    #[error("{step}")]
    Forge {
        step: &'static str,
        #[source]
        source: Box<ForgeError>,
    },
    #[error("read the cursor")]
    Cursor(#[source] serde_json::Error),
    #[error("write the next cursor")]
    NextCursor(#[source] serde_json::Error),
    #[error("list the coordination records")]
    Records(#[source] ListRecordsError),
    #[error("resolve the scope or worker")]
    Discovery(#[source] AwarenessError),
    #[error("claim lifetime must be 1..86400 seconds, not {seconds}")]
    Lifetime { seconds: u64 },
    #[error("claim {claim} not found")]
    ClaimMissing { claim: String },
    #[error("record {record} is not a claim")]
    NotAClaim { record: String },
    #[error("thread {thread} must publish work first")]
    NoWork { thread: ThreadId },
    #[error("normalize a resource")]
    Resource(#[source] CoordinationFailure),
}

impl CoordinationError {
    pub(crate) fn failure(&self) -> Failure<'_> {
        match self {
            Self::Forge { source, .. } => source.failure(),
            Self::Resource(source) => forge::coordination(source, self),
            Self::Records(error) => error.failure(),
            Self::Discovery(error) => error.failure(),
            Self::Arguments { .. }
            | Self::Missing { .. }
            | Self::Cursor(_)
            | Self::NextCursor(_)
            | Self::Lifetime { .. }
            | Self::ClaimMissing { .. }
            | Self::NotAClaim { .. }
            | Self::NoWork { .. } => Failure::Broken,
        }
    }
}

type Result<T, E = CoordinationError> = std::result::Result<T, E>;

fn forge<E: Into<ForgeError>>(step: &'static str) -> impl FnOnce(E) -> CoordinationError {
    move |source| CoordinationError::Forge {
        step,
        source: Box::new(source.into()),
    }
}

fn field<T: serde::de::DeserializeOwned>(
    args: &Value,
    name: &'static str,
    default: Option<Value>,
) -> Result<T> {
    let value = args
        .get(name)
        .cloned()
        .or(default)
        .ok_or(CoordinationError::Missing { field: name })?;
    serde_json::from_value(value).map_err(|source| CoordinationError::Arguments {
        field: name,
        source,
    })
}

fn record_output() -> Value {
    object(&["record"], json!({"record":record_schema()}))
}

fn work_output() -> Value {
    object(
        &["thread", "published", "record", "dependencies"],
        json!({"thread":{"type":"string"},"published":{"type":"boolean"},"record":optional_record_schema(),"dependencies":{"type":"array","items":object(&["thread","ready","revision","state"], json!({"thread":{"type":"string"},"ready":{"type":"boolean"},"revision":nullable("integer"),"state":nullable("string")}))}}),
    )
}

fn claim_output() -> Value {
    object(
        &[
            "advisory",
            "record",
            "overlaps_truncated",
            "overlaps",
            "next_action",
        ],
        json!({"advisory":{"const":true},"record":record_schema(),"overlaps_truncated":{"type":"boolean"},"overlaps":{"type":"array","maxItems":50,"items":record_schema()},"next_action":{"type":"string"}}),
    )
}

fn records_output() -> Value {
    object(
        &[
            "advisory",
            "scope",
            "records",
            "next_cursor",
            "event_cursor",
        ],
        json!({"advisory":{"const":true},"scope":object(&["project","host"], json!({"project":nullable("string"),"host":{"const":"current_forge"}})),"records":{"type":"array","items":object(&["active","record"], json!({"active":{"type":"boolean"},"record":record_schema()}))},"next_cursor":nullable("string"),"event_cursor":{"type":"integer","minimum":0}}),
    )
}

fn found_record_output() -> Value {
    object(&["record"], json!({"record":optional_record_schema()}))
}

fn message_output() -> Value {
    object(
        &["record", "delivery_is_not_agreement"],
        json!({"record":record_schema(),"delivery_is_not_agreement":{"const":true}}),
    )
}

fn wait_output() -> Value {
    object(
        &["terminal", "record", "timed_out"],
        json!({"terminal":{"type":"boolean"},"record":record_schema(),"timed_out":{"type":"boolean"}}),
    )
}

fn events_output() -> Value {
    object(
        &["next_cursor", "changes"],
        json!({"next_cursor":{"type":"integer","minimum":0},"changes":{"type":"array","items":object(&["seq","record_id","owner","project","kind","revision","created_at"], json!({"seq":{"type":"integer","minimum":0},"record_id":{"type":"string"},"owner":{"type":"string"},"project":{"type":"string"},"kind":{"type":"string"},"revision":{"type":"integer"},"created_at":{"type":"integer"}}))}}),
    )
}

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
            record_output,
            false,
        ),
        awareness::tool(
            "get_work",
            "Inspect a worker's published scope, dependency readiness, validation evidence, and current revision. Missing published work is reported explicitly. Defaults to this thread.",
            || json!({"thread":{"type":"string"}}),
            &[],
            work_output,
            true,
        ),
        awareness::tool(
            "claim_resources",
            "Declare advisory ownership of files, directories, builds, deployments, browser sessions, or other resources. Use before overlapping work. Overlaps are returned for negotiation; claims never block edits or reject competing declarations. Default lifetime is one hour.",
            || json!({"resources":resource_schema(),"purpose":{"type":"string","maxLength":4096},"lifetime_seconds":{"type":"integer","minimum":1,"maximum":86400,"default":3600}}),
            &["resources", "purpose"],
            claim_output,
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
            records_output,
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
            record_output,
            false,
        ),
        awareness::tool(
            "release_resources",
            "Release your advisory resource claim after work or an agreed handoff. Only the claimant may release it; other workers negotiate through request_resource_handoff.",
            record_properties,
            &["id", "revision"],
            record_output,
            false,
        ),
        awareness::tool(
            "request_resource_handoff",
            "Ask an advisory claimant to release or sequence work on their resources. Delivers an acknowledged coordination request to its owner; it never transfers or revokes a claim automatically.",
            || json!({"claim":{"type":"string"},"text":{"type":"string","maxLength":16384}}),
            &["claim", "text"],
            record_output,
            false,
        ),
        awareness::tool(
            "send_coordination_message",
            "Send a peer worker a boundary, dependency, handoff, or information request and track its acknowledgment separately from delivery. Starts or steers the recipient's thread. Use id to retry an existing prepared request without duplicating delivery.",
            || json!({"recipient":{"type":"string"},"intent":{"type":"string","enum":["information","boundary","dependency","handoff"]},"text":{"type":"string","maxLength":16384},"id":{"type":"string","description":"Retry this existing request; omit to create one."}}),
            &[],
            message_output,
            false,
        ),
        awareness::tool(
            "list_coordination_messages",
            "Read incoming or sent coordination requests and their delivery, acknowledgment, and resolution states. Defaults to this worker's inbox; reading does not acknowledge an agreement.",
            || json!({"direction":{"type":"string","enum":["incoming","sent"],"default":"incoming"},"cursor":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200}}),
            &[],
            records_output,
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
            record_output,
            false,
        ),
        awareness::tool(
            "get_coordination_record",
            "Read a claim, work declaration, coordination request, or watch by stable ID, including its revision for safe updates.",
            || json!({"id":{"type":"string"}}),
            &["id"],
            found_record_output,
            true,
        ),
        awareness::tool(
            "watch_work",
            "Subscribe this thread to another worker's terminal work result. Completion, failure, or cancellation sends a notification even after this turn ends or Forge restarts. Published work is required; after_revision avoids stale completions.",
            || json!({"thread":{"type":"string"},"after_revision":{"type":"integer","minimum":0}}),
            &["thread"],
            record_output,
            false,
        ),
        awareness::tool(
            "cancel_work_watch",
            "Cancel your pending dependency notification without changing the watched worker's work.",
            record_properties,
            &["id", "revision"],
            record_output,
            false,
        ),
        awareness::tool(
            "wait_for_work",
            "Wait briefly for published work to complete, fail, or be cancelled. Returns current readiness on timeout; use watch_work for a durable background notification while doing independent work.",
            || json!({"thread":{"type":"string"},"after_revision":{"type":"integer","minimum":0},"timeout_seconds":{"type":"integer","minimum":0,"maximum":30,"default":20}}),
            &["thread"],
            wait_output,
            true,
        ),
        awareness::tool(
            "list_coordination_events",
            "Read durable coordination changes after an event cursor. Use event_cursor from list_resource_claims or list_coordination_messages to avoid missing changes between observation and waiting.",
            || json!({"after":{"type":"integer","minimum":0},"scope":{"type":"string","enum":["project","host"]},"project":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200}}),
            &["after"],
            events_output,
            true,
        ),
    ]
}

pub async fn all_records(hub: &Hub, mut query: Query) -> Result<Vec<Record>, ListRecordsError> {
    let mut records = Vec::new();
    loop {
        query.limit = 200;
        let page = hub
            .store()
            .coordination_list(query.clone())
            .await
            .map_err(ListRecordsError::Forge)?;
        records.extend(page.records);
        match page.next_cursor {
            Some(cursor) => {
                query.cursor =
                    Some(serde_json::from_str(&cursor).map_err(ListRecordsError::Cursor)?);
            }
            None => break,
        }
    }
    Ok(records)
}

fn string(args: &Value, key: &'static str) -> Result<String> {
    Ok(args
        .get(key)
        .and_then(Value::as_str)
        .ok_or(CoordinationError::Missing { field: key })?
        .to_owned())
}

fn revision(args: &Value) -> Result<u64> {
    args.get("revision")
        .and_then(Value::as_u64)
        .ok_or(CoordinationError::Missing { field: "revision" })
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
        .await
        .map_err(forge("prepare the coordination message"))?;
    hub.deliver_coordination_message(caller, record.id)
        .await
        .map_err(forge("deliver the coordination message"))
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
            state: field(&args, "state", None)?,
            resources: field(&args, "resources", Some(json!([])))?,
            dependencies: field(&args, "dependencies", Some(json!([])))?,
            summary: args
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            evidence: field(&args, "evidence", Some(json!([])))?,
        };
        json!({"record":store.coordination_put(caller.clone(),None,content,args.get("revision").and_then(Value::as_u64)).await.map_err(forge("publish the work"))?})
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
        let record = store
            .coordination_get(format!("work:{thread}"))
            .await
            .map_err(forge("load the published work"))?;
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
                let work = store
                    .coordination_get(format!("work:{target}"))
                    .await
                    .map_err(forge("load a dependency's published work"))?;
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
        if !(1..=86400).contains(&seconds) {
            return Err(CoordinationError::Lifetime { seconds });
        }
        let resources: Vec<Resource> = field(&args, "resources", None)?;
        let record = store
            .coordination_put(
                caller.clone(),
                None,
                Content::Claim {
                    resources,
                    purpose: string(&args, "purpose")?,
                    state: ClaimState::Active,
                    expires_at: now()
                        + i64::try_from(seconds)
                            .expect("invariant: a claim lifetime is at most 86400 seconds")
                            * 1000,
                },
                None,
            )
            .await
            .map_err(forge("record the claim"))?;
        let Content::Claim { resources, .. } = &record.content else {
            unreachable!(
                "invariant: the store answers a claim with claim {}",
                record.id
            );
        };
        let mut conflicts:Vec<_>=all_records(hub,Query { kind:Some("claim".to_owned()),..Default::default() }).await.map_err(CoordinationError::Records)?.into_iter().filter(|other| other.owner!=*caller && other.active_claim(now()) && matches!(&other.content,Content::Claim { resources:others,.. } if others.iter().any(|a|resources.iter().any(|b|a.overlaps(b))))).collect();
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
            _ => awareness::project_scope(
                hub,
                caller,
                args.get("scope").and_then(Value::as_str),
                args.get("project")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            )
            .await
            .map_err(CoordinationError::Discovery)?,
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
                .transpose()
                .map_err(CoordinationError::Cursor)?,
            limit: args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(30)
                .clamp(1, 200) as u32,
        };
        let resources: Vec<Resource> = field(&args, "resources", Some(json!([])))?;
        let resources = resources
            .into_iter()
            .map(Resource::normalize)
            .collect::<Result<Vec<_>, _>>()
            .map_err(CoordinationError::Resource)?;
        if resources.is_empty() {
            let page = store
                .coordination_list(query)
                .await
                .map_err(forge("list the coordination records"))?;
            json!({"advisory":true,"scope":{"project":project,"host":"current_forge"},"records":page.records.into_iter().map(|record|json!({"active":record.active_claim(now()),"record":record})).collect::<Vec<_>>(),"next_cursor":page.next_cursor,"event_cursor":page.event_cursor})
        } else {
            let limit = query.limit as usize;
            let event_cursor = store
                .coordination_list(Query {
                    limit: 1,
                    ..query.clone()
                })
                .await
                .map_err(forge("read the event cursor"))?
                .event_cursor;
            let mut records=all_records(hub,query).await.map_err(CoordinationError::Records)?.into_iter().filter(|r|matches!(&r.content,Content::Claim { resources:others,.. } if others.iter().any(|a|resources.iter().any(|b|a.overlaps(b))))).collect::<Vec<_>>();
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
                    .transpose()
                    .map_err(CoordinationError::NextCursor)?
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
    let record = store
        .coordination_get(string(&args, "id")?)
        .await
        .map_err(forge("load the coordination record"))?;
    let value = json!({ "record": record });
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
            _ => {
                let seconds = args.get("lifetime_seconds").and_then(Value::as_u64).ok_or(
                    CoordinationError::Missing {
                        field: "lifetime_seconds",
                    },
                )?;
                if !(1..=86400).contains(&seconds) {
                    return Err(CoordinationError::Lifetime { seconds });
                }
                Update::RenewClaim {
                    seconds: u32::try_from(seconds)
                        .expect("invariant: a claim lifetime is at most 86400 seconds"),
                }
            }
        };
        json!({"record":store.coordination_update(caller.clone(),string(&args,"id")?,revision(&args)?,update).await.map_err(forge("update the coordination record"))?})
    };
    Ok(value)
}

async fn request_handoff(hub: &Hub, caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let store = hub.store();
    let value = {
        let id = string(&args, "claim")?;
        let claim = store
            .coordination_get(id.clone())
            .await
            .map_err(forge("load the claim"))?
            .ok_or(CoordinationError::ClaimMissing { claim: id })?;
        if !matches!(claim.content, Content::Claim { .. }) {
            return Err(CoordinationError::NotAClaim { record: claim.id });
        }
        json!({"record":send(hub,caller,claim.owner,Intent::Handoff,format!("Request handoff for advisory claim {} revision {}.\n\n{}",claim.id,claim.revision,string(&args,"text")?)).await?})
    };
    Ok(value)
}

async fn send_message(hub: &Hub, caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let value = {
        let record = match args.get("id").and_then(Value::as_str) {
            Some(id) => hub
                .deliver_coordination_message(caller, id.to_owned())
                .await
                .map_err(forge("deliver the coordination message"))?,
            None => {
                send(
                    hub,
                    caller,
                    awareness::resolve_agent(hub, &string(&args, "recipient")?)
                        .await
                        .map_err(CoordinationError::Discovery)?,
                    field(&args, "intent", Some(json!("information")))?,
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
        json!({"record":hub.acknowledge_coordination(caller,string(&args,"id")?,revision(&args)?,field(&args, "state", None)?,args.get("reply").and_then(Value::as_str).map(str::to_owned)).await.map_err(forge("acknowledge the coordination message"))?})
    };
    Ok(value)
}

async fn watch(hub: &Hub, caller: &ThreadId, _name: &str, args: Value) -> Result<Value> {
    let store = hub.store();
    let value = {
        json!({"record":store.coordination_put(caller.clone(),None,Content::Watch { target:ThreadId::from(string(&args,"thread")?),state:WatchState::Pending,after_revision:args.get("after_revision").and_then(Value::as_u64).unwrap_or(0),outcome:None },None).await.map_err(forge("record the watch"))?})
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
                .await
                .map_err(forge("load the watched work"))?
                .ok_or_else(|| CoordinationError::NoWork {
                    thread: target.clone(),
                })?;
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
            .ok_or(CoordinationError::Missing { field: "after" })?;
        let project = awareness::project_scope(
            hub,
            caller,
            args.get("scope").and_then(Value::as_str),
            args.get("project")
                .and_then(Value::as_str)
                .map(str::to_owned),
        )
        .await
        .map_err(CoordinationError::Discovery)?;
        let changes = store
            .coordination_changes(
                after,
                project,
                args.get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(100)
                    .clamp(1, 200) as u32,
            )
            .await
            .map_err(forge("read the coordination events"))?;
        json!({"next_cursor":changes.last().map_or(after,|change|change.seq),"changes":changes})
    };
    Ok(value)
}
