mod registry;

pub use registry::{
    Generation, Holds, Member, Overlap, Peer, Phase, Registry, RegistryFileError, UpgradeLock,
    read, write,
};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Reply {
    Ok(Value),
    Error(String),
}

pub async fn request(member: &Member, command: &Command) -> anyhow::Result<Value> {
    let reply: Reply = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(30))
        .build()?
        .post(format!("{}/control", member.control))
        .bearer_auth(&member.token)
        .json(command)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    match reply {
        Reply::Ok(value) => Ok(value),
        Reply::Error(error) => anyhow::bail!("generation {}: {error}", member.id),
    }
}

enum Drained {
    Reported(BTreeSet<ThreadId>),
    Unreported,
}

async fn drain_peer(member: &Member) -> anyhow::Result<Drained> {
    Ok(match request(member, &Command::Drain).await? {
        Value::Null => Drained::Unreported,
        threads @ (Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Array(_)
        | Value::Object(_)) => Drained::Reported(serde_json::from_value(threads)?),
    })
}

impl crate::Hub {
    pub async fn control(&self, command: Command) -> anyhow::Result<Value> {
        match command {
            Command::Health => Ok(
                serde_json::json!({"protocol":PROTOCOL,"schema":forge_store::Store::schema_version(),"active":self.generation().is_some_and(Generation::accepting),"runs":self.local_active_count(),"generation":self.generation().map(|generation|&generation.member)}),
            ),
            Command::Activate => {
                self.activate().await?;
                Ok(Value::Null)
            }
            Command::Drain => Ok(serde_json::to_value(self.drain().await?)?),
            Command::Accept {
                thread,
                message,
                acceptance,
            } => Ok(serde_json::to_value(
                self.accept_here(&thread, message, acceptance).await?,
            )?),
            Command::Answer { item, answer } => {
                self.answer_here(&item, answer).await?;
                Ok(Value::Null)
            }
            Command::Cancel { thread } => {
                self.cancel_local(&thread).await?;
                Ok(Value::Null)
            }
            Command::Update { thread, update } => Ok(serde_json::to_value(
                self.update_here(&thread, update).await?,
            )?),
            Command::ClearGoal { thread } => {
                self.clear_goal_here(&thread).await?;
                Ok(Value::Null)
            }
            Command::EditorReply { id, reply } => {
                Ok(Value::Bool(self.answer_editor_call(&id, reply)))
            }
            Command::Snapshot => Ok(serde_json::to_value(self.live_snapshot().1)?),
            Command::Retire | Command::Stop => {
                anyhow::bail!("lifecycle commands must be handled by the generation server")
            }
        }
    }

    fn managed(&self) -> anyhow::Result<&Generation> {
        self.generation()
            .ok_or_else(|| anyhow::anyhow!("not a managed generation"))
    }

    async fn activate(&self) -> anyhow::Result<()> {
        let generation = self.managed()?;
        if generation.accepting() {
            return Ok(());
        }
        let mut peers = Vec::new();
        for member in generation.registry.peers()? {
            if member.id == generation.member.id {
                continue;
            }
            let holds = match drain_peer(&member).await {
                Ok(Drained::Reported(threads)) => Holds::Reported(threads),
                Ok(Drained::Unreported) => {
                    tracing::info!(generation = %member.id, "stopping a generation that cannot report its threads; its turns carry on here");
                    if let Err(error) = request(&member, &Command::Stop).await {
                        tracing::warn!(generation = %member.id, "stop a generation that cannot report its threads: {error:#}");
                    }
                    Holds::Unreported(BTreeSet::new())
                }
                Err(error) => {
                    tracing::warn!(generation = %member.id, "drain a generation before taking over: {error:#}");
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

    async fn drain(&self) -> anyhow::Result<Vec<ThreadId>> {
        self.managed()?.set_phase(Phase::Draining);
        let _admission = self.inner.admission.lock().await;
        Ok(self.owned_thread_ids())
    }

    pub async fn follow_peers(&self) -> anyhow::Result<()> {
        let Some(generation) = self.generation() else {
            return Ok(());
        };
        let tracked = generation.peers();
        if tracked.is_empty() {
            return Ok(());
        }
        let alive = generation.registry.peers()?;
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
                        tracing::debug!(generation = %id, "ask a draining generation for its threads: {error:#}");
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
    ) -> anyhow::Result<bool> {
        if self.answer_editor_call(id, reply.clone()) {
            return Ok(true);
        }
        let Some(generation) = self
            .generation()
            .filter(|generation| generation.overlapping())
        else {
            return Ok(false);
        };
        for member in generation.registry.peers()? {
            if member.id == generation.member.id {
                continue;
            }
            if request(
                &member,
                &Command::EditorReply {
                    id: id.to_owned(),
                    reply: reply.clone(),
                },
            )
            .await?
            .as_bool()
                == Some(true)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub async fn distributed_live_snapshot(
        &self,
    ) -> anyhow::Result<(u64, Vec<artisan_api::Event>)> {
        let (revision, mut events) = self.live_snapshot();
        if let Some(generation) = self
            .generation()
            .filter(|generation| generation.overlapping())
        {
            for member in generation.registry.peers()? {
                if member.id == generation.member.id {
                    continue;
                }
                match request(&member, &Command::Snapshot).await {
                    Ok(value) => {
                        events.extend(serde_json::from_value::<Vec<artisan_api::Event>>(value)?);
                    }
                    Err(error) => {
                        tracing::debug!(generation=%member.id,"snapshot from retiring generation: {error:#}");
                    }
                }
            }
        }
        Ok((revision, events))
    }

    pub fn runtime_changed(&self) {
        self.agents_moved();
        self.resume_pending();
    }
}
