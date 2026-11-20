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
    #[error("make the gateway credential of thread {thread}")]
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

#[derive(Clone, Debug, thiserror::Error)]
pub enum GatewayRefusal {
    #[error("this MCP address is not valid")]
    Unknown,
    #[error(
        "thread {thread} has no live run, so its Artisan tools are unavailable until its next turn"
    )]
    Idle { thread: ThreadId },
}

#[derive(Debug)]
enum Activity {
    Idle,
    Live {
        passes: std::collections::BTreeSet<u64>,
        ended: CancellationToken,
    },
}

impl Activity {
    fn admit(&mut self, pass: u64) {
        match self {
            Self::Idle => {
                *self = Self::Live {
                    passes: std::collections::BTreeSet::from([pass]),
                    ended: CancellationToken::new(),
                };
            }
            Self::Live { passes, .. } => {
                passes.insert(pass);
            }
        }
    }

    fn release(&mut self, pass: u64) {
        if let Self::Live { passes, ended } = self {
            passes.remove(&pass);
            if passes.is_empty() {
                ended.cancel();
                *self = Self::Idle;
            }
        }
    }

    fn end(&self) {
        match self {
            Self::Idle => {}
            Self::Live { ended, .. } => ended.cancel(),
        }
    }
}

#[derive(Debug)]
struct Credential {
    token: String,
    activity: Activity,
}

#[derive(Debug, Default)]
pub(crate) struct Gateway {
    base: Option<String>,
    computer: bool,
    holders: HashMap<String, ThreadId>,
    credentials: HashMap<ThreadId, Credential>,
    next_pass: u64,
    calls: HashMap<String, PendingCall>,
}

#[derive(Debug)]
pub(crate) struct GatewayPass {
    hub: Hub,
    held: Option<(ThreadId, u64)>,
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
        if let Some((thread, pass)) = &self.held
            && let Some(credential) = self.hub.gateway_lock().credentials.get_mut(thread)
        {
            credential.activity.release(*pass);
        }
    }
}

impl Hub {
    pub fn set_gateway(&self, base: String, computer: bool) {
        let mut gateway = self.gateway_lock();
        gateway.base = Some(base);
        gateway.computer = computer;
    }

    pub fn gateway_caller(&self, token: &str) -> Result<GatewayCaller, GatewayRefusal> {
        let gateway = self.gateway_lock();
        let thread = gateway.holders.get(token).ok_or(GatewayRefusal::Unknown)?;
        let credential = gateway.credentials.get(thread).unwrap_or_else(|| {
            unreachable!(
                "invariant: thread {thread} holds a gateway address it has no credential for"
            )
        });
        match &credential.activity {
            Activity::Idle => Err(GatewayRefusal::Idle {
                thread: thread.clone(),
            }),
            Activity::Live { ended, .. } => Ok(GatewayCaller {
                thread: thread.clone(),
                ended: ended.clone(),
            }),
        }
    }

    #[must_use]
    pub fn run_cancellation(&self, thread: &ThreadId) -> Option<CancellationToken> {
        self.lock().get(thread).map(|run| run.cancel.child_token())
    }

    pub(crate) fn revoke_gateway_credential(&self, thread: &ThreadId) {
        let mut gateway = self.gateway_lock();
        if let Some(credential) = gateway.credentials.remove(thread) {
            gateway.holders.remove(&credential.token);
            credential.activity.end();
        }
    }

    pub(crate) async fn gateway_pass(
        &self,
        thread: &ThreadId,
    ) -> Result<GatewayPass, GatewayError> {
        let (base, computer) = {
            let gateway = self.gateway_lock();
            (gateway.base.clone(), gateway.computer)
        };
        let Some(base) = base else {
            return Ok(GatewayPass {
                hub: self.clone(),
                held: None,
                servers: Vec::new(),
            });
        };
        let computer = computer && self.computer_use_on(thread).await?;
        let (token, pass) = self.admit_pass(thread)?;
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
            held: Some((thread.clone(), pass)),
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

    fn admit_pass(&self, thread: &ThreadId) -> Result<(Secret, u64), GatewayError> {
        let mut gateway = self.gateway_lock();
        let gateway = &mut *gateway;
        let credential = match gateway.credentials.entry(thread.clone()) {
            std::collections::hash_map::Entry::Occupied(held) => held.into_mut(),
            std::collections::hash_map::Entry::Vacant(free) => {
                let mut bytes = [0_u8; 24];
                getrandom::fill(&mut bytes).map_err(|source| GatewayError::Token {
                    thread: thread.clone(),
                    source,
                })?;
                let token = hex::encode(bytes);
                gateway.holders.insert(token.clone(), thread.clone());
                free.insert(Credential {
                    token,
                    activity: Activity::Idle,
                })
            }
        };
        let pass = gateway.next_pass;
        gateway.next_pass += 1;
        credential.activity.admit(pass);
        Ok((Secret::from(credential.token.as_str()), pass))
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
