mod agents;
mod approvals;
mod beneath;
mod coordination;
mod disk;
pub mod error;
mod extensions;
pub mod failure;
mod gateway;
mod handoff;
mod launch;
mod mcp_auth;
mod mcp_reach;
mod operation;
mod park;
pub mod resources;
mod retention;
mod run;
pub mod runtime;
mod sandbox;
mod scratch;
mod side;
mod turns;

pub use agents::{
    AgentInfo, AgentLookupError, AgentQueryError, AgentState, AgentViewError, CancelAgentError,
    MessageAgentError, SendAsAgentError, SpawnAgent, SpawnAgentError, StartThreadError,
};
pub use approvals::ApprovalError;
pub use coordination::DeliverMessageError;
pub use error::{ErrorClass, Refused};
pub use extensions::{
    AdoptSkillError, CustomisationError, DiscardSkillError, EnabledSkillsError, Extensions,
    GlobalPromptError, HomesLockError, InstallSkillError, KeepSkillsError, MarketplaceInstallError,
    MarketplaceSearchError, PlaceSkillsError, PlacedSkills, ReconcileSkillsError,
    ResolveGlobalPromptError, SaveGlobalPromptError, SetSkillEnabledError, SkillCatalogError,
    UndoSkillsError,
};
pub use gateway::COMPUTER_USE_KEY;
pub use gateway::{CallEditorError, GatewayCaller, GatewayRefusal};
pub use mcp_auth::{
    Mcp, McpAuthError, McpSignInFinishError, McpSignInStartError, McpStoreError,
    ReadMcpServersError,
};
pub(crate) use operation::InputError;
pub use operation::{
    AnswerError, AnswerRefusal, CancelError, ClearGoalError, ClearGoalRefusal, ContinueError,
    ContinueRefusal, SendError, StopFailure, UpdateError,
};
pub use retention::{RemoveInactiveThreadsError, SettingKind};
pub use run::PrepareError;
pub use sandbox::auto_notice_on_this_host;
pub use scratch::{EnsureScratchProjectError, ThreadFolderError};
pub use side::{SUGGESTIONS_KEY, SUMMARY_TITLES_KEY, YOU_SHOULD_KNOW_KEY};

use artisan_api::{
    Answer, EngineId, Event, Item, ItemId, ItemKind, LiveTarget, RunId, SendMessage, Status,
    ThreadId,
};
use forge_engines::{Engines, Harness, Input};
use forge_store::Store;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub type HarnessFor = Arc<dyn Fn(EngineId) -> Arc<dyn Harness> + Send + Sync>;

#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("data_dir", &self.inner.data_dir)
            .field("store", &self.inner.store)
            .finish_non_exhaustive()
    }
}

struct Inner {
    generation: Option<runtime::Generation>,
    store: Store,
    engines: Engines,
    data_dir: PathBuf,
    harness_for: HarnessFor,
    turns: Mutex<turns::Turns>,
    side: side::SideQueries,
    extensions: Extensions,
    mcp: Mcp,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    harness_slots: Arc<Semaphore>,
    dispatch: Notify,
    admission: tokio::sync::Mutex<()>,
    gateway: gateway::Gateway,
    agents: agents::AgentWatch,
    approvals: approvals::OwnApprovals,
    disk: disk::Probe,
    sandbox: sandbox::Host,
    stop_deadline: std::time::Duration,
}

struct Surroundings {
    disk: disk::Probe,
    sandbox: sandbox::Host,
    stop_deadline: std::time::Duration,
}

impl Surroundings {
    fn system() -> Self {
        Self {
            disk: disk::Probe::system(),
            sandbox: forge_isolation::support().into(),
            stop_deadline: STOP_DEADLINE,
        }
    }
}

const STOP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(45);

const RUN_CAPACITY: usize = 8;

const FIRST_RETRY: std::time::Duration = std::time::Duration::from_secs(1);
const LONGEST_RETRY: std::time::Duration = std::time::Duration::from_mins(1);

fn retry_delay(failures: u32) -> std::time::Duration {
    FIRST_RETRY
        .saturating_mul(2_u32.saturating_pow(failures.saturating_sub(1).min(6)))
        .min(LONGEST_RETRY)
}

enum Stopping {
    Live(turns::StopWait),
    Idle,
}

#[derive(Debug)]
struct LiveRun {
    id: RunId,
    inbox: mpsc::Sender<Input>,
    cancel: CancellationToken,
    engine: Option<EngineId>,
    launched: Option<artisan_api::Permissions>,
    live: std::collections::BTreeMap<LiveTarget, LivePreview>,
    foreground_bytes: usize,
    terminal_bytes: usize,
    phase: turns::Phase,
    stop: turns::StopSignal,
    retries: u32,
}

const LIVE_TARGET_BYTES: usize = 256 * 1024;
const LIVE_FOREGROUND_BYTES: usize = 2 * 1024 * 1024;
const LIVE_TERMINAL_BYTES: usize = 1024 * 1024;
const LIVE_TARGETS: usize = 128;

const fn terminal(target: &LiveTarget) -> bool {
    matches!(
        target,
        LiveTarget::Shell { .. } | LiveTarget::Progress { .. }
    )
}

fn keep_newest(text: &mut String, bytes: usize) -> usize {
    let mut cut = text.len().saturating_sub(bytes);
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    text.drain(..cut);
    cut
}

impl LiveRun {
    fn terminal_share(&self, admitting: bool) -> usize {
        let terminals =
            self.live.keys().filter(|target| terminal(target)).count() + usize::from(admitting);
        LIVE_TARGET_BYTES.min(LIVE_TERMINAL_BYTES / terminals.max(1))
    }

    fn shrink_terminals(&mut self, share: usize) {
        for (target, preview) in &mut self.live {
            if terminal(target) && preview.text.len() > share {
                let dropped = keep_newest(&mut preview.text, share);
                preview.coverage.start = preview.coverage.start.saturating_add(dropped as u64);
                self.terminal_bytes = self.terminal_bytes.saturating_sub(dropped);
            }
        }
    }

    fn release(&mut self, target: &LiveTarget, bytes: usize) {
        if terminal(target) {
            self.terminal_bytes = self.terminal_bytes.saturating_sub(bytes);
        } else {
            self.foreground_bytes = self.foreground_bytes.saturating_sub(bytes);
        }
    }

    fn hold(&mut self, target: &LiveTarget, bytes: usize) {
        if terminal(target) {
            self.terminal_bytes += bytes;
        } else {
            self.foreground_bytes += bytes;
        }
    }
}

#[derive(Debug, Default)]
struct LivePreview {
    text: String,
    coverage: artisan_api::LiveCoverage,
}

impl Hub {
    #[must_use]
    pub fn new(store: Store, engines: Engines, data_dir: PathBuf) -> Self {
        Self::with_harness(store, engines, data_dir, Arc::new(forge_engines::harness))
    }

    #[must_use]
    pub fn with_harness(
        store: Store,
        engines: Engines,
        data_dir: PathBuf,
        harness_for: HarnessFor,
    ) -> Self {
        Self::build(
            store,
            engines,
            data_dir,
            harness_for,
            Surroundings::system(),
            None,
        )
    }

    pub fn with_generation(
        store: Store,
        engines: Engines,
        data_dir: PathBuf,
        harness_for: HarnessFor,
        generation: runtime::Generation,
    ) -> Self {
        Self::build(
            store,
            engines,
            data_dir,
            harness_for,
            Surroundings::system(),
            Some(generation),
        )
    }

    pub fn generation(&self) -> Option<&runtime::Generation> {
        self.inner.generation.as_ref()
    }

    fn build(
        store: Store,
        engines: Engines,
        data_dir: PathBuf,
        harness_for: HarnessFor,
        surroundings: Surroundings,
        generation: Option<runtime::Generation>,
    ) -> Self {
        let Surroundings {
            disk,
            sandbox,
            stop_deadline,
        } = surroundings;
        let extensions = Extensions::new(store.clone(), engines.clone(), data_dir.clone());
        let mcp = Mcp::new(store.clone());
        let side = side::SideQueries::new(store.clone());
        let hub = Self {
            inner: Arc::new(Inner {
                generation,
                store,
                engines,
                data_dir,
                harness_for,
                turns: Mutex::new(turns::Turns::default()),
                side,
                extensions,
                mcp,
                shutdown: CancellationToken::new(),
                tasks: TaskTracker::new(),
                harness_slots: Arc::new(Semaphore::new(RUN_CAPACITY)),
                dispatch: Notify::new(),
                admission: tokio::sync::Mutex::new(()),
                gateway: gateway::Gateway::default(),
                agents: agents::AgentWatch::default(),
                approvals: approvals::OwnApprovals::default(),
                disk,
                sandbox,
                stop_deadline,
            }),
        };
        hub.forward_engine_status();
        hub.start_dispatcher();
        hub.start_coordination();
        hub
    }

    #[must_use]
    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    #[must_use]
    pub fn engines(&self) -> &Engines {
        &self.inner.engines
    }

    #[must_use]
    fn data_dir(&self) -> &std::path::Path {
        &self.inner.data_dir
    }

    pub fn resume_pending(&self) {
        self.inner.dispatch.notify_one();
    }

    pub async fn send_message(
        &self,
        thread_id: &ThreadId,
        message: SendMessage,
    ) -> Result<Item, SendError> {
        if message.text.trim_start().starts_with('/') {
            let thread =
                self.store()
                    .thread(thread_id)
                    .await?
                    .ok_or_else(|| SendError::ThreadNotFound {
                        thread: thread_id.clone(),
                    })?;
            let requested = message
                .config
                .as_ref()
                .map_or(thread.config.engine, |config| config.engine);
            let engine = self
                .lock()
                .get(thread_id)
                .and_then(|run| run.engine)
                .unwrap_or(requested);
            forge_engines::normalize_command(engine, &message.text).map_err(|source| {
                SendError::Command {
                    thread: thread_id.clone(),
                    engine,
                    source,
                }
            })?;
        }
        let acceptance = forge_store::Acceptance {
            title: message
                .text
                .lines()
                .next()
                .map(|line| line.trim().chars().take(80).collect::<String>())
                .filter(|title| !title.is_empty()),
            remember_config: true,
            from_agent: None,
            sender: None,
        };
        self.accept_message(thread_id, message, acceptance).await
    }

    async fn accept_message(
        &self,
        thread_id: &ThreadId,
        message: SendMessage,
        acceptance: forge_store::Acceptance,
    ) -> Result<Item, SendError> {
        let route = self.route(thread_id).map_err(|source| SendError::Route {
            thread: thread_id.clone(),
            source,
        })?;
        if let Some(member) = route {
            let item = message.id.clone();
            return runtime::request_as(
                &member,
                &runtime::Command::Accept {
                    thread: thread_id.clone(),
                    message,
                    acceptance,
                },
            )
            .await
            .map_err(|source| SendError::Peer {
                thread: thread_id.clone(),
                item,
                source: Box::new(source),
            });
        }
        self.accept_here(thread_id, message, acceptance).await
    }

    async fn accept_here(
        &self,
        thread_id: &ThreadId,
        message: SendMessage,
        acceptance: forge_store::Acceptance,
    ) -> Result<Item, SendError> {
        let hub = self.clone();
        let thread = thread_id.clone();
        let item = message.id.clone();
        let accepting = self
            .inner
            .tasks
            .spawn(async move { hub.accept_and_dispatch(&thread, message, acceptance).await });
        match accepting.await {
            Ok(accepted) => accepted,
            Err(failure) if failure.is_panic() => std::panic::resume_unwind(failure.into_panic()),
            Err(source) => Err(SendError::Abandoned {
                thread: thread_id.clone(),
                item,
                source,
            }),
        }
    }

    async fn accept_and_dispatch(
        &self,
        thread_id: &ThreadId,
        message: SendMessage,
        acceptance: forge_store::Acceptance,
    ) -> Result<Item, SendError> {
        let (item, created, saved_engine) = self
            .inner
            .store
            .send_message(thread_id.clone(), message, acceptance)
            .await?;
        if created && let Err(error) = self.dispatch_accepted(thread_id, &item, saved_engine).await
        {
            tracing::warn!(thread = %thread_id, item = %item.id, "hand the saved message to the live run; it waits for the next turn: {}", diagnostics::chain(&error));
            self.resume_pending();
        }
        Ok(item)
    }

    async fn dispatch_accepted(
        &self,
        thread_id: &ThreadId,
        item: &Item,
        saved_engine: EngineId,
    ) -> Result<(), InputError> {
        let live = self
            .lock()
            .get(thread_id)
            .map(|run| (run.inbox.clone(), run.engine.unwrap_or(saved_engine)));
        if let Some((inbox, engine)) = live {
            let input = self.user_input(item, engine).await?;
            if inbox.send(Input::Steer(input)).await.is_err() {
                self.resume_pending();
            }
        } else {
            self.lock().undefer(thread_id);
            self.resume_pending();
        }
        Ok(())
    }

    pub async fn answer(&self, item_id: &ItemId, answer: Answer) -> Result<(), AnswerError> {
        let item = self.answerable(item_id).await?;
        let route = self
            .route(&item.thread_id)
            .map_err(|source| AnswerError::Route {
                item: item_id.clone(),
                source,
            })?;
        if let Some(member) = route {
            runtime::request(
                &member,
                &runtime::Command::Answer {
                    item: item_id.clone(),
                    answer,
                },
            )
            .await
            .map_err(|source| AnswerError::Peer {
                item: item_id.clone(),
                source: Box::new(source),
            })?;
            return Ok(());
        }
        self.answer_item(item, answer).await
    }

    async fn answer_here(&self, item_id: &ItemId, answer: Answer) -> Result<(), AnswerError> {
        let item = self.answerable(item_id).await?;
        self.answer_item(item, answer).await
    }

    async fn answerable(&self, item_id: &ItemId) -> Result<Item, AnswerError> {
        self.store()
            .item(item_id)
            .await?
            .ok_or_else(|| AnswerError::ItemNotFound {
                item: item_id.clone(),
            })
    }

    async fn answer_item(&self, item: Item, answer: Answer) -> Result<(), AnswerError> {
        let item_id = &item.id;
        if let Answer::Question { answers } = &answer {
            self.store()
                .validate_question_answers(item_id, answers)
                .await?;
        }
        if let Answer::Approval { decision } = &answer {
            let approval = match &item.kind {
                ItemKind::Tool { approval, .. } | ItemKind::Plan { approval, .. } => {
                    approval.as_ref()
                }
                ItemKind::UserMessage { .. }
                | ItemKind::AssistantMessage { .. }
                | ItemKind::Agent { .. }
                | ItemKind::Question { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => None,
            }
            .ok_or_else(|| AnswerError::refused(item_id, AnswerRefusal::NotAnApproval))?;
            if !approval.options.contains(decision) {
                return Err(AnswerError::refused(
                    item_id,
                    AnswerRefusal::DecisionNotOffered,
                ));
            }
            if self.answer_own_approval(item_id, *decision) {
                return Ok(());
            }
        }
        let inbox = self
            .lock()
            .get(&item.thread_id)
            .filter(|run| item.run_id.as_ref() == Some(&run.id))
            .map(|run| run.inbox.clone());
        if inbox.is_none()
            && matches!(
                item.kind,
                ItemKind::Question {
                    blocking: false,
                    ..
                }
            )
            && let Answer::Question { answers } = &answer
        {
            self.store()
                .answer_question(item_id.clone(), answers.clone())
                .await?;
            self.resume_pending();
            return Ok(());
        }
        if item.status != Status::AwaitingUser {
            return Err(AnswerError::refused(item_id, AnswerRefusal::NotWaiting));
        }
        let key =
            self.store()
                .native_id(item_id)
                .await?
                .ok_or_else(|| AnswerError::NoEngineId {
                    item: item_id.clone(),
                })?;
        let ended = || AnswerError::refused(item_id, AnswerRefusal::RunEnded);
        let inbox = inbox.ok_or_else(ended)?;
        if inbox.send(Input::Answer { key, answer }).await.is_err() {
            return Err(ended());
        }
        self.agent_attention(&item.thread_id).await;
        Ok(())
    }

    pub async fn continue_turn(&self, thread_id: &ThreadId) -> Result<(), ContinueError> {
        let route = self
            .route(thread_id)
            .map_err(|source| ContinueError::Route {
                thread: thread_id.clone(),
                source,
            })?;
        if let Some(member) = route {
            runtime::request(
                &member,
                &runtime::Command::Continue {
                    thread: thread_id.clone(),
                },
            )
            .await
            .map_err(|source| ContinueError::Peer {
                thread: thread_id.clone(),
                source: Box::new(source),
            })?;
            return Ok(());
        }
        self.continue_here(thread_id).await
    }

    async fn continue_here(&self, thread_id: &ThreadId) -> Result<(), ContinueError> {
        let _admission = self.inner.admission.lock().await;
        self.continue_admitted(thread_id).await
    }

    async fn continue_admitted(&self, thread_id: &ThreadId) -> Result<(), ContinueError> {
        let refused = |reason| ContinueError::Refused {
            thread: thread_id.clone(),
            reason,
        };
        let continued = self.lock().carry_on(thread_id);
        match continued {
            turns::Continued::Recorded => Ok(()),
            turns::Continued::Record => {
                self.store().request_continue(thread_id.clone()).await?;
                self.lock().continued(thread_id);
                Ok(())
            }
            turns::Continued::Busy => Err(refused(ContinueRefusal::Working)),
            turns::Continued::NothingStopped => Err(refused(ContinueRefusal::NothingStopped)),
            turns::Continued::Idle => {
                let stopped = self
                    .store()
                    .latest_run(thread_id)
                    .await?
                    .is_some_and(|run| run.status().is_stopped());
                if !stopped {
                    return Err(refused(ContinueRefusal::NothingStopped));
                }
                self.store().request_continue(thread_id.clone()).await?;
                self.lock().undefer(thread_id);
                self.resume_pending();
                Ok(())
            }
        }
    }

    pub async fn update_thread(
        &self,
        thread_id: &ThreadId,
        update: artisan_api::UpdateThread,
    ) -> Result<Option<artisan_api::Thread>, UpdateError> {
        let route = self.route(thread_id).map_err(|source| UpdateError::Route {
            thread: thread_id.clone(),
            source,
        })?;
        if let Some(member) = route {
            return runtime::request_as(
                &member,
                &runtime::Command::Update {
                    thread: thread_id.clone(),
                    update,
                },
            )
            .await
            .map_err(|source| UpdateError::Peer {
                thread: thread_id.clone(),
                source: Box::new(source),
            });
        }
        self.update_here(thread_id, update).await
    }

    async fn update_here(
        &self,
        thread_id: &ThreadId,
        update: artisan_api::UpdateThread,
    ) -> Result<Option<artisan_api::Thread>, UpdateError> {
        let before = self
            .store()
            .thread(thread_id)
            .await?
            .map(|thread| thread.config.permissions);
        if update.title.is_some() {
            self.inner.side.titled(thread_id);
        }
        let thread = self.store().update_thread(thread_id, update).await?;
        if let Some(thread) = &thread
            && before.is_some_and(|before| before != thread.config.permissions)
        {
            if thread.config.permissions == artisan_api::Permissions::Full {
                self.allow_own_approvals(thread_id);
            }
            let inbox = self.lock().get(thread_id).map(|run| run.inbox.clone());
            if let Some(inbox) = inbox
                && inbox
                    .send(Input::SetPermissions(thread.config.permissions))
                    .await
                    .is_err()
            {
                tracing::debug!(thread = %thread_id, "the run ended before its access changed");
            }
        }
        Ok(thread)
    }

    pub async fn clear_goal(&self, thread_id: &ThreadId) -> Result<(), ClearGoalError> {
        let route = self
            .route(thread_id)
            .map_err(|source| ClearGoalError::Route {
                thread: thread_id.clone(),
                source,
            })?;
        if let Some(member) = route {
            runtime::request(
                &member,
                &runtime::Command::ClearGoal {
                    thread: thread_id.clone(),
                },
            )
            .await
            .map_err(|source| ClearGoalError::Peer {
                thread: thread_id.clone(),
                source: Box::new(source),
            })?;
            return Ok(());
        }
        self.clear_goal_here(thread_id).await
    }

    async fn clear_goal_here(&self, thread_id: &ThreadId) -> Result<(), ClearGoalError> {
        let store = self.store();
        let thread =
            store
                .thread(thread_id)
                .await?
                .ok_or_else(|| ClearGoalError::ThreadNotFound {
                    thread: thread_id.clone(),
                })?;
        let goal = store
            .inspector(thread_id)
            .await?
            .goal
            .ok_or_else(|| ClearGoalError::refused(thread_id, ClearGoalRefusal::NoGoal))?;
        let live = self.lock().get(thread_id).map(|run| {
            (
                run.inbox.clone(),
                run.engine.unwrap_or(thread.config.engine),
            )
        });
        let engine = live
            .as_ref()
            .map_or(thread.config.engine, |(_, engine)| *engine);
        match engine {
            EngineId::Claude => {
                let message = SendMessage {
                    id: ItemId::new(),
                    text: "/goal clear".to_owned(),
                    images: Vec::new(),
                    config: None,
                    draft: None,
                };
                self.send_message(thread_id, message)
                    .await
                    .map_err(|source| ClearGoalError::Send {
                        thread: thread_id.clone(),
                        source: Box::new(source),
                    })?;
                return Ok(());
            }
            EngineId::Codex | EngineId::OpenCode => {}
        }
        if let Some((inbox, _)) = live {
            if inbox.send(Input::ClearGoal).await.is_err() {
                return Err(ClearGoalError::refused(thread_id, ClearGoalRefusal::Ending));
            }
        } else {
            self.clear_goal_offline(&thread, engine).await?;
        }
        store
            .upsert_item(forge_store::NewItem {
                id: ItemId::new(),
                thread_id: thread_id.clone(),
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::Done,
                kind: ItemKind::Notice {
                    kind: artisan_api::NoticeKind::Goal {
                        state: "cleared".to_owned(),
                    },
                    text: goal.objective,
                },
            })
            .await?;
        Ok(())
    }

    async fn clear_goal_offline(
        &self,
        thread: &artisan_api::Thread,
        engine: EngineId,
    ) -> Result<(), ClearGoalError> {
        let thread_id = &thread.id;
        let prepare_failed = |source: PrepareError| ClearGoalError::Prepare {
            thread: thread_id.clone(),
            engine,
            source: Box::new(source),
        };
        let latest = self.store().latest_run(thread_id).await?;
        let launch = launch::LaunchSession::current(self, engine, latest.as_ref())
            .await
            .map_err(|source| prepare_failed(source.into()))?;
        if launch.resume().is_none() {
            return Err(ClearGoalError::refused(
                thread_id,
                ClearGoalRefusal::NoSession { engine },
            ));
        }
        let lease = self.engines().lease(engine);
        let run::Prepared {
            turn,
            gateway: _gateway,
        } = run::prepare(
            self,
            thread,
            &launch,
            forge_engines::UserInput::default(),
            lease.as_ref().map(|lease| lease.path().to_path_buf()),
        )
        .await
        .map_err(prepare_failed)?;
        (self.inner.harness_for)(engine)
            .clear_goal(turn)
            .await
            .map_err(|source| ClearGoalError::Engine {
                thread: thread_id.clone(),
                engine,
                source,
            })?;
        drop(lease);
        Ok(())
    }

    pub async fn cancel(&self, thread_id: &ThreadId) -> Result<(), CancelError> {
        let mut failures = Vec::new();
        let mut elsewhere = Vec::new();
        let mut idle = Vec::new();
        let mut stopping = Vec::new();
        let total = {
            let _admission = self.inner.admission.lock().await;
            let tree = self.agent_tree(thread_id).await?;
            for thread in &tree {
                match self.route(thread) {
                    Ok(Some(member)) => elsewhere.push((thread.clone(), member)),
                    Ok(None) => match self.cancel_owned(thread, &mut failures).await {
                        Stopping::Live(saved) => stopping.push((thread.clone(), saved)),
                        Stopping::Idle => idle.push(thread.clone()),
                    },
                    Err(source) => failures.push(StopFailure::Route {
                        thread: thread.clone(),
                        source,
                    }),
                }
            }
            tree.len()
        };
        for (thread, saved) in stopping {
            if let Err(source) = saved.saved().await {
                failures.push(StopFailure::Unsaved { thread, source });
            }
        }
        for thread in idle {
            self.agent_attention(&thread).await;
        }
        for (thread, member) in elsewhere {
            if let Err(source) = runtime::request(
                &member,
                &runtime::Command::Cancel {
                    thread: thread.clone(),
                },
            )
            .await
            {
                failures.push(StopFailure::Peer {
                    thread,
                    source: Box::new(source),
                });
            }
        }
        CancelError::check(thread_id, total, failures)
    }

    async fn cancel_local(&self, thread_id: &ThreadId) -> Result<(), CancelError> {
        let mut failures = Vec::new();
        let stopped = {
            let _admission = self.inner.admission.lock().await;
            self.cancel_owned(thread_id, &mut failures).await
        };
        match stopped {
            Stopping::Live(saved) => {
                if let Err(source) = saved.saved().await {
                    failures.push(StopFailure::Unsaved {
                        thread: thread_id.clone(),
                        source,
                    });
                }
            }
            Stopping::Idle => self.agent_attention(thread_id).await,
        }
        CancelError::check(thread_id, 1, failures)
    }

    async fn cancel_owned(
        &self,
        thread_id: &ThreadId,
        failures: &mut Vec<StopFailure>,
    ) -> Stopping {
        let cutoff = self.store().head();
        let stopped = self.lock().stop(thread_id, cutoff);
        if let Err(error) = self
            .store()
            .cancel_pending_inputs(thread_id.clone(), cutoff)
            .await
        {
            failures.push(error.into());
        }
        if let Err(error) = self.store().unpark(thread_id.clone()).await {
            failures.push(error.into());
        }
        let stopping = match stopped {
            turns::Stopped::Live(saved) => Stopping::Live(saved),
            turns::Stopped::Idle => Stopping::Idle,
            turns::Stopped::Unsettled(settlement) => {
                match self.store().settle_run(*settlement).await {
                    Ok(_) => self.lock().settled(thread_id),
                    Err(error) => failures.push(StopFailure::Unsaved {
                        thread: thread_id.clone(),
                        source: Arc::new(error),
                    }),
                }
                Stopping::Idle
            }
        };
        self.resume_pending();
        stopping
    }

    pub fn shutdown_on_drop(&self) -> tokio_util::sync::DropGuard {
        self.inner.shutdown.clone().drop_guard()
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
        self.inner.tasks.close();
    }

    pub async fn stop(&self, grace: std::time::Duration) {
        self.shutdown();
        if tokio::time::timeout(grace, self.inner.tasks.wait())
            .await
            .is_err()
        {
            tracing::warn!(
                "runs still settling after {}s; stopping anyway",
                grace.as_secs()
            );
        }
    }

    #[must_use]
    pub fn is_live(&self, thread_id: &ThreadId) -> bool {
        self.lock().contains_key(thread_id)
            || self
                .inner
                .generation
                .as_ref()
                .is_some_and(|generation| generation.owner(thread_id).is_some())
    }

    fn owned_thread_ids(&self) -> Vec<ThreadId> {
        let ids: std::collections::BTreeSet<_> = self.lock().held().cloned().collect();
        ids.into_iter().collect()
    }

    pub fn active_thread_ids(&self) -> Vec<ThreadId> {
        let mut ids: std::collections::BTreeSet<_> = self.owned_thread_ids().into_iter().collect();
        if let Some(generation) = &self.inner.generation {
            ids.extend(generation.foreign_threads());
        }
        ids.into_iter().collect()
    }

    fn route(
        &self,
        thread: &ThreadId,
    ) -> Result<Option<runtime::Member>, runtime::RegistryFileError> {
        let Some(generation) = &self.inner.generation else {
            return Ok(None);
        };
        if self.lock().holds(thread) {
            return Ok(None);
        }
        if let Some(owner) = generation.owner(thread) {
            return Ok(Some(owner));
        }
        if !generation.draining() {
            return Ok(None);
        }
        Ok(generation
            .registry()
            .active()?
            .filter(|active| active.id != generation.member().id))
    }

    pub fn local_active_count(&self) -> usize {
        self.owned_thread_ids().len()
    }

    #[must_use]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the turns stay locked so the snapshot matches its revision"
    )]
    fn live_snapshot(&self) -> (u64, Vec<Event>) {
        let live = self.lock();
        let mut snapshot: Vec<_> = live
            .iter()
            .flat_map(|(thread_id, run)| {
                run.live.iter().map(|(target, text)| Event::Live {
                    thread_id: thread_id.clone(),
                    target: target.clone(),
                    text: text.text.clone(),
                    append: false,
                    coverage: text.coverage,
                })
            })
            .collect();
        self.inner.side.with_suggestions(|suggestions| {
            snapshot.extend(suggestions);
            snapshot.extend(self.pending_editor_calls());
            sort_snapshot(&mut snapshot);
            (self.inner.store.live_revision(), snapshot)
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, turns::Turns> {
        self.inner
            .turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    async fn settle_unsettled(&self) -> bool {
        if self.lock().unsettled().is_empty() {
            return true;
        }
        let mut settled = true;
        let mut ended = Vec::new();
        {
            let _admission = self.inner.admission.lock().await;
            let waiting = self.lock().unsettled();
            for settlement in waiting {
                let thread = settlement.run.thread_id.clone();
                let run = settlement.run.id.clone();
                if let Err(error) = self.store().settle_run(settlement).await {
                    settled = false;
                    tracing::warn!(%thread, %run, "saving the run's end failed again: {}", diagnostics::chain(&error));
                    continue;
                }
                tracing::info!(%thread, %run, "saved the run's end after an earlier failure");
                self.lock().settled(&thread);
                ended.push(thread);
            }
        }
        for thread in ended {
            self.agent_attention(&thread).await;
        }
        self.resume_pending();
        settled
    }

    fn start_dispatcher(&self) {
        let hub = self.clone();
        self.inner.tasks.spawn(async move {
            let mut unclaimed_at = None;
            let mut failures = 0_u32;
            let mut passed_at = tokio::time::Instant::now();
            loop {
                let (parked, readable) = park::until_next(&hub, unclaimed_at).await;
                let retry = async {
                    if failures == 0 && readable {
                        std::future::pending::<()>().await;
                    } else {
                        tokio::time::sleep(retry_delay(failures)).await;
                    }
                };
                let next_due = hub.lock().next_due(passed_at);
                let undeferred = async {
                    match next_due {
                        Some(until) => tokio::time::sleep_until(until).await,
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    biased;
                    () = hub.inner.shutdown.cancelled() => break,
                    () = hub.inner.dispatch.notified() => {},
                    () = parked => {},
                    () = retry => {},
                    () = undeferred => {},
                }
                let mut failed = !readable;
                failed |= !hub.settle_unsettled().await;
                unclaimed_at = Some(hub.store().now());
                passed_at = tokio::time::Instant::now();
                loop {
                    let _admission = hub.inner.admission.lock().await;
                    if hub.inner.shutdown.is_cancelled() {
                        return;
                    }
                    if hub
                        .inner
                        .generation
                        .as_ref()
                        .is_some_and(|generation| !generation.accepting())
                    {
                        break;
                    }
                    let mut excluded = hub.active_thread_ids();
                    let foreign = excluded.len().saturating_sub(hub.local_active_count());
                    if hub.inner.harness_slots.available_permits() <= foreign {
                        break;
                    }
                    excluded.extend(hub.lock().waiting(passed_at).cloned());
                    let Ok(permit) = Arc::clone(&hub.inner.harness_slots).try_acquire_owned()
                    else {
                        break;
                    };
                    let next = match hub.store().pending_threads(1, excluded).await {
                        Ok(threads) => threads.into_iter().next(),
                        Err(error) => {
                            tracing::error!(
                                "read pending run candidates: {}",
                                diagnostics::chain(&error)
                            );
                            failed = true;
                            break;
                        }
                    };
                    let Some(thread) = next else {
                        break;
                    };
                    unclaimed_at = None;
                    hub.claim(thread, permit);
                }
                failures = if failed {
                    failures.saturating_add(1)
                } else {
                    0
                };
            }
        });
    }

    fn claim(&self, thread_id: ThreadId, permit: OwnedSemaphorePermit) {
        let (inbox, receiver) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let stop = turns::StopSignal::default();
        let run_id = RunId::new();
        self.lock().claim(thread_id.clone(), |retries| LiveRun {
            id: run_id.clone(),
            inbox,
            cancel: cancel.clone(),
            engine: None,
            launched: None,
            live: std::collections::BTreeMap::new(),
            foreground_bytes: 0,
            terminal_bytes: 0,
            phase: turns::Phase::Working,
            stop: stop.clone(),
            retries,
        });
        let hub = self.clone();
        let span = tracing::info_span!("run", thread = %thread_id, run = %run_id);
        let run = async move {
            let drove = Box::pin(run::drive(
                &hub,
                &thread_id,
                run_id.clone(),
                receiver,
                cancel.clone(),
                stop,
            ))
            .await;
            let status = match drove {
                Ok(run::Drove::Concluded(status)) => Some(status),
                Ok(run::Drove::Nothing) => {
                    hub.end_live(&thread_id, &run_id, turns::Ended::Settled)
                        .await;
                    None
                }
                Err(error) => {
                    tracing::error!(thread = %thread_id, run = %run_id, "run failed: {}", diagnostics::chain(&error));
                    let ended = if error.retry_later() {
                        turns::Ended::Deferred
                    } else {
                        turns::Ended::Settled
                    };
                    hub.end_live(&thread_id, &run_id, ended).await;
                    None
                }
            };
            drop(permit);
            if !hub.inner.shutdown.is_cancelled() {
                hub.resume_pending();
            }
            if !hub.inner.shutdown.is_cancelled() {
                hub.agent_attention(&thread_id).await;
            }
            if status == Some(artisan_api::RunStatus::Done)
                && !hub.inner.shutdown.is_cancelled()
                && !hub.is_agent_thread(&thread_id).await
            {
                hub.after_turn(thread_id, run_id).await;
            }
        };
        self.inner
            .tasks
            .spawn(tracing::Instrument::instrument(run, span));
    }

    async fn end_live(&self, thread_id: &ThreadId, run_id: &RunId, ended: turns::Ended) {
        let _admission = self.inner.admission.lock().await;
        self.lock()
            .end(thread_id, run_id, ended, tokio::time::Instant::now());
    }

    #[must_use]
    pub fn extensions(&self) -> &Extensions {
        &self.inner.extensions
    }

    #[must_use]
    pub fn mcp(&self) -> &Mcp {
        &self.inner.mcp
    }

    async fn user_input(
        &self,
        item: &Item,
        engine: EngineId,
    ) -> Result<forge_engines::UserInput, InputError> {
        let ItemKind::UserMessage {
            text,
            images,
            sender,
            ..
        } = &item.kind
        else {
            unreachable!(
                "invariant: input {} of thread {} is a user message",
                item.id, item.thread_id
            );
        };
        let text = self.store().resolve_text(text).await?;
        let parent = match sender {
            Some(_) => self
                .store()
                .thread(&item.thread_id)
                .await?
                .and_then(|thread| thread.parent_thread_id),
            None => None,
        };
        let text = match sender {
            Some(sender) if parent.as_ref() == Some(&sender.thread) => text,
            Some(sender) => format!("{}\n\n{text}", sender_line(sender)),
            None => forge_engines::normalize_command(engine, &text)
                .map_err(|source| InputError::Command {
                    thread: item.thread_id.clone(),
                    item: item.id.clone(),
                    engine,
                    source,
                })?
                .unwrap_or(text),
        };
        let text = if text.contains('$') {
            let skills = match self.extensions().enabled_skills().await {
                Ok(skills) => skills,
                Err(error) => {
                    tracing::warn!(
                        thread = %item.thread_id,
                        item = %item.id,
                        "listing the skills for the message's mentions: {}", diagnostics::chain(&error)
                    );
                    Vec::new()
                }
            };
            forge_engines::with_skill_mentions(engine, &text, &skills)
        } else {
            text
        };
        let text = if text.contains('@') {
            match self.store().thread_titles().await {
                Ok(threads) => forge_engines::with_thread_mentions(&text, &threads),
                Err(error) => {
                    tracing::warn!(
                        thread = %item.thread_id,
                        item = %item.id,
                        "listing the threads for the message's mentions: {}", diagnostics::chain(&error)
                    );
                    text
                }
            }
        } else {
            text
        };
        let images = self.store().input_image_paths(images.clone()).await?;
        Ok(forge_engines::UserInput {
            id: item.id.clone(),
            text: with_image_paths(text, &images),
            images,
        })
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "published under the lock, so updates reach clients in the order they were recorded"
    )]
    fn publish_live(
        &self,
        thread_id: &ThreadId,
        target: LiveTarget,
        mut text: String,
        mut append: bool,
    ) {
        let mut live = self.lock();
        let Some(run) = live.get_mut(thread_id) else {
            return;
        };
        let admitting = !run.live.contains_key(&target);
        if admitting && run.live.len() >= LIVE_TARGETS {
            return;
        }
        let is_terminal = terminal(&target);
        let limit = if is_terminal {
            let share = run.terminal_share(admitting);
            run.shrink_terminals(share);
            share
        } else {
            let before = run
                .live
                .get(&target)
                .map_or(0, |preview| preview.text.len());
            LIVE_TARGET_BYTES.min(
                LIVE_FOREGROUND_BYTES.saturating_sub(run.foreground_bytes.saturating_sub(before)),
            )
        };
        let preview = run.live.entry(target.clone()).or_default();
        let incoming_bytes = text.len() as u64;
        preview.coverage.total = if append {
            preview.coverage.total.saturating_add(incoming_bytes)
        } else {
            incoming_bytes
        };
        if !append {
            preview.coverage.start = 0;
        }
        let buffer = &mut preview.text;
        let before = buffer.len();
        if append && before + text.len() <= limit {
            buffer.push_str(&text);
        } else if append && is_terminal {
            let window = limit / 2;
            if text.len() >= window {
                keep_newest(&mut text, window);
                buffer.clone_from(&text);
            } else {
                keep_newest(buffer, window - text.len());
                buffer.push_str(&text);
                text.clone_from(buffer);
            }
            preview.coverage.start = preview.coverage.total.saturating_sub(buffer.len() as u64);
            append = false;
        } else {
            let available = if append {
                limit.saturating_sub(before)
            } else {
                limit
            };
            let mut end = text.len().min(available);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            if append {
                buffer.push_str(&text);
            } else {
                buffer.clone_from(&text);
            }
        }
        let after = buffer.len();
        let coverage = preview.coverage;
        run.release(&target, before);
        run.hold(&target, after);
        if text.capacity() > LIVE_TARGET_BYTES * 2 {
            text.shrink_to_fit();
        }
        self.inner.store.publish_live(Event::Live {
            thread_id: thread_id.clone(),
            target,
            text,
            append,
            coverage,
        });
    }

    fn live_has_room(&self, thread_id: &ThreadId) -> bool {
        self.lock()
            .get(thread_id)
            .is_some_and(|run| run.live.len() < LIVE_TARGETS)
    }

    fn clear_live(&self, thread_id: &ThreadId, target: LiveTarget) {
        let shown = self
            .lock()
            .get(thread_id)
            .and_then(|run| run.live.get(&target))
            .is_some_and(|preview| !preview.text.is_empty());
        if shown {
            self.publish_live(thread_id, target, String::new(), false);
        }
    }

    fn forget_live(&self, thread_id: &ThreadId, target: &LiveTarget) {
        if let Some(run) = self.lock().get_mut(thread_id)
            && let Some(preview) = run.live.remove(target)
        {
            run.release(target, preview.text.len());
        }
    }

    fn forward_engine_status(&self) {
        let mut statuses = self.inner.engines.subscribe();
        let engines = self.inner.engines.clone();
        let store = self.inner.store.clone();
        let stop = self.inner.shutdown.clone();
        self.inner.tasks.spawn(async move {
            loop {
                let received = tokio::select! {
                    () = stop.cancelled() => break,
                    received = statuses.recv() => received,
                };
                match received {
                    Ok(forge_engines::EngineChange::Full(status)) => {
                        store.publish_live(Event::Engine { status });
                    }
                    Ok(forge_engines::EngineChange::Install { engine, install }) => {
                        store.publish_live(Event::EngineInstall { engine, install });
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::debug!(
                            missed,
                            "engine statuses lagged; publishing every engine's current one"
                        );
                        for engine in EngineId::ALL {
                            store.publish_live(Event::Engine {
                                status: engines.status(engine),
                            });
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

#[cfg(test)]
#[path = "../tests/lib/unit.rs"]
mod tests;

fn with_image_paths(text: String, images: &[PathBuf]) -> String {
    let lines: Vec<String> = images
        .iter()
        .enumerate()
        .map(|(index, path)| format!("[Image {} is saved at {}]", index + 1, path.display()))
        .collect();
    match (text.is_empty(), lines.is_empty()) {
        (_, true) => text,
        (true, false) => lines.join("\n"),
        (false, false) => format!("{text}\n\n{}", lines.join("\n")),
    }
}

fn sender_line(sender: &artisan_api::Sender) -> String {
    let title = sender
        .title
        .as_deref()
        .map(|title| format!(" \"{title}\""))
        .unwrap_or_default();
    format!(
        "[A {} agent in the Artisan thread{title} ({}) sent this message, not the user.]",
        sender.engine.display_name(),
        sender.thread
    )
}

fn sort_snapshot(snapshot: &mut [Event]) {
    snapshot.sort_by(|left, right| snapshot_key(left).cmp(&snapshot_key(right)));
}

fn snapshot_key(event: &Event) -> (Option<&ThreadId>, Option<&LiveTarget>, Option<&str>) {
    match event {
        Event::Live {
            thread_id, target, ..
        } => (Some(thread_id), Some(target), None),
        Event::EditorCall { id, .. } => (None, None, Some(id)),
        Event::Hello { .. }
        | Event::Project { .. }
        | Event::Thread { .. }
        | Event::ThreadRemoved { .. }
        | Event::Item { .. }
        | Event::Run { .. }
        | Event::ItemState { .. }
        | Event::Setting { .. }
        | Event::Suggestion { .. }
        | Event::Engine { .. }
        | Event::EngineInstall { .. }
        | Event::Reset => (event.thread_id(), None, None),
    }
}
