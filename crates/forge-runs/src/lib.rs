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

pub use extensions::{DISABLED_SKILLS_KEY, GLOBAL_PROMPT_KEY, Refused};

use anyhow::{Context, Result, bail};
use artisan_api::{
    Answer, EngineId, Event, Item, ItemId, ItemKind, LiveTarget, RunId, SendMessage, Status,
    ThreadId, UpdateThread,
};
use forge_engines::{Engines, Harness, Input};
use forge_store::{NewItem, Store};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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
}

/// One live run's handles and memory-only text.
struct LiveRun {
    inbox: mpsc::Sender<Input>,
    cancel: CancellationToken,
    /// Streaming text, reasoning, shell output and progress, by target.
    live: HashMap<LiveTarget, String>,
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
            }),
        };
        hub.forward_engine_status();
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

    /// Starts runs for messages left undelivered by a previous Forge.
    pub async fn resume_pending(&self) -> Result<()> {
        let threads = self.inner.store.state().await?.threads;
        for thread in threads {
            if !self
                .inner
                .store
                .undelivered_messages(&thread.id)
                .await?
                .is_empty()
            {
                self.start_if_idle(&thread.id);
            }
        }
        Ok(())
    }

    /// Stores the message, then starts a run or steers the live one.
    pub async fn send_message(&self, thread_id: &ThreadId, message: SendMessage) -> Result<Item> {
        let store = &self.inner.store;
        let thread = store
            .thread(thread_id)
            .await?
            .with_context(|| format!("no thread {thread_id}"))?;
        if let Some(config) = message.config.clone()
            && config != thread.config
        {
            store
                .update_thread(
                    thread_id,
                    UpdateThread {
                        config: Some(config),
                        ..UpdateThread::default()
                    },
                )
                .await?;
        }
        // A steer goes to the run the thread's current engine is running.
        let engine = thread.config.engine;
        let steer = self.is_live(thread_id);
        let (item, created) = store
            .insert_user_message(NewItem {
                id: message.id.clone(),
                thread_id: thread_id.clone(),
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::Running,
                kind: ItemKind::UserMessage {
                    text: message.text.clone(),
                    images: message.images.clone(),
                    steer,
                },
            })
            .await?;
        if !created {
            return Ok(item);
        }
        if thread.title.is_none() {
            let title = fallback_title(&message.text);
            if !title.is_empty() {
                store
                    .update_thread(
                        thread_id,
                        UpdateThread {
                            title: Some(title),
                            ..UpdateThread::default()
                        },
                    )
                    .await?;
            }
        }
        let inbox = self.lock().get(thread_id).map(|run| run.inbox.clone());
        match inbox {
            // A closed inbox means the run is ending; it starts the next run
            // with this message.
            Some(inbox) => {
                let input = self.user_input(&item, engine).await;
                let _ = inbox.send(Input::Steer(input)).await;
            }
            None => self.start_if_idle(thread_id),
        }
        Ok(item)
    }

    /// Answers the approval or question on an item of a live run.
    pub async fn answer(&self, item_id: &ItemId, answer: Answer) -> Result<()> {
        let item = self
            .inner
            .store
            .item(item_id)
            .await?
            .with_context(|| format!("no item {item_id}"))?;
        if item.status != Status::AwaitingUser {
            bail!("item {item_id} is not waiting for an answer");
        }
        let key = item
            .native_id
            .clone()
            .with_context(|| format!("item {item_id} has no engine id"))?;
        let inbox = self
            .lock()
            .get(&item.thread_id)
            .map(|run| run.inbox.clone());
        let inbox = inbox.with_context(|| format!("the run of item {item_id} has ended"))?;
        inbox
            .send(Input::Answer { key, answer })
            .await
            .ok()
            .context("the run has ended")?;
        Ok(())
    }

    /// Cancels the thread's live run, if any.
    pub fn cancel(&self, thread_id: &ThreadId) {
        if let Some(run) = self.lock().get(thread_id) {
            run.cancel.cancel();
        }
    }

    #[must_use]
    pub fn is_live(&self, thread_id: &ThreadId) -> bool {
        self.lock().contains_key(thread_id)
    }

    /// The current memory-only text of every live run, for a client that
    /// just connected.
    #[must_use]
    pub fn live_snapshot(&self) -> Vec<Event> {
        self.lock()
            .iter()
            .flat_map(|(thread_id, run)| {
                run.live.iter().map(|(target, text)| Event::Live {
                    thread_id: thread_id.clone(),
                    target: target.clone(),
                    text: text.clone(),
                    append: false,
                })
            })
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ThreadId, LiveRun>> {
        self.inner
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Claims the thread for a new run unless one is live, then starts it.
    fn start_if_idle(&self, thread_id: &ThreadId) {
        let (inbox, receiver) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let run_id = RunId::new();
        {
            let mut live = self.lock();
            if live.contains_key(thread_id) {
                return;
            }
            live.insert(
                thread_id.clone(),
                LiveRun {
                    inbox,
                    cancel: cancel.clone(),
                    live: HashMap::new(),
                },
            );
        }
        let hub = self.clone();
        let thread_id = thread_id.clone();
        tokio::spawn(async move {
            if let Err(error) = run::drive(&hub, &thread_id, run_id.clone(), receiver, cancel).await
            {
                tracing::error!(thread = %thread_id, run = %run_id, "run failed: {error:#}");
            }
            hub.lock().remove(&thread_id);
            match hub.inner.store.undelivered_messages(&thread_id).await {
                Ok(pending) if !pending.is_empty() => hub.start_if_idle(&thread_id),
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(thread = %thread_id, "read undelivered messages: {error:#}")
                }
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
    async fn user_input(&self, item: &Item, engine: EngineId) -> forge_engines::UserInput {
        let ItemKind::UserMessage { text, images, .. } = &item.kind else {
            return forge_engines::UserInput {
                id: item.id.clone(),
                ..Default::default()
            };
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
            forge_engines::with_skill_mentions(engine, text, &skills)
        } else {
            text.clone()
        };
        forge_engines::UserInput {
            id: item.id.clone(),
            text,
            images: images
                .iter()
                .filter_map(|hash| self.inner.store.blob_path(hash))
                .collect(),
        }
    }

    /// Records live text and publishes it.
    fn publish_live(&self, thread_id: &ThreadId, target: LiveTarget, text: String, append: bool) {
        if let Some(run) = self.lock().get_mut(thread_id) {
            let buffer = run.live.entry(target.clone()).or_default();
            if append {
                buffer.push_str(&text);
            } else {
                buffer.clone_from(&text);
            }
        }
        self.inner.store.publish_live(Event::Live {
            thread_id: thread_id.clone(),
            target,
            text,
            append,
        });
    }

    /// Empties memory-only text that clients still show.
    fn clear_live(&self, thread_id: &ThreadId, target: LiveTarget) {
        let shown = self
            .lock()
            .get(thread_id)
            .and_then(|run| run.live.get(&target))
            .is_some_and(|text| !text.is_empty());
        if shown {
            self.publish_live(thread_id, target, String::new(), false);
        }
    }

    /// Drops memory-only text for a target that ended.
    fn forget_live(&self, thread_id: &ThreadId, target: &LiveTarget) {
        if let Some(run) = self.lock().get_mut(thread_id) {
            run.live.remove(target);
        }
    }

    fn forward_engine_status(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut statuses = self.inner.engines.subscribe();
        let store = self.inner.store.clone();
        runtime.spawn(async move {
            loop {
                match statuses.recv().await {
                    Ok(status) => store.publish_live(Event::Engine { status }),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

/// The first line of the first message, until the engine names the thread.
fn fallback_title(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let mut title: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        title.push('…');
    }
    title
}

#[cfg(test)]
mod tests;
