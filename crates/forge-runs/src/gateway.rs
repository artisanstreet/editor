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

    pub(crate) async fn gateway_pass(&self, thread: &ThreadId) -> GatewayPass {
        let computer = self.gateway_lock().computer && self.computer_use_on().await;
        let ended = CancellationToken::new();
        let issued = self.issue_gateway_token(GatewayCaller {
            thread: thread.clone(),
            ended: ended.clone(),
        });
        let servers = issued.as_ref().map_or_else(Vec::new, |(base, token)| {
            let token = token.expose();
            let mut servers = vec![(
                forge_engines::ARTISAN_MCP.to_owned(),
                format!("{base}{}/{token}", forge_engines::ARTISAN_MCP),
            )];
            if computer {
                servers.push((
                    forge_engines::COMPUTER_MCP.to_owned(),
                    format!("{base}{}/{token}", forge_engines::COMPUTER_MCP),
                ));
            }
            servers
        });
        GatewayPass {
            hub: self.clone(),
            token: issued.map(|(_, token)| token),
            ended,
            servers,
        }
    }

    async fn computer_use_on(&self) -> bool {
        match self.store().setting(COMPUTER_USE_KEY).await {
            Ok(value) => value.and_then(|value| value.as_bool()).unwrap_or(false),
            Err(error) => {
                tracing::warn!(
                    key = COMPUTER_USE_KEY,
                    "reading a setting: {}",
                    diagnostics::chain(&error)
                );
                false
            }
        }
    }

    fn issue_gateway_token(&self, caller: GatewayCaller) -> Option<(String, Secret)> {
        let base = self.gateway_lock().base.clone()?;
        let mut bytes = [0_u8; 24];
        if let Err(error) = getrandom::fill(&mut bytes) {
            tracing::error!(thread = %caller.thread, "making the run's gateway credential: {error}");
            return None;
        }
        let token = hex::encode(bytes);
        self.gateway_lock().passes.insert(token.clone(), caller);
        Some((base, Secret::from(token.as_str())))
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
