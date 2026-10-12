//! One run: prepare the turn, run the adapter, store what it reports.

use crate::Hub;
use anyhow::{Context, Result};
use artisan_api::{
    ItemId, ItemKind, LiveTarget, Run, RunId, RunStatus, Status, ThreadId, UpdateThread, Usage,
};
use forge_engines::{EngineEvent, Input, LiveKind, Outcome, RunIo, Turn, UserInput};
use forge_store::NewItem;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Runs one engine run for the thread's undelivered messages.
pub(crate) async fn drive(
    hub: &Hub,
    thread_id: &ThreadId,
    run_id: RunId,
    inbox: mpsc::Receiver<Input>,
    cancel: CancellationToken,
) -> Result<()> {
    let store = hub.store();
    let pending = store.undelivered_messages(thread_id).await?;
    if pending.is_empty() {
        return Ok(());
    }
    let thread = store
        .thread(thread_id)
        .await?
        .with_context(|| format!("no thread {thread_id}"))?;
    let engine = thread.config.engine;
    let previous = store.latest_run(thread_id).await?;
    let mut run = Run {
        id: run_id.clone(),
        thread_id: thread_id.clone(),
        engine,
        model: thread.config.model.clone(),
        status: RunStatus::Running,
        usage: Usage::default(),
        session_id: previous
            .filter(|run| run.engine == engine)
            .and_then(|run| run.session_id),
        error: None,
        seq: 0,
        started_at: artisan_api::now(),
        ended_at: None,
    };
    run = store.upsert_run(run).await?;

    // Every pending message goes to the engine as one input.
    let mut input = UserInput::default();
    for item in &pending {
        let next = hub.user_input(item, engine).await;
        if !input.text.is_empty() && !next.text.is_empty() {
            input.text.push_str("\n\n");
        }
        input.text.push_str(&next.text);
        input.images.extend(next.images);
        input.id = next.id;
    }
    if run.session_id.is_none()
        && let Some(parent) = &thread.parent_thread_id
    {
        input.text = format!("{}\n\n{}", handoff_context(hub, parent).await?, input.text);
    }
    for item in pending {
        store
            .upsert_item(new_item(
                item.id,
                thread_id,
                Some(&run_id),
                None,
                item.native_id,
                Status::Done,
                item.kind,
            ))
            .await?;
    }

    let turn = match prepare(hub, &thread, run.session_id.clone(), input).await {
        Ok(turn) => turn,
        Err(error) => {
            finish(
                hub,
                &mut run,
                &mut Items::default(),
                Outcome::failed(format!("{error:#}")),
            )
            .await?;
            return Ok(());
        }
    };
    let (events, mut reports) = mpsc::channel(256);
    let harness = (hub.inner.harness_for)(engine);
    let outcome = harness.run(
        turn,
        RunIo {
            inbox,
            events,
            cancel: cancel.clone(),
        },
    );
    tokio::pin!(outcome);

    let mut items = Items::default();
    let outcome = loop {
        tokio::select! {
            biased;
            Some(event) = reports.recv() => apply(hub, &mut run, &mut items, event).await?,
            outcome = &mut outcome => {
                while let Ok(event) = reports.try_recv() {
                    apply(hub, &mut run, &mut items, event).await?;
                }
                break outcome;
            }
        }
    };
    let outcome = if cancel.is_cancelled() && outcome.status == RunStatus::Done {
        Outcome::cancelled()
    } else {
        outcome
    };
    finish(hub, &mut run, &mut items, outcome).await
}

async fn prepare(
    hub: &Hub,
    thread: &artisan_api::Thread,
    session_id: Option<String>,
    input: UserInput,
) -> Result<Turn> {
    let store = hub.store();
    let engine = thread.config.engine;
    let engines = hub.engines();
    let binary = engines
        .binary(engine)
        .with_context(|| format!("{} is not installed", engine.display_name()))?;
    let project = store
        .project(&thread.project_id)
        .await?
        .with_context(|| format!("no project {}", thread.project_id))?;
    let customisation = hub.customisation().await?;
    let mut launch = {
        let _homes = hub.homes_lock();
        engines.materialize(engine, &customisation)?
    };
    launch.env.splice(0..0, engines.launch_env(engine));
    Ok(Turn {
        thread_id: thread.id.clone(),
        engine,
        binary,
        home: engines.home(engine),
        cwd: PathBuf::from(&project.path),
        config: thread.config.clone(),
        session_id,
        input,
        launch,
    })
}

/// The last messages of the thread being handed off, so the new engine
/// starts with the gist.
async fn handoff_context(hub: &Hub, parent: &ThreadId) -> Result<String> {
    let page = hub
        .store()
        .items_page(
            parent,
            artisan_api::ItemsQuery {
                before: None,
                after: None,
                limit: Some(40),
            },
        )
        .await?;
    let mut context = String::from("Context from the previous conversation:\n");
    for item in &page.items {
        match &item.kind {
            ItemKind::UserMessage { text, .. } => context.push_str(&format!("\nUser: {text}\n")),
            ItemKind::AssistantMessage {
                text,
                phase: artisan_api::Phase::Final,
            } => {
                context.push_str(&format!("\nAssistant: {text}\n"));
            }
            _ => {}
        }
    }
    Ok(context)
}

/// Items of this run by adapter key.
#[derive(Default)]
struct Items {
    by_key: HashMap<String, Tracked>,
}

struct Tracked {
    id: ItemId,
    parent: Option<ItemId>,
    status: Status,
    kind: Option<ItemKind>,
}

impl Items {
    fn id(&mut self, key: &str) -> ItemId {
        self.by_key
            .entry(key.to_owned())
            .or_insert_with(|| Tracked {
                id: ItemId::new(),
                parent: None,
                status: Status::Running,
                kind: None,
            })
            .id
            .clone()
    }
}

/// The item an engine names by its key, as a parent or as an agent it
/// reports on. Usually one this run made; a subagent resumed in a later
/// process (Claude's `SendMessage`) still names its first run's agent, which
/// is found by its native id and reopened here.
/// Minting an id for it instead orphaned the resumed agent's whole work.
async fn parent_of(
    store: &forge_store::Store,
    run: &Run,
    items: &mut Items,
    key: &str,
) -> Result<ItemId> {
    if let Some(tracked) = items.by_key.get(key) {
        return Ok(tracked.id.clone());
    }
    let Some(found) = store.item_by_native(&run.thread_id, None, key).await? else {
        return Ok(items.id(key));
    };
    let item = store
        .reopen_agent(&found.id, &run.id)
        .await?
        .unwrap_or(found);
    items.by_key.insert(
        key.to_owned(),
        Tracked {
            id: item.id.clone(),
            parent: item.parent_id.clone(),
            status: item.status,
            kind: Some(item.kind),
        },
    );
    Ok(item.id)
}

async fn apply(hub: &Hub, run: &mut Run, items: &mut Items, event: EngineEvent) -> Result<()> {
    let store = hub.store();
    let thread_id = run.thread_id.clone();
    match event {
        EngineEvent::Session { id } => {
            if run.session_id.as_deref() != Some(&id) {
                run.session_id = Some(id);
                *run = store.upsert_run(run.clone()).await?;
            }
        }
        EngineEvent::Model { id } => {
            if run.model.as_deref() != Some(&id) {
                run.model = Some(id);
                *run = store.upsert_run(run.clone()).await?;
            }
        }
        EngineEvent::Title { title } => {
            let title = title.trim().to_owned();
            if !title.is_empty() {
                store
                    .update_thread(
                        &thread_id,
                        UpdateThread {
                            title: Some(title),
                            ..UpdateThread::default()
                        },
                    )
                    .await?;
            }
        }
        EngineEvent::Item {
            key,
            parent,
            status,
            kind,
        } => {
            let kind = stored_image(store, kind).await;
            let parent_id = match parent {
                Some(parent) => Some(parent_of(store, run, items, &parent).await?),
                None => None,
            };
            // An agent this run didn't start (resumed from an earlier
            // process) is the one stored under its tool id.
            let id = if matches!(kind, ItemKind::Agent { .. }) {
                parent_of(store, run, items, &key).await?
            } else {
                items.id(&key)
            };
            let tracked = items.by_key.get_mut(&key).expect("tracked above");
            let first = tracked.kind.is_none();
            tracked.parent = parent_id.clone().or_else(|| tracked.parent.clone());
            tracked.status = tracked.status.advance(status);
            tracked.kind = Some(kind.clone());
            // Reasoning precedes what it decided: a new top-level item
            // (text, a tool) means the thinking it showed is over.
            if first && tracked.parent.is_none() {
                hub.clear_live(
                    &thread_id,
                    LiveTarget::Reasoning {
                        run_id: run.id.clone(),
                    },
                );
            }
            let finished_text =
                tracked.status.is_terminal() && matches!(kind, ItemKind::AssistantMessage { .. });
            let shell_done = tracked.status.is_terminal() && matches!(kind, ItemKind::Tool { .. });
            store
                .upsert_item(new_item(
                    id.clone(),
                    &thread_id,
                    Some(&run.id),
                    tracked.parent.clone(),
                    Some(key),
                    status,
                    kind,
                ))
                .await?;
            if finished_text {
                hub.forget_live(
                    &thread_id,
                    &LiveTarget::Text {
                        item_id: id.clone(),
                    },
                );
            }
            if shell_done {
                hub.forget_live(
                    &thread_id,
                    &LiveTarget::Shell {
                        item_id: id.clone(),
                    },
                );
                hub.forget_live(&thread_id, &LiveTarget::Progress { item_id: id });
            }
        }
        EngineEvent::Status { key, status } => {
            let id = items.id(&key);
            let tracked = items.by_key.get_mut(&key).expect("tracked above");
            tracked.status = tracked.status.advance(status);
            if let Some(kind) = tracked.kind.clone() {
                store
                    .upsert_item(new_item(
                        id,
                        &thread_id,
                        Some(&run.id),
                        tracked.parent.clone(),
                        Some(key),
                        status,
                        kind,
                    ))
                    .await?;
            }
        }
        EngineEvent::Live {
            target,
            text,
            append,
        } => {
            let target = match target {
                LiveKind::Text { key } => LiveTarget::Text {
                    item_id: items.id(&key),
                },
                LiveKind::Reasoning => LiveTarget::Reasoning {
                    run_id: run.id.clone(),
                },
                LiveKind::Shell { key } => LiveTarget::Shell {
                    item_id: items.id(&key),
                },
                LiveKind::Progress { key } => LiveTarget::Progress {
                    item_id: items.id(&key),
                },
                LiveKind::Status => LiveTarget::Status {
                    run_id: run.id.clone(),
                },
            };
            hub.publish_live(&thread_id, target, text, append);
        }
        EngineEvent::Usage(usage) => {
            run.usage = usage;
            *run = store.upsert_run(run.clone()).await?;
        }
        EngineEvent::SteerAccepted { id } => {
            if let Some(item) = store.item(&id).await?
                && item.status == Status::Running
            {
                store
                    .upsert_item(new_item(
                        item.id,
                        &thread_id,
                        Some(&run.id),
                        None,
                        item.native_id,
                        Status::Done,
                        item.kind,
                    ))
                    .await?;
            }
        }
    }
    Ok(())
}

/// Settles the run and anything it left open.
async fn finish(hub: &Hub, run: &mut Run, items: &mut Items, outcome: Outcome) -> Result<()> {
    let store = hub.store();
    let leftover = if outcome.status == RunStatus::Done {
        Status::Done
    } else {
        Status::Cancelled
    };
    for (key, tracked) in &items.by_key {
        if tracked.status.is_terminal() {
            continue;
        }
        if let Some(kind) = tracked.kind.clone() {
            store
                .upsert_item(new_item(
                    tracked.id.clone(),
                    &run.thread_id,
                    Some(&run.id),
                    tracked.parent.clone(),
                    Some(key.clone()),
                    leftover,
                    kind,
                ))
                .await?;
        }
    }
    if let Some(error) = &outcome.error {
        tracing::warn!(thread = %run.thread_id, run = %run.id, "run ended {:?}: {error}", outcome.status);
    }
    run.status = outcome.status;
    run.error = outcome.error;
    run.ended_at = Some(artisan_api::now());
    store
        .upsert_run(run.clone())
        .await
        .with_context(|| format!("settle run {}", run.id))?;
    Ok(())
}

fn new_item(
    id: ItemId,
    thread_id: &ThreadId,
    run_id: Option<&RunId>,
    parent_id: Option<ItemId>,
    native_id: Option<String>,
    status: Status,
    kind: ItemKind,
) -> NewItem {
    NewItem {
        id,
        thread_id: thread_id.clone(),
        run_id: run_id.cloned(),
        parent_id,
        native_id,
        status,
        kind,
    }
}

/// Images the engine saved to disk (Codex image generation puts the path in
/// `caption`) become blobs, so every Editor can show them.
async fn stored_image(store: &forge_store::Store, kind: ItemKind) -> ItemKind {
    let ItemKind::Image {
        blob,
        caption: Some(path),
    } = &kind
    else {
        return kind;
    };
    if !blob.0.is_empty() || !std::path::Path::new(path).is_file() {
        return kind;
    }
    let stored = match tokio::fs::read(path).await {
        Ok(bytes) => store.put_image(bytes).await,
        Err(error) => Err(error.into()),
    };
    match stored {
        Ok(stored) => ItemKind::Image {
            blob: stored.hash,
            caption: None,
        },
        Err(error) => {
            tracing::warn!(path, "couldn't store the generated image: {error:#}");
            kind
        }
    }
}
