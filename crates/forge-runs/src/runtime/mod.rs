mod registry;

pub use registry::{
    Generation, Holds, Member, Overlap, Peer, Phase, Registry, RegistryFileError, UpgradeLock,
    read, write,
};

use crate::error::ErrorClass;
use artisan_api::{Answer, EditorReply, ItemId, SendMessage, ThreadId};
use forge_store::Acceptance;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const PROTOCOL: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Health,
    Activate,
    Drain,
    Retire,
    Stop,
    Accept {
        thread: ThreadId,
        message: SendMessage,
        acceptance: Acceptance,
    },
    Answer {
        item: ItemId,
        answer: Answer,
    },
    Cancel {
        thread: ThreadId,
    },
    Update {
        thread: ThreadId,
        update: artisan_api::UpdateThread,
    },
    ClearGoal {
        thread: ThreadId,
    },
    EditorReply {
        id: String,
        reply: EditorReply,
    },
    Snapshot,
}

impl Command {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::Activate => "activate",
            Self::Drain => "drain",
            Self::Retire => "retire",
            Self::Stop => "stop",
            Self::Accept { .. } => "accept",
            Self::Answer { .. } => "answer",
            Self::Cancel { .. } => "cancel",
            Self::Update { .. } => "update",
            Self::ClearGoal { .. } => "clear_goal",
            Self::EditorReply { .. } => "editor_reply",
            Self::Snapshot => "snapshot",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct RemoteFailure {
    pub class: ErrorClass,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReplyFailure {
    Typed(RemoteFailure),
    Untyped(String),
}

impl ReplyFailure {
    fn into_remote(self) -> RemoteFailure {
        match self {
            Self::Typed(failure) => failure,
            Self::Untyped(message) => RemoteFailure {
                class: ErrorClass::Internal,
                message,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Reply {
    Ok(Value),
    Error(ReplyFailure),
}

#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("build the client that reaches generation {generation}")]
    Client {
        generation: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("send {command} to generation {generation}")]
    Send {
        generation: String,
        command: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("generation {generation} turned down {command}")]
    Rejected {
        generation: String,
        command: &'static str,
        #[source]
        failure: RemoteFailure,
    },
    #[error("read generation {generation}'s reply to {command}")]
    Decode {
        generation: String,
        command: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

impl PeerError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Rejected { failure, .. } => failure.class,
            Self::Client { .. } | Self::Send { .. } | Self::Decode { .. } => ErrorClass::Internal,
        }
    }
}

pub async fn request(member: &Member, command: &Command) -> Result<Value, PeerError> {
    let name = command.name();
    let sent = |source| PeerError::Send {
        generation: member.id.clone(),
        command: name,
        source,
    };
    let reply: Reply = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|source| PeerError::Client {
            generation: member.id.clone(),
            source,
        })?
        .post(format!("{}/control", member.control))
        .bearer_auth(&member.token)
        .json(command)
        .send()
        .await
        .map_err(sent)?
        .error_for_status()
        .map_err(sent)?
        .json()
        .await
        .map_err(sent)?;
    match reply {
        Reply::Ok(value) => Ok(value),
        Reply::Error(failure) => Err(PeerError::Rejected {
            generation: member.id.clone(),
            command: name,
            failure: failure.into_remote(),
        }),
    }
}

pub async fn request_as<T: serde::de::DeserializeOwned>(
    member: &Member,
    command: &Command,
) -> Result<T, PeerError> {
    serde_json::from_value(request(member, command).await?).map_err(|source| PeerError::Decode {
        generation: member.id.clone(),
        command: command.name(),
        source,
    })
}

enum Drained {
    Reported(BTreeSet<ThreadId>),
    Unreported,
}

async fn drain_peer(member: &Member) -> Result<Drained, PeerError> {
    Ok(match request(member, &Command::Drain).await? {
        Value::Null => Drained::Unreported,
        threads @ (Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Array(_)
        | Value::Object(_)) => {
            Drained::Reported(serde_json::from_value(threads).map_err(|source| {
                PeerError::Decode {
                    generation: member.id.clone(),
                    command: "drain",
                    source,
                }
            })?)
        }
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("this Forge is not a managed generation")]
    Unmanaged,
    #[error(transparent)]
    Activate(#[from] ActivateError),
    #[error(transparent)]
    Accept(Box<crate::SendError>),
    #[error(transparent)]
    Answer(Box<crate::AnswerError>),
    #[error(transparent)]
    Cancel(Box<crate::CancelError>),
    #[error(transparent)]
    Update(Box<crate::UpdateError>),
    #[error(transparent)]
    ClearGoal(Box<crate::ClearGoalError>),
    #[error("encode the reply to {command}")]
    Encode {
        command: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("{command} is handled by the generation server")]
    Lifecycle { command: &'static str },
}

impl ControlError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Accept(error) => error.class(),
            Self::Answer(error) => error.class(),
            Self::Cancel(error) => error.class(),
            Self::Update(error) => error.class(),
            Self::ClearGoal(error) => error.class(),
            Self::Unmanaged | Self::Activate(_) | Self::Encode { .. } | Self::Lifecycle { .. } => {
                ErrorClass::Internal
            }
        }
    }

    #[must_use]
    pub fn failure(&self) -> RemoteFailure {
        let class = self.class();
        let refused =
            std::iter::successors(Some(self as &(dyn std::error::Error + 'static)), |cause| {
                cause.source()
            })
            .find_map(|cause| cause.downcast_ref::<crate::Refused>())
            .filter(|_| class != ErrorClass::Internal);
        RemoteFailure {
            class,
            message: refused.map_or_else(|| diagnostics::chain(self), |refused| refused.0.clone()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ActivateError {
    #[error("this Forge is not a managed generation")]
    Unmanaged,
    #[error("list the other generations before taking over")]
    Peers(#[source] RegistryFileError),
    #[error(transparent)]
    Running(#[from] forge_store::RunningThreadsError),
    #[error(transparent)]
    Recover(#[from] forge_store::RecoverThreadError),
}

#[derive(Debug, thiserror::Error)]
pub enum FollowPeersError {
    #[error("list the live generations")]
    Peers(#[source] RegistryFileError),
    #[error(transparent)]
    Recover(#[from] forge_store::RecoverThreadError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorCall {
    Answered,
    NotWaiting,
}

#[derive(Debug, thiserror::Error)]
pub enum EditorReplyError {
    #[error("list the generations that may wait for Editor call {call}")]
    Peers {
        call: String,
        #[source]
        source: RegistryFileError,
    },
    #[error("ask generation {generation} whether it waits for Editor call {call}")]
    Peer {
        call: String,
        generation: String,
        #[source]
        source: Box<PeerError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("list the generations whose live text a new client needs")]
    Peers(#[source] RegistryFileError),
}

fn encode(command: &'static str, value: impl Serialize) -> Result<Value, ControlError> {
    serde_json::to_value(value).map_err(|source| ControlError::Encode { command, source })
}

impl crate::Hub {
    pub async fn control(&self, command: Command) -> Result<Value, ControlError> {
        let name = command.name();
        match command {
            Command::Health => Ok(
                serde_json::json!({"protocol":PROTOCOL,"schema":forge_store::Store::schema_version(),"active":self.generation().is_some_and(Generation::accepting),"runs":self.local_active_count(),"generation":self.generation().map(|generation|&generation.member)}),
            ),
            Command::Activate => {
                self.activate().await?;
                Ok(Value::Null)
            }
            Command::Drain => encode(name, self.drain().await?),
            Command::Accept {
                thread,
                message,
                acceptance,
            } => encode(
                name,
                self.accept_here(&thread, message, acceptance)
                    .await
                    .map_err(|error| ControlError::Accept(Box::new(error)))?,
            ),
            Command::Answer { item, answer } => {
                self.answer_here(&item, answer)
                    .await
                    .map_err(|error| ControlError::Answer(Box::new(error)))?;
                Ok(Value::Null)
            }
            Command::Cancel { thread } => {
                self.cancel_local(&thread)
                    .await
                    .map_err(|error| ControlError::Cancel(Box::new(error)))?;
                Ok(Value::Null)
            }
            Command::Update { thread, update } => encode(
                name,
                self.update_here(&thread, update)
                    .await
                    .map_err(|error| ControlError::Update(Box::new(error)))?,
            ),
            Command::ClearGoal { thread } => {
                self.clear_goal_here(&thread)
                    .await
                    .map_err(|error| ControlError::ClearGoal(Box::new(error)))?;
                Ok(Value::Null)
            }
            Command::EditorReply { id, reply } => {
                Ok(Value::Bool(self.answer_editor_call(&id, reply)))
            }
            Command::Snapshot => encode(name, self.live_snapshot().1),
            Command::Retire | Command::Stop => Err(ControlError::Lifecycle { command: name }),
        }
    }

    async fn activate(&self) -> Result<(), ActivateError> {
        let generation = self.generation().ok_or(ActivateError::Unmanaged)?;
        if generation.accepting() {
            return Ok(());
        }
        let mut peers = Vec::new();
        for member in generation.registry.peers().map_err(ActivateError::Peers)? {
            if member.id == generation.member.id {
                continue;
            }
            let holds = match drain_peer(&member).await {
                Ok(Drained::Reported(threads)) => Holds::Reported(threads),
                Ok(Drained::Unreported) => {
                    tracing::info!(generation = %member.id, "stopping a generation that cannot report its threads; its turns carry on here");
                    if let Err(error) = request(&member, &Command::Stop).await {
                        tracing::warn!(generation = %member.id, "stop a generation that cannot report its threads: {}", diagnostics::chain(&error));
                    }
                    Holds::Unreported(BTreeSet::new())
                }
                Err(error) => {
                    tracing::warn!(generation = %member.id, "drain a generation before taking over: {}", diagnostics::chain(&error));
                    Holds::Unreported(BTreeSet::new())
                }
            };
            peers.push(Peer { member, holds });
        }
        let reported: BTreeSet<ThreadId> = peers
            .iter()
            .filter_map(|peer| match &peer.holds {
                Holds::Reported(threads) => Some(threads.iter().cloned()),
                Holds::Unreported(_) => None,
            })
            .flatten()
            .collect();
        let local: BTreeSet<ThreadId> = self.owned_thread_ids().into_iter().collect();
        let unclaimed: BTreeSet<ThreadId> = self
            .store()
            .running_threads()
            .await?
            .into_iter()
            .filter(|thread| !reported.contains(thread) && !local.contains(thread))
            .collect();
        let unreported = peers
            .iter()
            .any(|peer| matches!(peer.holds, Holds::Unreported(_)));
        if unreported {
            for peer in &mut peers {
                if let Holds::Unreported(threads) = &mut peer.holds {
                    threads.clone_from(&unclaimed);
                }
            }
        } else {
            for thread in unclaimed {
                self.store().recover_thread(thread).await?;
            }
        }
        generation.track(peers);
        generation.set_phase(Phase::Active);
        self.resume_pending();
        Ok(())
    }

    async fn drain(&self) -> Result<Vec<ThreadId>, ControlError> {
        self.generation()
            .ok_or(ControlError::Unmanaged)?
            .set_phase(Phase::Draining);
        let _admission = self.inner.admission.lock().await;
        Ok(self.owned_thread_ids())
    }

    pub async fn follow_peers(&self) -> Result<(), FollowPeersError> {
        let Some(generation) = self.generation() else {
            return Ok(());
        };
        let tracked = generation.peers();
        if tracked.is_empty() {
            return Ok(());
        }
        let alive = generation
            .registry
            .peers()
            .map_err(FollowPeersError::Peers)?;
        for peer in tracked {
            let id = &peer.member.id;
            if !alive.iter().any(|member| &member.id == id) {
                for thread in generation.forget(id) {
                    if generation.owner(&thread).is_none() {
                        self.store().recover_thread(thread).await?;
                    }
                }
                continue;
            }
            if matches!(peer.holds, Holds::Reported(_)) {
                match drain_peer(&peer.member).await {
                    Ok(Drained::Reported(threads)) => generation.reported(id, threads),
                    Ok(Drained::Unreported) => {}
                    Err(error) => {
                        tracing::debug!(generation = %id, "ask a draining generation for its threads: {}", diagnostics::chain(&error));
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn answer_distributed_editor_call(
        &self,
        id: &str,
        reply: EditorReply,
    ) -> Result<EditorCall, EditorReplyError> {
        if self.answer_editor_call(id, reply.clone()) {
            return Ok(EditorCall::Answered);
        }
        let Some(generation) = self
            .generation()
            .filter(|generation| generation.overlapping())
        else {
            return Ok(EditorCall::NotWaiting);
        };
        let peers = generation
            .registry
            .peers()
            .map_err(|source| EditorReplyError::Peers {
                call: id.to_owned(),
                source,
            })?;
        let mut unreachable = None;
        for member in peers {
            if member.id == generation.member.id {
                continue;
            }
            let command = Command::EditorReply {
                id: id.to_owned(),
                reply: reply.clone(),
            };
            match request(&member, &command).await {
                Ok(Value::Bool(true)) => return Ok(EditorCall::Answered),
                Ok(
                    Value::Null
                    | Value::Bool(false)
                    | Value::Number(_)
                    | Value::String(_)
                    | Value::Array(_)
                    | Value::Object(_),
                ) => {}
                Err(source) => {
                    unreachable.get_or_insert(EditorReplyError::Peer {
                        call: id.to_owned(),
                        generation: member.id.clone(),
                        source: Box::new(source),
                    });
                }
            }
        }
        unreachable.map_or(Ok(EditorCall::NotWaiting), Err)
    }

    pub async fn distributed_live_snapshot(
        &self,
    ) -> Result<(u64, Vec<artisan_api::Event>), SnapshotError> {
        let (revision, mut events) = self.live_snapshot();
        if let Some(generation) = self
            .generation()
            .filter(|generation| generation.overlapping())
        {
            for member in generation.registry.peers().map_err(SnapshotError::Peers)? {
                if member.id == generation.member.id {
                    continue;
                }
                match request_as::<Vec<artisan_api::Event>>(&member, &Command::Snapshot).await {
                    Ok(peer) => events.extend(peer),
                    Err(error) => {
                        tracing::debug!(generation = %member.id, "snapshot from a retiring generation: {}", diagnostics::chain(&error));
                    }
                }
            }
        }
        crate::sort_snapshot(&mut events);
        Ok((revision, events))
    }

    pub fn runtime_changed(&self) {
        self.agents_moved();
        self.resume_pending();
    }
}
