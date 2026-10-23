//! Live runs.
//!
//! [`Hub`] starts engine runs, delivers steers and answers to them, and turns
//! what adapters report into stored items. Reasoning, shell output and
//! streaming text stay in memory here and are published as live events.
//!
//! A user message is stored `running` and becomes `done` once an engine has
//! it. Whatever is still `running` when a run ends starts the next run, so a
//! message sent while a run winds down is never lost.

mod agents;
mod approvals;
mod disk;
mod extensions;
mod gateway;
mod handoff;
mod mcp_auth;
mod park;
mod run;
mod scratch;
mod side;

pub use agents::{AgentInfo, AgentState, MAX_AGENT_DEPTH, SpawnAgent};
pub use extensions::{DISABLED_SKILLS_KEY, GLOBAL_PROMPT_KEY};
pub use forge_store::Refused;
pub use gateway::COMPUTER_USE_KEY;
pub use side::{SUGGESTIONS_KEY, SUMMARY_TITLES_KEY, YOU_SHOULD_KNOW_KEY};

use anyhow::{Context, Result, bail};
use artisan_api::{
    Answer, EngineId, Event, Item, ItemId, ItemKind, LiveTarget, RunId, SendMessage, Status,
    ThreadId,
};
use forge_engines::{Engines, Harness, Input};
use forge_store::Store;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Picks the adapter for an engine. Tests swap in a scripted one.
pub type HarnessFor = Arc<dyn Fn(EngineId) -> Arc<dyn Harness> + Send + Sync>;

#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

struct Inner {
    store: Store,
    engines: Engines,
    data_dir: PathBuf,
    harness_for: HarnessFor,
    live: Mutex<HashMap<ThreadId, LiveRun>>,
    suggestions: Mutex<HashMap<ThreadId, Suggested>>,
    /// How much each thread has done since its last "You should know" check.
    attention: Mutex<HashMap<ThreadId, side::Attention>>,
    /// Threads whose first run brought no title from the engine; the side
    /// query after their next finished turn names them.
    untitled: Mutex<std::collections::HashSet<ThreadId>>,
    /// Held while skills or the prompt are written into engine homes.
    homes: Mutex<()>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    harness_slots: Arc<Semaphore>,
    dispatch: Notify,
    admission: tokio::sync::Mutex<()>,
    /// Where engines reach the Artisan MCP, and each thread's credential.
    gateway: Mutex<gateway::Gateway>,
    /// Bumped whenever an agent's state may have changed; waiters watch it.
    agents_changed: tokio::sync::watch::Sender<u64>,
    /// How many `agent_wait` calls are waiting on each agent item now.
    agent_waiters: Mutex<HashMap<ItemId, usize>>,
    /// Approval cards Forge showed itself, by item.
    approvals: Mutex<HashMap<ItemId, approvals::OwnApproval>>,
    /// How runs read the free space where they work.
    disk: disk::Probe,
    /// Where the Marketplace searches skills.sh.
    skills_sh: Mutex<String>,
    /// MCP sign-ins waiting for their redirect.
    mcp_flows: Mutex<mcp_auth::Flows>,
    /// Serialises changes to the `mcp_auth` setting.
    mcp_auth_writes: tokio::sync::Mutex<()>,
}

/// One live run's handles and memory-only text.
struct LiveRun {
    inbox: mpsc::Sender<Input>,
    cancel: CancellationToken,
    engine: Option<EngineId>,
    cancel_through: Arc<AtomicU64>,
    /// Streaming text, reasoning, shell output and progress, by target.
    live: HashMap<LiveTarget, LivePreview>,
    live_bytes: usize,
}

/// The guess at a thread's next message, kept until the thread runs again.
struct Suggested {
    /// The run after which it is asked.
    run: RunId,
    /// Stops the side query asking for it once the thread moves on.
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
        Self::build(store, engines, data_dir, harness_for, disk::Probe::system())
    }

    fn build(
        store: Store,
        engines: Engines,
        data_dir: PathBuf,
        harness_for: HarnessFor,
        disk: disk::Probe,
    ) -> Self {
        let hub = Self {
            inner: Arc::new(Inner {
                store,
                engines,
                data_dir,
                harness_for,
                live: Mutex::new(HashMap::new()),
                suggestions: Mutex::new(HashMap::new()),
                attention: Mutex::new(HashMap::new()),
                untitled: Mutex::new(std::collections::HashSet::new()),
                homes: Mutex::new(()),
                shutdown: CancellationToken::new(),
                tasks: TaskTracker::new(),
                harness_slots: Arc::new(Semaphore::new(8)),
                dispatch: Notify::new(),
                admission: tokio::sync::Mutex::new(()),
                gateway: Mutex::new(gateway::Gateway::default()),
                agents_changed: tokio::sync::watch::channel(0).0,
                agent_waiters: Mutex::new(HashMap::new()),
                approvals: Mutex::new(HashMap::new()),
                disk,
                skills_sh: Mutex::new(forge_engines::SKILLS_SH_URL.to_owned()),
                mcp_flows: Mutex::new(mcp_auth::Flows::default()),
                mcp_auth_writes: tokio::sync::Mutex::new(()),
            }),
        };
        hub.forward_engine_status();
        hub.start_dispatcher();
        hub
    }

    #[must_use]
    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    /// Points the Marketplace's skills.sh search elsewhere: a test's stub.
    pub fn set_skills_sh_url(&self, url: impl Into<String>) {
        *self
            .inner
            .skills_sh
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = url.into();
    }

    pub(crate) fn skills_sh_url(&self) -> String {
        self.inner
            .skills_sh
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn engines(&self) -> &Engines {
        &self.inner.engines
    }

    #[must_use]
    pub fn data_dir(&self) -> &std::path::Path {
        &self.inner.data_dir
    }

    /// Starts runs for threads with undelivered messages, including ones a
    /// previous Forge left.
    pub fn resume_pending(&self) {
        self.inner.dispatch.notify_one();
    }

    /// Stores the message, then starts a run or steers the live one.
    pub async fn send_message(&self, thread_id: &ThreadId, message: SendMessage) -> Result<Item> {
        let store = &self.inner.store;
        if message.text.trim_start().starts_with('/') {
            let thread = store
                .thread(thread_id)
                .await?
                .ok_or_else(|| forge_store::SendRejected::ThreadNotFound(thread_id.clone()))?;
            let requested = message
                .config
                .as_ref()
                .map_or(thread.config.engine, |config| config.engine);
            let engine = self
                .lock()
                .get(thread_id)
                .and_then(|run| run.engine)
                .unwrap_or(requested);
            // A command the engine can't read fails the send, not the run.
            forge_engines::normalize_command(engine, &message.text)
                .map_err(|error| forge_store::SendRejected::Payload(error.to_string()))?;
        }
        // The first line names an untitled thread; the config a message
        // runs with becomes the default for new threads.
        let acceptance = forge_store::Acceptance {
            title: Some(
                message
                    .text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .chars()
                    .take(80)
                    .collect::<String>(),
            )
            .filter(|title| !title.is_empty()),
            remember_config: true,
            from_agent: None,
            sender: None,
        };
        self.accept_message(thread_id, message, acceptance).await
    }

    /// Stores an already checked message, then starts a run or steers the
    /// live one.
    async fn accept_message(
        &self,
        thread_id: &ThreadId,
        message: SendMessage,
        acceptance: forge_store::Acceptance,
    ) -> Result<Item> {
        let steer = self.is_live(thread_id);
        let (item, created, saved_engine) = self
            .inner
            .store
            .send_message(thread_id.clone(), message, steer, acceptance)
            .await?;
        if !created {
            return Ok(item);
        }
        let live = self
            .lock()
            .get(thread_id)
            .map(|run| (run.inbox.clone(), run.engine.unwrap_or(saved_engine)));
        match live {
            // A closed inbox means the run is ending; it starts the next run
            // with this message.
            Some((inbox, engine)) => {
                let input = self.user_input(&item, engine).await?;
                inbox.send(Input::Steer(input)).await.ok();
            }
            None => self.resume_pending(),
        }
        Ok(item)
    }

    /// Answers the approval or question on an item of a live run. What the
    /// item's state turns down is [`Refused`]; anything else is Forge's fault.
    pub async fn answer(&self, item_id: &ItemId, answer: Answer) -> Result<()> {
        let refused = |message: String| anyhow::Error::new(Refused(message));
        let item = self
            .inner
            .store
            .item(item_id)
            .await?
            .ok_or_else(|| forge_store::ItemNotFound(item_id.clone()))?;
        if let Answer::Question { answers } = &answer {
            self.inner
                .store
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
            .ok_or_else(|| refused(format!("Item {item_id} is not an approval request.")))?;
            if !approval.options.contains(decision) {
                return Err(refused(
                    "This request does not offer that decision.".to_owned(),
                ));
            }
            if self.answer_own_approval(item_id, *decision) {
                return Ok(());
            }
        }
        let inbox = self
            .lock()
            .get(&item.thread_id)
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
            return Err(refused(format!(
                "Item {item_id} is not waiting for an answer."
            )));
        }
        let key = self
            .inner
            .store
            .native_id(item_id)
            .await?
            .with_context(|| format!("item {item_id} has no engine id"))?;
        let ended = || refused(format!("The run of item {item_id} has ended."));
        let inbox = inbox.ok_or_else(ended)?;
        inbox
            .send(Input::Answer { key, answer })
            .await
            .map_err(|_unsent| ended())?;
        self.agent_attention(&item.thread_id).await;
        Ok(())
    }

    /// Carries on the thread's turn that stopped before it finished (the
    /// user stopped it, or Forge restarted), with no new message. Messages
    /// the stop caught before the engine read them are sent instead.
    pub async fn continue_turn(&self, thread_id: &ThreadId) -> Result<()> {
        let refused = |message: &str| anyhow::Error::new(Refused(message.to_owned()));
        let _admission = self.inner.admission.lock().await;
        if self.is_live(thread_id) {
            return Err(refused("The thread is already working."));
        }
        let stopped = self
            .store()
            .latest_run(thread_id)
            .await?
            .is_some_and(|run| run.status.is_stopped());
        if !stopped {
            return Err(refused("The thread has no stopped turn to continue."));
        }
        if self
            .store()
            .restore_undelivered_inputs(thread_id.clone())
            .await?
            == 0
        {
            self.store().request_continue(thread_id.clone()).await?;
        }
        self.resume_pending();
        Ok(())
    }

    /// Saves a thread's title, archive state, settings or read mark. A
    /// change of access reaches its live run at once: Full approves what
    /// waits and the engine stops asking.
    pub async fn update_thread(
        &self,
        thread_id: &ThreadId,
        update: artisan_api::UpdateThread,
    ) -> Result<Option<artisan_api::Thread>> {
        let before = self
            .store()
            .thread(thread_id)
            .await?
            .map(|thread| thread.config.permissions);
        // A title the user gave wins over one a side query would generate.
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

    /// Clears the thread's goal, so the engine stops working toward it.
    /// Codex clears through its app-server, on the live run's connection
    /// or on its own; Claude takes `/goal clear` as a message.
    pub async fn clear_goal(&self, thread_id: &ThreadId) -> Result<()> {
        let refused = |message: &str| anyhow::Error::new(Refused(message.to_owned()));
        let store = self.store();
        let thread = store
            .thread(thread_id)
            .await?
            .ok_or_else(|| forge_store::SendRejected::ThreadNotFound(thread_id.clone()))?;
        let goal = store
            .inspector(thread_id)
            .await?
            .goal
            .ok_or_else(|| refused("The thread has no goal."))?;
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
                self.send_message(thread_id, message).await?;
                // Claude's own reply reports the goal cleared.
                return Ok(());
            }
            EngineId::Codex => {}
            EngineId::OpenCode => {
                return Err(refused("This engine can't clear its goal from Artisan."));
            }
        }
        // A closed inbox means the run is ending; Codex keeps the goal
        // and the next turn shows it again.
        if let Some((inbox, _)) = live {
            inbox
                .send(Input::ClearGoal)
                .await
                .map_err(|_unsent| refused("The run is ending; try again in a moment."))?;
        } else {
            let session = store
                .latest_run(thread_id)
                .await?
                .filter(|run| run.engine == engine)
                .and_then(|run| run.session_id)
                .ok_or_else(|| refused("Codex never started this thread's session."))?;
            let lease = self.engines().lease(engine);
            let turn = run::prepare(
                self,
                &thread,
                Some(session),
                forge_engines::UserInput::default(),
                lease.as_ref().map(|lease| lease.path().to_path_buf()),
            )
            .await?;
            (self.inner.harness_for)(engine)
                .clear_goal(turn)
                .await
                .with_context(|| format!("clearing the goal of thread {thread_id}"))?;
            drop(lease);
        }
        // Codex reports a clear only to a session that showed the goal, so
        // the strip learns it here.
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

    /// Cancels the thread's live run, if any, and the agents it started.
    pub async fn cancel(&self, thread_id: &ThreadId) -> Result<()> {
        self.cancel_one(thread_id).await?;
        self.cancel_agents_of(thread_id).await
    }

    async fn cancel_one(&self, thread_id: &ThreadId) -> Result<()> {
        let _admission = self.inner.admission.lock().await;
        let cutoff = self.store().head();
        if let Some(run) = self.lock().get(thread_id) {
            run.cancel_through.store(cutoff, Ordering::Release);
            run.cancel.cancel();
        }
        self.store()
            .cancel_pending_inputs(thread_id.clone(), cutoff)
            .await?;
        // A parked turn is stopped too: it isn't picked up again.
        self.store().unpark(thread_id.clone()).await?;
        self.resume_pending();
        Ok(())
    }

    /// Cancels startup-owned work if setup fails or its future is dropped.
    /// Disarm only after ownership transfers to the serving controller.
    pub fn shutdown_on_drop(&self) -> tokio_util::sync::DropGuard {
        self.inner.shutdown.clone().drop_guard()
    }

    /// Stops all work owned by this controller without touching another Forge.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
        self.inner.tasks.close();
    }

    /// Stops every run and waits up to `grace` for them to settle, so they
    /// end interrupted and carry on after the restart.
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
    }

    /// The current memory-only text of every live run, and every thread's
    /// suggested next message, for a client that just connected.
    #[must_use]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "both locks are held so the snapshot matches its revision"
    )]
    pub fn live_snapshot(&self) -> (u64, Vec<Event>) {
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
        (self.inner.store.live_revision(), snapshot)
    }

    /// Opens the slot for the guess at the thread's next message after
    /// `run`. The thread's previous guess is withdrawn, and a side query
    /// still working on one stops.
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

    /// What stops the side queries after `run` once the thread moves on;
    /// `None` if it already has.
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
        // Published under the lock, so a newer run's withdrawal follows it.
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

    fn untitled_lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashSet<ThreadId>> {
        self.inner
            .untitled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// One dispatcher claims only threads that can start immediately, and
    /// wakes by itself when the earliest parked turn is due.
    fn start_dispatcher(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let hub = self.clone();
        self.inner.tasks.spawn_on(
            async move {
                loop {
                    let parked = park::until_next(&hub).await;
                    tokio::select! {
                        biased;
                        () = hub.inner.shutdown.cancelled() => break,
                        () = hub.inner.dispatch.notified() => {},
                        () = parked => {},
                    }
                    loop {
                        let _admission = hub.inner.admission.lock().await;
                        if hub.inner.shutdown.is_cancelled() {
                            return;
                        }
                        let Ok(permit) = Arc::clone(&hub.inner.harness_slots).try_acquire_owned()
                        else {
                            break;
                        };
                        let excluded = hub.lock().keys().cloned().collect();
                        let next = match hub.store().pending_threads(1, excluded).await {
                            Ok(threads) => threads.into_iter().next(),
                            Err(error) => {
                                tracing::error!("read pending run candidates: {error:#}");
                                break;
                            }
                        };
                        let Some(thread) = next else {
                            break;
                        };
                        hub.claim(thread, permit);
                    }
                }
            },
            &runtime,
        );
    }

    fn claim(&self, thread_id: ThreadId, permit: OwnedSemaphorePermit) {
        let (inbox, receiver) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let cancel_through = Arc::new(AtomicU64::new(0));
        let run_id = RunId::new();
        self.lock().insert(
            thread_id.clone(),
            LiveRun {
                inbox,
                cancel: cancel.clone(),
                engine: None,
                cancel_through: Arc::clone(&cancel_through),
                live: HashMap::new(),
                live_bytes: 0,
            },
        );
        let hub = self.clone();
        self.inner.tasks.spawn(async move {
            let outcome =
                run::drive(&hub, &thread_id, run_id.clone(), receiver, cancel.clone()).await;
            if let Err(error) = &outcome {
                tracing::error!(thread = %thread_id, run = %run_id, "run failed: {error:#}");
            }
            hub.lock().remove(&thread_id);
            drop(permit);
            if outcome.is_ok() && !hub.inner.shutdown.is_cancelled() {
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
        });
    }

    /// Serialises writes into engine homes (launches, the catalog, the
    /// prompt).
    fn homes_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.inner
            .homes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// `<data>/extensions/skills`, where Forge keeps the skills it installs.
    fn skills_dir(&self) -> PathBuf {
        self.inner.data_dir.join("extensions").join("skills")
    }

    /// A user message as `engine` receives it: images as files, and skill
    /// mentions in the engine's own form.
    async fn user_input(&self, item: &Item, engine: EngineId) -> Result<forge_engines::UserInput> {
        let ItemKind::UserMessage {
            text,
            images,
            sender,
            ..
        } = &item.kind
        else {
            bail!("input {} is not a user message", item.id);
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
            // An agent's own parent briefs it: its words go as they are.
            Some(sender) if parent.as_ref() == Some(&sender.thread) => text,
            // Another thread's agent wrote it: the engine is told so, and a
            // leading slash is text, never one of the user's commands.
            Some(sender) => format!("{}\n\n{text}", sender_line(sender)),
            // Normalize each native prompt before pending inputs/context are joined.
            None => forge_engines::normalize_command(engine, &text)?.unwrap_or(text),
        };
        let text = if text.contains('$') {
            let skills = match self.enabled_skills().await {
                Ok(skills) => skills,
                Err(error) => {
                    // The message still goes; only the closing line naming
                    // the mentioned skills is left off.
                    tracing::warn!(
                        thread = %item.thread_id,
                        item = %item.id,
                        "listing the skills for the message's mentions: {error:#}"
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
                    // The message still goes; only the closing line naming
                    // the mentioned threads is left off.
                    tracing::warn!(
                        thread = %item.thread_id,
                        item = %item.id,
                        "listing the threads for the message's mentions: {error:#}"
                    );
                    text
                }
            }
        } else {
            text
        };
        Ok(forge_engines::UserInput {
            id: item.id.clone(),
            text,
            images: self.store().input_image_paths(images.clone()).await?,
        })
    }

    /// Records live text and publishes it.
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
        const TARGET_BYTES: usize = 256 * 1024;
        const RUN_BYTES: usize = 2 * 1024 * 1024;
        const TARGETS: usize = 128;
        let mut live = self.lock();
        let Some(run) = live.get_mut(thread_id) else {
            return;
        };
        if !run.live.contains_key(&target) && run.live.len() >= TARGETS {
            return;
        }
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
        let limit =
            TARGET_BYTES.min(RUN_BYTES.saturating_sub(run.live_bytes.saturating_sub(before)));
        if append && before + text.len() <= limit {
            buffer.push_str(&text);
        } else if append
            && matches!(
                target,
                LiveTarget::Shell { .. } | LiveTarget::Progress { .. }
            )
        {
            // Terminal previews retain their latest bytes. Publish one replacement,
            // so the client drops the same prefix instead of accumulating it forever.
            if text.len() >= limit {
                let mut start = text.len().saturating_sub(limit);
                while !text.is_char_boundary(start) {
                    start += 1;
                }
                buffer.clear();
                buffer.push_str(&text[start..]);
            } else {
                let mut discard = (before + text.len()).saturating_sub(limit);
                while !buffer.is_char_boundary(discard) {
                    discard += 1;
                }
                buffer.drain(..discard);
                buffer.push_str(&text);
            }
            preview.coverage.start = preview.coverage.total.saturating_sub(buffer.len() as u64);
            text.clone_from(buffer);
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
        run.live_bytes = run.live_bytes.saturating_sub(before) + buffer.len();
        if text.capacity() > TARGET_BYTES * 2 {
            text.shrink_to_fit();
        }
        self.inner.store.publish_live(Event::Live {
            thread_id: thread_id.clone(),
            target,
            text,
            append,
            coverage: preview.coverage,
        });
    }

    /// Empties memory-only text that clients still show.
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

    /// Drops memory-only text for a target that ended.
    fn forget_live(&self, thread_id: &ThreadId, target: &LiveTarget) {
        if let Some(run) = self.lock().get_mut(thread_id)
            && let Some(text) = run.live.remove(target)
        {
            run.live_bytes = run.live_bytes.saturating_sub(text.text.len());
        }
    }

    fn forward_engine_status(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut statuses = self.inner.engines.subscribe();
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
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            },
            &runtime,
        );
    }
}

#[cfg(test)]
mod tests;

/// The line an engine reads above a message another thread's agent wrote.
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
