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
        drop(self.hub.inner.gateway.close_call(&self.id));
    }
}

#[derive(Clone, Debug)]
pub struct GatewayCaller {
    pub thread: ThreadId,
    pub ended: CancellationToken,
    pub run_stopped: Option<CancellationToken>,
}

impl GatewayCaller {
    pub async fn finished(&self) {
        match &self.run_stopped {
            Some(stopped) => tokio::select! {
                () = self.ended.cancelled() => {}
                () = stopped.cancelled() => {}
            },
            None => self.ended.cancelled().await,
        }
    }
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
struct Routes {
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
        if let Some((thread, pass)) = &self.held {
            self.hub.inner.gateway.release(thread, *pass);
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Gateway(std::sync::Mutex<Routes>);

impl Gateway {
    fn routes(&self) -> std::sync::MutexGuard<'_, Routes> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set(&self, base: String, computer: bool) {
        let mut routes = self.routes();
        routes.base = Some(base);
        routes.computer = computer;
    }

    fn address(&self) -> Option<(String, bool)> {
        let routes = self.routes();
        routes.base.clone().map(|base| (base, routes.computer))
    }

    fn caller(&self, token: &str) -> Result<(ThreadId, CancellationToken), GatewayRefusal> {
        let routes = self.routes();
        let thread = routes.holders.get(token).ok_or(GatewayRefusal::Unknown)?;
        let credential = routes.credentials.get(thread).unwrap_or_else(|| {
            unreachable!(
                "invariant: thread {thread} holds a gateway address it has no credential for"
            )
        });
        match &credential.activity {
            Activity::Idle => Err(GatewayRefusal::Idle {
                thread: thread.clone(),
            }),
            Activity::Live { ended, .. } => Ok((thread.clone(), ended.clone())),
        }
    }

    fn revoke(&self, thread: &ThreadId) {
        let mut routes = self.routes();
        if let Some(credential) = routes.credentials.remove(thread) {
            routes.holders.remove(&credential.token);
            credential.activity.end();
        }
    }

    fn admit(&self, thread: &ThreadId) -> Result<(Secret, u64), GatewayError> {
        let mut routes = self.routes();
        let routes = &mut *routes;
        let credential = match routes.credentials.entry(thread.clone()) {
            std::collections::hash_map::Entry::Occupied(held) => held.into_mut(),
            std::collections::hash_map::Entry::Vacant(free) => {
                let mut bytes = [0_u8; 24];
                getrandom::fill(&mut bytes).map_err(|source| GatewayError::Token {
                    thread: thread.clone(),
                    source,
                })?;
                let token = hex::encode(bytes);
                routes.holders.insert(token.clone(), thread.clone());
                free.insert(Credential {
                    token,
                    activity: Activity::Idle,
                })
            }
        };
        let pass = routes.next_pass;
        routes.next_pass += 1;
        credential.activity.admit(pass);
        Ok((Secret::from(credential.token.as_str()), pass))
    }

    fn release(&self, thread: &ThreadId, pass: u64) {
        if let Some(credential) = self.routes().credentials.get_mut(thread) {
            credential.activity.release(pass);
        }
    }

    fn open_call(&self, id: String, pending: PendingCall) {
        self.routes().calls.insert(id, pending);
    }

    fn close_call(&self, id: &str) -> Option<PendingCall> {
        self.routes().calls.remove(id)
    }

    fn pending_calls(&self) -> Vec<Event> {
        self.routes()
            .calls
            .iter()
            .map(|(id, pending)| Event::EditorCall {
                id: id.clone(),
                call: pending.call.clone(),
            })
            .collect()
    }
}

impl Hub {
    pub fn set_gateway(&self, base: String, computer: bool) {
        self.inner.gateway.set(base, computer);
    }

    pub fn gateway_caller(&self, token: &str) -> Result<GatewayCaller, GatewayRefusal> {
        let (thread, ended) = self.inner.gateway.caller(token)?;
        let run_stopped = self.run_cancellation(&thread);
        Ok(GatewayCaller {
            thread,
            ended,
            run_stopped,
        })
    }

    #[must_use]
    pub fn run_cancellation(&self, thread: &ThreadId) -> Option<CancellationToken> {
        self.lock().get(thread).map(|run| run.cancel.child_token())
    }

    pub(crate) fn revoke_gateway_credential(&self, thread: &ThreadId) {
        self.inner.gateway.revoke(thread);
    }

    pub(crate) async fn gateway_pass(
        &self,
        thread: &ThreadId,
    ) -> Result<GatewayPass, GatewayError> {
        let Some((base, computer)) = self.inner.gateway.address() else {
            return Ok(GatewayPass {
                hub: self.clone(),
                held: None,
                servers: Vec::new(),
            });
        };
        let computer = computer && self.computer_use_on(thread).await?;
        let (token, pass) = self.inner.gateway.admit(thread)?;
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
        self.inner.gateway.open_call(id.clone(), pending);
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
        let pending = self.inner.gateway.close_call(id);
        pending.is_some_and(|pending| pending.answer.send(reply).is_ok())
    }

    pub(crate) fn pending_editor_calls(&self) -> Vec<Event> {
        self.inner.gateway.pending_calls()
    }
}
