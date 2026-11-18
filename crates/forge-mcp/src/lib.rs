#![expect(
    clippy::string_slice,
    clippy::unused_result_ok,
    missing_debug_implementations,
    unreachable_pub,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
mod awareness;
mod coordination;
mod format;
mod names;
pub mod server;
mod tools;
mod transcript;
mod validation;

pub use awareness::AwarenessError;
pub use coordination::{CoordinationError, ListRecordsError};
pub use server::{Content, Failure, StructuredError, Tool, ToolError, ToolResult, ToolSet};
pub use tools::CallToolError;
pub use transcript::{RenderError, ResolveError};

use artisan_api::ThreadId;
use forge_runs::{Hub, Refused};
use serde_json::Value;

const INSTRUCTIONS: &str = "Artisan is this workspace's collaboration interface. \
Before overlapping edits or investigating unexplained changes, use list_relevant_agents or \
list_relevant_threads; they rank advisory claims, declared scope, dependencies, and project activity. \
Publish your objective and exact worktree with publish_work, then claim_resources to declare \
your intended edit or shared-resource scope. Claims are advisory: overlaps invite coordination \
and never block filesystem writes. Negotiate boundaries with send_coordination_message; delivery \
is not agreement, so acknowledge requests and state the actual agreement. Release claims after \
handoff. Publish completion with evidence; watch_work delivers dependency results even after \
this turn ends. Use search_threads and read_thread for past decisions. Discovery defaults to \
the caller's project on this Forge; scope=host explicitly broadens it. Top-level and child workers \
are both discoverable; parent restricts the scope. Administrative tools change only what the user \
asked for. navigate_user is false unless navigation was requested. User questions and approvals \
can only be answered by the user. Legacy tool names remain callable for existing sessions.";

#[derive(Debug, thiserror::Error)]
pub enum ArtisanError {
    #[error(transparent)]
    Refused(#[from] Refused),
    #[error("check that thread {thread} still exists")]
    Caller {
        thread: ThreadId,
        #[source]
        source: anyhow::Error,
    },
    #[error(transparent)]
    Awareness(#[from] AwarenessError),
    #[error(transparent)]
    Coordination(#[from] CoordinationError),
    #[error(transparent)]
    Tool(#[from] CallToolError),
    #[error(transparent)]
    Structured(#[from] StructuredError),
    #[error("{field} is required")]
    Missing { field: &'static str },
    #[error("send the message to thread {thread}")]
    Send {
        thread: ThreadId,
        #[source]
        source: anyhow::Error,
    },
}

impl ToolError for ArtisanError {
    fn failure(&self) -> Failure<'_> {
        match self {
            Self::Refused(refused) => Failure::Refused(refused.to_string()),
            Self::Caller { source, .. } | Self::Send { source, .. } => Failure::of_forge(source),
            Self::Awareness(error) => error.failure(),
            Self::Coordination(error) => error.failure(),
            Self::Tool(error) => error.failure(),
            Self::Structured(_) | Self::Missing { .. } => Failure::Broken,
        }
    }
}

#[derive(Clone)]
pub struct ArtisanMcp {
    hub: Hub,
}

impl ArtisanMcp {
    #[must_use]
    pub const fn new(hub: Hub) -> Self {
        Self { hub }
    }

    async fn invoke(
        &self,
        caller: &ThreadId,
        name: &str,
        mut arguments: Value,
    ) -> Result<ToolResult, ArtisanError> {
        let found =
            self.hub
                .store()
                .thread(caller)
                .await
                .map_err(|source| ArtisanError::Caller {
                    thread: caller.clone(),
                    source: source.into(),
                })?;
        if found.is_none() {
            return Err(Refused(
                "The thread this MCP address was issued to no longer exists.".to_owned(),
            )
            .into());
        }
        if let Some(value) = awareness::call(&self.hub, caller, name, arguments.clone()).await? {
            return Ok(ToolResult::structured(value)?);
        }
        let canonical = names::canonical(name);
        if let Some(value) =
            coordination::call(&self.hub, caller, canonical, arguments.clone()).await?
        {
            return Ok(ToolResult::structured(value)?);
        }
        if matches!(canonical, "read_agent" | "message_agent")
            && !matches!(name, "agent_read" | "agent_message")
        {
            let agent = arguments
                .get("agent")
                .and_then(Value::as_str)
                .ok_or(ArtisanError::Missing { field: "agent" })?;
            let thread = awareness::resolve_agent(&self.hub, agent).await?;
            if canonical == "message_agent" {
                let text = arguments
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or(ArtisanError::Missing { field: "text" })?
                    .to_owned();
                let item = self
                    .hub
                    .send_as_agent(caller, &thread, text)
                    .await
                    .map_err(|source| ArtisanError::Send {
                        thread: thread.clone(),
                        source,
                    })?;
                return Ok(ToolResult::structured(
                    serde_json::json!({"delivered_item":item.id,"thread":thread,"acknowledged":false}),
                )?);
            }
            arguments
                .as_object_mut()
                .expect("invariant: arguments with an agent field are an object")
                .remove("agent");
            arguments["thread"] = serde_json::json!(thread);
            let text = tools::call(&self.hub, caller, "read_thread", arguments).await?;
            return Ok(ToolResult {
                content: vec![Content::Text(text)],
                structured_content: None,
            });
        }
        let text = tools::call(&self.hub, caller, canonical, arguments).await?;
        Ok(ToolResult {
            content: vec![Content::Text(text)],
            structured_content: None,
        })
    }
}

impl ToolSet for ArtisanMcp {
    type Error = ArtisanError;

    fn name(&self) -> &'static str {
        "artisan"
    }

    fn title(&self) -> &'static str {
        "Artisan"
    }

    fn instructions(&self) -> &'static str {
        INSTRUCTIONS
    }

    fn tools(&self) -> Vec<Tool> {
        let core = awareness::tools();
        let mut tools = tools::tools();
        tools.retain(|tool| !core.iter().any(|new| new.name == tool.name));
        core.into_iter()
            .chain(coordination::tools())
            .chain(tools)
            .collect()
    }

    async fn call(
        &self,
        caller: &ThreadId,
        name: &str,
        arguments: Value,
    ) -> Result<Vec<Content>, ArtisanError> {
        Ok(self.invoke(caller, name, arguments).await?.content)
    }

    fn canonical_name<'a>(&self, name: &'a str) -> &'a str {
        names::canonical(name)
    }

    async fn call_result(
        &self,
        caller: &ThreadId,
        name: &str,
        arguments: Value,
    ) -> Result<ToolResult, ArtisanError> {
        self.invoke(caller, name, arguments).await
    }
}
