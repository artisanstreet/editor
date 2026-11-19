use crate::Hub;
use artisan_api::{
    Answer, Decision, EngineConfig, EngineId, ItemId, ItemKind, LiveTarget, ModelSwitch,
    NamedModel, Permissions, Run, RunId, RunOutcome, RunState, RunStatus, Status, ThreadId,
    ToolKind, UpdateThread, Usage,
};
use forge_engines::{EngineEvent, Input, LiveKind, Outcome, RunIo, Turn, UserInput};
use forge_store::NewItem;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    #[error("{} is not installed", engine.display_name())]
    NotInstalled { engine: EngineId },
    #[error("prepare the folder of thread {thread}")]
    Folder {
        thread: ThreadId,
        #[source]
        source: Box<crate::scratch::ThreadFolderError>,
    },
    #[error("read what {engine} gets at launch")]
    Customisation {
        engine: EngineId,
        #[source]
        source: Box<crate::extensions::CustomisationError>,
    },
    #[error("prepare {engine}'s home")]
    HomeTask {
        engine: EngineId,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error(transparent)]
    HomesLock(#[from] crate::extensions::HomesLockError),
    #[error("write {engine}'s home")]
    Materialize {
        engine: EngineId,
        #[source]
        source: Box<forge_engines::MaterializeError>,
    },
    #[error(transparent)]
    ReadSetting(#[from] forge_store::ReadSettingError),
    #[error("read the model customizations for {engine}")]
    Customizations {
        engine: EngineId,
        #[source]
        source: artisan_api::ModelCustomizationsError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DriveError {
    #[error(transparent)]
    Continuation(#[from] forge_store::ReadContinueError),
    #[error(transparent)]
    PendingInputs(#[from] forge_store::PendingInputsError),
    #[error(transparent)]
    ReadThread(#[from] forge_store::ReadThreadError),
    #[error("thread {thread} was removed before its run started")]
    ThreadGone { thread: ThreadId },
    #[error(transparent)]
    LatestRun(#[from] forge_store::LatestRunError),
    #[error(transparent)]
    CancelledItems(#[from] forge_store::CancelledItemsError),
    #[error(transparent)]
    Begin(#[from] forge_store::BeginRunError),
    #[error(transparent)]
    Settle(#[from] forge_store::SettleRunError),
    #[error("read the Claude transport for thread {thread}")]
    Transport {
        thread: ThreadId,
        #[source]
        source: forge_store::ReadSettingError,
    },
}

impl DriveError {
    pub(crate) const fn retry_later(&self) -> bool {
        match self {
            Self::Continuation(_)
            | Self::PendingInputs(_)
            | Self::ReadThread(_)
            | Self::Begin(_) => true,
            Self::ThreadGone { .. }
            | Self::LatestRun(_)
            | Self::CancelledItems(_)
            | Self::Settle(_)
            | Self::Transport { .. } => false,
        }
    }
}

enum Ending {
    Finished(Outcome),
    Stopped(Cleanup),
}

enum Cleanup {
    Confirmed,
    Failed(String),
    Unconfirmed,
}

impl Ending {
    fn after_stop(outcome: &Outcome) -> Self {
        Self::Stopped(outcome.failure().map_or(Cleanup::Confirmed, |failure| {
            Cleanup::Failed(failure.message.clone())
        }))
    }

    fn stopped_by_user(self) -> Self {
        match self {
            Self::Finished(Outcome::Failed(failure)) => {
                Self::Stopped(Cleanup::Failed(failure.message))
            }
            kept @ (Self::Finished(Outcome::Done | Outcome::Cancelled | Outcome::Refused)
            | Self::Stopped(_)) => kept,
        }
    }

    fn unconfirmed(self) -> Self {
        match self {
            Self::Stopped(Cleanup::Confirmed | Cleanup::Unconfirmed) => {
                Self::Stopped(Cleanup::Unconfirmed)
            }
            Self::Stopped(Cleanup::Failed(error)) => {
                Self::Stopped(Cleanup::Failed(format!("{error}\n\n{STOP_UNCONFIRMED}")))
            }
            Self::Finished(Outcome::Failed(mut failure)) => {
                failure.message = format!("{}\n\n{STOP_UNCONFIRMED}", failure.message);
                Self::Finished(Outcome::Failed(failure))
            }
            Self::Finished(Outcome::Done | Outcome::Cancelled | Outcome::Refused) => {
                Self::Finished(Outcome::failed(STOP_UNCONFIRMED))
            }
        }
    }

    fn failed_cleanup(self, error: String) -> Self {
        match self {
            Self::Stopped(_) => Self::Stopped(Cleanup::Failed(error)),
            Self::Finished(_) => Self::Finished(Outcome::failed(error)),
        }
    }

    fn error(&self) -> Option<&str> {
        match self {
            Self::Finished(outcome) => outcome.error(),
            Self::Stopped(Cleanup::Confirmed) => None,
            Self::Stopped(Cleanup::Failed(error)) => Some(error),
            Self::Stopped(Cleanup::Unconfirmed) => Some(STOP_UNCONFIRMED),
        }
    }

    fn run_outcome(&self, stopped_by: Option<StoppedBy>, run: &RunId) -> RunOutcome {
        match (self, stopped_by) {
            (Self::Stopped(_), Some(StoppedBy::User)) => RunOutcome::Cancelled,
            (Self::Stopped(_), Some(StoppedBy::Shutdown)) => RunOutcome::Interrupted,
            (Self::Stopped(_), None) => {
                unreachable!("invariant: run {run} stopped without a Stop or a shutdown")
            }
            (Self::Finished(outcome), _) => match outcome.run_outcome() {
                RunOutcome::Cancelled if stopped_by == Some(StoppedBy::Shutdown) => {
                    RunOutcome::Interrupted
                }
                ended @ (RunOutcome::Done
                | RunOutcome::Cancelled
                | RunOutcome::Failed
                | RunOutcome::Interrupted
                | RunOutcome::Refused) => ended,
            },
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StoppedBy {
    User,
    Shutdown,
}

const STOP_UNCONFIRMED: &str = "The engine did not confirm it stopped within 45 seconds; underlying work may still be running.";

#[derive(Debug, thiserror::Error)]
enum ExecuteError {
    #[error("hand the history of thread {thread} to the new session")]
    Handoff {
        thread: ThreadId,
        #[source]
        source: Box<crate::handoff::ContextError>,
    },
    #[error(transparent)]
    Input(#[from] crate::InputError),
    #[error(transparent)]
    Prepare(#[from] PrepareError),
}

#[derive(Debug, thiserror::Error)]
enum ApplyError {
    #[error(transparent)]
    SaveRun(#[from] forge_store::UpsertRunError),
    #[error(transparent)]
    Title(#[from] forge_store::UpdateThreadError),
    #[error(transparent)]
    SaveItem(#[from] forge_store::UpsertItemError),
    #[error(transparent)]
    Status(#[from] forge_store::SetItemStatusError),
    #[error(transparent)]
    Accept(#[from] forge_store::AcceptInputsError),
    #[error(transparent)]
    Address(#[from] forge_store::AgentByAddressError),
    #[error(transparent)]
    ReadThread(#[from] forge_store::ReadThreadError),
    #[error(transparent)]
    ReadItem(#[from] forge_store::ReadItemError),
    #[error(transparent)]
    ByNative(#[from] forge_store::ItemByNativeError),
    #[error(transparent)]
    Reopen(#[from] forge_store::ReopenAgentError),
    #[error("read generated image {path}")]
    ReadImage {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("generated image {path} is over the {limit} byte limit")]
    ImageTooLarge { path: PathBuf, limit: usize },
    #[error(transparent)]
    StoreImage(#[from] forge_store::PutImageError),
}

fn carried_usage(carried: Option<&Run>, config: &EngineConfig) -> Usage {
    Usage {
        context_used: carried.and_then(|run| run.usage.context_used),
        context_window: carried
            .filter(|run| run.model == config.model)
            .and_then(|run| run.usage.context_window),
        ..Usage::default()
    }
}

pub(crate) async fn drive(
    hub: &Hub,
    thread_id: &ThreadId,
    run_id: RunId,
    inbox: mpsc::Receiver<Input>,
    cancel: CancellationToken,
) -> Result<RunStatus, DriveError> {
    let store = hub.store();
    let continuation = store.continue_request(thread_id).await?;
    let observed = store.pending_inputs(thread_id).await?;
    let pending = observed.items;
    let continuation = continuation.filter(|_| pending.is_empty());
    if pending.is_empty() && continuation.is_none() {
        return Ok(RunStatus::Done);
    }
    let thread = store
        .thread(thread_id)
        .await?
        .ok_or_else(|| DriveError::ThreadGone {
            thread: thread_id.clone(),
        })?;
    let engine = thread.config.engine;
    if let Some(live) = hub.lock().get_mut(thread_id) {
        live.engine = Some(engine);
    }
    let unstarted = unstarted(
        &thread,
        run_id.clone(),
        observed.cutoff,
        continuation,
        hub.store().now(),
    );
    let Started {
        mut run,
        previous,
        carry_on,
        transport,
        first_run,
    } = match Box::pin(start(hub, &thread, unstarted.clone())).await {
        Ok(started) => started,
        Err(error) => return Box::pin(fail_unstarted(hub, unstarted, &pending, &error)).await,
    };

    let claude_ai_home =
        (transport == Some(ClaudeTransport::Chat)).then(|| hub.engines().home(engine));
    let claude_ai_before = claude_ai_home
        .as_deref()
        .map(forge_engines::claudeai::Session::state);
    let mut items = Work::new(hub, &thread, first_run, &pending);
    hub.open_suggestion(thread_id, &run_id);
    let adapter_cancel = cancel.child_token();
    items.binary = hub.engines().lease(engine);
    let mut ending = match Box::pin(execute(
        hub,
        &thread,
        &mut run,
        Delivery {
            pending: &pending,
            carry_on,
            previous,
            transport,
        },
        inbox,
        adapter_cancel.clone(),
        &mut items,
    ))
    .await
    {
        Ok(ending) => ending,
        Err(error) => Ending::Finished(Outcome::failed(diagnostics::chain(&error))),
    };
    adapter_cancel.cancel();
    let stopped_through = hub.stopped_through(thread_id);
    if let Some(cancel_cutoff) = stopped_through
        && let Err(error) = store
            .cancel_pending_inputs(thread_id.clone(), cancel_cutoff)
            .await
    {
        run.input_cutoff = run.input_cutoff.max(cancel_cutoff);
        ending = ending.failed_cleanup(format!(
            "persist cancellation: {}",
            diagnostics::chain(&error)
        ));
    }
    let stopped_by = match (stopped_through, hub.inner.shutdown.is_cancelled()) {
        (Some(_), _) => Some(StoppedBy::User),
        (None, true) => Some(StoppedBy::Shutdown),
        (None, false) => None,
    };
    if stopped_by == Some(StoppedBy::User) {
        ending = ending.stopped_by_user();
    }
    let ended = ending.run_outcome(stopped_by, &run.id);
    let status = ended.status();
    hub.attention_lock()
        .entry(thread_id.clone())
        .or_default()
        .add(&items.weighed);
    let attempted = pending.iter().map(|item| item.id.clone()).collect();
    finish(hub, &mut run, &items, ending, ended, attempted).await?;
    if let Some(home) = &claude_ai_home
        && claude_ai_before.as_ref() != Some(&forge_engines::claudeai::Session::state(home))
    {
        hub.engines().changed(engine);
    }
    Ok(status)
}

fn unstarted(
    thread: &artisan_api::Thread,
    id: RunId,
    input_cutoff: u64,
    continuation: Option<artisan_api::Continuation>,
    started_at: artisan_api::Timestamp,
) -> Run {
    Run {
        config: Some(thread.config.clone()),
        id,
        thread_id: thread.id.clone(),
        engine: thread.config.engine,
        model: thread.config.model.clone(),
        state: RunState::Running,
        usage: Usage::default(),
        session_id: None,
        seq: 0,
        input_cutoff,
        started_at,
        continuation,
    }
}

struct Started {
    run: Run,
    previous: Option<Run>,
    carry_on: Option<String>,
    transport: Option<ClaudeTransport>,
    first_run: bool,
}

async fn start(
    hub: &Hub,
    thread: &artisan_api::Thread,
    mut run: Run,
) -> Result<Started, DriveError> {
    let store = hub.store();
    let previous = store.latest_run(&thread.id).await?;
    let first_run = previous.is_none();
    let carry_on = match run.continuation {
        Some(continuation) => Some(continue_prompt(hub, continuation, previous.as_ref()).await?),
        None => None,
    };
    let (transport, session_id) =
        launch_choice(hub, &thread.id, run.engine, previous.as_ref()).await?;
    let carried = session_id.as_ref().and(previous.as_ref());
    run.usage = carried_usage(carried, &thread.config);
    run.session_id = session_id;
    let run = store.begin_run(run).await?;
    Ok(Started {
        run,
        previous,
        carry_on,
        transport,
        first_run,
    })
}

async fn fail_unstarted(
    hub: &Hub,
    run: Run,
    pending: &[artisan_api::Item],
    error: &DriveError,
) -> Result<RunStatus, DriveError> {
    tracing::error!(thread = %run.thread_id, run = %run.id, "the run could not start: {}", diagnostics::chain(error));
    let mut run = hub.store().begin_run(run).await?;
    let attempted = pending.iter().map(|item| item.id.clone()).collect();
    finish(
        hub,
        &mut run,
        &Work::default(),
        Ending::Finished(Outcome::failed(diagnostics::chain(error))),
        RunOutcome::Failed,
        attempted,
    )
    .await?;
    Ok(RunStatus::Failed)
}

struct Delivery<'a> {
    pending: &'a [artisan_api::Item],
    carry_on: Option<String>,
    previous: Option<Run>,
    transport: Option<ClaudeTransport>,
}

async fn continue_prompt(
    hub: &Hub,
    continuation: artisan_api::Continuation,
    previous: Option<&Run>,
) -> Result<String, forge_store::CancelledItemsError> {
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

async fn settle_handoff<T, E: std::error::Error>(
    thread: &ThreadId,
    handing_off: &mut std::pin::Pin<&mut impl Future<Output = Result<T, E>>>,
) {
    match tokio::time::timeout(Duration::from_secs(20), handing_off).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            tracing::debug!(%thread, "the stopped handoff ended with: {}", diagnostics::chain(&error));
        }
        Err(_elapsed) => {
            tracing::warn!(%thread, "the stopped handoff did not settle within 20 seconds");
        }
    }
}

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
        transport,
    }: Delivery<'_>,
    mut inbox: mpsc::Receiver<Input>,
    cancel: CancellationToken,
    items: &mut Work,
) -> Result<Ending, ExecuteError> {
    let engine = run.engine;
    let handoff = if run.session_id.is_none() {
        let pending_ids = pending.iter().map(|item| item.id.clone()).collect();
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
                settle_handoff(&run.thread_id, &mut handing_off).await;
                return Ok(Ending::Stopped(Cleanup::Confirmed));
            }
            () = hub.inner.shutdown.cancelled() => {
                cancel.cancel();
                settle_handoff(&run.thread_id, &mut handing_off).await;
                return Ok(Ending::Stopped(Cleanup::Confirmed));
            }
            handoff = &mut handing_off => handoff.map_err(|source| ExecuteError::Handoff { thread: thread.id.clone(), source: Box::new(source) })?,
        }
    } else {
        None
    };
    let preparing = async {
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
        let turn = prepare_on(
            hub,
            thread,
            run.session_id.clone(),
            input,
            items
                .binary
                .as_ref()
                .map(|lease| lease.path().to_path_buf()),
            transport,
        )
        .await?;
        Ok::<_, ExecuteError>((turn, initial_ids, initial_ack))
    };
    let (turn, initial_ids, initial_ack) = tokio::select! {
        biased;
        () = cancel.cancelled() => return Ok(Ending::Stopped(Cleanup::Confirmed)),
        () = hub.inner.shutdown.cancelled() => return Ok(Ending::Stopped(Cleanup::Confirmed)),
        prepared = preparing => prepared?,
    };
    run.config = Some(turn.config.clone());
    if let Some(live) = hub.lock().get_mut(&run.thread_id)
        && live.id == run.id
    {
        live.launched = Some(turn.config.permissions);
    }
    let mut coverage = turn_coverage(hub, run, turn.config.permissions).await;
    if let Some(reason) = coverage.observe(turn.config.permissions) {
        announce_unsandboxed(hub, &run.thread_id, &run.id, reason).await;
    }
    let (covered_thread, covered_run) = (run.thread_id.clone(), run.id.clone());
    let mut delivered: HashSet<_> = initial_ids.iter().cloned().collect();
    let (forward, adapter_inbox) = mpsc::channel(64);
    let approvals = Arc::new(Mutex::new(Approvals {
        full: thread.config.permissions == Permissions::Full,
        waiting: HashSet::new(),
    }));
    let auto_answers = forward.clone();
    let (events, mut reports) = mpsc::channel(256);
    let harness = (hub.inner.harness_for)(engine);
    let mut disk = crate::disk::Watch::new(
        hub.inner.disk.clone(),
        vec![turn.cwd.clone(), hub.data_dir().to_path_buf()],
    );
    let adapter_outcome = tracing::Instrument::instrument(
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
    tokio::pin!(adapter_outcome);

    items.initial = Some((initial_ack, initial_ids));
    let mut forwarding_done = false;
    let mut adapter_finished = false;
    let outcome = {
        let forwarding = async {
            while let Some(input) = inbox.recv().await {
                if let Input::Steer(message) = &input
                    && !delivered.insert(message.id.clone())
                {
                    continue;
                }
                let waiting = match &input {
                    Input::SetPermissions(permissions) => {
                        if let Some(reason) = coverage.observe(*permissions) {
                            announce_unsandboxed(hub, &covered_thread, &covered_run, reason).await;
                        }
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
        };
        tokio::pin!(forwarding);
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break Ending::Stopped(Cleanup::Confirmed),
                () = hub.inner.shutdown.cancelled() => { cancel.cancel(); break Ending::Stopped(Cleanup::Confirmed); },
                Some(event) = reports.recv() => {
                    if let Some(key) = approvals_lock(&approvals).observe(&event)
                        && auto_answers.try_send(allow_once(key.clone())).is_err()
                    {
                        tracing::warn!(thread = %run.thread_id, run = %run.id, key, "couldn't approve under Full access; it waits for the user");
                        approvals_lock(&approvals).waiting.insert(key);
                    }
                    if let Err(error) = apply(hub, run, items, event).await {
                        cancel.cancel();
                        break Ending::Finished(Outcome::failed(format!("persist engine event: {}", diagnostics::chain(&error))));
                    }
                },
                () = &mut forwarding, if !forwarding_done => { forwarding_done = true; },
                outcome = &mut adapter_outcome => {
                    adapter_finished = true;
                    break if cancel.is_cancelled() {
                        Ending::after_stop(&outcome)
                    } else {
                        Ending::Finished(outcome)
                    };
                }
                () = disk.due() => {
                    if let Some(stopped) = guard_disk(hub, run, &mut disk).await {
                        break Ending::Finished(stopped);
                    }
                }
            }
        }
    };
    inbox.close();
    cancel.cancel();
    let mut ending = outcome;
    if !adapter_finished
        && matches!(ending, Ending::Stopped(_))
        && !hub.inner.shutdown.is_cancelled()
    {
        match announce_stop(hub, run, items, pending).await {
            Ok(stopped) => *run = stopped,
            Err(error) => {
                tracing::warn!(thread = %run.thread_id, run = %run.id, "announce the stop before the engine confirms it: {}", diagnostics::chain(&error));
            }
        }
    }
    if !adapter_finished {
        let deadline = tokio::time::sleep(Duration::from_secs(45));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                stopped = &mut adapter_outcome => {
                    if let Some(failure) = stopped.failure() {
                        ending = match ending {
                            Ending::Stopped(Cleanup::Confirmed) => Ending::Stopped(Cleanup::Failed(failure.message.clone())),
                            Ending::Finished(outcome) if outcome.failure().is_none() => Ending::Finished(stopped),
                            kept @ (Ending::Stopped(_) | Ending::Finished(_)) => kept,
                        };
                    }
                    break;
                }
                Some(event) = reports.recv() => {
                    if let Err(error) = apply(hub, run, items, event).await {
                        ending = ending.failed_cleanup(format!(
                            "persist engine event during cancellation: {}",
                            diagnostics::chain(&error)
                        ));
                    }
                }
                () = &mut deadline => {
                    ending = ending.unconfirmed();
                    break;
                }
            }
        }
    }
    for _ in 0..256 {
        let Ok(event) = reports.try_recv() else { break };
        if let Err(error) = apply(hub, run, items, event).await {
            ending = ending.failed_cleanup(format!(
                "persist engine event: {}",
                diagnostics::chain(&error)
            ));
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
    Ok(ending)
}

struct Approvals {
    full: bool,
    waiting: HashSet<String>,
}

impl Approvals {
    fn set(&mut self, permissions: Permissions) -> Vec<String> {
        self.full = permissions == Permissions::Full;
        if self.full {
            self.waiting.drain().collect()
        } else {
            Vec::new()
        }
    }

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
            EngineEvent::Status { key, status } if *status != Status::AwaitingUser => (key, false),
            EngineEvent::Status { .. }
            | EngineEvent::Session { .. }
            | EngineEvent::SessionGone
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

async fn warn_disk_low(hub: &Hub, run: &Run, path: &std::path::Path, free: u64) -> bool {
    let Some(inbox) = hub
        .lock()
        .get(&run.thread_id)
        .map(|live| live.inbox.clone())
    else {
        return false;
    };
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
            "store the disk-low notice: {}",
            diagnostics::chain(&error)
        );
    }
    true
}

async fn turn_coverage(hub: &Hub, run: &Run, launched: Permissions) -> crate::sandbox::Coverage {
    match hub.store().turn_said_unsandboxed(&run.thread_id).await {
        Ok(true) => crate::sandbox::Coverage::Announced,
        Ok(false) => crate::sandbox::Coverage::new(hub.inner.sandbox, launched),
        Err(error) => {
            tracing::warn!(
                thread = %run.thread_id,
                run = %run.id,
                "read whether this turn said it isn't sandboxed, so this run says it: {}",
                diagnostics::chain(&error)
            );
            crate::sandbox::Coverage::new(hub.inner.sandbox, launched)
        }
    }
}

async fn announce_unsandboxed(
    hub: &Hub,
    thread: &ThreadId,
    run: &RunId,
    reason: artisan_api::UnsandboxedReason,
) {
    let notice = new_item(
        ItemId::new(),
        thread,
        Some(run),
        None,
        None,
        Status::Done,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::Unsandboxed { reason },
            text: crate::sandbox::notice(reason).into(),
        },
    );
    if let Err(error) = hub.store().upsert_item(notice).await {
        tracing::warn!(
            %thread,
            %run,
            ?reason,
            "store the notice that the run isn't sandboxed: {}",
            diagnostics::chain(&error)
        );
    }
}

pub(crate) async fn prepare(
    hub: &Hub,
    thread: &artisan_api::Thread,
    session_id: Option<String>,
    input: UserInput,
    binary: Option<PathBuf>,
) -> Result<Turn, PrepareError> {
    let transport = claude_transport(hub, thread.config.engine).await?;
    prepare_on(hub, thread, session_id, input, binary, transport).await
}

async fn prepare_on(
    hub: &Hub,
    thread: &artisan_api::Thread,
    session_id: Option<String>,
    input: UserInput,
    binary: Option<PathBuf>,
    transport: Option<ClaudeTransport>,
) -> Result<Turn, PrepareError> {
    let engine = thread.config.engine;
    let engines = hub.engines();
    let binary = binary.ok_or(PrepareError::NotInstalled { engine })?;
    let cwd = hub
        .thread_folder(thread)
        .await
        .map_err(|source| PrepareError::Folder {
            thread: thread.id.clone(),
            source: Box::new(source),
        })?;
    let mut customisation =
        hub.customisation()
            .await
            .map_err(|source| PrepareError::Customisation {
                engine,
                source: Box::new(source),
            })?;
    customisation.mcp_servers = hub
        .mcp()
        .reachability()
        .answering(std::mem::take(&mut customisation.mcp_servers), &thread.id)
        .await;
    let owner = hub.clone();
    let mut launch = tokio::task::spawn_blocking(move || {
        let _homes = owner.extensions().homes_lock()?;
        owner
            .engines()
            .materialize(engine, &customisation)
            .map_err(|source| PrepareError::Materialize {
                engine,
                source: Box::new(source),
            })
    })
    .await
    .map_err(|source| PrepareError::HomeTask { engine, source })??;
    launch
        .env
        .splice(0..0, engines.launch_env_for_binary(engine, &binary));
    for (name, url) in hub.gateway_servers(&thread.id).await {
        forge_engines::with_gateway_mcp(engine, &mut launch, &name, &url);
    }
    launch.readable.push(hub.store().blob_dir().to_path_buf());
    launch.tool_caches = Some(hub.sandbox_tool_caches(&thread.project_id));
    let mut config = thread.config.clone();
    match transport.and_then(ClaudeTransport::option) {
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
            let customizations = hub
                .store()
                .setting(artisan_api::MODEL_CUSTOMIZATIONS_KEY)
                .await?
                .as_ref()
                .map(artisan_api::model_customizations)
                .transpose()
                .map_err(|source| PrepareError::Customizations { engine, source })?
                .unwrap_or_default();
            let resolution = model.resolve_config(&config, &customizations);
            for change in &resolution.changes {
                tracing::warn!(model = %model.id, requested = %change.requested, reason = %change.reason, "model configuration resolved");
            }
            config = resolution.effective;
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
        trace_dir: Some(forge_engines::trace::folder(hub.data_dir())),
    })
}

async fn launch_choice(
    hub: &Hub,
    thread_id: &ThreadId,
    engine: EngineId,
    previous: Option<&Run>,
) -> Result<(Option<ClaudeTransport>, Option<String>), DriveError> {
    let transport =
        claude_transport(hub, engine)
            .await
            .map_err(|source| DriveError::Transport {
                thread: thread_id.clone(),
                source,
            })?;
    Ok((transport, carried_session(previous, engine, transport)))
}

fn carried_session(
    previous: Option<&Run>,
    engine: EngineId,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClaudeTransport {
    Code,
    Chat,
    ChatSignedOut,
}

impl ClaudeTransport {
    const fn speaks(self) -> &'static str {
        match self {
            Self::Chat => artisan_api::CLAUDE_AI,
            Self::Code | Self::ChatSignedOut => artisan_api::CLAUDE_CODE,
        }
    }

    const fn option(self) -> Option<&'static str> {
        match self {
            Self::Code => None,
            Self::Chat => Some(forge_engines::CLAUDE_AI_TRANSPORT),
            Self::ChatSignedOut => Some(forge_engines::CLAUDE_AI_UNAVAILABLE),
        }
    }
}

async fn claude_transport(
    hub: &Hub,
    engine: EngineId,
) -> Result<Option<ClaudeTransport>, forge_store::ReadSettingError> {
    if engine != EngineId::Claude {
        return Ok(None);
    }
    let setting = hub.store().setting(artisan_api::TRANSPORT_KEY).await?;
    if artisan_api::transport_of(setting.as_ref()) != artisan_api::CLAUDE_AI {
        return Ok(Some(ClaudeTransport::Code));
    }
    let home = hub.engines().home(engine);
    Ok(Some(
        if forge_engines::claudeai::Session::path(&home).is_file() {
            ClaudeTransport::Chat
        } else {
            ClaudeTransport::ChatSignedOut
        },
    ))
}

const RECENT_KEYS: usize = 1024;

struct Recent<V> {
    entries: HashMap<String, V>,
    order: VecDeque<String>,
    forgot: bool,
}

impl<V> Default for Recent<V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            forgot: false,
        }
    }
}

impl<V> Recent<V> {
    fn insert(&mut self, key: String, value: V) {
        if self.entries.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > RECENT_KEYS
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
            self.forgot = true;
        }
    }

    fn take(&mut self, key: &str) -> Option<V> {
        let value = self.entries.remove(key)?;
        self.order.retain(|kept| kept != key);
        Some(value)
    }

    fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }
}

#[derive(Default)]
struct Work {
    binary: Option<forge_engines::BinaryLease>,
    by_key: HashMap<String, Tracked>,
    settled: Recent<Tracked>,
    initial: Option<(ItemId, Vec<ItemId>)>,
    weighed: crate::side::Work,
    hidden: Recent<()>,
    errored: bool,
    early_title: Option<Box<(artisan_api::Thread, Vec<artisan_api::Item>)>>,
}

impl Work {
    fn new(
        hub: &Hub,
        thread: &artisan_api::Thread,
        first: bool,
        messages: &[artisan_api::Item],
    ) -> Self {
        let early_title = first.then(|| {
            hub.untitled_lock().insert(thread.id.clone());
            Box::new((thread.clone(), messages.to_vec()))
        });
        Self {
            early_title,
            ..Self::default()
        }
    }
}

fn spawns_agent(kind: &ItemKind) -> bool {
    matches!(kind, ItemKind::Tool { tool: ToolKind::Mcp { server, tool, .. }, .. }
        if server == "artisan" && tool == "agent_spawn")
}

async fn artisan_title(
    store: &forge_store::Store,
    input: &str,
) -> Result<Option<String>, ApplyError> {
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

enum Tracked {
    Named {
        id: ItemId,
    },
    Stored {
        id: ItemId,
        parent: Option<ItemId>,
        status: Status,
    },
}

impl Tracked {
    const fn id(&self) -> &ItemId {
        match self {
            Self::Named { id } | Self::Stored { id, .. } => id,
        }
    }

    const fn status(&self) -> Option<Status> {
        match self {
            Self::Named { .. } => None,
            Self::Stored { status, .. } => Some(*status),
        }
    }

    fn stored_parent(&self) -> Option<&ItemId> {
        match self {
            Self::Named { .. } => None,
            Self::Stored { parent, .. } => parent.as_ref(),
        }
    }

    fn store(&mut self, parent: Option<ItemId>, status: Status) {
        let id = self.id().clone();
        *self = Self::Stored { id, parent, status };
    }

    fn open(&self) -> Option<&ItemId> {
        match self {
            Self::Stored { id, status, .. } if !status.is_terminal() => Some(id),
            Self::Named { .. } | Self::Stored { .. } => None,
        }
    }
}

pub(crate) struct Compaction<'a> {
    pub(crate) hub: &'a Hub,
    pub(crate) run: &'a Run,
    pub(crate) id: ItemId,
    work: &'a mut Work,
    cancel: CancellationToken,
    pub(crate) switch: Option<ModelSwitch>,
}

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

    pub(crate) fn stored(&mut self, status: Status) {
        if let Some(tracked) = self.work.by_key.get_mut(crate::handoff::COMPACTION_KEY) {
            tracked.store(None, status);
        }
    }
}

impl Work {
    fn settle(&mut self, key: &str) {
        if key == crate::handoff::COMPACTION_KEY {
            return;
        }
        if let Some(tracked) = self.by_key.remove(key) {
            self.settled.insert(key.to_owned(), tracked);
        }
    }

    fn id(&mut self, key: &str) -> ItemId {
        self.by_key
            .entry(key.to_owned())
            .or_insert_with(|| Tracked::Named { id: ItemId::new() })
            .id()
            .clone()
    }
}

async fn identify(
    store: &forge_store::Store,
    run: &Run,
    items: &mut Work,
    key: &str,
) -> Result<ItemId, ApplyError> {
    if let Some(tracked) = items.by_key.get(key) {
        return Ok(tracked.id().clone());
    }
    if let Some(tracked) = items.settled.take(key) {
        let id = tracked.id().clone();
        items.by_key.insert(key.to_owned(), tracked);
        return Ok(id);
    }
    if items.settled.forgot
        && let Some(found) = store
            .item_by_native(&run.thread_id, Some(&run.id), key)
            .await?
    {
        items.by_key.insert(
            key.to_owned(),
            Tracked::Stored {
                id: found.id.clone(),
                parent: found.parent_id,
                status: found.status,
            },
        );
        return Ok(found.id);
    }
    Ok(items.id(key))
}

async fn known_item(
    store: &forge_store::Store,
    run: &Run,
    items: &mut Work,
    key: &str,
) -> Result<Option<ItemId>, ApplyError> {
    if let Some(tracked) = items.by_key.get(key) {
        return Ok(Some(tracked.id().clone()));
    }
    let Some(found) = store.item_by_native(&run.thread_id, None, key).await? else {
        return Ok(None);
    };
    let item = store
        .reopen_agent(&found.id, &run.id)
        .await?
        .unwrap_or(found);
    items.by_key.insert(
        key.to_owned(),
        Tracked::Stored {
            id: item.id.clone(),
            parent: item.parent_id.clone(),
            status: item.status,
        },
    );
    Ok(Some(item.id))
}

async fn stored_parent(
    store: &forge_store::Store,
    run: &Run,
    items: &mut Work,
    key: &str,
) -> Result<Option<ItemId>, ApplyError> {
    match items.by_key.get(key) {
        Some(Tracked::Stored { id, .. }) => Ok(Some(id.clone())),
        Some(Tracked::Named { .. }) => Ok(None),
        None => known_item(store, run, items, key).await,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per engine event; splitting would scatter the dispatch"
)]
async fn apply(
    hub: &Hub,
    run: &mut Run,
    items: &mut Work,
    event: EngineEvent,
) -> Result<(), ApplyError> {
    let store = hub.store();
    let thread_id = run.thread_id.clone();
    match event {
        EngineEvent::Session { id } => {
            if run.session_id.as_deref() != Some(&id) {
                run.session_id = Some(id);
                *run = store.upsert_run(run.clone()).await?;
            }
            if let Some(title) = items.early_title.take()
                && (run.engine == EngineId::Codex
                    || run
                        .session_id
                        .as_deref()
                        .is_some_and(|id| id.starts_with(artisan_api::CLAUDE_AI_SESSION_PREFIX)))
            {
                let (thread, messages) = *title;
                hub.start_title(thread, messages);
            }
        }
        EngineEvent::SessionGone => {
            if run.session_id.is_some() {
                run.session_id = None;
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
                hub.untitled_lock().remove(&thread_id);
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
            kind,
        } => apply_item(hub, run, items, key, parent, status, kind).await?,
        EngineEvent::Image {
            key,
            parent,
            path,
            caption,
        } => {
            let blob = store_image(hub, &path).await?;
            apply_item(
                hub,
                run,
                items,
                key,
                parent,
                Status::Done,
                ItemKind::Image {
                    blob,
                    caption: caption.map(Into::into),
                },
            )
            .await?;
        }
        EngineEvent::Status { key, status } => {
            if items.hidden.contains(&key) {
                return Ok(());
            }
            let id = identify(store, run, items, &key).await?;
            let tracked = items.by_key.get_mut(&key).unwrap_or_else(|| {
                unreachable!(
                    "invariant: item {id} of run {} is tracked once named",
                    run.id
                )
            });
            if let Tracked::Stored {
                status: tracked_status,
                ..
            } = tracked
                && let Some(item) = store.set_item_status(id, status).await?
            {
                let asked = (*tracked_status == Status::AwaitingUser)
                    != (item.status == Status::AwaitingUser);
                *tracked_status = item.status;
                if asked {
                    hub.agent_attention(&thread_id).await;
                }
                if item.status.is_terminal() && !matches!(item.kind, ItemKind::Agent { .. }) {
                    items.settle(&key);
                }
            }
        }
        EngineEvent::Live {
            target,
            text,
            append,
        } => {
            if let LiveKind::Text { key } | LiveKind::Shell { key } | LiveKind::Progress { key } =
                &target
                && !items.by_key.contains_key(key)
                && !hub.live_has_room(&thread_id)
            {
                return Ok(());
            }
            let target = match target {
                LiveKind::Text { key } => LiveTarget::Text {
                    item_id: identify(store, run, items, &key).await?,
                },
                LiveKind::Reasoning | LiveKind::Narration => LiveTarget::Reasoning {
                    run_id: run.id.clone(),
                },
                LiveKind::Shell { key } => LiveTarget::Shell {
                    item_id: identify(store, run, items, &key).await?,
                },
                LiveKind::Progress { key } => LiveTarget::Progress {
                    item_id: identify(store, run, items, &key).await?,
                },
                LiveKind::Status => LiveTarget::Status {
                    run_id: run.id.clone(),
                },
            };
            hub.publish_live(&thread_id, target, text, append);
        }
        EngineEvent::Commands(commands) => hub.engines().set_commands(run.engine, commands),
        EngineEvent::Usage(usage) => {
            run.usage = Usage {
                context_used: usage.context_used.or(run.usage.context_used),
                context_window: usage.context_window.or(run.usage.context_window),
                ..usage
            };
            *run = store.upsert_run(run.clone()).await?;
        }
        EngineEvent::InputAccepted { id } => {
            let (ids, delivered) = items.initial.take_if(|(ack, _)| *ack == id).map_or_else(
                || (vec![id], forge_store::Delivered::Steer),
                |(_, ids)| (ids, forge_store::Delivered::Initial),
            );
            store
                .accept_inputs(thread_id, run.id.clone(), ids, delivered)
                .await?;
        }
    }
    Ok(())
}

async fn store_image(
    hub: &Hub,
    path: &std::path::Path,
) -> Result<artisan_api::BlobHash, ApplyError> {
    use tokio::io::AsyncReadExt as _;
    let read = |source| ApplyError::ReadImage {
        path: path.to_owned(),
        source,
    };
    let file = tokio::fs::File::open(path).await.map_err(read)?;
    let mut bytes = Vec::new();
    file.take((forge_store::MAX_BLOB_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(read)?;
    if bytes.len() > forge_store::MAX_BLOB_BYTES {
        return Err(ApplyError::ImageTooLarge {
            path: path.to_owned(),
            limit: forge_store::MAX_BLOB_BYTES,
        });
    }
    Ok(hub.store().put_image(bytes).await?.hash)
}

#[expect(
    clippy::too_many_lines,
    reason = "one item's normalization, agent naming and storage stay together"
)]
async fn apply_item(
    hub: &Hub,
    run: &Run,
    items: &mut Work,
    key: String,
    parent: Option<String>,
    status: Status,
    mut kind: ItemKind,
) -> Result<(), ApplyError> {
    let store = hub.store();
    let thread_id = run.thread_id.clone();
    if matches!(kind, ItemKind::Error { .. }) {
        items.errored = true;
    }
    if items.hidden.contains(&key) || spawns_agent(&kind) {
        items.hidden.insert(key, ());
        return Ok(());
    }
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
        tool: ToolKind::Agents {
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
        Some(parent) => stored_parent(store, run, items, &parent).await?,
        None => None,
    };
    let id = if matches!(kind, ItemKind::Agent { .. }) {
        match known_item(store, run, items, &key).await? {
            Some(id) => id,
            None => items.id(&key),
        }
    } else {
        identify(store, run, items, &key).await?
    };
    if matches!(kind, ItemKind::Agent { .. })
        && status == Status::Running
        && items
            .by_key
            .get(&key)
            .and_then(Tracked::status)
            .is_some_and(Status::is_terminal)
        && let Some(item) = store.reopen_agent(&id, &run.id).await?
        && let Some(Tracked::Stored {
            status: tracked_status,
            ..
        }) = items.by_key.get_mut(&key)
    {
        *tracked_status = item.status;
    }
    let tracked = items.by_key.get_mut(&key).unwrap_or_else(|| {
        unreachable!(
            "invariant: item {id} of run {} is tracked once named",
            run.id
        )
    });
    let before = tracked.status();
    let parent_id = parent_id.or_else(|| tracked.stored_parent().cloned());
    if before.is_none() && parent_id.is_none() {
        hub.clear_live(
            &thread_id,
            LiveTarget::Reasoning {
                run_id: run.id.clone(),
            },
        );
    }
    let finished_text = status.is_terminal() && matches!(kind, ItemKind::AssistantMessage { .. });
    let shown = if finished_text && let ItemKind::AssistantMessage { text, .. } = &kind {
        text.inline()
            .map(crate::resources::written_sources)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let shell_done = status.is_terminal() && matches!(kind, ItemKind::Tool { .. });
    if status.is_terminal() && !before.is_some_and(Status::is_terminal) {
        items.weighed.finished(&kind, status);
    }
    hub.snapshot_resources(&thread_id, &id, shown).await;
    let saved = store
        .upsert_item(new_item(
            id.clone(),
            &thread_id,
            Some(&run.id),
            parent_id.clone(),
            Some(key.clone()),
            status,
            kind,
        ))
        .await?;
    let asked = (before == Some(Status::AwaitingUser)) != (saved.status == Status::AwaitingUser);
    tracked.store(parent_id, saved.status);
    if saved.status.is_terminal() && !matches!(saved.kind, ItemKind::Agent { .. }) {
        items.settle(&key);
    }
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
    Ok(())
}

fn open_items(items: &Work) -> Vec<ItemId> {
    items
        .by_key
        .values()
        .filter_map(Tracked::open)
        .cloned()
        .collect()
}

async fn announce_stop(
    hub: &Hub,
    run: &Run,
    items: &Work,
    pending: &[artisan_api::Item],
) -> Result<Run, forge_store::SettleRunError> {
    let mut stopped = run.clone();
    stopped
        .finish(RunOutcome::Cancelled, None, hub.store().now())
        .expect("invariant: a run announces its stop while it is still running");
    let (stopped, _) = Box::pin(hub.store().settle_run(forge_store::Settlement {
        run: stopped,
        open: open_items(items),
        attempted: pending.iter().map(|item| item.id.clone()).collect(),
        park: None,
        note: None,
    }))
    .await?;
    Ok(stopped)
}

async fn finish(
    hub: &Hub,
    run: &mut Run,
    items: &Work,
    ending: Ending,
    ended: RunOutcome,
    attempted: Vec<ItemId>,
) -> Result<(), forge_store::SettleRunError> {
    let open = open_items(items);
    if let Some(error) = ending.error() {
        tracing::warn!(thread = %run.thread_id, run = %run.id, outcome = ?ended, failure_bytes = error.len(), "the run ended with a failure, shown in the thread");
    }
    let note = match ending.error() {
        Some(error) if !items.errored => Some(new_item(
            ItemId::new(),
            &run.thread_id,
            Some(&run.id),
            None,
            None,
            Status::Done,
            ItemKind::Error {
                kind: artisan_api::ErrorKind::Execution,
                message: error.to_owned().into(),
                retryable: false,
            },
        )),
        Some(_) | None => None,
    };
    let park = match &ending {
        Ending::Finished(outcome @ Outcome::Failed(_)) => {
            crate::park::plan(hub, run, outcome).await
        }
        Ending::Finished(Outcome::Done | Outcome::Cancelled | Outcome::Refused)
        | Ending::Stopped(_) => None,
    };
    let error = ending.error().map(str::to_owned);
    match run.finish(ended, error.clone(), hub.store().now()) {
        Ok(()) => {}
        Err(announced) if announced.ended == ended => {
            if let RunState::Finished {
                error: recorded, ..
            } = &mut run.state
            {
                *recorded = error;
            }
        }
        Err(announced) => unreachable!("invariant: {announced}"),
    }
    let settlement = forge_store::Settlement {
        run: run.clone(),
        open,
        attempted,
        park,
        note,
    };
    match hub.store().settle_run(settlement.clone()).await {
        Ok((_, parked)) => {
            if let Some(park) = park.filter(|_| parked) {
                tracing::info!(thread = %run.thread_id, run = %run.id, continuation = ?park.continuation,
                    due_at = park.due_at, "parked a failed turn");
            }
            Ok(())
        }
        Err(error) => {
            hub.keep_unsettled(settlement);
            Err(error)
        }
    }
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

#[cfg(test)]
#[path = "../tests/run/ending.rs"]
mod tests;
