//! How engines reach Forge's own MCP servers (the Artisan MCP, and computer
//! use where this machine has it): Forge's loopback gateway, and one
//! credential per thread for as long as this Forge runs. The credential is
//! the caller: a request carrying it acts for that thread, on every server.

use crate::Hub;
use anyhow::Result;
use artisan_api::{EditorCall, EditorReply, Event, ThreadId};
use forge_store::Refused;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::oneshot;

/// Whether engines get computer use; default off.
pub const COMPUTER_USE_KEY: &str = "computer_use";

/// How long an Editor gets to act on a call; pairing a host takes longest.
const EDITOR_ANSWERS_WITHIN: Duration = Duration::from_secs(90);

#[derive(Default)]
pub struct Gateway {
    /// `http://127.0.0.1:<port>/mcp/`, once the gateway listens.
    base: Option<String>,
    /// The gateway serves computer use (this machine has a display for it).
    computer: bool,
    tokens: HashMap<ThreadId, String>,
    threads: HashMap<String, ThreadId>,
    /// Calls sent to Editors, waiting for the first answer.
    calls: HashMap<String, oneshot::Sender<EditorReply>>,
}

impl Hub {
    /// Where the gateway serves its servers (`<base><server>/<credential>`),
    /// and whether computer use is one of them; set once it listens.
    pub fn set_gateway(&self, base: String, computer: bool) {
        let mut gateway = self.gateway_lock();
        gateway.base = Some(base);
        gateway.computer = computer;
    }

    /// The thread a gateway credential acts for.
    #[must_use]
    pub fn gateway_thread(&self, token: &str) -> Option<ThreadId> {
        self.gateway_lock().threads.get(token).cloned()
    }

    /// Forge's own servers for a thread's engine, as (name, address with
    /// the thread's credential): the Artisan MCP, and computer use when the
    /// gateway serves it and the user turned it on. Empty until the gateway
    /// listens.
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
                tracing::warn!(key = COMPUTER_USE_KEY, "reading a setting: {error:#}");
                false
            }
        }
    }

    fn gateway_credential(&self, thread: &ThreadId) -> Option<(String, String)> {
        let mut gateway = self.gateway_lock();
        let base = gateway.base.clone()?;
        let token = if let Some(token) = gateway.tokens.get(thread) {
            token.clone()
        } else {
            let mut bytes = [0_u8; 24];
            if let Err(error) = getrandom::fill(&mut bytes) {
                tracing::error!(%thread, "making the thread's gateway credential: {error}");
                return None;
            }
            let token = hex::encode(bytes);
            gateway.tokens.insert(thread.clone(), token.clone());
            gateway.threads.insert(token.clone(), thread.clone());
            token
        };
        drop(gateway);
        Some((base, token))
    }

    /// Asks the connected Editors to act for an agent and returns the first
    /// answer. Fails when no Editor answers in time.
    pub async fn call_editor(&self, call: EditorCall) -> Result<serde_json::Value> {
        let id = artisan_api::ItemId::new().to_string();
        let (answer, answered) = oneshot::channel();
        self.gateway_lock().calls.insert(id.clone(), answer);
        self.store().publish_live(Event::EditorCall {
            id: id.clone(),
            call,
        });
        let reply = tokio::time::timeout(EDITOR_ANSWERS_WITHIN, answered).await;
        self.gateway_lock().calls.remove(&id);
        match reply {
            Ok(Ok(EditorReply::Ok(value))) => Ok(value),
            Ok(Ok(EditorReply::Error(error))) => Err(Refused(error).into()),
            _ => Err(Refused(
                "No Editor answered. This needs the Artisan Editor open and connected to this Forge."
                    .to_owned(),
            )
            .into()),
        }
    }

    /// An Editor's answer to a call; `false` when nothing waits for it.
    pub fn answer_editor_call(&self, id: &str, reply: EditorReply) -> bool {
        let answer = self.gateway_lock().calls.remove(id);
        answer.is_some_and(|answer| answer.send(reply).is_ok())
    }

    fn gateway_lock(&self) -> std::sync::MutexGuard<'_, Gateway> {
        self.inner
            .gateway
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
