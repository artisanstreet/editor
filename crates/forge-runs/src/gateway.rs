use crate::Hub;
use crate::Refused;
use crate::error::ErrorClass;
use artisan_api::{EditorCall, EditorReply, Event, Secret, ThreadId};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub const COMPUTER_USE_KEY: &str = "computer_use";

const EDITOR_ANSWERS_WITHIN: Duration = Duration::from_secs(90);

const NO_EDITOR: &str =
    "No Editor answered. This needs the Artisan Editor open and connected to this Forge.";

#[derive(Debug, thiserror::Error)]
pub enum CallEditorError {
    #[error("the Editor refused call {call}")]
    Editor {
        call: String,
        #[source]
        refused: Refused,
    },
    #[error("no Editor answered call {call} within {} s", EDITOR_ANSWERS_WITHIN.as_secs())]
    TimedOut {
        call: String,
        #[source]
        refused: Refused,
    },
    #[error("call {call} was dropped before an Editor answered")]
    Dropped {
        call: String,
        #[source]
        refused: Refused,
    },
}

impl CallEditorError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Editor { .. } | Self::TimedOut { .. } | Self::Dropped { .. } => {
                ErrorClass::Conflict
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("read whether computer use is on for the run of thread {thread}")]
    Setting {
        thread: ThreadId,
        #[source]
        source: forge_store::ReadSettingError,
    },
    #[error(
        "the {COMPUTER_USE_KEY} setting is neither true nor false for the run of thread {thread}"
    )]
    NotOnOrOff { thread: ThreadId },
    #[error("make the gateway credential of the run of thread {thread}")]
    Token {
        thread: ThreadId,
        #[source]
        source: getrandom::Error,
    },
}

#[derive(Debug)]
struct PendingCall {
    call: EditorCall,
    answer: oneshot::Sender<EditorReply>,
}

struct CallGuard {
    hub: Hub,
    id: String,
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        self.hub.gateway_lock().calls.remove(&self.id);
    }
}

#[derive(Clone, Debug)]
pub struct GatewayCaller {
    pub thread: ThreadId,
    pub ended: CancellationToken,
}

#[derive(Debug, Default)]
pub(crate) struct Gateway {
    base: Option<String>,
    computer: bool,
    passes: HashMap<String, GatewayCaller>,
    calls: HashMap<String, PendingCall>,
}

#[derive(Debug)]
pub(crate) struct GatewayPass {
    hub: Hub,
    token: Option<Secret>,
    ended: CancellationToken,
    servers: Vec<(String, String)>,
}

impl GatewayPass {
    #[must_use]
    pub(crate) fn servers(&self) -> &[(String, String)] {
        &self.servers
    }
}

impl Drop for GatewayPass {
    fn drop(&mut self) {
        if let Some(token) = &self.token {
            self.hub.gateway_lock().passes.remove(token.expose());
        }
        self.ended.cancel();
    }
}

impl Hub {
    pub fn set_gateway(&self, base: String, computer: bool) {
        let mut gateway = self.gateway_lock();
        gateway.base = Some(base);
        gateway.computer = computer;
    }

    #[must_use]
    pub fn gateway_caller(&self, token: &str) -> Option<GatewayCaller> {
        self.gateway_lock().passes.get(token).cloned()
    }

    #[must_use]
    pub fn run_cancellation(&self, thread: &ThreadId) -> Option<CancellationToken> {
        self.lock().get(thread).map(|run| run.cancel.child_token())
    }

    pub(crate) fn revoke_gateway_credential(&self, thread: &ThreadId) {
        self.gateway_lock().passes.retain(|_, holder| {
            let kept = holder.thread != *thread;
            if !kept {
                holder.ended.cancel();
            }
            kept
        });
    }

    pub(crate) async fn gateway_pass(
        &self,
        thread: &ThreadId,
    ) -> Result<GatewayPass, GatewayError> {
        let (base, computer) = {
            let gateway = self.gateway_lock();
            (gateway.base.clone(), gateway.computer)
        };
        let ended = CancellationToken::new();
        let Some(base) = base else {
            return Ok(GatewayPass {
                hub: self.clone(),
                token: None,
                ended,
                servers: Vec::new(),
            });
        };
        let computer = computer && self.computer_use_on(thread).await?;
        let token = self.issue_gateway_token(GatewayCaller {
            thread: thread.clone(),
            ended: ended.clone(),
        })?;
        let mut servers = vec![(
            forge_engines::ARTISAN_MCP.to_owned(),
            format!("{base}{}/{}", forge_engines::ARTISAN_MCP, token.expose()),
        )];
        if computer {
            servers.push((
                forge_engines::COMPUTER_MCP.to_owned(),
                format!("{base}{}/{}", forge_engines::COMPUTER_MCP, token.expose()),
            ));
        }
        Ok(GatewayPass {
            hub: self.clone(),
            token: Some(token),
            ended,
            servers,
        })
    }

    async fn computer_use_on(&self, thread: &ThreadId) -> Result<bool, GatewayError> {
        let value = self
            .store()
            .setting(COMPUTER_USE_KEY)
            .await
            .map_err(|source| GatewayError::Setting {
                thread: thread.clone(),
                source,
            })?;
        match value {
            None => Ok(false),
            Some(serde_json::Value::Bool(on)) => Ok(on),
            Some(_) => Err(GatewayError::NotOnOrOff {
                thread: thread.clone(),
            }),
        }
    }

    fn issue_gateway_token(&self, caller: GatewayCaller) -> Result<Secret, GatewayError> {
        let mut bytes = [0_u8; 24];
        getrandom::fill(&mut bytes).map_err(|source| GatewayError::Token {
            thread: caller.thread.clone(),
            source,
        })?;
        let token = hex::encode(bytes);
        self.gateway_lock().passes.insert(token.clone(), caller);
        Ok(Secret::from(token.as_str()))
    }

    pub async fn call_editor(
        &self,
        call: EditorCall,
    ) -> Result<serde_json::Value, CallEditorError> {
        let id = artisan_api::ItemId::new().to_string();
        let (answer, answered) = oneshot::channel();
        let pending = PendingCall {
            call: call.clone(),
            answer,
        };
        self.gateway_lock().calls.insert(id.clone(), pending);
        let guard = CallGuard {
            hub: self.clone(),
            id: id.clone(),
        };
        self.store().publish_live(Event::EditorCall {
            id: id.clone(),
            call,
        });
        let reply = tokio::time::timeout(EDITOR_ANSWERS_WITHIN, answered).await;
        drop(guard);
        let no_editor = || Refused(NO_EDITOR.to_owned());
        match reply {
            Ok(Ok(EditorReply::Ok(value))) => Ok(value),
            Ok(Ok(EditorReply::Error(error))) => Err(CallEditorError::Editor {
                call: id,
                refused: Refused(error),
            }),
            Ok(Err(oneshot::error::RecvError { .. })) => Err(CallEditorError::Dropped {
                call: id,
                refused: no_editor(),
            }),
            Err(tokio::time::error::Elapsed { .. }) => Err(CallEditorError::TimedOut {
                call: id,
                refused: no_editor(),
            }),
        }
    }

    pub(crate) fn answer_editor_call(&self, id: &str, reply: EditorReply) -> bool {
        let pending = self.gateway_lock().calls.remove(id);
        pending.is_some_and(|pending| pending.answer.send(reply).is_ok())
    }

    pub(crate) fn pending_editor_calls(&self) -> Vec<Event> {
        self.gateway_lock()
            .calls
            .iter()
            .map(|(id, pending)| Event::EditorCall {
                id: id.clone(),
                call: pending.call.clone(),
            })
            .collect()
    }

    fn gateway_lock(&self) -> std::sync::MutexGuard<'_, Gateway> {
        self.inner
            .gateway
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
