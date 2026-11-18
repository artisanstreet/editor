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

#[derive(Default)]
pub(crate) struct Gateway {
    base: Option<String>,
    computer: bool,
    tokens: HashMap<ThreadId, Secret>,
    threads: HashMap<String, ThreadId>,
    calls: HashMap<String, PendingCall>,
}

impl Hub {
    pub fn set_gateway(&self, base: String, computer: bool) {
        let mut gateway = self.gateway_lock();
        gateway.base = Some(base);
        gateway.computer = computer;
    }

    #[must_use]
    pub fn gateway_thread(&self, token: &str) -> Option<ThreadId> {
        let gateway = self.gateway_lock();
        gateway
            .threads
            .get(token)
            .filter(|thread| {
                gateway
                    .tokens
                    .get(*thread)
                    .is_some_and(|issued| issued.expose() == token)
            })
            .cloned()
    }

    #[must_use]
    pub fn run_cancellation(&self, thread: &ThreadId) -> Option<CancellationToken> {
        self.lock().get(thread).map(|run| run.cancel.child_token())
    }

    pub(crate) fn revoke_gateway_credential(&self, thread: &ThreadId) {
        let mut gateway = self.gateway_lock();
        if let Some(token) = gateway.tokens.remove(thread) {
            gateway.threads.remove(token.expose());
        }
    }

    pub(crate) async fn gateway_servers(&self, thread: &ThreadId) -> Vec<(String, String)> {
        let computer = self.gateway_lock().computer && self.computer_use_on().await;
        let Some((base, token)) = self.gateway_credential(thread) else {
            return Vec::new();
        };
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

    fn gateway_credential(&self, thread: &ThreadId) -> Option<(String, String)> {
        let mut gateway = self.gateway_lock();
        let base = gateway.base.clone()?;
        let token = if let Some(token) = gateway.tokens.get(thread) {
            token.expose().to_owned()
        } else {
            let mut bytes = [0_u8; 24];
            if let Err(error) = getrandom::fill(&mut bytes) {
                tracing::error!(%thread, "making the thread's gateway credential: {error}");
                return None;
            }
            let token = hex::encode(bytes);
            gateway
                .tokens
                .insert(thread.clone(), Secret::from(token.as_str()));
            gateway.threads.insert(token.clone(), thread.clone());
            token
        };
        drop(gateway);
        Some((base, token))
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
