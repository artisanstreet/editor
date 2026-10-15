//! Live runs.
//!
//! [`Hub`] starts engine runs, delivers steers and answers to them, and turns
//! what adapters report into stored items. Reasoning, shell output and
//! streaming text stay in memory here and are published as live events.
//!
//! A user message is stored `running` and becomes `done` once an engine has
//! it. Whatever is still `running` when a run ends starts the next run, so a
//! message sent while a run winds down is never lost.

mod extensions;
mod run;

pub use extensions::{DISABLED_SKILLS_KEY, GLOBAL_PROMPT_KEY};
pub use forge_store::Refused;

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
    /// Held while skills or the prompt are written into engine homes.
    homes: Mutex<()>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    harness_slots: Arc<Semaphore>,
    dispatch: Notify,
    admission: tokio::sync::Mutex<()>,
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
        let hub = Self {
            inner: Arc::new(Inner {
                store,
                engines,
                data_dir,
                harness_for,
                live: Mutex::new(HashMap::new()),
                homes: Mutex::new(()),
                shutdown: CancellationToken::new(),
                tasks: TaskTracker::new(),
                harness_slots: Arc::new(Semaphore::new(8)),
                dispatch: Notify::new(),
                admission: tokio::sync::Mutex::new(()),
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
        let steer = self.is_live(thread_id);
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
            if let Some(forge_engines::PromptCommand::Configure {
                option,
                label,
                enabled,
            }) = forge_engines::normalize_command(engine, &message.text)
                .map_err(|error| forge_store::SendRejected::Payload(error.to_string()))?
            {
                anyhow::ensure!(
                    engine == requested,
                    forge_store::SendRejected::Payload(
                        "A setting command cannot switch the active engine".to_owned()
                    )
                );
                anyhow::ensure!(
                    message.images.is_empty(),
                    forge_store::SendRejected::Payload(format!(
                        "{label} setting commands cannot include attachments"
                    ))
                );
                let prefix = format!("{}\n\n{label} ", message.text);
                let notice = prefix.clone();
                return store
                    .configure_thread(thread_id.clone(), message, prefix, true, move |config| {
                        let current = config
                            .session_options
                            .get(option)
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        let enabled = enabled.unwrap_or(!current);
                        config
                            .session_options
                            .insert(option.to_owned(), serde_json::json!(enabled));
                        let state = if enabled {
                            "requested on"
                        } else {
                            "requested off"
                        };
                        let when = if steer {
                            "Applies to the next run; the current run is unchanged."
                        } else {
                            "Applies to the next message."
                        };
                        format!("{notice}{state}. {when} Effort is unchanged.")
                    })
                    .await;
            }
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
        };
        let (item, created, saved_engine) = store
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
                let _ = inbox.send(Input::Steer(input)).await;
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
                ItemKind::Approval { approval, .. } => Some(approval),
                ItemKind::Tool { approval, .. } | ItemKind::Plan { approval, .. } => {
                    approval.as_ref()
                }
                _ => None,
            }
            .ok_or_else(|| refused(format!("Item {item_id} is not an approval request.")))?;
            if !approval.options.contains(decision) {
                return Err(refused(
                    "This request does not offer that decision.".to_owned(),
                ));
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
        let key = item
            .native_id
            .clone()
            .with_context(|| format!("item {item_id} has no engine id"))?;
        let ended = || refused(format!("The run of item {item_id} has ended."));
        let inbox = inbox.ok_or_else(ended)?;
        inbox
            .send(Input::Answer { key, answer })
            .await
            .map_err(|_| ended())?;
        Ok(())
    }

    /// Cancels the thread's live run, if any.
    pub async fn cancel(&self, thread_id: &ThreadId) -> Result<()> {
        let _admission = self.inner.admission.lock().await;
        let cutoff = self.store().head();
        if let Some(run) = self.lock().get(thread_id) {
            run.cancel_through.store(cutoff, Ordering::Release);
            run.cancel.cancel();
        }
        self.store()
            .cancel_pending_inputs(thread_id.clone(), cutoff)
            .await?;
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

    #[must_use]
    pub fn is_live(&self, thread_id: &ThreadId) -> bool {
        self.lock().contains_key(thread_id)
    }

    /// The current memory-only text of every live run, for a client that
    /// just connected.
    #[must_use]
    pub fn live_snapshot(&self) -> (u64, Vec<Event>) {
        let live = self.lock();
        let snapshot = live
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
        (self.inner.store.live_revision(), snapshot)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ThreadId, LiveRun>> {
        self.inner
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// One dispatcher claims only threads that can start immediately.
    fn start_dispatcher(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let hub = self.clone();
        self.inner.tasks.spawn_on(
            async move {
                loop {
                    tokio::select! {
                        biased;
                        () = hub.inner.shutdown.cancelled() => break,
                        () = hub.inner.dispatch.notified() => {},
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
            let _permit = permit;
            let outcome =
                run::drive(&hub, &thread_id, run_id.clone(), receiver, cancel.clone()).await;
            if let Err(error) = &outcome {
                tracing::error!(thread = %thread_id, run = %run_id, "run failed: {error:#}");
            }
            hub.lock().remove(&thread_id);
            drop(_permit);
            if outcome.is_ok() && !hub.inner.shutdown.is_cancelled() {
                hub.resume_pending();
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
        let ItemKind::UserMessage { text, images, .. } = &item.kind else {
            bail!("input {} is not a user message", item.id);
        };
        let text = self.store().resolve_text(text).await?;
        // Normalize each native prompt before pending inputs/context are joined.
        let text = match forge_engines::normalize_command(engine, &text)? {
            Some(forge_engines::PromptCommand::Prompt { text }) => text,
            Some(forge_engines::PromptCommand::Configure { .. }) => {
                bail!("a configuration command cannot be a pending model input")
            }
            None => text,
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
        Ok(forge_engines::UserInput {
            id: item.id.clone(),
            text,
            images: self.store().input_image_paths(images.clone()).await?,
        })
    }

    /// Records live text and publishes it.
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
                            store.publish_live(Event::Engine { status })
                        }
                        Ok(forge_engines::EngineChange::Install { engine, install }) => {
                            store.publish_live(Event::EngineInstall { engine, install })
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
