//! Agents Forge runs for a thread on any harness: the Artisan MCP's
//! `agent_*` tools.
//!
//! An agent is a child thread (`parent_thread_id`, `spawned_by`) that starts
//! with only its prompt, plus an `Agent` item in the parent that stands for
//! it, so the parent shows it like one of its engine's own subagents. The
//! item follows the child: working while it runs or has input waiting,
//! waiting for the user while it asks something, and settled with the
//! child's last reply as its result once it stops.

use crate::Hub;
use anyhow::{Context, Result};
use artisan_api::{
    CreateThread, EngineConfig, EngineId, InstallState, Item, ItemId, ItemKind, Permissions,
    RunStatus, SendMessage, SignInState, Status, Thread, ThreadId,
};
use forge_store::{NewItem, Refused};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::time::Duration;

/// How deep agents may start agents: a thread's agent is depth 1.
pub const MAX_AGENT_DEPTH: usize = 3;
/// The longest one `agent_wait` blocks, under every harness's MCP call
/// timeout (Codex's is set to ten minutes for the Artisan MCP).
const LONGEST_WAIT: Duration = Duration::from_mins(9);

/// What `agent_spawn` asks for. Anything left out comes from the parent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnAgent {
    /// A few words naming the work; the agent's label.
    pub task: String,
    /// The whole brief: nothing from the parent's conversation goes along.
    pub prompt: String,
    pub harness: Option<EngineId>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub speed: Option<String>,
    pub variant: Option<String>,
    pub context_window: Option<u64>,
    pub options: Option<serde_json::Map<String, serde_json::Value>>,
    pub permissions: Option<Permissions>,
    /// Deliver the result to the parent as a message when it finishes,
    /// instead of waiting for `agent_wait`.
    #[serde(default)]
    pub background: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Working,
    /// It asked the user something, in its own thread.
    WaitingForUser,
    Done,
    Failed,
    Cancelled,
}

impl AgentState {
    #[must_use]
    pub const fn is_settled(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    const fn status(self) -> Status {
        match self {
            Self::Working => Status::Running,
            Self::WaitingForUser => Status::AwaitingUser,
            Self::Done => Status::Done,
            Self::Failed => Status::Failed,
            Self::Cancelled => Status::Cancelled,
        }
    }
}

/// One agent as the tools report it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub agent: ItemId,
    pub thread: ThreadId,
    pub task: String,
    pub harness: EngineId,
    pub model: Option<String>,
    pub effort: Option<String>,
    #[serde(default)]
    pub variant: Option<String>,
    pub permissions: Permissions,
    pub state: AgentState,
    /// Its last reply, once it settled.
    pub result: Option<String>,
    pub error: Option<String>,
    /// What the spawn filled in or changed, and why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// The config an agent runs with: `request` over the parent's, checked
/// against the harness's catalog. Returns what was filled in or changed.
fn resolve(
    parent: &EngineConfig,
    request: &SpawnAgent,
    status: &artisan_api::EngineStatus,
) -> Result<(EngineConfig, Vec<String>)> {
    let engine = status.engine;
    let name = engine.display_name();
    if matches!(status.install, InstallState::Absent) {
        return Err(Refused(format!(
            "{name} isn't installed. Install it in Settings → Engines, or spawn the agent on another harness."
        ))
        .into());
    }
    if matches!(
        status.sign_in,
        SignInState::SignedOut | SignInState::LoggingIn { .. }
    ) {
        return Err(Refused(format!(
            "{name} isn't signed in. Ask the user to sign in to {name} in Settings → Engines, or spawn the agent on another harness."
        ))
        .into());
    }
    let mut warnings = Vec::new();
    let same = engine == parent.engine;
    let known = |id: &str| status.models.iter().find(|model| model.id == id);
    let fallback = || {
        status
            .default_model
            .as_deref()
            .and_then(known)
            .or_else(|| status.models.first())
    };
    let mut config = EngineConfig::new(engine);
    let model = match request.model.as_deref() {
        Some(id) if status.models.is_empty() || known(id).is_some() => Some(id.to_owned()),
        Some(id) => {
            let chosen = fallback().map(|model| model.id.clone());
            warnings.push(format!(
                "{name} has no model {id}; using {}.",
                chosen.as_deref().unwrap_or("its default")
            ));
            chosen
        }
        None if same => parent.model.clone(),
        None => {
            let chosen = fallback().map(|model| model.id.clone());
            warnings.push(format!(
                "No model given for {name}; using {} (its default).",
                chosen.as_deref().unwrap_or("whatever it defaults to")
            ));
            chosen
        }
    };
    let catalog = model.as_deref().and_then(known);
    config.model = model;
    // Effort and speed: asked for, else the parent's, kept only when the
    // model offers it.
    // OpenCode sends reasoning through variant, never through effort.
    let effort = if engine == EngineId::OpenCode {
        if request.effort.is_some() {
            warnings.push(
                "OpenCode uses variant for reasoning; effort was ignored. Set variant instead."
                    .to_owned(),
            );
        }
        None
    } else {
        request.effort.clone().or_else(|| parent.effort.clone())
    };
    config.effort = match (effort, catalog) {
        (Some(effort), Some(model))
            if !model.efforts.is_empty() && !model.efforts.contains(&effort) =>
        {
            let fallback = model.default_effort.clone();
            warnings.push(format!(
                "{} has no effort {effort}; using {}.",
                model.id,
                fallback.as_deref().unwrap_or("its default")
            ));
            fallback
        }
        (effort, _) => effort,
    };
    let speed = request
        .speed
        .clone()
        .or_else(|| same.then(|| parent.speed.clone()).flatten());
    config.speed = match (speed, catalog) {
        (Some(speed), Some(model)) if !model.speeds.iter().any(|tier| tier.id == speed) => {
            warnings.push(format!(
                "{} has no speed {speed}; using its default.",
                model.id
            ));
            None
        }
        (speed, _) => speed,
    };
    if same && config.model == parent.model {
        config.variant.clone_from(&parent.variant);
        config.options.clone_from(&parent.options);
    }
    if let Some(options) = &request.options {
        config.options.extend(options.clone());
    }
    if let Some(tokens) = request.context_window {
        config
            .options
            .insert("context_window".into(), tokens.into());
    }
    if config
        .options
        .get("context_window")
        .is_some_and(serde_json::Value::is_null)
    {
        config.options.remove("context_window");
    }
    if let Some(variant) = &request.variant {
        config.variant = Some(variant.clone());
    }
    if let Some(model) = catalog {
        let variant = model.variant_for(config.variant.as_deref());
        if config.variant.is_some() && config.variant != variant {
            warnings.push(format!(
                "{} has no variant {}; using {}.",
                model.id,
                config.variant.as_deref().unwrap_or_default(),
                variant.as_deref().unwrap_or("its default")
            ));
        }
        config.variant = variant;
        let context = config.options.get("context_window").cloned();
        model.normalize_context_window(&mut config);
        if let Some(context) = context.filter(|_| !config.options.contains_key("context_window")) {
            warnings.push(format!(
                "{} has no context window override {context}; using its standard window.",
                model.id
            ));
        }
    }
    config.permissions = match request.permissions {
        Some(Permissions::Full) if parent.permissions == Permissions::Auto => {
            warnings.push("Permissions capped at auto (the parent's).".to_owned());
            Permissions::Auto
        }
        Some(permissions) => permissions,
        None => parent.permissions,
    };
    Ok((config, warnings))
}

/// Whether `target` may do more without asking than `caller`: an auto
/// thread's agent reaching a full-access one.
fn exceeds(target: &EngineConfig, caller: &EngineConfig) -> bool {
    target.permissions == Permissions::Full && caller.permissions == Permissions::Auto
}

/// `thread` as the sender of a message its agent writes.
fn sender(thread: &Thread) -> artisan_api::Sender {
    artisan_api::Sender {
        thread: thread.id.clone(),
        engine: thread.config.engine,
        title: thread.title.clone(),
    }
}

impl Hub {
    /// Starts an agent for `parent` and returns it at once.
    pub async fn spawn_agent(&self, parent: &ThreadId, request: SpawnAgent) -> Result<AgentInfo> {
        let store = self.store();
        let task = request.task.trim().to_owned();
        if task.is_empty() || request.prompt.trim().is_empty() {
            return Err(Refused("An agent needs a task and a prompt.".to_owned()).into());
        }
        let thread = store
            .thread(parent)
            .await?
            .with_context(|| format!("no thread {parent}"))?;
        let depth = self.agent_depth(&thread).await?;
        if depth >= MAX_AGENT_DEPTH {
            return Err(Refused(format!(
                "Agents can start agents only {MAX_AGENT_DEPTH} deep; do this work here instead."
            ))
            .into());
        }
        let engine = request.harness.unwrap_or(thread.config.engine);
        let (config, warnings) = resolve(&thread.config, &request, &self.engines().status(engine))?;
        let child = ThreadId::new();
        let agent = ItemId::new();
        store
            .create_thread(CreateThread {
                id: child.clone(),
                project_id: thread.project_id.clone(),
                config: config.clone(),
                title: Some(task.clone()),
                parent_thread_id: Some(parent.clone()),
                spawned_by: Some(agent.clone()),
            })
            .await?;
        let run_id = store
            .latest_run(parent)
            .await?
            .filter(|run| run.status == RunStatus::Running)
            .map(|run| run.id);
        store
            .upsert_item(NewItem {
                id: agent.clone(),
                thread_id: parent.clone(),
                run_id,
                parent_id: None,
                native_id: None,
                status: Status::Running,
                kind: ItemKind::Agent {
                    name: Some(engine.display_name().to_owned()),
                    description: task.clone().into(),
                    prompt: Some(request.prompt.clone().into()),
                    agent_id: None,
                    context_used: None,
                    model: config.model.clone(),
                    background: request.background,
                    result: None,
                    label: None,
                    engine: Some(engine),
                    thread: Some(child.clone()),
                },
            })
            .await?;
        self.accept_message(
            &child,
            SendMessage {
                id: ItemId::new(),
                text: request.prompt,
                images: Vec::new(),
                config: None,
                draft: None,
            },
            forge_store::Acceptance {
                title: None,
                remember_config: false,
                from_agent: None,
                sender: Some(sender(&thread)),
            },
        )
        .await?;
        self.agents_moved();
        Ok(AgentInfo {
            agent,
            thread: child,
            task,
            harness: engine,
            model: config.model,
            effort: config.effort,
            variant: config.variant,
            permissions: config.permissions,
            state: AgentState::Working,
            result: None,
            error: None,
            warnings,
        })
    }

    /// Starts a top-level thread (not an agent) in `project`, by default the
    /// caller's, configured like an agent: what `request` gives over the
    /// caller's config. Its first message is `request.prompt` unless empty.
    pub async fn start_thread(
        &self,
        caller: &ThreadId,
        project: Option<artisan_api::ProjectId>,
        request: SpawnAgent,
    ) -> Result<(Thread, Vec<String>)> {
        let store = self.store();
        let from = store
            .thread(caller)
            .await?
            .with_context(|| format!("no thread {caller}"))?;
        let project = project.unwrap_or_else(|| from.project_id.clone());
        if store.project(&project).await?.is_none() {
            return Err(Refused(format!("There is no project {project}.")).into());
        }
        let engine = request.harness.unwrap_or(from.config.engine);
        let (config, warnings) = resolve(&from.config, &request, &self.engines().status(engine))?;
        let title = Some(request.task.trim().to_owned()).filter(|title| !title.is_empty());
        let thread = store
            .create_thread(CreateThread {
                id: ThreadId::new(),
                project_id: project,
                config,
                title: title.clone(),
                parent_thread_id: None,
                spawned_by: None,
            })
            .await?;
        if !request.prompt.trim().is_empty() {
            let first_line = request.prompt.lines().next().unwrap_or_default().trim();
            self.accept_message(
                &thread.id,
                SendMessage {
                    id: ItemId::new(),
                    text: request.prompt.clone(),
                    images: Vec::new(),
                    config: None,
                    draft: None,
                },
                forge_store::Acceptance {
                    title: title
                        .or_else(|| Some(first_line.chars().take(80).collect()))
                        .filter(|title: &String| !title.is_empty()),
                    remember_config: false,
                    from_agent: None,
                    sender: Some(sender(&from)),
                },
            )
            .await?;
        }
        Ok((thread, warnings))
    }

    /// Sends `caller`'s agent's message to `thread`. It never changes the
    /// user's default config, carries its sender, and asks the user first
    /// when it would reach a thread allowed more than the caller.
    pub async fn send_as_agent(
        &self,
        caller: &ThreadId,
        thread: &ThreadId,
        text: String,
    ) -> Result<Item> {
        let store = self.store();
        let from = store
            .thread(caller)
            .await?
            .with_context(|| format!("no thread {caller}"))?;
        let target = store
            .thread(thread)
            .await?
            .ok_or_else(|| Refused(format!("There is no thread {thread}.")))?;
        if exceeds(&target.config, &from.config) {
            self.ask_approval(
                caller,
                "thread_send",
                format!(
                    "Send a message to {}, which runs with full access (this thread runs with auto)",
                    target.title.as_deref().unwrap_or("a thread")
                ),
            )
            .await?;
        }
        self.accept_message(
            thread,
            SendMessage {
                id: ItemId::new(),
                text,
                images: Vec::new(),
                config: None,
                draft: None,
            },
            forge_store::Acceptance {
                title: None,
                remember_config: false,
                from_agent: None,
                sender: Some(sender(&from)),
            },
        )
        .await
    }

    /// How many agents up the chain `thread` is.
    async fn agent_depth(&self, thread: &Thread) -> Result<usize> {
        let mut depth = 0;
        let mut current = thread.clone();
        while current.spawned_by.is_some() {
            depth += 1;
            let Some(parent) = current.parent_thread_id.clone() else {
                break;
            };
            let Some(next) = self.store().thread(&parent).await? else {
                break;
            };
            current = next;
            if depth > MAX_AGENT_DEPTH {
                break;
            }
        }
        Ok(depth)
    }

    pub(crate) async fn is_agent_thread(&self, thread: &ThreadId) -> bool {
        matches!(self.store().thread(thread).await, Ok(Some(thread)) if thread.spawned_by.is_some())
    }

    /// The agent `id` names, as `caller` sees it: an agent item of the
    /// caller's, or the thread one runs in.
    async fn agent_item(&self, caller: &ThreadId, id: &str) -> Result<(Item, ThreadId)> {
        let store = self.store();
        let item = match store.item(&ItemId::from(id)).await? {
            Some(item) => Some(item),
            None => match store.thread(&ThreadId::from(id)).await? {
                Some(thread) => match &thread.spawned_by {
                    Some(agent) => store.item(agent).await?,
                    None => None,
                },
                None => None,
            },
        };
        let not_yours = || Refused(format!("{caller} has no agent {id}."));
        let item = item.ok_or_else(not_yours)?;
        if &item.thread_id != caller {
            return Err(not_yours().into());
        }
        let ItemKind::Agent {
            thread: Some(child),
            ..
        } = &item.kind
        else {
            return Err(Refused(format!(
                "{id} is one of the engine's own subagents; use the engine's tools for it."
            ))
            .into());
        };
        let child = child.clone();
        Ok((item, child))
    }

    /// Where an agent's thread is now.
    async fn child_state(&self, child: &ThreadId) -> Result<(AgentState, Option<String>)> {
        let store = self.store();
        if store.awaits_user(child).await? {
            return Ok((AgentState::WaitingForUser, None));
        }
        if self.is_live(child) || !store.pending_inputs(child).await?.items.is_empty() {
            return Ok((AgentState::Working, None));
        }
        let run = store.latest_run(child).await?;
        Ok(match run {
            None => (AgentState::Working, None),
            Some(run) => match run.status {
                RunStatus::Running => (AgentState::Working, None),
                RunStatus::Done => (AgentState::Done, None),
                // A restart cut it short, or it is parked until the
                // provider has room or the usage limit resets, and it
                // carries on.
                RunStatus::Interrupted | RunStatus::Failed
                    if store.continue_pending(child).await? =>
                {
                    (AgentState::Working, None)
                }
                RunStatus::Cancelled => (AgentState::Cancelled, None),
                RunStatus::Failed | RunStatus::Refused | RunStatus::Interrupted => {
                    (AgentState::Failed, run.error)
                }
            },
        })
    }

    /// Brings an agent's item in line with its thread, and reports it.
    async fn refresh_agent(&self, item: Item, child: &ThreadId) -> Result<AgentInfo> {
        let store = self.store();
        let (state, error) = self.child_state(child).await?;
        let result = if state.is_settled() {
            store.last_reply(child).await?
        } else {
            None
        };
        let thread = store.thread(child).await?;
        let config = thread.map(|thread| thread.config);
        let ItemKind::Agent {
            name,
            description,
            prompt,
            agent_id,
            context_used,
            model,
            background,
            result: stored,
            label,
            engine,
            thread: child_thread,
        } = item.kind.clone()
        else {
            anyhow::bail!("item {} is not an agent", item.id);
        };
        let next = state.status();
        let result_text = result.clone().map(Into::into).or(stored);
        let changed = item.status != next
            || (state.is_settled()
                && matches!(&item.kind, ItemKind::Agent { result, .. } if *result != result_text));
        if changed {
            let new = NewItem {
                id: item.id.clone(),
                thread_id: item.thread_id.clone(),
                run_id: item.run_id.clone(),
                parent_id: item.parent_id.clone(),
                native_id: None,
                status: next,
                kind: ItemKind::Agent {
                    name,
                    description: description.clone(),
                    prompt,
                    agent_id,
                    context_used,
                    model: model.clone(),
                    background,
                    result: result_text,
                    label: label.clone(),
                    engine,
                    thread: child_thread,
                },
            };
            // A settled agent that was messaged works again.
            if item.status.is_terminal()
                && !next.is_terminal()
                && let Some(run) = &item.run_id
            {
                store.reopen_agent(&item.id, run).await?;
            }
            store.upsert_item(new).await?;
            self.agents_moved();
        }
        Ok(AgentInfo {
            agent: item.id.clone(),
            thread: child.clone(),
            task: label.unwrap_or_else(|| store_text(&description)),
            harness: engine.unwrap_or(EngineId::Claude),
            model: config
                .as_ref()
                .and_then(|config| config.model.clone())
                .or(model),
            effort: config.as_ref().and_then(|config| {
                (config.engine != EngineId::OpenCode)
                    .then(|| config.effort.clone())
                    .flatten()
            }),
            variant: config.as_ref().and_then(|config| config.variant.clone()),
            permissions: config.map(|config| config.permissions).unwrap_or_default(),
            state,
            result,
            error,
            warnings: Vec::new(),
        })
    }

    /// One agent of the caller's, as it is now.
    pub async fn agent_status(&self, caller: &ThreadId, id: &str) -> Result<AgentInfo> {
        let (item, child) = self.agent_item(caller, id).await?;
        self.refresh_agent(item, &child).await
    }

    /// The caller's agents, newest last.
    pub async fn agents(&self, caller: &ThreadId) -> Result<Vec<AgentInfo>> {
        let mut agents = Vec::new();
        for thread in self.store().agent_threads(caller).await? {
            if let Some(agent) = &thread.spawned_by
                && let Some(item) = self.store().item(agent).await?
            {
                agents.push(self.refresh_agent(item, &thread.id).await?);
            }
        }
        Ok(agents)
    }

    /// Waits until every listed agent (or, with `any`, one of them) has
    /// settled or asks the user something, or `timeout` passes; returns
    /// them as they are then.
    pub async fn wait_agents(
        &self,
        caller: &ThreadId,
        ids: &[String],
        any: bool,
        timeout: Duration,
    ) -> Result<Vec<AgentInfo>> {
        let mut found = Vec::new();
        for id in ids {
            found.push(self.agent_item(caller, id).await?);
        }
        if found.is_empty() {
            return Err(Refused("Name at least one agent to wait for.".to_owned()).into());
        }
        let _waiting = Waiting::new(self, found.iter().map(|(item, _)| item.id.clone()));
        let deadline = tokio::time::Instant::now() + timeout.min(LONGEST_WAIT);
        let mut changes = self.inner.agents_changed.subscribe();
        loop {
            changes.borrow_and_update();
            let mut agents = Vec::new();
            for (item, child) in &found {
                let item = self
                    .store()
                    .item(&item.id)
                    .await?
                    .unwrap_or_else(|| item.clone());
                agents.push(self.refresh_agent(item, child).await?);
            }
            let stopped = |agent: &AgentInfo| {
                agent.state.is_settled() || agent.state == AgentState::WaitingForUser
            };
            let ready = if any {
                agents.iter().any(stopped)
            } else {
                agents.iter().all(stopped)
            };
            if ready {
                return Ok(agents);
            }
            tokio::select! {
                () = tokio::time::sleep_until(deadline) => return Ok(agents),
                () = self.inner.shutdown.cancelled() => return Ok(agents),
                outcome = changes.changed() => if outcome.is_err() { return Ok(agents) },
            }
        }
    }

    /// Sends an agent a follow-up; a settled one works again.
    pub async fn message_agent(
        &self,
        caller: &ThreadId,
        id: &str,
        text: String,
    ) -> Result<AgentInfo> {
        let (item, child) = self.agent_item(caller, id).await?;
        if text.trim().is_empty() {
            return Err(Refused("The message is empty.".to_owned()).into());
        }
        // The user may have raised the agent's permissions since it started.
        self.send_as_agent(caller, &child, text).await?;
        self.refresh_agent(item, &child).await
    }

    /// Stops an agent and every agent under it.
    pub async fn cancel_agent(&self, caller: &ThreadId, id: &str) -> Result<AgentInfo> {
        let (item, child) = self.agent_item(caller, id).await?;
        self.cancel(&child).await?;
        self.refresh_agent(item, &child).await
    }

    /// Cancels the agents a thread started, and theirs.
    pub(crate) async fn cancel_agents_of(&self, thread: &ThreadId) -> Result<()> {
        let mut stack = vec![thread.clone()];
        let mut seen = std::collections::HashSet::new();
        while let Some(next) = stack.pop() {
            if !seen.insert(next.clone()) {
                continue;
            }
            for child in self.store().agent_threads(&next).await? {
                self.cancel_one(&child.id).await?;
                stack.push(child.id);
            }
        }
        Ok(())
    }

    /// A run of `thread` ended: if it runs an agent, update the parent, and
    /// deliver a background agent's result.
    pub(crate) async fn agent_run_ended(&self, thread: &ThreadId) {
        if let Err(error) = self.settle_agent(thread).await {
            tracing::error!(%thread, "updating the agent this thread runs: {error:#}");
        }
    }

    /// Something in `thread` started or stopped waiting on the user.
    pub(crate) async fn agent_attention(&self, thread: &ThreadId) {
        if let Err(error) = self.settle_agent(thread).await {
            tracing::error!(%thread, "updating the agent this thread runs: {error:#}");
        }
    }

    async fn settle_agent(&self, thread: &ThreadId) -> Result<()> {
        // Serialize reports with Stop and with reconciliation, so a report
        // is either included in Stop's cutoff or observes its cancellation.
        let _admission = self.inner.admission.lock().await;
        let store = self.store();
        let Some(child) = store.thread(thread).await? else {
            return Ok(());
        };
        let Some(agent) = &child.spawned_by else {
            return Ok(());
        };
        let Some(item) = store.item(agent).await? else {
            return Ok(());
        };
        let was = item.status;
        let parent = item.thread_id.clone();
        let background = matches!(
            item.kind,
            ItemKind::Agent {
                background: true,
                ..
            }
        );
        let info = self.refresh_agent(item, thread).await?;
        let waited = self
            .waiters_lock()
            .get(agent)
            .is_some_and(|count| *count > 0);
        if background
            && !waited
            && !was.is_terminal()
            && info.state.is_settled()
            && info.state != AgentState::Cancelled
        {
            self.deliver_result(&parent, &info).await?;
        }
        Ok(())
    }

    /// Tells the parent a background agent finished, as a message.
    async fn deliver_result(&self, parent: &ThreadId, info: &AgentInfo) -> Result<()> {
        // A child can finish successfully while Stop is winding the tree
        // down. Its late result must not start another turn either.
        let cancelling = self.lock().get(parent).map(|run| run.cancel.is_cancelled());
        if cancelling == Some(true)
            || (cancelling.is_none()
                && self
                    .store()
                    .latest_run(parent)
                    .await?
                    .is_some_and(|run| run.status == RunStatus::Cancelled))
        {
            return Ok(());
        }
        let outcome = match info.state {
            AgentState::Done => "finished",
            AgentState::Failed => "failed",
            AgentState::Working | AgentState::WaitingForUser | AgentState::Cancelled => {
                "was cancelled"
            }
        };
        let mut text = format!(
            "[Artisan] Agent \"{}\" ({}, {}) {outcome}.",
            info.task,
            info.harness.display_name(),
            info.agent
        );
        if let Some(error) = &info.error {
            write!(text, "\nError: {error}")?;
        }
        if let Some(result) = &info.result {
            text.push_str("\nIts last reply:\n\n");
            text.push_str(result);
        }
        self.accept_message(
            parent,
            SendMessage {
                id: ItemId::new(),
                text,
                images: Vec::new(),
                config: None,
                draft: None,
            },
            forge_store::Acceptance {
                title: None,
                remember_config: false,
                from_agent: Some(info.agent.clone()),
                sender: None,
            },
        )
        .await?;
        Ok(())
    }

    /// Agent items a previous Forge left working catch up with their
    /// threads; a background agent that settled meanwhile reports to its
    /// parent.
    pub async fn reconcile_agents(&self) {
        let items = match self.store().working_agents().await {
            Ok(items) => items,
            Err(error) => {
                tracing::error!("reading working agents: {error:#}");
                return;
            }
        };
        for item in items {
            if let ItemKind::Agent {
                thread: Some(child),
                ..
            } = &item.kind
                && let Err(error) = self.settle_agent(child).await
            {
                tracing::error!(thread = %child, "updating an agent: {error:#}");
            }
        }
    }

    fn agents_moved(&self) {
        self.inner.agents_changed.send_modify(|count| *count += 1);
    }

    fn waiters_lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<ItemId, usize>> {
        self.inner
            .agent_waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn store_text(text: &artisan_api::TextContent) -> String {
    text.inline().unwrap_or_default().to_owned()
}

/// Marks agents as waited on while an `agent_wait` holds them, so a
/// background agent's result isn't also delivered as a message.
struct Waiting<'h> {
    hub: &'h Hub,
    agents: Vec<ItemId>,
}

impl<'h> Waiting<'h> {
    fn new(hub: &'h Hub, agents: impl Iterator<Item = ItemId>) -> Self {
        let agents: Vec<_> = agents.collect();
        let mut waiters = hub.waiters_lock();
        for agent in &agents {
            *waiters.entry(agent.clone()).or_default() += 1;
        }
        drop(waiters);
        Self { hub, agents }
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut waiters = self.hub.waiters_lock();
        for agent in &self.agents {
            if let Some(count) = waiters.get_mut(agent) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    waiters.remove(agent);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_api::{EngineStatus, Model};

    fn model(id: &str, efforts: &[&str]) -> Model {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "efforts": efforts, "default_effort": efforts.first(),
            "context_window": null, "supports_images": true,
        }))
        .unwrap()
    }

    fn status(engine: EngineId, models: Vec<Model>, default: &str) -> EngineStatus {
        let mut status: EngineStatus = serde_json::from_value(serde_json::json!({
            "engine": engine, "install": {"state": "absent"}, "sign_in": {"state": "unknown"},
            "models": [], "default_model": default, "usage": null,
        }))
        .unwrap_or_else(|_| panic!("status shape"));
        status.install = InstallState::Installed {
            version: "1".into(),
            update: None,
        };
        status.models = models.into();
        status
    }

    fn parent() -> EngineConfig {
        EngineConfig {
            model: Some("opus".into()),
            effort: Some("high".into()),
            ..EngineConfig::new(EngineId::Claude)
        }
    }

    #[test]
    fn nothing_given_inherits_the_parent_without_a_warning() {
        let claude = status(
            EngineId::Claude,
            vec![model("opus", &["low", "high"])],
            "opus",
        );
        let (config, warnings) = resolve(&parent(), &SpawnAgent::default(), &claude).unwrap();
        assert_eq!(config.model.as_deref(), Some("opus"));
        assert_eq!(config.effort.as_deref(), Some("high"));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn another_harness_without_a_model_gets_its_default_and_says_so() {
        let codex = status(
            EngineId::Codex,
            vec![
                model("gpt-a", &["low", "medium", "high"]),
                model("gpt-b", &["low"]),
            ],
            "gpt-a",
        );
        let request = SpawnAgent {
            harness: Some(EngineId::Codex),
            ..SpawnAgent::default()
        };
        let (config, warnings) = resolve(&parent(), &request, &codex).unwrap();
        assert_eq!(config.engine, EngineId::Codex);
        assert_eq!(config.model.as_deref(), Some("gpt-a"));
        assert_eq!(
            config.effort.as_deref(),
            Some("high"),
            "the parent's effort fits"
        );
        assert_eq!(
            warnings,
            ["No model given for Codex; using gpt-a (its default)."]
        );
    }

    #[test]
    fn what_the_model_lacks_falls_back_with_a_warning_and_permissions_cap() {
        let codex = status(EngineId::Codex, vec![model("gpt-b", &["low"])], "gpt-b");
        let request = SpawnAgent {
            harness: Some(EngineId::Codex),
            model: Some("claude-opus".into()),
            effort: Some("max".into()),
            permissions: Some(Permissions::Full),
            ..SpawnAgent::default()
        };
        let (config, warnings) = resolve(&parent(), &request, &codex).unwrap();
        assert_eq!(config.model.as_deref(), Some("gpt-b"));
        assert_eq!(config.effort.as_deref(), Some("low"));
        assert_eq!(config.permissions, Permissions::Auto);
        assert_eq!(
            warnings,
            [
                "Codex has no model claude-opus; using gpt-b.",
                "gpt-b has no effort max; using low.",
                "Permissions capped at auto (the parent's).",
            ]
        );
    }

    fn configurable_model() -> Model {
        Model {
            variants: vec![
                artisan_api::Variant {
                    id: "medium".into(),
                    label: "Medium".into(),
                },
                artisan_api::Variant {
                    id: "xhigh".into(),
                    label: "Extra high".into(),
                },
            ],
            default_variant: Some("medium".into()),
            context_windows: vec![artisan_api::ContextWindow {
                tokens: 1_000_000,
                override_tokens: Some(1_000_000),
            }],
            ..model("muse", &[])
        }
    }

    #[test]
    fn opencode_variant_context_and_options_can_be_requested_across_harnesses() {
        let status = status(EngineId::OpenCode, vec![configurable_model()], "muse");
        let request = SpawnAgent {
            model: Some("muse".into()),
            variant: Some("xhigh".into()),
            context_window: Some(1_000_000),
            options: Some(
                serde_json::from_value(serde_json::json!({
                    "context_window": 200_000, "custom": true,
                }))
                .unwrap(),
            ),
            ..SpawnAgent::default()
        };
        let (config, warnings) = resolve(&parent(), &request, &status).unwrap();
        assert_eq!(config.variant.as_deref(), Some("xhigh"));
        assert_eq!(config.effort, None);
        assert_eq!(config.context_window_override(), Some(1_000_000));
        assert_eq!(config.options["custom"], true);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn opencode_inherits_variant_without_stale_effort() {
        let status = status(EngineId::OpenCode, vec![configurable_model()], "muse");
        let mut parent = EngineConfig::new(EngineId::OpenCode);
        parent.model = Some("muse".into());
        parent.effort = Some("medium".into());
        parent.variant = Some("xhigh".into());
        let (config, warnings) = resolve(&parent, &SpawnAgent::default(), &status).unwrap();
        assert_eq!(config.variant.as_deref(), Some("xhigh"));
        assert_eq!(config.effort, None);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn opencode_warns_when_effort_is_requested_instead_of_variant() {
        let status = status(EngineId::OpenCode, vec![configurable_model()], "muse");
        let request = SpawnAgent {
            effort: Some("xhigh".into()),
            ..SpawnAgent::default()
        };
        let (config, warnings) = resolve(&parent(), &request, &status).unwrap();
        assert_eq!(config.effort, None);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("Set variant instead"))
        );
    }

    #[test]
    fn unsupported_variant_and_context_fall_back_with_warnings() {
        let status = status(EngineId::OpenCode, vec![configurable_model()], "muse");
        let request = SpawnAgent {
            model: Some("muse".into()),
            variant: Some("unknown".into()),
            context_window: Some(123),
            ..SpawnAgent::default()
        };
        let (config, warnings) = resolve(&parent(), &request, &status).unwrap();
        assert_eq!(config.variant.as_deref(), Some("medium"));
        assert_eq!(config.context_window_override(), None);
        assert_eq!(
            warnings,
            [
                "muse has no variant unknown; using medium.",
                "muse has no context window override 123; using its standard window.",
            ]
        );
    }

    #[test]
    fn model_options_inherit_merge_and_reset_context() {
        let status = status(EngineId::OpenCode, vec![configurable_model()], "muse");
        let mut parent = EngineConfig::new(EngineId::OpenCode);
        parent.model = Some("muse".into());
        parent.variant = Some("xhigh".into());
        parent.options = serde_json::from_value(serde_json::json!({
            "context_window": 1_000_000, "keep": true, "replace": "old",
        }))
        .unwrap();
        let (inherited, warnings) = resolve(&parent, &SpawnAgent::default(), &status).unwrap();
        assert_eq!(inherited, parent);
        assert!(warnings.is_empty());
        let request = SpawnAgent {
            options: Some(
                serde_json::from_value(serde_json::json!({
                    "context_window": null, "replace": "new",
                }))
                .unwrap(),
            ),
            ..SpawnAgent::default()
        };
        let (config, warnings) = resolve(&parent, &request, &status).unwrap();
        assert_eq!(config.variant, parent.variant);
        assert_eq!(config.context_window_override(), None);
        assert!(!config.options.contains_key("context_window"));
        assert_eq!(config.options["keep"], true);
        assert_eq!(config.options["replace"], "new");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn changing_models_drops_inherited_options_and_uses_variant_default() {
        let status = status(EngineId::Claude, vec![configurable_model()], "muse");
        let mut parent = parent();
        parent.variant = Some("xhigh".into());
        parent.options.insert("custom".into(), true.into());
        let request = SpawnAgent {
            model: Some("muse".into()),
            ..SpawnAgent::default()
        };
        let (config, warnings) = resolve(&parent, &request, &status).unwrap();
        assert_eq!(config.variant.as_deref(), Some("medium"));
        assert!(config.options.is_empty());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn a_signed_out_harness_refuses_with_sign_in_first() {
        let mut codex = status(EngineId::Codex, vec![model("gpt-b", &["low"])], "gpt-b");
        codex.sign_in = SignInState::SignedOut;
        let request = SpawnAgent {
            harness: Some(EngineId::Codex),
            ..SpawnAgent::default()
        };
        let error = resolve(&parent(), &request, &codex).unwrap_err();
        assert!(error.to_string().contains("isn't signed in"), "{error}");
    }
}
