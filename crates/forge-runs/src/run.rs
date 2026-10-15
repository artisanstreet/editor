//! One run: prepare the turn, run the adapter, store what it reports.

use crate::Hub;
use anyhow::{Context, Result};
use artisan_api::{
    ItemId, ItemKind, LiveTarget, Run, RunId, RunStatus, Status, ThreadId, UpdateThread, Usage,
};
use forge_engines::{EngineEvent, Input, LiveKind, Outcome, RunIo, Turn, UserInput};
use forge_store::NewItem;
use std::collections::{HashMap, HashSet};
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
) -> Result<RunStatus> {
    let store = hub.store();
    let observed = store.pending_inputs(thread_id).await?;
    let pending = observed.items;
    if pending.is_empty() {
        return Ok(RunStatus::Done);
    }
    let thread = store
        .thread(thread_id)
        .await?
        .with_context(|| format!("no thread {thread_id}"))?;
    let engine = thread.config.engine;
    if let Some(live) = hub.lock().get_mut(thread_id) {
        live.engine = Some(engine);
    }
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
        input_cutoff: observed.cutoff,
        started_at: artisan_api::now(),
        ended_at: None,
    };
    run = store.upsert_run(run).await?;

    let mut items = Work::default();
    let adapter_cancel = cancel.child_token();
    items.binary = hub.engines().lease(engine);
    let mut outcome = match execute(
        hub,
        &thread,
        &mut run,
        &pending,
        inbox,
        adapter_cancel.clone(),
        &mut items,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => Outcome::failed(format!("{error:#}")),
    };
    adapter_cancel.cancel();
    let cancel_cutoff = hub
        .lock()
        .get(thread_id)
        .map(|live| {
            live.cancel_through
                .load(std::sync::atomic::Ordering::Acquire)
        })
        .unwrap_or(0);
    if cancel.is_cancelled() && cancel_cutoff > 0 {
        if let Err(error) = store
            .cancel_pending_inputs(thread_id.clone(), cancel_cutoff)
            .await
        {
            // Settle once as Failed, and do not retry the inputs whose durable
            // cancellation could not be saved. Newer inputs remain eligible.
            run.input_cutoff = run.input_cutoff.max(cancel_cutoff);
            outcome = Outcome::failed(format!("persist cancellation: {error:#}"));
        }
    }
    let status = outcome.status;
    finish(hub, &mut run, &mut items, outcome).await?;
    Ok(status)
}

/// Every fallible operation after Run creation returns through drive's finalizer.
async fn execute(
    hub: &Hub,
    thread: &artisan_api::Thread,
    run: &mut Run,
    pending: &[artisan_api::Item],
    mut inbox: mpsc::Receiver<Input>,
    cancel: CancellationToken,
    items: &mut Work,
) -> Result<Outcome> {
    let engine = run.engine;
    let preparing = async {
        // Every pending message goes to the engine as one input.
        let mut input = UserInput::default();
        for item in pending {
            let next = hub.user_input(item, engine).await?;
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
            let context = handoff_context(hub, parent).await?;
            anyhow::ensure!(
                context
                    .len()
                    .saturating_add(input.text.len())
                    .saturating_add(2)
                    <= 32 * 1024 * 1024,
                "fork handoff and pending input exceed 32 MiB admission"
            );
            input.text = format!("{context}\n\n{}", input.text);
        }
        let initial_ids: Vec<_> = pending.iter().map(|item| item.id.clone()).collect();
        let initial_ack = input.id.clone();
        let turn = prepare(
            hub,
            thread,
            run.session_id.clone(),
            input,
            items
                .binary
                .as_ref()
                .map(|lease| lease.path().to_path_buf()),
        )
        .await?;
        Ok::<_, anyhow::Error>((turn, initial_ids, initial_ack))
    };
    let (turn, initial_ids, initial_ack) = tokio::select! {
        biased;
        () = cancel.cancelled() => return Ok(Outcome::cancelled()),
        () = hub.inner.shutdown.cancelled() => return Ok(Outcome::cancelled()),
        prepared = preparing => prepared?,
    };
    // A send can land after the live slot is claimed but before pending inputs
    // are read. Filter that same ID from the steer channel exactly once.
    let mut delivered: HashSet<_> = initial_ids.iter().cloned().collect();
    let (forward, adapter_inbox) = mpsc::channel(64);
    let forwarding = async move {
        while let Some(input) = inbox.recv().await {
            if let Input::Steer(message) = &input
                && !delivered.insert(message.id.clone())
            {
                continue;
            }
            if forward.send(input).await.is_err() {
                break;
            }
        }
    };
    tokio::pin!(forwarding);
    let (events, mut reports) = mpsc::channel(256);
    let harness = (hub.inner.harness_for)(engine);
    let outcome = harness.run(
        turn,
        RunIo {
            inbox: adapter_inbox,
            events,
            cancel: cancel.clone(),
        },
    );
    tokio::pin!(outcome);

    items.initial = Some((initial_ack, initial_ids));
    let mut forwarding_done = false;
    let outcome = loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break Outcome::cancelled(),
            () = hub.inner.shutdown.cancelled() => { cancel.cancel(); break Outcome::cancelled(); },
            Some(event) = reports.recv() => {
                if let Err(error) = apply(hub, run, items, event).await {
                    cancel.cancel();
                    break Outcome::failed(format!("persist engine event: {error:#}"));
                }
            },
            () = &mut forwarding, if !forwarding_done => { forwarding_done = true; },
            outcome = &mut outcome => {
                break outcome;
            }
        }
    };
    cancel.cancel();
    // Preserve acknowledgments already published before shutdown/cancellation.
    // The bounded drain also prevents a faulty detached producer delaying teardown.
    let mut outcome = outcome;
    for _ in 0..256 {
        let Ok(event) = reports.try_recv() else { break };
        if let Err(error) = apply(hub, run, items, event).await {
            outcome = Outcome::failed(format!("persist engine event: {error:#}"));
            break;
        }
    }
    Ok(outcome)
}

async fn prepare(
    hub: &Hub,
    thread: &artisan_api::Thread,
    session_id: Option<String>,
    input: UserInput,
    binary: Option<PathBuf>,
) -> Result<Turn> {
    let store = hub.store();
    let engine = thread.config.engine;
    let engines = hub.engines();
    let binary = binary.with_context(|| format!("{} is not installed", engine.display_name()))?;
    let project = store
        .project(&thread.project_id)
        .await?
        .with_context(|| format!("no project {}", thread.project_id))?;
    let customisation = hub.customisation().await?;
    let owner = hub.clone();
    let mut launch = tokio::task::spawn_blocking(move || {
        let _homes = owner.homes_lock();
        owner.engines().materialize(engine, &customisation)
    })
    .await
    .context("prepare engine home task")??;
    launch
        .env
        .splice(0..0, engines.launch_env_for_binary(engine, &binary));
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
        trace_dir: None,
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
    let prefix = "Context from the previous conversation:\n";
    let messages: Vec<_> = page
        .items
        .iter()
        .filter_map(|item| match &item.kind {
            ItemKind::UserMessage { text, .. } => Some(("User", text)),
            ItemKind::AssistantMessage {
                text,
                phase: artisan_api::Phase::Final,
            } => Some(("Assistant", text)),
            _ => None,
        })
        .collect();
    let bytes = messages
        .iter()
        .fold(prefix.len() as u64, |bytes, (role, text)| {
            bytes
                .saturating_add(text.byte_len())
                .saturating_add(role.len() as u64 + 4)
        });
    anyhow::ensure!(
        bytes <= 32 * 1024 * 1024,
        "fork handoff exceeds 32 MiB admission"
    );
    let mut context = String::with_capacity(usize::try_from(bytes)?);
    context.push_str(prefix);
    for (role, text) in messages {
        let text = hub.store().resolve_text(text).await?;
        context.push('\n');
        context.push_str(role);
        context.push_str(": ");
        context.push_str(&text);
        context.push('\n');
    }
    Ok(context)
}

/// Work of this run by adapter key.
#[derive(Default)]
struct Work {
    binary: Option<forge_engines::BinaryLease>,
    by_key: HashMap<String, Tracked>,
    initial: Option<(ItemId, Vec<ItemId>)>,
}

struct Tracked {
    id: ItemId,
    parent: Option<ItemId>,
    status: Status,
    persisted: bool,
}

impl Work {
    fn id(&mut self, key: &str) -> ItemId {
        self.by_key
            .entry(key.to_owned())
            .or_insert_with(|| Tracked {
                id: ItemId::new(),
                parent: None,
                status: Status::Running,
                persisted: false,
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
    items: &mut Work,
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
            persisted: true,
        },
    );
    Ok(item.id)
}

async fn apply(hub: &Hub, run: &mut Run, items: &mut Work, event: EngineEvent) -> Result<()> {
    let store = hub.store();
    let thread_id = run.thread_id.clone();
    let event = match event {
        EngineEvent::Approval {
            key,
            parent,
            target,
            status,
            approval,
        } => EngineEvent::Item {
            key,
            parent,
            status,
            kind: ItemKind::Approval {
                target: target.map(|key| items.id(&key)),
                approval,
            },
        },
        event => event,
    };
    let event = if let EngineEvent::Image {
        key,
        parent,
        path,
        caption,
    } = event
    {
        use tokio::io::AsyncReadExt as _;
        let file = tokio::fs::File::open(&path)
            .await
            .with_context(|| format!("read generated image {}", path.display()))?;
        let mut bytes = Vec::new();
        file.take((forge_store::MAX_BLOB_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        anyhow::ensure!(
            bytes.len() <= forge_store::MAX_BLOB_BYTES,
            "generated image exceeds encoded size limit"
        );
        let blob = store
            .put_image(bytes)
            .await
            .context("store generated image")?;
        EngineEvent::Item {
            key,
            parent,
            status: Status::Done,
            kind: ItemKind::Image {
                blob: blob.hash,
                caption: caption.map(Into::into),
            },
        }
    } else {
        event
    };
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
            let first = !tracked.persisted;
            tracked.parent = parent_id.clone().or_else(|| tracked.parent.clone());
            tracked.persisted = true;
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
                status.is_terminal() && matches!(kind, ItemKind::AssistantMessage { .. });
            let shell_done = status.is_terminal() && matches!(kind, ItemKind::Tool { .. });
            let saved = store
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
            tracked.status = saved.status;
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
            if tracked.persisted {
                if let Some(item) = store.set_item_status(id, status).await? {
                    tracked.status = item.status;
                }
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
        EngineEvent::InputAccepted { id } => {
            let ids = if items.initial.as_ref().is_some_and(|(ack, _)| ack == &id) {
                items.initial.take().expect("matched initial batch").1
            } else {
                vec![id]
            };
            store.accept_inputs(thread_id, run.id.clone(), ids).await?;
        }
        EngineEvent::Image { .. } | EngineEvent::Approval { .. } => {
            unreachable!("normalized before dispatch")
        }
    }
    Ok(())
}

/// Settles the run and anything it left open.
async fn finish(hub: &Hub, run: &mut Run, items: &mut Work, outcome: Outcome) -> Result<()> {
    let store = hub.store();
    let open = items
        .by_key
        .values()
        .filter(|item| item.persisted && !item.status.is_terminal())
        .map(|item| item.id.clone())
        .collect();
    if let Some(error) = &outcome.error {
        tracing::warn!(thread = %run.thread_id, run = %run.id, "run ended {:?}: {error}", outcome.status);
    }
    run.status = outcome.status;
    run.error = outcome.error;
    run.ended_at = Some(artisan_api::now());
    store
        .settle_run(run.clone(), open)
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
