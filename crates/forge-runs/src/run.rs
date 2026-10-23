//! One run: prepare the turn, run the adapter, store what it reports.

use crate::Hub;
use anyhow::{Context, Result};
use artisan_api::{
    Answer, Decision, EngineId, ItemId, ItemKind, LiveTarget, ModelSwitch, NamedModel, Permissions,
    Run, RunId, RunStatus, Status, ThreadId, ToolKind, UpdateThread, Usage,
};
use forge_engines::{EngineEvent, Input, LiveKind, Outcome, RunIo, Turn, UserInput};
use forge_store::NewItem;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Runs one engine run for the thread's undelivered messages.
pub async fn drive(
    hub: &Hub,
    thread_id: &ThreadId,
    run_id: RunId,
    inbox: mpsc::Receiver<Input>,
    cancel: CancellationToken,
) -> Result<RunStatus> {
    let store = hub.store();
    let continuation = store.take_continue(thread_id.clone()).await?;
    let observed = store.pending_inputs(thread_id).await?;
    let pending = observed.items;
    // Undelivered messages carry a stopped turn on by themselves.
    let continuation = continuation.filter(|_| pending.is_empty());
    if pending.is_empty() && continuation.is_none() {
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
    // The thread's first run: the engine may name the thread during it.
    let first_run = previous.is_none();
    let carry_on = match continuation {
        Some(continuation) => Some(continue_prompt(hub, continuation, previous.as_ref()).await?),
        None => None,
    };
    // A session made on the other Claude transport cannot carry on: the
    // run starts fresh, from the thread's history. An empty one is a
    // session the engine said is gone.
    let transport = claude_transport(hub, engine).await;
    let session_id = carried_session(previous.as_ref(), engine, transport);
    let mut run = Run {
        config: Some(thread.config.clone()),
        id: run_id.clone(),
        thread_id: thread_id.clone(),
        engine,
        model: thread.config.model.clone(),
        status: RunStatus::Running,
        usage: Usage::default(),
        session_id,
        error: None,
        seq: 0,
        input_cutoff: observed.cutoff,
        started_at: artisan_api::now(),
        ended_at: None,
        continuation,
    };
    run = store.upsert_run(run).await?;

    // A claude.ai run can find its session needs renewing or is signed
    // out; the Editor hears of it when the run ends.
    let claude_ai_home =
        (transport == Some(ClaudeTransport::Chat)).then(|| hub.engines().home(engine));
    let claude_ai_before = claude_ai_home
        .as_deref()
        .map(forge_engines::claudeai::Session::state);
    let mut items = Work::default();
    hub.open_suggestion(thread_id, &run_id);
    let adapter_cancel = cancel.child_token();
    items.binary = hub.engines().lease(engine);
    let mut outcome = match execute(
        hub,
        &thread,
        &mut run,
        Delivery {
            pending: &pending,
            carry_on,
            previous,
        },
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
    let cancel_cutoff = hub.lock().get(thread_id).map_or(0, |live| {
        live.cancel_through
            .load(std::sync::atomic::Ordering::Acquire)
    });
    if cancel.is_cancelled()
        && cancel_cutoff > 0
        && let Err(error) = store
            .cancel_pending_inputs(thread_id.clone(), cancel_cutoff)
            .await
    {
        // Settle once as Failed, and do not retry the inputs whose durable
        // cancellation could not be saved. Newer inputs remain eligible.
        run.input_cutoff = run.input_cutoff.max(cancel_cutoff);
        outcome = Outcome::failed(format!("persist cancellation: {error:#}"));
    }
    // Forge stopping ends a run unfinished, not cancelled by the user, so
    // it can carry on after the restart.
    if hub.inner.shutdown.is_cancelled() && outcome.status == RunStatus::Cancelled {
        outcome.status = RunStatus::Interrupted;
    }
    let status = outcome.status;
    hub.attention_lock()
        .entry(thread_id.clone())
        .or_default()
        .add(&items.weighed);
    // An engine that named nothing (claude.ai never does; Claude Code can
    // miss) leaves the thread to the side query after its next done turn.
    if first_run && !items.titled {
        hub.untitled_lock().insert(thread_id.clone());
    }
    finish(hub, &mut run, &items, outcome).await?;
    if let Some(home) = &claude_ai_home
        && claude_ai_before.as_ref() != Some(&forge_engines::claudeai::Session::state(home))
    {
        hub.engines().changed(engine);
    }
    Ok(status)
}

/// What a run delivers: the thread's undelivered messages, or the prompt
/// that carries a stopped turn on, and the run before it, whose session a
/// fresh one may need the history from.
struct Delivery<'a> {
    pending: &'a [artisan_api::Item],
    carry_on: Option<String>,
    previous: Option<Run>,
}

/// What the engine reads to carry a stopped turn on. The user sees none of
/// it: the turn just goes on. Background work the stop ended is named, so
/// the agent runs it again instead of waiting on results that won't come.
async fn continue_prompt(
    hub: &Hub,
    continuation: artisan_api::Continuation,
    previous: Option<&Run>,
) -> Result<String> {
    let mut text = match continuation {
        artisan_api::Continuation::Requested => "Continue from where you left off.".to_owned(),
        artisan_api::Continuation::Restart => "Artisan restarted and interrupted this turn. \
            Continue from where you left off; check the current state before repeating \
            anything that may already have finished."
            .to_owned(),

        artisan_api::Continuation::Overloaded => "The provider was overloaded and this turn \
            failed. It is available again: continue from where you left off; check the \
            current state before repeating anything that may already have finished."
            .to_owned(),
        artisan_api::Continuation::UsageLimit => "The usage limit stopped this turn and has \
            now reset. Continue from where you left off; check the current state before \
            repeating anything that may already have finished."
            .to_owned(),
    };
    let Some(previous) = previous else {
        return Ok(text);
    };
    let mut ended = Vec::new();
    for item in hub.store().cancelled_items(&previous.id).await? {
        let summary = |content: &artisan_api::TextContent| {
            content
                .inline()
                .map(|text| {
                    text.lines()
                        .next()
                        .unwrap_or_default()
                        .chars()
                        .take(120)
                        .collect::<String>()
                })
                .filter(|text| !text.trim().is_empty())
        };
        match &item.kind {
            ItemKind::Agent {
                engine: None,
                description,
                name,
                ..
            } => {
                let task = summary(description)
                    .or_else(|| name.clone())
                    .unwrap_or_else(|| "a task".to_owned());
                ended.push(format!("- subagent: {task}"));
            }
            ItemKind::Tool {
                tool:
                    ToolKind::Shell {
                        command,
                        background: true,
                        ..
                    },
                ..
            } => {
                let command = summary(command).unwrap_or_else(|| "a command".to_owned());
                ended.push(format!("- background shell: {command}"));
            }
            ItemKind::UserMessage { .. }
            | ItemKind::AssistantMessage { .. }
            | ItemKind::Tool { .. }
            | ItemKind::Agent { .. }
            | ItemKind::Question { .. }
            | ItemKind::Plan { .. }
            | ItemKind::Image { .. }
            | ItemKind::Compaction { .. }
            | ItemKind::Notice { .. }
            | ItemKind::Error { .. } => {}
        }
    }
    if !ended.is_empty() {
        text.push_str(
            "\n\nThe stop ended this background work; its results will not arrive. \
             Start again what is still needed:\n",
        );
        text.push_str(&ended.join("\n"));
    }
    Ok(text)
}

/// Every fallible operation after Run creation returns through drive's finalizer.
#[expect(
    clippy::too_many_lines,
    reason = "one linear turn: prepare, start, pump events, settle"
)]
async fn execute(
    hub: &Hub,
    thread: &artisan_api::Thread,
    run: &mut Run,
    Delivery {
        pending,
        carry_on,
        previous,
    }: Delivery<'_>,
    mut inbox: mpsc::Receiver<Input>,
    cancel: CancellationToken,
    items: &mut Work,
) -> Result<Outcome> {
    let engine = run.engine;
    // A fresh session starts from the thread's history so far.
    let handoff = if run.session_id.is_none() {
        let pending_ids = pending.iter().map(|item| item.id.clone()).collect();
        // The thread moved here from another engine: the divider says so.
        let switch = previous
            .as_ref()
            .filter(|previous| previous.engine != engine)
            .map(|previous| ModelSwitch {
                from: named_model(hub, previous.engine, previous.model.as_deref()),
                to: named_model(hub, engine, run.model.as_deref()),
            });
        let mut compaction = Compaction {
            hub,
            run,
            id: items.id(crate::handoff::COMPACTION_KEY),
            work: items,
            cancel: cancel.clone(),
            switch,
        };
        let handing_off =
            crate::handoff::context(hub, thread, previous, &pending_ids, &mut compaction);
        tokio::pin!(handing_off);
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                // Give the adapter a bounded chance to interrupt and discard
                // its temporary fork before dropping the handoff future.
                tokio::time::timeout(Duration::from_secs(20), &mut handing_off).await.ok();
                return Ok(Outcome::cancelled());
            }
            () = hub.inner.shutdown.cancelled() => {
                cancel.cancel();
                tokio::time::timeout(Duration::from_secs(20), &mut handing_off).await.ok();
                return Ok(Outcome::cancelled());
            }
            handoff = &mut handing_off => handoff.context("hand the thread's history to the new session")?,
        }
    } else {
        None
    };
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
        if let Some(prompt) = carry_on {
            input.id = ItemId::new();
            input.text = prompt;
        }
        if let Some(context) = &handoff {
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
    // Preserve the effective launch choices, including the account's Claude
    // transport and any normalized model settings, for later handoff forks.
    run.config = Some(turn.config.clone());
    // A send can land after the live slot is claimed but before pending inputs
    // are read. Filter that same ID from the steer channel exactly once.
    let mut delivered: HashSet<_> = initial_ids.iter().cloned().collect();
    let (forward, adapter_inbox) = mpsc::channel(64);
    let approvals = Arc::new(Mutex::new(Approvals {
        full: thread.config.permissions == Permissions::Full,
        waiting: HashSet::new(),
    }));
    let auto_answers = forward.clone();
    let forwarding = {
        let approvals = Arc::clone(&approvals);
        async move {
            while let Some(input) = inbox.recv().await {
                if let Input::Steer(message) = &input
                    && !delivered.insert(message.id.clone())
                {
                    continue;
                }
                // Full access approves what already waits.
                let waiting = match &input {
                    Input::SetPermissions(permissions) => {
                        approvals_lock(&approvals).set(*permissions)
                    }
                    Input::Steer(_) | Input::Answer { .. } | Input::ClearGoal => Vec::new(),
                };
                if forward.send(input).await.is_err() {
                    break;
                }
                for key in waiting {
                    if forward.send(allow_once(key)).await.is_err() {
                        break;
                    }
                }
            }
        }
    };
    tokio::pin!(forwarding);
    let (events, mut reports) = mpsc::channel(256);
    let harness = (hub.inner.harness_for)(engine);
    let mut disk = crate::disk::Watch::new(
        hub.inner.disk.clone(),
        vec![turn.cwd.clone(), hub.data_dir().to_path_buf()],
    );
    // Everything the adapter logs names the thread and run it belongs to.
    let outcome = tracing::Instrument::instrument(
        harness.run(
            turn,
            RunIo {
                inbox: adapter_inbox,
                events,
                cancel: cancel.clone(),
            },
        ),
        tracing::warn_span!("run", thread = %run.thread_id, run = %run.id, %engine),
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
                if let Some(key) = approvals_lock(&approvals).observe(&event)
                    && auto_answers.try_send(allow_once(key.clone())).is_err()
                {
                    tracing::warn!(thread = %run.thread_id, run = %run.id, key, "couldn't approve under Full access; it waits for the user");
                    approvals_lock(&approvals).waiting.insert(key);
                }
                if let Err(error) = apply(hub, run, items, event).await {
                    cancel.cancel();
                    break Outcome::failed(format!("persist engine event: {error:#}"));
                }
            },
            () = &mut forwarding, if !forwarding_done => { forwarding_done = true; },
            outcome = &mut outcome => {
                break outcome;
            }
            () = disk.due() => {
                if let Some(stopped) = guard_disk(hub, run, &mut disk).await {
                    break stopped;
                }
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
    if let Some(config) = &mut run.config {
        config.permissions = if approvals_lock(&approvals).full {
            Permissions::Full
        } else {
            Permissions::Auto
        };
    }
    Ok(outcome)
}

/// Tool approvals of a live run: under Full access Forge gives them
/// itself, so a switch to Full mid-run stops the run asking.
struct Approvals {
    full: bool,
    /// Approvals the user hasn't given yet, by adapter key.
    waiting: HashSet<String>,
}

impl Approvals {
    /// Notes an access change: the keys to approve now.
    fn set(&mut self, permissions: Permissions) -> Vec<String> {
        self.full = permissions == Permissions::Full;
        if self.full {
            self.waiting.drain().collect()
        } else {
            Vec::new()
        }
    }

    /// Follows the approvals an event asks or settles: the key to approve
    /// at once under Full.
    fn observe(&mut self, event: &EngineEvent) -> Option<String> {
        let (key, asks) = match event {
            EngineEvent::Item {
                key, status, kind, ..
            } => (
                key,
                *status == Status::AwaitingUser
                    && matches!(kind, ItemKind::Tool { approval: Some(approval), .. }
                        if approval.decision.is_none()
                            && approval.options.contains(&Decision::AllowOnce)),
            ),
            // A bare status names no approval; only one that settles counts.
            EngineEvent::Status { key, status } if *status != Status::AwaitingUser => (key, false),
            EngineEvent::Status { .. }
            | EngineEvent::Session { .. }
            | EngineEvent::Model { .. }
            | EngineEvent::Title { .. }
            | EngineEvent::Image { .. }
            | EngineEvent::Live { .. }
            | EngineEvent::Usage(..)
            | EngineEvent::Commands(..)
            | EngineEvent::InputAccepted { .. } => return None,
        };
        if !asks {
            self.waiting.remove(key);
            return None;
        }
        if self.full {
            self.waiting.remove(key);
            return Some(key.clone());
        }
        self.waiting.insert(key.clone());
        None
    }
}

fn approvals_lock(approvals: &Mutex<Approvals>) -> std::sync::MutexGuard<'_, Approvals> {
    approvals
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

const fn allow_once(key: String) -> Input {
    Input::Answer {
        key,
        answer: Answer::Approval {
            decision: Decision::AllowOnce,
        },
    }
}

/// Checks the run's disks: the outcome that stops it on a full one.
async fn guard_disk(hub: &Hub, run: &Run, disk: &mut crate::disk::Watch) -> Option<Outcome> {
    match disk.check().await? {
        crate::disk::Alarm::Full { path, free } => {
            Some(Outcome::failed(crate::disk::stopped(&path, free)))
        }
        crate::disk::Alarm::Low { path, free } => {
            if warn_disk_low(hub, run, &path, free).await {
                disk.warned();
            }
            None
        }
    }
}

/// Tells the agent to free space, and the user that it was told. False
/// if the steer could not go now; the next check tries again.
async fn warn_disk_low(hub: &Hub, run: &Run, path: &std::path::Path, free: u64) -> bool {
    let Some(inbox) = hub
        .lock()
        .get(&run.thread_id)
        .map(|live| live.inbox.clone())
    else {
        return false;
    };
    // Never awaited: the loop that drains the inbox is the one running this.
    let steer = Input::Steer(UserInput {
        id: ItemId::new(),
        text: crate::disk::steer(path, free),
        images: Vec::new(),
    });
    if inbox.try_send(steer).is_err() {
        return false;
    }
    let notice = new_item(
        ItemId::new(),
        &run.thread_id,
        Some(&run.id),
        None,
        None,
        Status::Done,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::Info,
            text: crate::disk::notice(path, free).into(),
        },
    );
    if let Err(error) = hub.store().upsert_item(notice).await {
        tracing::warn!(
            thread = %run.thread_id,
            run = %run.id,
            "store the disk-low notice: {error:#}"
        );
    }
    true
}

pub async fn prepare(
    hub: &Hub,
    thread: &artisan_api::Thread,
    session_id: Option<String>,
    input: UserInput,
    binary: Option<PathBuf>,
) -> Result<Turn> {
    let engine = thread.config.engine;
    let engines = hub.engines();
    let binary = binary.with_context(|| format!("{} is not installed", engine.display_name()))?;
    let cwd = hub.thread_folder(thread).await?;
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
    for (name, url) in hub.gateway_servers(&thread.id).await {
        forge_engines::with_gateway_mcp(engine, &mut launch, &name, &url);
    }
    // A config without a variant (or one the model dropped) runs the
    // model's default.
    let mut config = thread.config.clone();
    // The transport is the account's choice, not the thread's: every
    // Claude thread on this Forge follows the same one.
    match claude_transport(hub, config.engine)
        .await
        .and_then(ClaudeTransport::option)
    {
        Some(option) => {
            config.options.insert(
                "transport".to_owned(),
                serde_json::Value::String(option.to_owned()),
            );
        }
        None => {
            config.options.remove("transport");
        }
    }
    if let Some(id) = config.model.as_deref() {
        let status = engines.status(engine);
        if let Some(model) = status.models.iter().find(|model| model.id == id) {
            config.variant = model.variant_for(config.variant.as_deref());
        }
    }
    Ok(Turn {
        thread_id: thread.id.clone(),
        engine,
        binary,
        home: engines.home(engine),
        cwd,
        config,
        session_id,
        input,
        launch,
        trace_dir: None,
    })
}

/// The session a run carries on: the previous run's, when it was on the
/// same engine and the same Claude transport. An empty one is a session
/// the engine said is gone.
fn carried_session(
    previous: Option<&Run>,
    engine: artisan_api::EngineId,
    transport: Option<ClaudeTransport>,
) -> Option<String> {
    previous
        .filter(|run| run.engine == engine)
        .and_then(|run| run.session_id.clone())
        .filter(|session| !session.is_empty())
        .filter(|session| {
            transport.is_none_or(|transport| {
                artisan_api::transport_of_session(Some(session)) == Some(transport.speaks())
            })
        })
}

/// How a Claude turn runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClaudeTransport {
    /// Claude Code, the official way.
    Code,
    /// claude.ai's chat backend: Native Claude Mode.
    Chat,
    /// Native Claude Mode is on with no session: Claude Code, and the
    /// turn says why.
    ChatSignedOut,
}

impl ClaudeTransport {
    /// The transport a session from this one was made on.
    const fn speaks(self) -> &'static str {
        match self {
            Self::Chat => artisan_api::CLAUDE_AI,
            Self::Code | Self::ChatSignedOut => artisan_api::CLAUDE_CODE,
        }
    }

    /// The `transport` option the Claude engine reads; Claude Code, the
    /// official way, needs none.
    const fn option(self) -> Option<&'static str> {
        match self {
            Self::Code => None,
            Self::Chat => Some(forge_engines::CLAUDE_AI_TRANSPORT),
            Self::ChatSignedOut => Some(forge_engines::CLAUDE_AI_UNAVAILABLE),
        }
    }
}

/// How the engine's turns run: `None` for engines with one way.
async fn claude_transport(hub: &Hub, engine: artisan_api::EngineId) -> Option<ClaudeTransport> {
    if engine != artisan_api::EngineId::Claude {
        return None;
    }
    let setting = hub
        .store()
        .setting(artisan_api::TRANSPORT_KEY)
        .await
        .unwrap_or(None);
    if artisan_api::transport_of(setting.as_ref()) != artisan_api::CLAUDE_AI {
        return Some(ClaudeTransport::Code);
    }
    let home = hub.engines().home(engine);
    Some(if forge_engines::claudeai::Session::path(&home).is_file() {
        ClaudeTransport::Chat
    } else {
        ClaudeTransport::ChatSignedOut
    })
}

/// Work of this run by adapter key.
#[derive(Default)]
struct Work {
    binary: Option<forge_engines::BinaryLease>,
    by_key: HashMap<String, Tracked>,
    initial: Option<(ItemId, Vec<ItemId>)>,
    /// What the run's finished items weigh toward a "You should know" check.
    weighed: crate::side::Work,
    /// Keys of calls Forge shows another way (see [`spawns_agent`]).
    hidden: HashSet<String>,
    /// The engine reported an error of its own.
    errored: bool,
    /// The engine named the thread during this run.
    titled: bool,
}

/// An Artisan MCP `agent_spawn` call: the `Agent` item Forge stores stands
/// for it.
fn spawns_agent(kind: &ItemKind) -> bool {
    matches!(kind, ItemKind::Tool { tool: ToolKind::Mcp { server, tool, .. }, .. }
        if server == "artisan" && tool == "agent_spawn")
}

/// What an Artisan call's row names: the thread it reads or writes, or the
/// agents it waits on or messages, by title; `None` when it names neither.
async fn artisan_title(store: &forge_store::Store, input: &str) -> Result<Option<String>> {
    let Ok(serde_json::Value::Object(input)) = serde_json::from_str(input) else {
        return Ok(None);
    };
    if let Some(thread) = input.get("thread").and_then(serde_json::Value::as_str) {
        return Ok(store.thread(&ThreadId::from(thread)).await?.map(|thread| {
            thread
                .title
                .unwrap_or_else(|| "an untitled thread".to_owned())
        }));
    }
    let agents: Vec<&str> = match (input.get("agent"), input.get("agents")) {
        (Some(serde_json::Value::String(agent)), _) => vec![agent.as_str()],
        (_, Some(serde_json::Value::Array(agents))) => agents
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect(),
        _ => return Ok(None),
    };
    let mut names = Vec::new();
    for agent in agents {
        if let Some(name) = store
            .item(&ItemId::from(agent))
            .await?
            .and_then(|item| item.agent_label())
        {
            names.push(name);
        }
    }
    Ok((!names.is_empty()).then(|| names.join(", ")))
}

struct Tracked {
    id: ItemId,
    parent: Option<ItemId>,
    status: Status,
    persisted: bool,
}

/// The `Compaction` item a handoff shows while the previous session
/// summarizes the thread. Tracked with the run's work, so a run stopped
/// mid-summary settles it.
pub struct Compaction<'a> {
    pub(crate) hub: &'a Hub,
    pub(crate) run: &'a Run,
    pub(crate) id: ItemId,
    work: &'a mut Work,
    cancel: CancellationToken,
    pub(crate) switch: Option<ModelSwitch>,
}

/// A model as its engine names it: the catalogue's name, else its id, else
/// the engine's own name when the run took the engine's default.
fn named_model(hub: &Hub, engine: EngineId, model: Option<&str>) -> NamedModel {
    let status = hub.engines().status(engine);
    let name = model.or(status.default_model.as_deref()).map_or_else(
        || engine.display_name().to_owned(),
        |id| {
            status
                .models
                .iter()
                .find(|model| model.id == id)
                .map_or_else(|| id.to_owned(), |model| model.name.clone())
        },
    );
    NamedModel { engine, name }
}

impl Compaction<'_> {
    pub(crate) fn cancel(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub(crate) fn persisted(&mut self) {
        if let Some(tracked) = self.work.by_key.get_mut(crate::handoff::COMPACTION_KEY) {
            tracked.persisted = true;
        }
    }

    pub(crate) fn settled(&mut self) {
        if let Some(tracked) = self.work.by_key.get_mut(crate::handoff::COMPACTION_KEY) {
            tracked.status = Status::Done;
        }
    }
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

#[expect(
    clippy::too_many_lines,
    reason = "one arm per engine event; splitting would scatter the dispatch"
)]
async fn apply(hub: &Hub, run: &mut Run, items: &mut Work, event: EngineEvent) -> Result<()> {
    let store = hub.store();
    let thread_id = run.thread_id.clone();
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
                items.titled = true;
            }
            if !title.is_empty() && hub.enabled(crate::SUMMARY_TITLES_KEY).await {
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
            mut kind,
        } => {
            if matches!(kind, ItemKind::Error { .. }) {
                items.errored = true;
            }
            // Forge stores the agent an `agent_spawn` call starts; the call
            // itself would only repeat it.
            if items.hidden.contains(&key) || spawns_agent(&kind) {
                items.hidden.insert(key);
                return Ok(());
            }
            // A message to an agent names the agent's item: the engine
            // knows it only by its own address (Claude's `agentId`).
            if let ItemKind::Tool {
                tool:
                    ToolKind::AgentMessage {
                        recipient,
                        target: target @ None,
                        ..
                    },
                ..
            } = &mut kind
            {
                *target = store.agent_by_address(&thread_id, recipient).await?;
            }
            if let ItemKind::Tool {
                tool:
                    ToolKind::Agents {
                        addresses, targets, ..
                    },
                ..
            } = &mut kind
                && targets.is_empty()
            {
                for address in addresses.iter() {
                    if let Some(agent) = store.agent_by_address(&thread_id, address).await? {
                        targets.push(agent);
                    }
                }
            }
            // An Artisan call that names a thread or agents by id shows
            // their titles instead.
            if let ItemKind::Tool {
                tool: ToolKind::Mcp { server, input, .. },
                title: title @ None,
                ..
            } = &mut kind
                && server == forge_engines::ARTISAN_MCP
                && let Some(input) = input.as_ref().and_then(|input| input.inline())
            {
                *title = artisan_title(store, input).await?;
            }
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
            let tracked = items
                .by_key
                .get_mut(&key)
                .with_context(|| format!("item {id} is not tracked"))?;
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
            if status.is_terminal() && !tracked.status.is_terminal() {
                items.weighed.finished(&kind, status);
            }
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
            let asked =
                (tracked.status == Status::AwaitingUser) != (saved.status == Status::AwaitingUser);
            tracked.status = saved.status;
            if asked {
                hub.agent_attention(&thread_id).await;
            }
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
            if items.hidden.contains(&key) {
                return Ok(());
            }
            let id = items.id(&key);
            let tracked = items
                .by_key
                .get_mut(&key)
                .with_context(|| format!("item {id} is not tracked"))?;
            if tracked.persisted
                && let Some(item) = store.set_item_status(id, status).await?
            {
                let asked = (tracked.status == Status::AwaitingUser)
                    != (item.status == Status::AwaitingUser);
                tracked.status = item.status;
                if asked {
                    hub.agent_attention(&thread_id).await;
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
                // The provider's recap is the run's reasoning line, and it
                // is already a line: it is not trimmed the way the model's
                // own prose is.
                LiveKind::Reasoning | LiveKind::Narration => LiveTarget::Reasoning {
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
        EngineEvent::Commands(commands) => hub.engines().set_commands(run.engine, commands),
        EngineEvent::Usage(usage) => {
            run.usage = usage;
            *run = store.upsert_run(run.clone()).await?;
        }
        EngineEvent::InputAccepted { id } => {
            let ids = items
                .initial
                .take_if(|(ack, _)| *ack == id)
                .map_or_else(|| vec![id], |(_, ids)| ids);
            store.accept_inputs(thread_id, run.id.clone(), ids).await?;
        }
        EngineEvent::Image { .. } => unreachable!("normalized before dispatch"),
    }
    Ok(())
}

/// Settles the run and anything it left open.
async fn finish(hub: &Hub, run: &mut Run, items: &Work, outcome: Outcome) -> Result<()> {
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
    // A run that failed without the engine saying why (it never started,
    // or Forge stopped it) shows the reason in the transcript; otherwise
    // the user's message just hangs there unanswered.
    if outcome.status == RunStatus::Failed
        && !items.errored
        && let Some(error) = &outcome.error
    {
        store
            .upsert_item(new_item(
                ItemId::new(),
                &run.thread_id,
                Some(&run.id),
                None,
                None,
                Status::Done,
                ItemKind::Error {
                    kind: artisan_api::ErrorKind::Execution,
                    message: error.clone().into(),
                    retryable: false,
                },
            ))
            .await
            .with_context(|| format!("store why run {} failed", run.id))?;
    }
    let park = if outcome.status == RunStatus::Failed {
        crate::park::plan(hub, run, &outcome).await
    } else {
        None
    };
    run.status = outcome.status;
    run.error = outcome.error;
    run.ended_at = Some(artisan_api::now());
    let (_, parked) = store
        .settle_run(run.clone(), open, park)
        .await
        .with_context(|| format!("settle run {}", run.id))?;
    if let Some(park) = park.filter(|_| parked) {
        tracing::info!(thread = %run.thread_id, run = %run.id, continuation = ?park.continuation,
            due_at = park.due_at, "parked a failed turn");
    }
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
