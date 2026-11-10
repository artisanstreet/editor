mod registry;

pub use registry::{Generation, Lease, Member, Registry};

use artisan_api::{Answer, EditorReply, ItemId, SendMessage, ThreadId};
use forge_store::Acceptance;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

impl crate::Hub {
    pub async fn control(&self, command: Command) -> anyhow::Result<Value> {
        match command {
            Command::Health => Ok(
                serde_json::json!({"protocol":PROTOCOL,"schema":forge_store::Store::schema_version(),"active":self.generation().is_some_and(Generation::accepting),"runs":self.local_active_count(),"generation":self.generation().map(|generation|&generation.member)}),
            ),
            Command::Activate => {
                self.generation()
                    .ok_or_else(|| anyhow::anyhow!("not a managed generation"))?
                    .set_accepting(true);
                self.resume_pending();
                Ok(Value::Null)
            }
            Command::Drain => {
                self.generation()
                    .ok_or_else(|| anyhow::anyhow!("not a managed generation"))?
                    .set_accepting(false);
                Ok(Value::Null)
            }
            Command::Accept {
                thread,
                message,
                acceptance,
            } => Ok(serde_json::to_value(
                self.accept_message(&thread, message, acceptance).await?,
            )?),
            Command::Answer { item, answer } => {
                self.answer(&item, answer).await?;
                Ok(Value::Null)
            }
            Command::Cancel { thread } => {
                self.cancel_local(&thread).await?;
                Ok(Value::Null)
            }
            Command::Update { thread, update } => Ok(serde_json::to_value(
                self.update_thread(&thread, update).await?,
            )?),
            Command::ClearGoal { thread } => {
                self.clear_goal(&thread).await?;
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

    pub async fn answer_distributed_editor_call(
        &self,
        id: &str,
        reply: EditorReply,
    ) -> anyhow::Result<bool> {
        if self.answer_editor_call(id, reply.clone()) {
            return Ok(true);
        }
        let Some(generation) = self.generation() else {
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
        if let Some(generation) = self.generation() {
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
