#![expect(
    missing_debug_implementations,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![allow(
    clippy::assertions_on_result_states,
    clippy::inconsistent_struct_constructor,
    clippy::iter_over_hash_type,
    clippy::partial_pub_fields,
    clippy::string_slice,
    trivial_casts,
    reason = "pre-existing violations that only occur in some builds, cleared in plan.md phase 2"
)]

mod agents;
mod approvals;
mod coordination;
mod disk;
pub mod error;
mod extensions;
mod gateway;
mod handoff;
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

pub use agents::{
    AgentInfo, AgentLookupError, AgentQueryError, AgentState, AgentViewError, CancelAgentError,
    MessageAgentError, SendAsAgentError, SpawnAgent, SpawnAgentError, StartThreadError,
};
pub use approvals::ApprovalError;
pub use coordination::DeliverMessageError;
pub use error::{ErrorClass, Refused};
pub use extensions::{
    AdoptSkillError, CustomisationError, DiscardSkillError, EnabledSkillsError, Extensions,
    GlobalPromptError, HomesLockError, InstallSkillError, MarketplaceInstallError,
    MarketplaceSearchError, ReconcileSkillsError, ResolveGlobalPromptError, SaveGlobalPromptError,
    SetSkillEnabledError, SkillCatalogError,
};
pub use gateway::COMPUTER_USE_KEY;
pub use gateway::CallEditorError;
pub use mcp_auth::{
    Mcp, McpAuthError, McpSignInFinishError, McpSignInStartError, McpStoreError,
    ReadMcpServersError,
};
pub use operation::{
    AnswerError, CancelError, ClearGoalError, ContinueError, InputError, SendError, UpdateError,
};
pub use retention::RemoveInactiveThreadsError;
pub use run::PrepareError;
pub use scratch::{EnsureScratchProjectError, ThreadFolderError};
pub use side::{SUGGESTIONS_KEY, SUMMARY_TITLES_KEY, YOU_SHOULD_KNOW_KEY};

use artisan_api::{
    Answer, EngineId, Event, Item, ItemId, ItemKind, LiveTarget, RunId, SendMessage, Status,
    ThreadId,
};
use forge_engines::{Engines, Harness, Input};
use forge_store::Store;
use std::collections::HashMap;
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

struct Inner {
    generation: Option<runtime::Generation>,
    store: Store,
    engines: Engines,
    data_dir: PathBuf,
    harness_for: HarnessFor,
    live: Mutex<HashMap<ThreadId, LiveRun>>,
    unsettled: Mutex<HashMap<ThreadId, forge_store::Settlement>>,
    owed_continues: Mutex<std::collections::HashSet<ThreadId>>,
    suggestions: Mutex<HashMap<ThreadId, Suggested>>,
    attention: Mutex<HashMap<ThreadId, side::Attention>>,
    untitled: Mutex<std::collections::HashSet<ThreadId>>,
    title_queries: Mutex<std::collections::HashSet<ThreadId>>,
    extensions: Extensions,
    mcp: Mcp,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    harness_slots: Arc<Semaphore>,
    dispatch: Notify,
    admission: tokio::sync::Mutex<()>,
    gateway: Mutex<gateway::Gateway>,
    agents_changed: tokio::sync::watch::Sender<u64>,
    agent_waiters: Mutex<HashMap<ItemId, usize>>,
    approvals: Mutex<HashMap<ItemId, approvals::OwnApproval>>,
    disk: disk::Probe,
    sandbox: sandbox::Host,
}

const RUN_CAPACITY: usize = 8;

const FIRST_RETRY: std::time::Duration = std::time::Duration::from_secs(1);
const LONGEST_RETRY: std::time::Duration = std::time::Duration::from_mins(1);

fn retry_delay(failures: u32) -> std::time::Duration {
    FIRST_RETRY
        .saturating_mul(2_u32.saturating_pow(failures.saturating_sub(1).min(6)))
        .min(LONGEST_RETRY)
}

enum Continuing {
    Continued,
    Reported,
    Owed,
}

struct LiveRun {
    id: RunId,
    inbox: mpsc::Sender<Input>,
    cancel: CancellationToken,
    engine: Option<EngineId>,
    live: HashMap<LiveTarget, LivePreview>,
    foreground_bytes: usize,
    terminal_bytes: usize,
    phase: Phase,
}

#[derive(Clone, Copy)]
enum Phase {
    Working,
    Stopping { through: u64, then: AfterStop },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AfterStop {
    Settle,
    Continue,
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

struct Suggested {
    run: RunId,
    cancel: CancellationToken,
    text: Option<String>,
}

#[derive(Default)]
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
            disk::Probe::system(),
            forge_isolation::support().into(),
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
            disk::Probe::system(),
            forge_isolation::support().into(),
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
        disk: disk::Probe,
        sandbox: sandbox::Host,
        generation: Option<runtime::Generation>,
    ) -> Self {
        let extensions = Extensions::new(store.clone(), engines.clone(), data_dir.clone());
        let mcp = Mcp::new(store.clone());
        let hub = Self {
            inner: Arc::new(Inner {
                generation,
                store,
                engines,
                data_dir,
                harness_for,
                live: Mutex::new(HashMap::new()),
                unsettled: Mutex::new(HashMap::new()),
                owed_continues: Mutex::new(std::collections::HashSet::new()),
                suggestions: Mutex::new(HashMap::new()),
                attention: Mutex::new(HashMap::new()),
                untitled: Mutex::new(std::collections::HashSet::new()),
                title_queries: Mutex::new(std::collections::HashSet::new()),
                extensions,
                mcp,
                shutdown: CancellationToken::new(),
                tasks: TaskTracker::new(),
                harness_slots: Arc::new(Semaphore::new(RUN_CAPACITY)),
                dispatch: Notify::new(),
                admission: tokio::sync::Mutex::new(()),
                gateway: Mutex::new(gateway::Gateway::default()),
                agents_changed: tokio::sync::watch::channel(0).0,
                agent_waiters: Mutex::new(HashMap::new()),
                approvals: Mutex::new(HashMap::new()),
                disk,
                sandbox,
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
        if !created {
            return Ok(item);
        }
        self.dispatch_accepted(thread_id, &item, saved_engine)
            .await
            .map_err(|source| SendError::Steer {
                thread: thread_id.clone(),
                item: item.id.clone(),
                source: Box::new(source),
            })?;
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
        match live {
            Some((inbox, engine)) => {
                let input = self.user_input(item, engine).await?;
                if inbox.send(Input::Steer(input)).await.is_err() {
                    self.resume_pending();
                }
            }
            None => self.resume_pending(),
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
            .ok_or_else(|| {
                AnswerError::refused(
                    item_id,
                    format!("Item {item_id} is not an approval request."),
                )
            })?;
            if !approval.options.contains(decision) {
                return Err(AnswerError::refused(
                    item_id,
                    "This request does not offer that decision.",
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
            return Err(AnswerError::refused(
                item_id,
                format!("Item {item_id} is not waiting for an answer."),
            ));
        }
        let key =
            self.store()
                .native_id(item_id)
                .await?
                .ok_or_else(|| AnswerError::NoEngineId {
                    item: item_id.clone(),
                })?;
        let ended =
            || AnswerError::refused(item_id, format!("The run of item {item_id} has ended."));
        let inbox = inbox.ok_or_else(ended)?;
        if inbox.send(Input::Answer { key, answer }).await.is_err() {
            return Err(ended());
        }
        self.agent_attention(&item.thread_id).await;
        Ok(())
    }

    pub async fn continue_turn(&self, thread_id: &ThreadId) -> Result<(), ContinueError> {
        let busy = || ContinueError::Refused {
            thread: thread_id.clone(),
            refused: Refused("The thread is already working.".to_owned()),
        };
        let _admission = self.inner.admission.lock().await;
        let phase = self.lock().get_mut(thread_id).map(|run| {
            if let Phase::Stopping { then, .. } = &mut run.phase {
                *then = AfterStop::Continue;
            }
            run.phase
        });
        match phase {
            Some(Phase::Stopping { .. }) => return Ok(()),
            Some(Phase::Working) => return Err(busy()),
            None if self.is_live(thread_id) => return Err(busy()),
            None => {}
        }
        self.carry_on(thread_id).await?;
        self.owed_continues_lock().remove(thread_id);
        self.resume_pending();
        Ok(())
    }

    async fn fulfil_continue(&self, thread_id: &ThreadId) -> Continuing {
        if self.unsettled_lock().contains_key(thread_id) {
            self.owed_continues_lock().insert(thread_id.clone());
            return Continuing::Owed;
        }
        let refused = match self.carry_on(thread_id).await {
            Ok(()) => {
                self.owed_continues_lock().remove(thread_id);
                return Continuing::Continued;
            }
            Err(ContinueError::Refused { refused, .. }) => refused,
            Err(error) => {
                tracing::warn!(thread = %thread_id, "continue the turn once its stop wound down; retrying: {}", diagnostics::chain(&error));
                self.owed_continues_lock().insert(thread_id.clone());
                return Continuing::Owed;
            }
        };
        let report = forge_store::NewItem {
            id: ItemId::new(),
            thread_id: thread_id.clone(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::Error {
                kind: artisan_api::ErrorKind::Execution,
                message: format!("Continue could not start: {}", refused.0).into(),
                retryable: false,
            },
        };
        match self.store().upsert_item(report).await {
            Ok(_) => {
                self.owed_continues_lock().remove(thread_id);
                Continuing::Reported
            }
            Err(error) => {
                tracing::warn!(thread = %thread_id, "report that the continue could not start; retrying: {}", diagnostics::chain(&error));
                self.owed_continues_lock().insert(thread_id.clone());
                Continuing::Owed
            }
        }
    }

    async fn continue_owed(&self) -> bool {
        let owed: Vec<ThreadId> = self.owed_continues_lock().iter().cloned().collect();
        if owed.is_empty() {
            return true;
        }
        let _admission = self.inner.admission.lock().await;
        let mut done = true;
        for thread in owed {
            if self.lock().contains_key(&thread) {
                continue;
            }
            match self.fulfil_continue(&thread).await {
                Continuing::Continued | Continuing::Reported => {}
                Continuing::Owed => done = false,
            }
        }
        done
    }

    fn owed_continues_lock(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashSet<ThreadId>> {
        self.inner
            .owed_continues
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    async fn carry_on(&self, thread_id: &ThreadId) -> Result<(), ContinueError> {
        let stopped = self
            .store()
            .latest_run(thread_id)
            .await?
            .is_some_and(|run| run.status().is_stopped());
        if !stopped {
            return Err(ContinueError::Refused {
                thread: thread_id.clone(),
                refused: Refused("The thread has no stopped turn to continue.".to_owned()),
            });
        }
        if self
            .store()
            .restore_undelivered_inputs(thread_id.clone())
            .await?
            == 0
        {
            self.store().request_continue(thread_id.clone()).await?;
        }
        Ok(())
    }

    fn stopped_through(&self, thread_id: &ThreadId) -> Option<u64> {
        let phase = self.lock().get(thread_id)?.phase;
        match phase {
            Phase::Working => None,
            Phase::Stopping { through, .. } => Some(through),
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
            self.untitled_lock().remove(thread_id);
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
            .ok_or_else(|| ClearGoalError::refused(thread_id, "The thread has no goal."))?;
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
                return Err(ClearGoalError::refused(
                    thread_id,
                    "The run is ending; try again in a moment.",
                ));
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
        let session = self
            .store()
            .latest_run(thread_id)
            .await?
            .filter(|run| run.engine == engine)
            .and_then(|run| run.session_id)
            .ok_or_else(|| {
                ClearGoalError::refused(
                    thread_id,
                    "This engine never started this thread's session.",
                )
            })?;
        let lease = self.engines().lease(engine);
        let turn = run::prepare(
            self,
            thread,
            Some(session),
            forge_engines::UserInput::default(),
            lease.as_ref().map(|lease| lease.path().to_path_buf()),
        )
        .await
        .map_err(|source| ClearGoalError::Prepare {
            thread: thread_id.clone(),
            engine,
            source: Box::new(source),
        })?;
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
        let elsewhere = {
            let _admission = self.inner.admission.lock().await;
            let mut elsewhere = Vec::new();
            for thread in self.agent_tree(thread_id).await? {
                let route = self.route(&thread).map_err(|source| CancelError::Route {
                    thread: thread.clone(),
                    source,
                })?;
                match route {
                    Some(member) => elsewhere.push((thread, member)),
                    None => self.cancel_owned(&thread).await?,
                }
            }
            elsewhere
        };
        for (thread, member) in elsewhere {
            runtime::request(
                &member,
                &runtime::Command::Cancel {
                    thread: thread.clone(),
                },
            )
            .await
            .map_err(|source| CancelError::Peer {
                thread,
                source: Box::new(source),
            })?;
        }
        Ok(())
    }

    async fn cancel_local(&self, thread_id: &ThreadId) -> Result<(), CancelError> {
        let _admission = self.inner.admission.lock().await;
        self.cancel_owned(thread_id).await
    }

    async fn cancel_owned(&self, thread_id: &ThreadId) -> Result<(), CancelError> {
        let cutoff = self.store().head();
        if let Some(run) = self.lock().get_mut(thread_id) {
            run.phase = Phase::Stopping {
                through: cutoff,
                then: AfterStop::Settle,
            };
            run.cancel.cancel();
        }
        self.store()
            .cancel_pending_inputs(thread_id.clone(), cutoff)
            .await?;
        self.store().unpark(thread_id.clone()).await?;
        self.resume_pending();
        Ok(())
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
        let mut ids: std::collections::BTreeSet<_> = self.lock().keys().cloned().collect();
        ids.extend(self.unsettled_lock().keys().cloned());
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
        if self.lock().contains_key(thread) || self.unsettled_lock().contains_key(thread) {
            return Ok(None);
        }
        if let Some(owner) = generation.owner(thread) {
            return Ok(Some(owner));
        }
        if !generation.draining() {
            return Ok(None);
        }
        Ok(generation
            .registry
            .active()?
            .filter(|active| active.id != generation.member.id))
    }

    pub fn local_active_count(&self) -> usize {
        self.owned_thread_ids().len()
    }

    #[must_use]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "both locks are held so the snapshot matches its revision"
    )]
    fn live_snapshot(&self) -> (u64, Vec<Event>) {
        let live = self.lock();
        let suggestions = self.suggestions_lock();
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
        snapshot.extend(suggestions.iter().filter_map(|(thread_id, suggested)| {
            suggested.text.clone().map(|text| Event::Suggestion {
                thread_id: thread_id.clone(),
                text: Some(text),
            })
        }));
        snapshot.extend(self.pending_editor_calls());
        sort_snapshot(&mut snapshot);
        (self.inner.store.live_revision(), snapshot)
    }

    fn open_suggestion(&self, thread_id: &ThreadId, run: &RunId) {
        let cancel = self.inner.shutdown.child_token();
        let previous = self.suggestions_lock().insert(
            thread_id.clone(),
            Suggested {
                run: run.clone(),
                cancel,
                text: None,
            },
        );
        if let Some(previous) = previous {
            previous.cancel.cancel();
            if previous.text.is_some() {
                self.inner.store.publish_live(Event::Suggestion {
                    thread_id: thread_id.clone(),
                    text: None,
                });
            }
        }
    }

    fn suggestion_slot(&self, thread_id: &ThreadId, run: &RunId) -> Option<CancellationToken> {
        self.suggestions_lock()
            .get(thread_id)
            .filter(|suggested| &suggested.run == run && !suggested.cancel.is_cancelled())
            .map(|suggested| suggested.cancel.clone())
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "published under the lock, so a newer run's withdrawal follows it"
    )]
    fn offer_suggestion(&self, thread_id: &ThreadId, run: &RunId, text: String) {
        let mut suggestions = self.suggestions_lock();
        let Some(suggested) = suggestions
            .get_mut(thread_id)
            .filter(|suggested| &suggested.run == run && !suggested.cancel.is_cancelled())
        else {
            return;
        };
        suggested.text = Some(text.clone());
        self.inner.store.publish_live(Event::Suggestion {
            thread_id: thread_id.clone(),
            text: Some(text),
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ThreadId, LiveRun>> {
        self.inner
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn suggestions_lock(&self) -> std::sync::MutexGuard<'_, HashMap<ThreadId, Suggested>> {
        self.inner
            .suggestions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn attention_lock(&self) -> std::sync::MutexGuard<'_, HashMap<ThreadId, side::Attention>> {
        self.inner
            .attention
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn unsettled_lock(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<ThreadId, forge_store::Settlement>> {
        self.inner
            .unsettled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn keep_unsettled(&self, settlement: forge_store::Settlement) {
        tracing::error!(
            thread = %settlement.run.thread_id,
            run = %settlement.run.id,
            "the run's end could not be saved; retrying until it is"
        );
        self.unsettled_lock()
            .insert(settlement.run.thread_id.clone(), settlement);
        self.resume_pending();
    }

    async fn settle_unsettled(&self) -> bool {
        let waiting: Vec<forge_store::Settlement> =
            self.unsettled_lock().values().cloned().collect();
        let mut settled = true;
        for settlement in waiting {
            let thread = settlement.run.thread_id.clone();
            let run = settlement.run.id.clone();
            match self.store().settle_run(settlement).await {
                Ok(_) => {
                    self.unsettled_lock().remove(&thread);
                    tracing::info!(%thread, %run, "saved the run's end after an earlier failure");
                    self.agent_run_ended(&thread).await;
                    self.resume_pending();
                }
                Err(error) => {
                    settled = false;
                    tracing::warn!(%thread, %run, "saving the run's end failed again: {}", diagnostics::chain(&error));
                }
            }
        }
        settled
    }

    fn untitled_lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashSet<ThreadId>> {
        self.inner
            .untitled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn start_dispatcher(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let hub = self.clone();
        self.inner.tasks.spawn_on(
            async move {
                let mut stalled = false;
                let mut failures = 0_u32;
                loop {
                    let (parked, readable) = park::until_next(&hub, stalled).await;
                    let retry = async {
                        if failures == 0 && readable {
                            std::future::pending::<()>().await;
                        } else {
                            tokio::time::sleep(retry_delay(failures)).await;
                        }
                    };
                    tokio::select! {
                        biased;
                        () = hub.inner.shutdown.cancelled() => break,
                        () = hub.inner.dispatch.notified() => {},
                        () = parked => {},
                        () = retry => {},
                    }
                    let mut failed = !readable;
                    failed |= !hub.settle_unsettled().await;
                    failed |= !hub.continue_owed().await;
                    stalled = true;
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
                        let excluded = hub.active_thread_ids();
                        let foreign = excluded.len().saturating_sub(hub.local_active_count());
                        if hub.inner.harness_slots.available_permits() <= foreign {
                            break;
                        }
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
                        stalled = false;
                        hub.claim(thread, permit);
                    }
                    failures = if failed {
                        failures.saturating_add(1)
                    } else {
                        0
                    };
                }
            },
            &runtime,
        );
    }

    fn claim(&self, thread_id: ThreadId, permit: OwnedSemaphorePermit) {
        let (inbox, receiver) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let run_id = RunId::new();
        self.lock().insert(
            thread_id.clone(),
            LiveRun {
                id: run_id.clone(),
                inbox,
                cancel: cancel.clone(),
                engine: None,
                live: HashMap::new(),
                foreground_bytes: 0,
                terminal_bytes: 0,
                phase: Phase::Working,
            },
        );
        let hub = self.clone();
        let span = tracing::info_span!("run", thread = %thread_id, run = %run_id);
        let run = async move {
            let outcome = Box::pin(run::drive(
                &hub,
                &thread_id,
                run_id.clone(),
                receiver,
                cancel.clone(),
            ))
            .await;
            if let Err(error) = &outcome {
                tracing::error!(thread = %thread_id, run = %run_id, "run failed: {}", diagnostics::chain(&error));
            }
            let continued = hub.end_live(&thread_id).await;
            drop(permit);
            if (outcome.is_ok() || continued) && !hub.inner.shutdown.is_cancelled() {
                hub.resume_pending();
            }
            if !hub.inner.shutdown.is_cancelled() {
                hub.agent_run_ended(&thread_id).await;
            }
            if matches!(outcome, Ok(artisan_api::RunStatus::Done))
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

    async fn end_live(&self, thread_id: &ThreadId) -> bool {
        let _admission = self.inner.admission.lock().await;
        let phase = self.lock().get(thread_id).map(|run| run.phase);
        let continued = match phase {
            Some(Phase::Stopping {
                then: AfterStop::Continue,
                ..
            }) => match self.fulfil_continue(thread_id).await {
                Continuing::Continued => true,
                Continuing::Reported => false,
                Continuing::Owed => {
                    self.resume_pending();
                    false
                }
            },
            Some(Phase::Working | Phase::Stopping { .. }) | None => false,
        };
        self.lock().remove(thread_id);
        continued
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
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut statuses = self.inner.engines.subscribe();
        let engines = self.inner.engines.clone();
        let store = self.inner.store.clone();
        let stop = self.inner.shutdown.clone();
        self.inner.tasks.spawn_on(
            async move {
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
            },
            &runtime,
        );
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

fn snapshot_key(event: &Event) -> (Option<&ThreadId>, Option<&LiveTarget>) {
    match event {
        Event::Live {
            thread_id, target, ..
        } => (Some(thread_id), Some(target)),
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
        | Event::EditorCall { .. }
        | Event::Reset => (event.thread_id(), None),
    }
}
