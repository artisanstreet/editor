//! The Artisan MCP: what an agent can do with Artisan, the way the user does
//! in the app. Search and read every thread, write to threads, run agents on
//! any harness, and change settings, hosts, projects and extensions.
//!
//! Forge's gateway serves it to every engine over MCP's Streamable HTTP, as
//! JSON responses (no server-sent stream). The credential in the address
//! names the calling thread, so "this thread" and "my agents" need no ids.

mod format;
pub mod server;
mod tools;
mod transcript;

pub use server::{Content, Tool, ToolSet};

use anyhow::Result;
use artisan_api::ThreadId;
use forge_runs::Hub;
use serde_json::Value;

const INSTRUCTIONS: &str = "Artisan's own tools, for the thread you run in. \
Recall: threads_search ranks every thread's messages, plans, commands and errors; threads_grep \
runs a regex over them; thread_read reads a thread around an item; threads_touching finds the \
threads that read or changed a file. Use them when the user refers to another thread or earlier \
work. Agents: agent_spawn starts an agent on any harness (Claude, Codex, OpenCode) \
and model from `harnesses`, with only the prompt you give it; omitted settings come from this \
thread. Wait for it with agent_wait, or spawn it in the background and its result arrives as a \
message. The app: thread_*, settings_*, hosts_*, projects_*, mcp_servers_*, skills_*, \
global_prompt_* and open_* do what the user can do in Artisan. An HTTP MCP server that \
answers 401 signs in with mcp_server_sign_in, which returns the link for the user. Tools that \
change something only take the user to its page with navigate_user=true, when they asked for it. Changes that remove something or \
add trust show the user an approval card first, unless this thread has full access.";

/// The Artisan MCP over one [`Hub`].
#[derive(Clone)]
pub struct ArtisanMcp {
    hub: Hub,
}

impl ArtisanMcp {
    #[must_use]
    pub const fn new(hub: Hub) -> Self {
        Self { hub }
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
        tools::tools()
    }

    async fn call(&self, caller: &ThreadId, name: &str, arguments: Value) -> Result<Vec<Content>> {
        let text = tools::call(&self.hub, caller, name, arguments).await?;
        Ok(vec![Content::Text(text)])
    }
}
