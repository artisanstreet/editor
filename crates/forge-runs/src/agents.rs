use crate::error::{self, ErrorClass, Refused};
use crate::{CancelError, Hub, InputError, SendError};
use artisan_api::{
    CreateThread, EngineConfig, EngineId, InstallState, Item, ItemId, ItemKind, Permissions,
    RunStatus, SendMessage, SignInState, Status, Thread, ThreadId,
};
use forge_store::NewItem;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const MAX_AGENT_DEPTH: usize = 3;
const LONGEST_WAIT: Duration = Duration::from_mins(9);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnAgent {
    pub task: String,
    pub prompt: String,
    pub harness: Option<EngineId>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub speed: Option<String>,
    pub variant: Option<String>,
    pub context_window: Option<u64>,
    pub options: Option<serde_json::Map<String, serde_json::Value>>,
    pub permissions: Option<Permissions>,
    #[serde(default)]
    pub background: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Working,
    WaitingForUser,
    Done,
    Failed,
    Cancelled,
}

impl AgentState {
    #[must_use]
    pub(crate) const fn is_settled(self) -> bool {
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
    pub result: Option<String>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[expect(
    clippy::too_many_lines,
    reason = "variant/context/options fallbacks in one resolve"
)]
fn resolve(
    parent: &EngineConfig,
    request: &SpawnAgent,
    status: &artisan_api::EngineStatus,
) -> Result<(EngineConfig, Vec<String>), Refused> {
    let engine = status.engine;
    let name = engine.display_name();
    if matches!(status.install, InstallState::Absent) {
        return Err(Refused(format!(
            "{name} isn't installed. Install it in Settings → Engines, or spawn the agent on another harness."
        )));
    }
    if matches!(
        status.sign_in,
        SignInState::SignedOut | SignInState::LoggingIn { .. }
    ) {
        return Err(Refused(format!(
            "{name} isn't signed in. Ask the user to sign in to {name} in Settings → Engines, or spawn the agent on another harness."
        )));
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

fn exceeds(target: &EngineConfig, caller: &EngineConfig) -> bool {
    target.permissions == Permissions::Full && caller.permissions == Permissions::Auto
}

fn sender(thread: &Thread) -> artisan_api::Sender {
    artisan_api::Sender {
        thread: thread.id.clone(),
        engine: thread.config.engine,
        title: thread.title.clone(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpawnAgentError {
    #[error("start an agent for thread {parent}")]
    Refused {
        parent: ThreadId,
        #[source]
        refused: Refused,
    },
    #[error("thread {parent} does not exist")]
    ParentNotFound { parent: ThreadId },
    #[error(transparent)]
    ReadThread(#[from] forge_store::ReadThreadError),
    #[error(transparent)]
    LatestRun(#[from] forge_store::LatestRunError),
    #[error(transparent)]
    Store(#[from] forge_store::SpawnAgentError),
}

impl SpawnAgentError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Refused { .. } => ErrorClass::Conflict,
            Self::ParentNotFound { .. } => ErrorClass::NotFound,
            Self::Store(store) => error::spawn(&store.cause),
            Self::ReadThread(_) | Self::LatestRun(_) => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StartThreadError {
    #[error("start a thread for thread {caller}")]
    Refused {
        caller: ThreadId,
        #[source]
        refused: Refused,
    },
    #[error("thread {caller} does not exist")]
    CallerNotFound { caller: ThreadId },
    #[error(transparent)]
    ReadThread(#[from] forge_store::ReadThreadError),
    #[error(transparent)]
    ReadProject(#[from] forge_store::ReadProjectError),
    #[error(transparent)]
    Create(#[from] forge_store::CreateThreadError),
    #[error("send the first message of thread {thread}")]
    Send {
        thread: ThreadId,
        #[source]
        source: Box<SendError>,
    },
}

impl StartThreadError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Refused { .. } => ErrorClass::Conflict,
            Self::CallerNotFound { .. } => ErrorClass::NotFound,
            Self::Create(create) => error::create_thread(create),
            Self::Send { source, .. } => source.class(),
            Self::ReadThread(_) | Self::ReadProject(_) => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SendAsAgentError {
    #[error("thread {caller} does not exist")]
    CallerNotFound { caller: ThreadId },
    #[error("thread {caller} cannot message thread {thread}")]
    NoThread {
        caller: ThreadId,
        thread: ThreadId,
        #[source]
        refused: Refused,
    },
    #[error(transparent)]
    ReadThread(#[from] forge_store::ReadThreadError),
    #[error("ask the user before thread {caller} messages thread {thread}")]
    Approval {
        caller: ThreadId,
        thread: ThreadId,
        #[source]
        source: Box<crate::approvals::ApprovalError>,
    },
    #[error(transparent)]
    Send(Box<SendError>),
}

impl SendAsAgentError {
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::NoThread { .. } => ErrorClass::Conflict,
            Self::CallerNotFound { .. } => ErrorClass::NotFound,
            Self::Approval { source, .. } => source.class(),
            Self::Send(send) => send.class(),
            Self::ReadThread(_) => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentLookupError {
    #[error("find agent {agent} of thread {caller}")]
    Refused {
        caller: ThreadId,
        agent: String,
        #[source]
        refused: Refused,
    },
    #[error(transparent)]
    ReadItem(#[from] forge_store::ReadItemError),
    #[error(transparent)]
    ReadThread(#[from] forge_store::ReadThreadError),
}

impl AgentLookupError {
    #[must_use]
    const fn class(&self) -> ErrorClass {
        match self {
            Self::Refused { .. } => ErrorClass::Conflict,
            Self::ReadItem(_) | Self::ReadThread(_) => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentViewError {
    #[error(transparent)]
    AwaitsUser(#[from] forge_store::AwaitsUserError),
    #[error(transparent)]
    RunnableInput(#[from] forge_store::RunnableInputError),
    #[error(transparent)]
    LatestRun(#[from] forge_store::LatestRunError),
    #[error(transparent)]
    ContinuePending(#[from] forge_store::ContinuePendingError),
    #[error(transparent)]
    LastReply(#[from] forge_store::LastReplyError),
    #[error(transparent)]
    ReadThread(#[from] forge_store::ReadThreadError),
    #[error(transparent)]
    Reopen(#[from] forge_store::ReopenAgentError),
    #[error(transparent)]
    Save(#[from] forge_store::UpsertItemError),
    #[error("read the stored task description of agent {agent}")]
    Description {
        agent: ItemId,
        #[source]
        source: forge_store::ResolveTextError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum AgentQueryError {
    #[error("name at least one agent of thread {caller} to wait for")]
    NothingNamed { caller: ThreadId },
    #[error(transparent)]
    Lookup(#[from] AgentLookupError),
    #[error(transparent)]
    View(#[from] AgentViewError),
    #[error(transparent)]
    Agents(#[from] forge_store::AgentThreadsError),
    #[error(transparent)]
    ReadItem(#[from] forge_store::ReadItemError),
}

impl AgentQueryError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::NothingNamed { .. } => ErrorClass::BadRequest,
            Self::Lookup(lookup) => lookup.class(),
            Self::View(_) | Self::Agents(_) | Self::ReadItem(_) => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MessageAgentError {
    #[error("the message to agent {agent} of thread {caller} is empty")]
    Empty { caller: ThreadId, agent: String },
    #[error(transparent)]
    Lookup(#[from] AgentLookupError),
    #[error(transparent)]
    Send(Box<SendAsAgentError>),
    #[error(transparent)]
    View(#[from] AgentViewError),
}

impl MessageAgentError {
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Empty { .. } => ErrorClass::BadRequest,
            Self::Lookup(lookup) => lookup.class(),
            Self::Send(send) => send.class(),
            Self::View(_) => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CancelAgentError {
    #[error(transparent)]
    Lookup(#[from] AgentLookupError),
    #[error(transparent)]
    Cancel(Box<CancelError>),
    #[error(transparent)]
    View(#[from] AgentViewError),
}

impl CancelAgentError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Lookup(lookup) => lookup.class(),
            Self::Cancel(cancel) => cancel.class(),
            Self::View(_) => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum SettleAgentError {
    #[error(transparent)]
    ReadThread(#[from] forge_store::ReadThreadError),
    #[error(transparent)]
    ReadItem(#[from] forge_store::ReadItemError),
    #[error(transparent)]
    LatestRun(#[from] forge_store::LatestRunError),
    #[error(transparent)]
    View(#[from] AgentViewError),
    #[error(transparent)]
    Reopen(#[from] forge_store::ReopenAgentError),
    #[error(transparent)]
    Save(#[from] forge_store::UpsertItemError),
    #[error(transparent)]
    Report(#[from] forge_store::SettleAgentError),
    #[error("hand agent {agent}'s report to the live run of thread {parent}")]
    Deliver {
        agent: ItemId,
        parent: ThreadId,
        #[source]
        source: Box<InputError>,
    },
}

impl Hub {
    fn acting_config(&self, caller: &Thread) -> EngineConfig {
        let launched = self.lock().get(&caller.id).and_then(|live| live.launched);
        let mut config = caller.config.clone();
        config.permissions = match (caller.config.permissions, launched) {
            (Permissions::Auto, _) | (_, Some(Permissions::Auto)) => Permissions::Auto,
            (Permissions::Full, Some(Permissions::Full) | None) => Permissions::Full,
        };
        config
    }

    pub async fn spawn_agent(
        &self,
        parent: &ThreadId,
        request: SpawnAgent,
    ) -> Result<AgentInfo, SpawnAgentError> {
        let refused = |refused| SpawnAgentError::Refused {
            parent: parent.clone(),
            refused,
        };
        let store = self.store();
        let task = request.task.trim().to_owned();
        if task.is_empty() || request.prompt.trim().is_empty() {
            return Err(refused(Refused(
                "An agent needs a task and a prompt.".to_owned(),
            )));
        }
        let thread =
            store
                .thread(parent)
                .await?
                .ok_or_else(|| SpawnAgentError::ParentNotFound {
                    parent: parent.clone(),
                })?;
        let depth = self.agent_depth(&thread).await?;
        if depth >= MAX_AGENT_DEPTH {
            return Err(refused(Refused(format!(
                "Agents can start agents only {MAX_AGENT_DEPTH} deep; do this work here instead."
            ))));
        }
        let engine = request.harness.unwrap_or(thread.config.engine);
        let acting = self.acting_config(&thread);
        let (config, warnings) =
            resolve(&acting, &request, &self.engines().status(engine)).map_err(refused)?;
        let child = ThreadId::new();
        let agent = ItemId::new();
        let run_id = store
            .latest_run(parent)
            .await?
            .filter(|run| run.status() == RunStatus::Running)
            .map(|run| run.id);
        store
            .spawn_agent(
                NewItem {
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
                },
                CreateThread {
                    id: child.clone(),
                    project_id: thread.project_id.clone(),
                    config: config.clone(),
                    title: Some(task.clone()),
                    parent_thread_id: Some(parent.clone()),
                    spawned_by: Some(agent.clone()),
                },
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
        self.resume_pending();
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

    pub async fn start_thread(
        &self,
        caller: &ThreadId,
        project: Option<artisan_api::ProjectId>,
        request: SpawnAgent,
    ) -> Result<(Thread, Vec<String>), StartThreadError> {
        let refused = |refused| StartThreadError::Refused {
            caller: caller.clone(),
            refused,
        };
        let store = self.store();
        let from = store
            .thread(caller)
            .await?
            .ok_or_else(|| StartThreadError::CallerNotFound {
                caller: caller.clone(),
            })?;
        let project = project.unwrap_or_else(|| from.project_id.clone());
        if store.project(&project).await?.is_none() {
            return Err(refused(Refused(format!("There is no project {project}."))));
        }
        let engine = request.harness.unwrap_or(from.config.engine);
        let acting = self.acting_config(&from);
        let (config, warnings) =
            resolve(&acting, &request, &self.engines().status(engine)).map_err(refused)?;
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
            .await
            .map_err(|source| StartThreadError::Send {
                thread: thread.id.clone(),
                source: Box::new(source),
            })?;
        }
        Ok((thread, warnings))
    }

    pub async fn send_as_agent(
        &self,
        caller: &ThreadId,
        thread: &ThreadId,
        text: String,
    ) -> Result<Item, SendAsAgentError> {
        self.send_as_agent_with_id(caller, thread, text, ItemId::new())
            .await
    }

    pub(crate) async fn send_as_agent_with_id(
        &self,
        caller: &ThreadId,
        thread: &ThreadId,
        text: String,
        id: ItemId,
    ) -> Result<Item, SendAsAgentError> {
        let store = self.store();
        let from = store
            .thread(caller)
            .await?
            .ok_or_else(|| SendAsAgentError::CallerNotFound {
                caller: caller.clone(),
            })?;
        let target = store
            .thread(thread)
            .await?
            .ok_or_else(|| SendAsAgentError::NoThread {
                caller: caller.clone(),
                thread: thread.clone(),
                refused: Refused(format!("There is no thread {thread}.")),
            })?;
        if exceeds(&target.config, &self.acting_config(&from)) {
            self.ask_approval(
                caller,
                "thread_send",
                format!(
                    "Send a message to {}, which runs with full access (this thread runs with auto)",
                    target.title.as_deref().unwrap_or("a thread")
                ),
            )
            .await
            .map_err(|source| SendAsAgentError::Approval {
                caller: caller.clone(),
                thread: thread.clone(),
                source: Box::new(source),
            })?;
        }
        self.accept_message(
            thread,
            SendMessage {
                id,
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
        .map_err(|error| SendAsAgentError::Send(Box::new(error)))
    }

    async fn agent_depth(&self, thread: &Thread) -> Result<usize, forge_store::ReadThreadError> {
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

    async fn agent_item(
        &self,
        caller: &ThreadId,
        id: &str,
    ) -> Result<(Item, ThreadId), AgentLookupError> {
        let refused = |message: String| AgentLookupError::Refused {
            caller: caller.clone(),
            agent: id.to_owned(),
            refused: Refused(message),
        };
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
        let not_yours = || refused(format!("{caller} has no agent {id}."));
        let item = item.ok_or_else(not_yours)?;
        if &item.thread_id != caller {
            return Err(not_yours());
        }
        let ItemKind::Agent {
            thread: Some(child),
            ..
        } = &item.kind
        else {
            return Err(refused(format!(
                "{id} is one of the engine's own subagents; use the engine's tools for it."
            )));
        };
        let child = child.clone();
        Ok((item, child))
    }

    async fn child_state(
        &self,
        child: &ThreadId,
    ) -> Result<(AgentState, Option<String>), AgentViewError> {
        let store = self.store();
        if store.awaits_user(child).await? {
            return Ok((AgentState::WaitingForUser, None));
        }
        if self.is_live(child) || store.has_runnable_input(child).await? {
            return Ok((AgentState::Working, None));
        }
        let run = store.latest_run(child).await?;
        Ok(match run {
            None => (AgentState::Cancelled, None),
            Some(run) => match run.status() {
                RunStatus::Running => (AgentState::Working, None),
                RunStatus::Done => (AgentState::Done, None),
                RunStatus::Interrupted | RunStatus::Failed
                    if store.continue_pending(child).await? =>
                {
                    (AgentState::Working, None)
                }
                RunStatus::Cancelled => (AgentState::Cancelled, None),
                RunStatus::Failed | RunStatus::Refused | RunStatus::Interrupted => {
                    (AgentState::Failed, run.error().map(str::to_owned))
                }
            },
        })
    }

    async fn refresh_agent(
        &self,
        item: Item,
        child: &ThreadId,
    ) -> Result<AgentInfo, AgentViewError> {
        let (info, update) = self.agent_view(&item, child).await?;
        if let Some(update) = update {
            if item.status.is_terminal()
                && !update.status.is_terminal()
                && let Some(run) = &item.run_id
            {
                self.store().reopen_agent(&item.id, run).await?;
            }
            self.store().upsert_item(update).await?;
            self.agents_moved();
        }
        Ok(info)
    }

    async fn agent_view(
        &self,
        item: &Item,
        child: &ThreadId,
    ) -> Result<(AgentInfo, Option<NewItem>), AgentViewError> {
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
            unreachable!(
                "invariant: item {} of thread {} that runs agent thread {child} is an agent",
                item.id, item.thread_id
            );
        };
        let next = state.status();
        let result_text = result.clone().map(Into::into).or(stored);
        let changed = item.status != next
            || (state.is_settled()
                && matches!(&item.kind, ItemKind::Agent { result, .. } if *result != result_text));
        let update = changed.then(|| NewItem {
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
        });
        let task = match label {
            Some(label) => label,
            None => store.resolve_text(&description).await.map_err(|source| {
                AgentViewError::Description {
                    agent: item.id.clone(),
                    source,
                }
            })?,
        };
        let info = AgentInfo {
            agent: item.id.clone(),
            thread: child.clone(),
            task,
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
        };
        Ok((info, update))
    }

    pub async fn agent_status(
        &self,
        caller: &ThreadId,
        id: &str,
    ) -> Result<AgentInfo, AgentQueryError> {
        let (item, child) = self.agent_item(caller, id).await?;
        Ok(self.refresh_agent(item, &child).await?)
    }

    pub async fn agents(&self, caller: &ThreadId) -> Result<Vec<AgentInfo>, AgentQueryError> {
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

    pub async fn wait_agents(
        &self,
        caller: &ThreadId,
        ids: &[String],
        any: bool,
        timeout: Duration,
    ) -> Result<Vec<AgentInfo>, AgentQueryError> {
        let mut found = Vec::new();
        for id in ids {
            found.push(self.agent_item(caller, id).await?);
        }
        if found.is_empty() {
            return Err(AgentQueryError::NothingNamed {
                caller: caller.clone(),
            });
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

    pub async fn message_agent(
        &self,
        caller: &ThreadId,
        id: &str,
        text: String,
    ) -> Result<AgentInfo, MessageAgentError> {
        let (item, child) = self.agent_item(caller, id).await?;
        if text.trim().is_empty() {
            return Err(MessageAgentError::Empty {
                caller: caller.clone(),
                agent: id.to_owned(),
            });
        }
        self.send_as_agent(caller, &child, text)
            .await
            .map_err(|error| MessageAgentError::Send(Box::new(error)))?;
        Ok(self.refresh_agent(item, &child).await?)
    }

    pub async fn cancel_agent(
        &self,
        caller: &ThreadId,
        id: &str,
    ) -> Result<AgentInfo, CancelAgentError> {
        let (item, child) = self.agent_item(caller, id).await?;
        self.cancel(&child)
            .await
            .map_err(|error| CancelAgentError::Cancel(Box::new(error)))?;
        Ok(self.refresh_agent(item, &child).await?)
    }

    pub(crate) async fn agent_tree(
        &self,
        thread: &ThreadId,
    ) -> Result<Vec<ThreadId>, forge_store::AgentThreadsError> {
        let mut tree = Vec::new();
        let mut stack = vec![thread.clone()];
        let mut seen = std::collections::HashSet::new();
        while let Some(next) = stack.pop() {
            if !seen.insert(next.clone()) {
                continue;
            }
            stack.extend(
                self.store()
                    .agent_threads(&next)
                    .await?
                    .into_iter()
                    .map(|child| child.id),
            );
            tree.push(next);
        }
        Ok(tree)
    }

    pub(crate) async fn agent_run_ended(&self, thread: &ThreadId) {
        if let Err(error) = self.settle_agent(thread).await {
            tracing::error!(%thread, "updating the agent this thread runs: {}", diagnostics::chain(&error));
        }
    }

    pub(crate) async fn agent_attention(&self, thread: &ThreadId) {
        if let Err(error) = self.settle_agent(thread).await {
            tracing::error!(%thread, "updating the agent this thread runs: {}", diagnostics::chain(&error));
        }
    }

    async fn settle_agent(&self, thread: &ThreadId) -> Result<(), SettleAgentError> {
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
        let parent = item.thread_id.clone();
        let background = matches!(
            item.kind,
            ItemKind::Agent {
                background: true,
                ..
            }
        );
        let (info, update) = self.agent_view(&item, thread).await?;
        let waited = self
            .waiters_lock()
            .get(agent)
            .is_some_and(|count| *count > 0);
        let reports = background
            && !waited
            && !item.status.is_terminal()
            && info.state.is_settled()
            && info.state != AgentState::Cancelled
            && !self.stopping(&parent).await?;
        if !reports {
            if let Some(update) = update {
                if item.status.is_terminal()
                    && !update.status.is_terminal()
                    && let Some(run) = &item.run_id
                {
                    store.reopen_agent(&item.id, run).await?;
                }
                store.upsert_item(update).await?;
                self.agents_moved();
            }
            return Ok(());
        }
        let (report, engine) = store
            .settle_agent(
                update,
                forge_store::AgentReport {
                    thread: parent.clone(),
                    message: SendMessage {
                        id: ItemId::new(),
                        text: report_text(&info),
                        images: Vec::new(),
                        config: None,
                        draft: None,
                    },
                    acceptance: forge_store::Acceptance {
                        title: None,
                        remember_config: false,
                        from_agent: Some(info.agent.clone()),
                        sender: None,
                    },
                },
            )
            .await?;
        self.agents_moved();
        self.dispatch_accepted(&parent, &report, engine)
            .await
            .map_err(|source| SettleAgentError::Deliver {
                agent: agent.clone(),
                parent: parent.clone(),
                source: Box::new(source),
            })
    }

    async fn stopping(&self, parent: &ThreadId) -> Result<bool, forge_store::LatestRunError> {
        let cancelling = self.lock().get(parent).map(|run| run.cancel.is_cancelled());
        Ok(match cancelling {
            Some(cancelling) => cancelling,
            None => self
                .store()
                .latest_run(parent)
                .await?
                .is_some_and(|run| run.status() == RunStatus::Cancelled),
        })
    }

    pub async fn reconcile_agents(&self) {
        let items = match self.store().working_agents().await {
            Ok(items) => items,
            Err(error) => {
                tracing::error!("reading working agents: {}", diagnostics::chain(&error));
                return;
            }
        };
        for item in items {
            if let ItemKind::Agent {
                thread: Some(child),
                ..
            } = &item.kind
                && self
                    .generation()
                    .is_none_or(|generation| generation.owner(child).is_none())
                && let Err(error) = self.settle_agent(child).await
            {
                tracing::error!(thread = %child, "updating an agent: {}", diagnostics::chain(&error));
            }
        }
    }

    pub(crate) fn agents_moved(&self) {
        self.inner.agents_changed.send_modify(|count| *count += 1);
    }

    fn waiters_lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<ItemId, usize>> {
        self.inner
            .agent_waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn report_text(info: &AgentInfo) -> String {
    let outcome = match info.state {
        AgentState::Done => "finished",
        AgentState::Failed => "failed",
        AgentState::Working | AgentState::WaitingForUser | AgentState::Cancelled => "was cancelled",
    };
    let mut text = format!(
        "[Artisan] Agent \"{}\" ({}, {}) {outcome}.",
        info.task,
        info.harness.display_name(),
        info.agent
    );
    if let Some(error) = &info.error {
        text.push_str("\nError: ");
        text.push_str(error);
    }
    if let Some(result) = &info.result {
        text.push_str("\nIts last reply:\n\n");
        text.push_str(result);
    }
    text
}

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
#[path = "../tests/agents/unit.rs"]
mod tests;
