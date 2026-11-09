mod awareness;
mod coordination;
mod format;
mod names;
pub mod server;
mod tools;
mod transcript;
mod validation;

pub use server::{Content, Tool, ToolResult, ToolSet};

use anyhow::Result;
use artisan_api::ThreadId;
use forge_runs::Hub;
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
    ) -> Result<ToolResult> {
        if let Some(value) = awareness::call(&self.hub, caller, name, arguments.clone()).await? {
            return ToolResult::structured(value);
        }
        let canonical = names::canonical(name);
        if let Some(value) =
            coordination::call(&self.hub, caller, canonical, arguments.clone()).await?
        {
            return ToolResult::structured(value);
        }
        if matches!(canonical, "read_agent" | "message_agent")
            && !matches!(name, "agent_read" | "agent_message")
        {
            let agent = arguments
                .get("agent")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("agent ID is required"))?;
            let thread = awareness::resolve_agent(&self.hub, agent).await?;
            if canonical == "message_agent" {
                let text = arguments
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("text is required"))?
                    .to_owned();
                let item = self.hub.send_as_agent(caller, &thread, text).await?;
                return ToolResult::structured(
                    serde_json::json!({"delivered_item":item.id,"thread":thread,"acknowledged":false}),
                );
            }
            arguments
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("arguments must be an object"))?
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

    async fn call(&self, caller: &ThreadId, name: &str, arguments: Value) -> Result<Vec<Content>> {
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
    ) -> Result<ToolResult> {
        self.invoke(caller, name, arguments).await
    }
}
