//! Every Artisan MCP tool: its schema for `tools/list`, and what it does.

use crate::format;
use crate::server::Tool;
use crate::transcript::{self, Options};
use anyhow::{Context, Result};
use artisan_api::{
    EditorCall, EngineId, InstallState, ItemId, ItemsQuery, McpMutation, McpServer, Permissions,
    ProjectId, SettingOwner, SignInState, ThreadId, ThreadsQuery, UpdateThread, setting_decl,
};
use forge_runs::{Hub, Refused, SpawnAgent};
use forge_store::{GrepQuery, RecallScope};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

/// The names engines know Forge's own servers by (the Artisan MCP and
/// computer use); no added server may take them.
const OWN_SERVERS: [&str; 2] = ["artisan", "computer"];

const HARNESS: &str = "claude, codex or opencode";

fn scope_properties() -> Value {
    json!({
        "thread": { "type": "string", "description": "Only this thread." },
        "project": { "type": "string", "description": "Only this project's threads (an id from projects_list)." },
        "harness": { "type": "string", "description": format!("Only threads on this harness: {HARNESS}.") },
        "kinds": { "type": "array", "items": { "type": "string", "enum": ["user", "assistant", "tool", "agent", "plan", "question", "notice", "error", "image"] }, "description": "Only these item kinds." },
        "since_days": { "type": "number", "description": "Only items from the last this many days." },
        "skip_archived": { "type": "boolean", "description": "Leave archived threads out (default false)." },
    })
}

fn with_scope(mut properties: Value) -> Value {
    if let (Some(properties), Value::Object(scope)) =
        (properties.as_object_mut(), scope_properties())
    {
        properties.extend(scope);
    }
    properties
}

fn read_properties() -> Value {
    json!({
        "thread": { "type": "string", "description": "The thread's id or artisan:// link; this one when left out." },
        "item": { "type": "string", "description": "Read around this item (from a search or grep hit)." },
        "before": { "type": "integer", "description": "With item: how many items before it (default 5)." },
        "after": { "type": "integer", "description": "With item: how many items after it (default 5)." },
        "cursor": { "type": "integer", "description": "Without item: read items older than this (`older` from a previous page)." },
        "limit": { "type": "integer", "description": "Without item: how many of the newest items (default 20, at most 100)." },
        "whole": { "type": "boolean", "description": "Don't cut long texts (use with item and before=0, after=0 for one whole item)." },
        "include_output": { "type": "boolean", "description": "Show tool output (failed commands always show theirs)." },
    })
}

pub fn tools() -> Vec<Tool> {
    [
        recall_tools(),
        thread_tools(),
        agent_tools(),
        setting_tools(),
        host_tools(),
        project_tools(),
        extension_tools(),
        navigation_tools(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Finding and reading threads.
fn recall_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "threads_search",
            description: "Ranked full-text search over every thread's messages, plans, commands, errors and file paths. Use it when the user refers to another thread or to earlier work (\"like we did in the thread where...\"). Each hit names its thread and item; read around it with thread_read. Words of three letters or more match anywhere in a word.",
            properties: || {
                with_scope(json!({
                    "query": { "type": "string", "description": "Words to find; an exact phrase ranks first." },
                    "include_output": { "type": "boolean", "description": "Also search tool output (slower; default false)." },
                    "limit": { "type": "integer", "description": "Hits per page (default 15, at most 100)." },
                    "offset": { "type": "integer", "description": "Skip this many hits (`next` from a previous page)." },
                }))
            },
            required: &["query"],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "threads_grep",
            description: "Regex search, line by line, over every thread's messages, plans, commands and errors, newest first, like rg. Returns each matching line with its thread, item and context; when older items remain, it says which cursor continues.",
            properties: || {
                with_scope(json!({
                    "pattern": { "type": "string", "description": "A Rust regex." },
                    "ignore_case": { "type": "boolean" },
                    "include_output": { "type": "boolean", "description": "Also read tool output." },
                    "context": { "type": "integer", "description": "Lines around each match (at most 10)." },
                    "max_matches": { "type": "integer", "description": "Default 50, at most 500." },
                    "cursor": { "type": "integer", "description": "`next` from a previous page." },
                }))
            },
            required: &["pattern"],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "thread_read",
            description: "Reads a thread's items: the newest page, older pages by cursor, or the items around one item. Long texts are cut unless whole=true.",
            properties: read_properties,
            required: &[],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "threads_touching",
            description: "The threads that read, created, edited or deleted a file whose path contains `path`, newest first.",
            properties: || {
                json!({
                    "path": { "type": "string", "description": "A path or part of one, such as scene.rs or crates/editor/." },
                    "project": { "type": "string" },
                    "limit": { "type": "integer", "description": "Default 30." },
                })
            },
            required: &["path"],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "thread_list",
            description: "Threads, newest activity first, with their harness, model and state. Agent threads are listed only with parent.",
            properties: || {
                json!({
                    "project": { "type": "string" },
                    "state": { "type": "string", "enum": ["any", "working", "unread", "archived"], "description": "Default any (not archived)." },
                    "title": { "type": "string", "description": "Only threads whose title or project contains this." },
                    "parent": { "type": "string", "description": "Only the agents this thread started." },
                    "limit": { "type": "integer", "description": "Default 30, at most 200." },
                })
            },
            required: &[],
            read_only: true,
            destructive: false,
        },
    ]
}

/// Starting, messaging and changing threads, and what waits on the user.
fn thread_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "thread_create",
            description: "Starts a new top-level thread the user will see in the sidebar, and sends it the prompt. Harness, model, effort, speed, variant, context window, engine options and permissions default to this thread's; anything changed or filled in comes back as a warning. For work you need back, use agent_spawn instead.",
            properties: || {
                json!({
                    "title": { "type": "string" },
                    "prompt": { "type": "string", "description": "Its first message; empty starts it idle." },
                    "project": { "type": "string", "description": "Default this thread's project." },
                    "harness": { "type": "string", "description": HARNESS },
                    "model": { "type": "string" },
                    "effort": { "type": "string", "description": "An effort id from harnesses. For OpenCode reasoning choices use variant." },
                    "speed": { "type": "string", "description": "A speed id from harnesses." },
                    "variant": { "type": "string", "description": "A variant id from harnesses (including OpenCode reasoning choices)." },
                    "context_window": { "type": "integer", "minimum": 1, "maximum": 100_000_000, "description": "A context_windows override_tokens value from harnesses. Use options.context_window=null to restore the standard window." },
                    "options": { "type": "object", "additionalProperties": true, "description": "Engine-specific options, merged over inherited options for the same harness and model. context_window takes precedence over options.context_window." },
                    "permissions": { "type": "string", "enum": ["auto", "full"] },
                })
            },
            required: &["title"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "thread_send",
            description: "Sends a message to another thread: it starts a turn, or steers the running one. It shows as sent by this thread's agent; reaching a full-access thread from an auto one asks the user first.",
            properties: || {
                json!({
                    "thread": { "type": "string" },
                    "text": { "type": "string" },
                })
            },
            required: &["thread", "text"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "thread_cancel",
            description: "Stops another thread's running turn, and the agents it started.",
            properties: || json!({ "thread": { "type": "string" } }),
            required: &["thread"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "thread_update",
            description: "Renames, archives or unarchives a thread, settles it (marks it seen), or marks it unread. The thread defaults to this one.",
            properties: || {
                json!({
                    "thread": { "type": "string" },
                    "title": { "type": "string" },
                    "archived": { "type": "boolean" },
                    "settled": { "type": "boolean", "description": "true marks it read and settled." },
                    "unread": { "type": "boolean", "description": "true marks it unread." },
                })
            },
            required: &[],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "requests_list",
            description: "Approvals and questions waiting on the user, across threads. Only the user answers them.",
            properties: || json!({}),
            required: &[],
            read_only: true,
            destructive: false,
        },
    ]
}

/// Running agents on any harness.
fn agent_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "harnesses",
            description: "Each harness with whether it is installed and signed in, its default model, and its complete model catalog: efforts and notes, speeds, variants, defaults, exact context windows and override values, image support, descriptions, labs and gateways. OpenCode reasoning choices are variants, not efforts. Pick agent_spawn's harness and model from here.",
            properties: || json!({}),
            required: &[],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "agent_spawn",
            description: "Starts an agent on any harness and model, working on `prompt` alone: nothing of this conversation goes along, so the prompt is the whole brief. Left-out settings come from this thread; when the harness differs or a value doesn't fit, a default is used and named in `warnings`. Returns at once with the agent's id. Then agent_wait for its result, or set background=true and the result arrives as a message when it finishes. Permissions never exceed this thread's. The user sees the agent with its harness in the Agents panel.",
            properties: || {
                json!({
                    "task": { "type": "string", "description": "A few words naming the work; the agent's label." },
                    "prompt": { "type": "string" },
                    "harness": { "type": "string", "description": HARNESS },
                    "model": { "type": "string" },
                    "effort": { "type": "string", "description": "An effort id from harnesses. For OpenCode reasoning choices use variant." },
                    "speed": { "type": "string", "description": "A speed id from harnesses." },
                    "variant": { "type": "string", "description": "A variant id from harnesses (including OpenCode reasoning choices)." },
                    "context_window": { "type": "integer", "minimum": 1, "maximum": 100_000_000, "description": "A context_windows override_tokens value from harnesses. Use options.context_window=null to restore the standard window." },
                    "options": { "type": "object", "additionalProperties": true, "description": "Engine-specific options, merged over inherited options for the same harness and model. context_window takes precedence over options.context_window." },
                    "permissions": { "type": "string", "enum": ["auto", "full"] },
                    "background": { "type": "boolean" },
                })
            },
            required: &["task", "prompt"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "agent_wait",
            description: "Waits until the agents settle or ask the user something (with any=true: until one does), up to timeout_seconds (default 300, at most 540), and returns each one's state and, once done, its last reply. Call again to keep waiting.",
            properties: || {
                json!({
                    "agents": { "type": "array", "items": { "type": "string" } },
                    "any": { "type": "boolean" },
                    "timeout_seconds": { "type": "integer" },
                })
            },
            required: &["agents"],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "agent_message",
            description: "Sends an agent a follow-up. A finished agent starts working again.",
            properties: || {
                json!({
                    "agent": { "type": "string" },
                    "text": { "type": "string" },
                })
            },
            required: &["agent", "text"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "agent_read",
            description: "Reads an agent's own thread, like thread_read.",
            properties: || {
                let mut properties = read_properties();
                if let Some(properties) = properties.as_object_mut() {
                    properties.remove("thread");
                    properties.insert("agent".into(), json!({ "type": "string" }));
                }
                properties
            },
            required: &["agent"],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "agent_cancel",
            description: "Stops an agent and every agent it started.",
            properties: || json!({ "agent": { "type": "string" } }),
            required: &["agent"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "agent_list",
            description: "This thread's agents and where each one is.",
            properties: || json!({}),
            required: &[],
            read_only: true,
            destructive: false,
        },
    ]
}

/// Reading and changing settings.
fn setting_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "settings_list",
            description: "Every setting with its value, owner (forge: shared by every Editor of this host; editor: this device), description and allowed values.",
            properties: || json!({}),
            required: &[],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "settings_get",
            description: "One setting's value.",
            properties: || json!({ "key": { "type": "string" } }),
            required: &["key"],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "settings_set",
            description: "Changes a setting. The value must fit its declaration (settings_list).",
            properties: || json!({ "key": { "type": "string" }, "value": {}, "navigate_user": navigate_user_schema() }),
            required: &["key", "value"],
            read_only: false,
            destructive: false,
        },
    ]
}

/// The Forge hosts the Editor connects to.
fn host_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "hosts_list",
            description: "The Forges this Editor is paired with, and which one is connected. Needs the Editor open.",
            properties: || json!({}),
            required: &[],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "host_add",
            description: "Pairs the Editor with a Forge: kind this_computer, wsl (with distro), or remote (with address, code and fingerprint, which `ae pair` shows on the other machine; ask the user for them).",
            properties: || {
                json!({
                    "kind": { "type": "string", "enum": ["this_computer", "wsl", "remote"] },
                    "distro": { "type": "string" },
                    "address": { "type": "string", "description": "host:port" },
                    "code": { "type": "string" },
                    "fingerprint": { "type": "string" },
                    "navigate_user": navigate_user_schema(),
                })
            },
            required: &["kind"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "host_remove",
            description: "Unpairs a host (by id or name). Asks the user first.",
            properties: || json!({ "host": { "type": "string" } }),
            required: &["host"],
            read_only: false,
            destructive: true,
        },
        Tool {
            name: "host_switch",
            description: "Moves the Editor to another host (by id or name).",
            properties: || json!({ "host": { "type": "string" } }),
            required: &["host"],
            read_only: false,
            destructive: false,
        },
    ]
}

/// The projects threads work in.
fn project_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "projects_list",
            description: "The projects on this host: id, name and folder.",
            properties: || json!({}),
            required: &[],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "project_add",
            description: "Adds a folder on this host as a project.",
            properties: || json!({ "path": { "type": "string" }, "name": { "type": "string" } }),
            required: &["path"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "project_remove",
            description: "Removes a project from Artisan (the folder stays). Asks the user first.",
            properties: || json!({ "project": { "type": "string" } }),
            required: &["project"],
            read_only: false,
            destructive: true,
        },
    ]
}

/// MCP servers, skills, the global prompt and harness setup.
#[expect(
    clippy::too_many_lines,
    reason = "one schema per tool, in the order the docs list them"
)]
fn extension_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "mcp_servers_list",
            description: "The MCP servers every harness gets (environment values hidden).",
            properties: || json!({}),
            required: &[],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "mcp_server_add",
            description: "Adds or replaces an MCP server for every harness: a command (with args and env) or a url. Asks the user first. New threads get it.",
            properties: || {
                json!({
                    "name": { "type": "string" },
                    "command": { "type": "string" },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "env": { "type": "object", "additionalProperties": { "type": "string" } },
                    "url": { "type": "string" },
                    "navigate_user": navigate_user_schema(),
                })
            },
            required: &["name"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "mcp_server_remove",
            description: "Removes an MCP server from every harness. Asks the user first.",
            properties: || json!({ "name": { "type": "string" } }),
            required: &["name"],
            read_only: false,
            destructive: true,
        },
        Tool {
            name: "mcp_server_sign_in",
            description: "Signs in to an HTTP MCP server that refuses without one (HTTP 401; mcp_servers_list says `needs sign-in`), with the OAuth flow MCP specifies. Returns the link the user approves in their browser; Artisan then keeps the token for every harness and new threads get the server's tools. Needs the Editor open. Don't open settings for this.",
            properties: || json!({ "name": { "type": "string" }, "navigate_user": navigate_user_schema() }),
            required: &["name"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "skills_list",
            description: "The skills Artisan manages and which are on.",
            properties: || json!({}),
            required: &[],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "skill_set_enabled",
            description: "Turns a managed skill on or off in every harness.",
            properties: || json!({ "id": { "type": "string" }, "enabled": { "type": "boolean" } }),
            required: &["id", "enabled"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "skill_install",
            description: "Installs a skill folder (with a SKILL.md) for every harness: Artisan copies it into its own store and keeps it in each harness. Use this instead of writing into a harness's skills folder. `replace` updates a skill of the same name and asks the user first.",
            properties: || {
                json!({
                    "path": { "type": "string", "description": "Absolute path of the skill folder" },
                    "replace": { "type": "boolean" },
                })
            },
            required: &["path"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "global_prompt_get",
            description: "The global prompt every harness reads.",
            properties: || json!({}),
            required: &[],
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "global_prompt_set",
            description: "Replaces the global prompt every harness reads. Asks the user first.",
            properties: || json!({ "text": { "type": "string" }, "navigate_user": navigate_user_schema() }),
            required: &["text"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "harness_set_default_model",
            description: "The model new threads on a harness start with.",
            properties: || json!({ "harness": { "type": "string", "description": HARNESS }, "model": { "type": "string" }, "navigate_user": navigate_user_schema() }),
            required: &["harness", "model"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "harness_install",
            description: "Installs or updates a harness. Asks the user first; the install continues in the background (watch harnesses).",
            properties: || json!({ "harness": { "type": "string", "description": HARNESS }, "navigate_user": navigate_user_schema() }),
            required: &["harness"],
            read_only: false,
            destructive: false,
        },
    ]
}

/// Opening places in the Editor.
fn navigation_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "open_thread",
            description: "Opens a thread in the user's Editor.",
            properties: || json!({ "thread": { "type": "string" } }),
            required: &["thread"],
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "open_settings",
            description: "Opens a settings page in the user's Editor: general, hosts, about, threads, global-prompt, extensions, engines, or engines/<harness> for one harness.",
            properties: || json!({ "page": { "type": "string" } }),
            required: &["page"],
            read_only: false,
            destructive: false,
        },
    ]
}

fn navigate_user_schema() -> Value {
    json!({
        "type": "boolean",
        "description": "Also take the user to the page this changes in their Editor. Only when the user asked to be taken there; default false.",
    })
}

/// Opens a settings page when the user asked to be taken there. Best
/// effort: the change stands when no Editor answers.
async fn navigate_if_asked(hub: &Hub, navigate_user: bool, page: &str) -> String {
    if !navigate_user {
        return String::new();
    }
    match hub
        .call_editor(EditorCall::OpenSettings {
            page: page.to_owned(),
        })
        .await
    {
        Ok(_) => format!(" Opened settings: {page}."),
        Err(error) => format!(" Couldn't open settings: {error}"),
    }
}

/// The settings page a setting is changed on.
fn setting_page(key: &str) -> &'static str {
    match key {
        "suggestions" | "you_should_know" | "summary_titles" => "threads",
        _ => "general",
    }
}

/// What `mcp_servers_list` says about a server's sign-in.
const fn auth_note(status: Option<&artisan_api::McpAuthStatus>) -> &'static str {
    match status {
        Some(artisan_api::McpAuthStatus::NeedsSignIn) => " · needs sign-in (mcp_server_sign_in)",
        Some(artisan_api::McpAuthStatus::SignedIn) => " · signed in",
        Some(artisan_api::McpAuthStatus::NeedsToken) => {
            " · refused without a token: it needs an Authorization header, which the user adds on its card"
        }
        None => "",
    }
}

fn args<T: DeserializeOwned>(arguments: Value) -> Result<T> {
    serde_json::from_value(arguments)
        .map_err(|error| Refused(format!("Those arguments don't fit: {error}")).into())
}

/// A thread argument: its id, or the `artisan://thread/<host>/<thread>`
/// link that opens it in the Editor.
fn thread_id(value: &str) -> ThreadId {
    let id = value
        .trim()
        .strip_prefix("artisan://thread/")
        .and_then(|rest| rest.trim_end_matches('/').rsplit('/').next())
        .unwrap_or_else(|| value.trim());
    ThreadId::from(id)
}

fn harness(name: &str) -> Result<EngineId> {
    EngineId::ALL
        .into_iter()
        .find(|engine| engine.as_str() == name.to_ascii_lowercase())
        .ok_or_else(|| {
            Refused(format!(
                "There is no harness {name}; harnesses are {HARNESS}."
            ))
            .into()
        })
}

fn permissions(name: Option<&str>) -> Result<Option<Permissions>> {
    match name {
        None => Ok(None),
        Some("auto") => Ok(Some(Permissions::Auto)),
        Some("full") => Ok(Some(Permissions::Full)),
        Some(other) => Err(Refused(format!("Permissions are auto or full, not {other}.")).into()),
    }
}

#[derive(Deserialize)]
struct Scope {
    thread: Option<String>,
    project: Option<String>,
    harness: Option<String>,
    #[serde(default)]
    kinds: Vec<String>,
    since_days: Option<f64>,
    #[serde(default)]
    skip_archived: bool,
}

impl Scope {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a day count is fractional; `as` truncates it to whole milliseconds and saturates"
    )]
    fn recall(self) -> Result<RecallScope> {
        Ok(RecallScope {
            thread: self.thread.as_deref().map(thread_id),
            project: self.project.map(ProjectId::from),
            engine: self.harness.as_deref().map(harness).transpose()?,
            kinds: self.kinds,
            since: self
                .since_days
                .map(|days| artisan_api::now() - (days * 86_400_000.0) as i64),
            until: None,
            skip_archived: self.skip_archived,
        })
    }
}

#[derive(Deserialize, Default)]
struct Read {
    thread: Option<String>,
    agent: Option<String>,
    item: Option<String>,
    before: Option<u32>,
    after: Option<u32>,
    cursor: Option<u64>,
    limit: Option<u32>,
    #[serde(default)]
    whole: bool,
    #[serde(default)]
    include_output: bool,
}

/// A page of a thread's top-level items as text.
async fn read_thread(hub: &Hub, thread: &ThreadId, read: Read) -> Result<String> {
    let store = hub.store();
    let found = store
        .thread(thread)
        .await?
        .ok_or_else(|| Refused(format!("There is no thread {thread}.")))?;
    let options = Options {
        whole: read.whole,
        output: read.include_output,
    };
    let (items, older) = if let Some(id) = &read.item {
        let item = store
            .item(&ItemId::from(id.as_str()))
            .await?
            .filter(|item| &item.thread_id == thread)
            .ok_or_else(|| Refused(format!("Thread {thread} has no item {id}.")))?;
        let before = read.before.unwrap_or(5).min(50);
        let after = read.after.unwrap_or(5).min(50);
        let parent = item.parent_id.clone();
        let earlier = if before == 0 {
            Vec::new()
        } else {
            store
                .transcript_page(
                    thread,
                    parent.as_ref(),
                    ItemsQuery {
                        before: Some(item.position),
                        after: None,
                        limit: Some(before),
                    },
                )
                .await?
                .items
        };
        let later = if after == 0 {
            Vec::new()
        } else {
            store
                .transcript_page(
                    thread,
                    parent.as_ref(),
                    ItemsQuery {
                        before: None,
                        after: Some(item.position),
                        limit: Some(after),
                    },
                )
                .await?
                .items
        };
        let older = earlier.first().map(|first| first.position);
        let mut items = earlier;
        items.push(item);
        items.extend(later);
        (items, older)
    } else {
        let page = store
            .transcript_page(
                thread,
                None,
                ItemsQuery {
                    before: read.cursor,
                    after: None,
                    limit: Some(read.limit.unwrap_or(20).clamp(1, 100)),
                },
            )
            .await?;
        let older = page
            .more
            .then(|| page.items.first().map(|item| item.position))
            .flatten();
        (page.items, older)
    };
    let mut items = items;
    items.sort_by_key(|item| item.position);
    let mut out = format!("{}\n\n---\n\n", format::thread(hub, &found));
    out.push_str(&transcript::render(store, &items, &options).await?);
    match older {
        Some(cursor) => {
            writeln!(out, "---\n\nOlder items: call again with cursor {cursor}.")?;
        }
        None => out.push_str("---\n\nThat is the start of the thread.\n"),
    }
    Ok(out)
}

/// Asks the user before an agent changes trust or removes something, unless
/// the thread has Full access.
async fn approve(hub: &Hub, caller: &ThreadId, tool: &str, description: String) -> Result<()> {
    hub.ask_approval(caller, tool, description).await
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per MCP tool, in the order tools() lists them"
)]
pub async fn call(hub: &Hub, caller: &ThreadId, name: &str, arguments: Value) -> Result<String> {
    let store = hub.store();
    match name {
        "threads_search" => {
            #[derive(Deserialize)]
            struct Search {
                query: String,
                #[serde(default)]
                include_output: bool,
                limit: Option<u32>,
                offset: Option<u32>,
                #[serde(flatten)]
                scope: Scope,
            }
            let search: Search = args(arguments)?;
            let offset = search.offset.unwrap_or(0);
            let page = store
                .recall_search(
                    search.scope.recall()?,
                    search.query,
                    search.include_output,
                    search.limit.unwrap_or(15),
                    search.offset.unwrap_or(0),
                )
                .await?;
            if page.hits.is_empty() {
                return Ok("No matches.".to_owned());
            }
            Ok(format::search(&page, offset))
        }
        "threads_grep" => {
            #[derive(Deserialize)]
            struct Grep {
                pattern: String,
                #[serde(default)]
                ignore_case: bool,
                #[serde(default)]
                include_output: bool,
                context: Option<u32>,
                max_matches: Option<u32>,
                cursor: Option<i64>,
                #[serde(flatten)]
                scope: Scope,
            }
            let grep: Grep = args(arguments)?;
            let page = store
                .recall_grep(
                    grep.scope.recall()?,
                    GrepQuery {
                        pattern: grep.pattern,
                        ignore_case: grep.ignore_case,
                        output: grep.include_output,
                        context: grep.context.unwrap_or(0),
                        max_matches: grep.max_matches,
                        cursor: grep.cursor,
                    },
                )
                .await?;
            if page.matches.is_empty() && page.next.is_none() {
                return Ok("No matches.".to_owned());
            }
            Ok(format::grep(&page))
        }
        "thread_read" => {
            let read: Read = args(arguments)?;
            // A hit's item names its thread.
            let thread = match (&read.thread, &read.item) {
                (Some(thread), _) => thread_id(thread),
                (None, Some(item)) => store
                    .item(&ItemId::from(item.as_str()))
                    .await?
                    .map(|item| item.thread_id)
                    .ok_or_else(|| Refused(format!("There is no item {item}.")))?,
                (None, None) => caller.clone(),
            };
            read_thread(hub, &thread, read).await
        }
        "threads_touching" => {
            #[derive(Deserialize)]
            struct Touching {
                path: String,
                project: Option<String>,
                limit: Option<u32>,
            }
            let touching: Touching = args(arguments)?;
            let touches = store
                .recall_touching(
                    RecallScope {
                        project: touching.project.map(ProjectId::from),
                        ..RecallScope::default()
                    },
                    touching.path,
                    touching.limit.unwrap_or(30),
                )
                .await?;
            if touches.is_empty() {
                return Ok("No thread touched a matching path.".to_owned());
            }
            Ok(format::touches(&touches))
        }
        "thread_list" => {
            #[derive(Deserialize)]
            struct List {
                project: Option<String>,
                state: Option<String>,
                title: Option<String>,
                parent: Option<String>,
                limit: Option<u32>,
            }
            let list: List = args(arguments)?;
            let limit = list.limit.unwrap_or(30).clamp(1, 200) as usize;
            if let Some(parent) = list.parent {
                let threads = store.agent_threads(&thread_id(&parent)).await?;
                return Ok(format::threads(hub, &threads));
            }
            let state = list.state.as_deref().unwrap_or("any");
            let page = store
                .threads_page(ThreadsQuery {
                    archived: state == "archived",
                    limit: Some(200),
                    query: list.title,
                    ..ThreadsQuery::default()
                })
                .await?;
            let threads: Vec<&artisan_api::Thread> = page
                .threads
                .iter()
                .filter(|thread| thread.spawned_by.is_none())
                .filter(|thread| {
                    list.project
                        .as_deref()
                        .is_none_or(|project| thread.project_id.as_str() == project)
                })
                .filter(|thread| match state {
                    "working" => hub.is_live(&thread.id),
                    "unread" => thread.is_unread(),
                    _ => true,
                })
                .take(limit)
                .collect();
            Ok(format::threads(hub, threads))
        }
        "thread_create" => {
            #[derive(Deserialize)]
            struct Create {
                title: String,
                #[serde(default)]
                prompt: String,
                project: Option<String>,
                harness: Option<String>,
                model: Option<String>,
                effort: Option<String>,
                speed: Option<String>,
                variant: Option<String>,
                context_window: Option<u64>,
                options: Option<serde_json::Map<String, Value>>,
                permissions: Option<String>,
            }
            let create: Create = args(arguments)?;
            let (thread, warnings) = hub
                .start_thread(
                    caller,
                    create.project.map(ProjectId::from),
                    SpawnAgent {
                        task: create.title,
                        prompt: create.prompt,
                        harness: create.harness.as_deref().map(harness).transpose()?,
                        model: create.model,
                        effort: create.effort,
                        speed: create.speed,
                        variant: create.variant,
                        context_window: create.context_window,
                        options: create.options,
                        permissions: permissions(create.permissions.as_deref())?,
                        background: false,
                    },
                )
                .await?;
            let mut out = format!("Started {}", format::thread(hub, &thread));
            format::warnings(&mut out, &warnings);
            Ok(out)
        }
        "thread_send" => {
            #[derive(Deserialize)]
            struct Send {
                thread: String,
                text: String,
            }
            let send: Send = args(arguments)?;
            let thread = thread_id(&send.thread);
            if &thread == caller {
                return Err(Refused("That's this thread; just keep going.".to_owned()).into());
            }
            let target = store
                .thread(&thread)
                .await?
                .ok_or_else(|| Refused(format!("There is no thread {thread}.")))?;
            let item = hub.send_as_agent(caller, &thread, send.text).await?;
            let title = format::thread(hub, &target);
            Ok(
                if matches!(
                    item.kind,
                    artisan_api::ItemKind::UserMessage { steer: true, .. }
                ) {
                    format!("Steered the running turn of {title}")
                } else {
                    format!("Sent to {title}")
                },
            )
        }
        "thread_cancel" => {
            #[derive(Deserialize)]
            struct Cancel {
                thread: String,
            }
            let cancel: Cancel = args(arguments)?;
            let thread = thread_id(&cancel.thread);
            if &thread == caller {
                return Err(
                    Refused("That's this thread; to stop, end your turn.".to_owned()).into(),
                );
            }
            let target = store
                .thread(&thread)
                .await?
                .ok_or_else(|| Refused(format!("There is no thread {thread}.")))?;
            hub.cancel(&thread).await?;
            Ok(format!("Stopped {}", format::thread(hub, &target)))
        }
        "thread_update" => {
            #[derive(Deserialize)]
            struct Update {
                thread: Option<String>,
                title: Option<String>,
                archived: Option<bool>,
                settled: Option<bool>,
                unread: Option<bool>,
            }
            let update: Update = args(arguments)?;
            let thread = update
                .thread
                .as_deref()
                .map_or_else(|| caller.clone(), thread_id);
            #[expect(
                clippy::useless_let_if_seq,
                reason = "the lookup refuses an unknown thread before any change"
            )]
            let mut current = store
                .thread(&thread)
                .await?
                .ok_or_else(|| Refused(format!("There is no thread {thread}.")))?;
            if update.title.is_some() || update.archived.is_some() {
                current = store
                    .update_thread(
                        &thread,
                        UpdateThread {
                            title: update.title.map(|title| title.trim().to_owned()),
                            archived: update.archived,
                            ..UpdateThread::default()
                        },
                    )
                    .await?
                    .context("the thread disappeared")?;
            }
            if update.settled == Some(true) {
                current = store
                    .settle_thread(&thread, current.activity_seq)
                    .await?
                    .context("the thread disappeared")?;
            }
            if update.unread == Some(true) {
                current = store
                    .mark_unread(&thread)
                    .await?
                    .context("the thread disappeared")?;
            }
            Ok(format!("Now {}", format::thread(hub, &current)))
        }
        "requests_list" => {
            let mut requests = Vec::new();
            for pending in store.items_awaiting_user().await? {
                let Some(item) = store.item(&pending.id).await? else {
                    continue;
                };
                let title = store
                    .thread(&pending.thread_id)
                    .await?
                    .and_then(|thread| thread.title);
                let what = match &item.kind {
                    artisan_api::ItemKind::Question { .. } => "question",
                    artisan_api::ItemKind::UserMessage { .. }
                    | artisan_api::ItemKind::AssistantMessage { .. }
                    | artisan_api::ItemKind::Tool { .. }
                    | artisan_api::ItemKind::Agent { .. }
                    | artisan_api::ItemKind::Plan { .. }
                    | artisan_api::ItemKind::Image { .. }
                    | artisan_api::ItemKind::Compaction { .. }
                    | artisan_api::ItemKind::Notice { .. }
                    | artisan_api::ItemKind::Error { .. } => "approval",
                };
                requests.push(format!(
                    "- A {what} in **{}** `{}`",
                    title.as_deref().unwrap_or("an untitled thread"),
                    item.id
                ));
            }
            if requests.is_empty() {
                return Ok("Nothing waits on the user.".to_owned());
            }
            Ok(requests.join("\n"))
        }
        "harnesses" => {
            let mut out = String::new();
            for engine in EngineId::ALL {
                let status = hub.engines().status(engine);
                let installed = matches!(status.install, InstallState::Installed { .. });
                let sign_in = match &status.sign_in {
                    SignInState::SignedIn { .. } => "signed in",
                    SignInState::SignedOut => "signed out",
                    SignInState::LoggingIn { .. } => "signing in",
                    SignInState::Unknown | SignInState::Failed { .. } => "sign-in unknown",
                };
                writeln!(
                    out,
                    "### {} (`{engine}`)\n\n{}{}\n",
                    engine.display_name(),
                    if installed { sign_in } else { "not installed" },
                    status
                        .default_model
                        .as_deref()
                        .map(|model| format!(" · new threads start on `{model}`"))
                        .unwrap_or_default(),
                )?;
                if !installed || status.models.is_empty() {
                    continue;
                }
                out.push_str(&format::models(&status.models)?);
            }
            Ok(out.trim_end().to_owned())
        }
        "agent_spawn" => {
            #[derive(Deserialize)]
            struct Spawn {
                task: String,
                prompt: String,
                harness: Option<String>,
                model: Option<String>,
                effort: Option<String>,
                speed: Option<String>,
                variant: Option<String>,
                context_window: Option<u64>,
                options: Option<serde_json::Map<String, Value>>,
                permissions: Option<String>,
                #[serde(default)]
                background: bool,
            }
            let spawn: Spawn = args(arguments)?;
            let spawn_background = spawn.background;
            let info = hub
                .spawn_agent(
                    caller,
                    SpawnAgent {
                        task: spawn.task,
                        prompt: spawn.prompt,
                        harness: spawn.harness.as_deref().map(harness).transpose()?,
                        model: spawn.model,
                        effort: spawn.effort,
                        speed: spawn.speed,
                        variant: spawn.variant,
                        context_window: spawn.context_window,
                        options: spawn.options,
                        permissions: permissions(spawn.permissions.as_deref())?,
                        background: spawn.background,
                    },
                )
                .await?;
            let mut out = format!("Started {}", format::agent(&info));
            out.push_str(if spawn_background {
                "\nIts result arrives as a message when it finishes."
            } else {
                "\nWait for it with agent_wait."
            });
            Ok(out)
        }
        "agent_wait" => {
            #[derive(Deserialize)]
            struct Wait {
                agents: Vec<String>,
                #[serde(default)]
                any: bool,
                timeout_seconds: Option<u64>,
            }
            let wait: Wait = args(arguments)?;
            let agents = hub
                .wait_agents(
                    caller,
                    &wait.agents,
                    wait.any,
                    Duration::from_secs(wait.timeout_seconds.unwrap_or(300)),
                )
                .await?;
            Ok(format::agents(&agents))
        }
        "agent_message" => {
            #[derive(Deserialize)]
            struct Message {
                agent: String,
                text: String,
            }
            let message: Message = args(arguments)?;
            let info = hub
                .message_agent(caller, &message.agent, message.text)
                .await?;
            Ok(format!("Sent to {}", format::agent(&info)))
        }
        "agent_read" => {
            let read: Read = args(arguments)?;
            let agent = read
                .agent
                .clone()
                .ok_or_else(|| Refused("Name the agent to read.".to_owned()))?;
            let info = hub.agent_status(caller, &agent).await?;
            read_thread(hub, &info.thread, read).await
        }
        "agent_cancel" => {
            #[derive(Deserialize)]
            struct Cancel {
                agent: String,
            }
            let cancel: Cancel = args(arguments)?;
            let info = hub.cancel_agent(caller, &cancel.agent).await?;
            Ok(format::agent(&info))
        }
        "agent_list" => {
            let agents = hub.agents(caller).await?;
            if agents.is_empty() {
                return Ok("This thread has started no agents.".to_owned());
            }
            Ok(format::agents(&agents))
        }
        "settings_list" => {
            let mut editor_open = true;
            let mut out = String::from("| Setting | Key | Value | Kept by |\n|---|---|---|---|\n");
            for decl in artisan_api::SETTINGS {
                let value = match decl.owner {
                    SettingOwner::Forge => Some(
                        store
                            .setting(decl.key)
                            .await?
                            .unwrap_or_else(|| decl.default_value()),
                    ),
                    SettingOwner::Editor if editor_open => hub
                        .call_editor(EditorCall::SettingGet {
                            key: decl.key.to_owned(),
                        })
                        .await
                        .map_or_else(
                            |_| {
                                editor_open = false;
                                None
                            },
                            Some,
                        ),
                    SettingOwner::Editor => None,
                };
                let allowed = match &decl.kind {
                    artisan_api::SettingKind::Bool => "true or false".to_owned(),
                    artisan_api::SettingKind::Text { hint } => (*hint).to_owned(),
                    artisan_api::SettingKind::Choice { values } => values
                        .iter()
                        .map(|value| {
                            value.map_or_else(|| "null".to_owned(), |value| value.to_string())
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                };
                writeln!(
                    out,
                    "| **{}**: {} | `{}` | {} | {} |",
                    decl.label,
                    format::cell(decl.description),
                    decl.key,
                    value.map_or_else(
                        || "the Editor isn't open".to_owned(),
                        |value| format!("`{value}` ({allowed})")
                    ),
                    match decl.owner {
                        SettingOwner::Forge => "this host",
                        SettingOwner::Editor => "this device",
                    },
                )?;
            }
            Ok(out)
        }
        "settings_get" => {
            #[derive(Deserialize)]
            struct Get {
                key: String,
            }
            let get: Get = args(arguments)?;
            let decl = setting_decl(&get.key).ok_or_else(|| {
                Refused(format!(
                    "There is no setting {}; see settings_list.",
                    get.key
                ))
            })?;
            let value = match decl.owner {
                SettingOwner::Forge => store
                    .setting(decl.key)
                    .await?
                    .unwrap_or_else(|| decl.default_value()),
                SettingOwner::Editor => {
                    hub.call_editor(EditorCall::SettingGet { key: get.key })
                        .await?
                }
            };
            Ok(format!("{} (`{}`) is `{value}`.", decl.label, decl.key))
        }
        "settings_set" => {
            #[derive(Deserialize)]
            struct Set {
                key: String,
                value: Value,
                #[serde(default)]
                navigate_user: bool,
            }
            let set: Set = args(arguments)?;
            let decl = setting_decl(&set.key).ok_or_else(|| {
                Refused(format!(
                    "There is no setting {}; see settings_list.",
                    set.key
                ))
            })?;
            decl.check(&set.value).map_err(Refused)?;
            if decl.needs_approval {
                approve(
                    hub,
                    caller,
                    name,
                    format!("Set {} to {}", decl.label, set.value),
                )
                .await?;
            }
            match decl.owner {
                SettingOwner::Forge => {
                    store
                        .set_setting(set.key.clone(), set.value.clone())
                        .await?;
                }
                SettingOwner::Editor => {
                    hub.call_editor(EditorCall::SettingSet {
                        key: set.key.clone(),
                        value: set.value.clone(),
                    })
                    .await?;
                }
            }
            let note = navigate_if_asked(hub, set.navigate_user, setting_page(&set.key)).await;
            Ok(format!(
                "{} (`{}`) is now `{}`.{note}",
                decl.label, set.key, set.value
            ))
        }
        "hosts_list" => {
            let hosts = hub.call_editor(EditorCall::HostsList).await?;
            let lines: Vec<String> = hosts
                .as_array()
                .into_iter()
                .flatten()
                .map(|host| {
                    let text =
                        |key: &str| host.get(key).and_then(Value::as_str).unwrap_or_default();
                    let mut states = Vec::new();
                    if host.get("connected") == Some(&Value::Bool(true)) {
                        states.push("connected");
                    }
                    if host.get("is_caller") == Some(&Value::Bool(true)) {
                        states.push("this thread's host");
                    }
                    format!(
                        "- **{}** · {}{} `{}`",
                        text("name"),
                        text("address"),
                        if states.is_empty() {
                            String::new()
                        } else {
                            format!(" · {}", states.join(", "))
                        },
                        text("id"),
                    )
                })
                .collect();
            Ok(if lines.is_empty() {
                "No hosts.".to_owned()
            } else {
                lines.join("\n")
            })
        }
        "host_add" => {
            #[derive(Deserialize)]
            struct Add {
                kind: String,
                distro: Option<String>,
                address: Option<String>,
                code: Option<String>,
                fingerprint: Option<String>,
                #[serde(default)]
                navigate_user: bool,
            }
            let add: Add = args(arguments)?;
            let navigate_user = add.navigate_user;
            let call = EditorCall::HostAdd {
                kind: add.kind,
                distro: add.distro,
                address: add.address,
                code: add.code,
                fingerprint: add.fingerprint,
            };
            let added = hub.call_editor(call).await?;
            let note = navigate_if_asked(hub, navigate_user, "hosts").await;
            Ok(format!(
                "Paired with **{}** ({}).{note}",
                added
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("the host"),
                added
                    .get("address")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ))
        }
        "host_remove" => {
            #[derive(Deserialize)]
            struct Remove {
                host: String,
            }
            let remove: Remove = args(arguments)?;
            approve(
                hub,
                caller,
                name,
                format!("Unpair the host {}", remove.host),
            )
            .await?;
            hub.call_editor(EditorCall::HostRemove {
                host: remove.host.clone(),
            })
            .await?;
            Ok(format!("Unpaired {}.", remove.host))
        }
        "host_switch" => {
            #[derive(Deserialize)]
            struct Switch {
                host: String,
            }
            let switch: Switch = args(arguments)?;
            hub.call_editor(EditorCall::HostSwitch {
                host: switch.host.clone(),
            })
            .await?;
            Ok(format!("The Editor is on {} now.", switch.host))
        }
        "projects_list" => {
            let state = store.state().await?;
            if state.projects.is_empty() {
                return Ok("No projects.".to_owned());
            }
            Ok(state
                .projects
                .iter()
                .map(|project| {
                    format!("- **{}** `{}` `{}`", project.name, project.path, project.id)
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "project_add" => {
            #[derive(Deserialize)]
            struct Add {
                path: String,
                name: Option<String>,
            }
            let add: Add = args(arguments)?;
            if !std::path::Path::new(&add.path).is_dir() {
                return Err(Refused(format!("{} is not a folder on this host.", add.path)).into());
            }
            let project = store
                .create_project(artisan_api::CreateProject {
                    id: ProjectId::new(),
                    path: add.path,
                    name: add.name,
                })
                .await?;
            Ok(format!(
                "Added **{}** `{}` `{}`",
                project.name, project.path, project.id
            ))
        }
        "project_remove" => {
            #[derive(Deserialize)]
            struct Remove {
                project: String,
            }
            let remove: Remove = args(arguments)?;
            let id = ProjectId::from(remove.project);
            let project = store
                .project(&id)
                .await?
                .ok_or_else(|| Refused(format!("There is no project {id}.")))?;
            approve(
                hub,
                caller,
                name,
                format!(
                    "Remove the project {} ({}) from Artisan; the folder stays",
                    project.name, project.path
                ),
            )
            .await?;
            store.remove_project(&id).await?;
            Ok(format!("Removed {}.", project.name))
        }
        "mcp_servers_list" => {
            let servers = mcp_servers(hub).await?;
            if servers.is_empty() {
                return Ok("No MCP servers besides Artisan's own.".to_owned());
            }
            let auth = hub.mcp_auth().await?;
            Ok(servers
                .iter()
                .map(|server| {
                    let target = server.url.clone().unwrap_or_else(|| {
                        format!("{} {}", server.command, server.args.join(" "))
                            .trim()
                            .to_owned()
                    });
                    let env = if server.env.is_empty() {
                        String::new()
                    } else {
                        let keys: Vec<&str> = server.env.keys().map(String::as_str).collect();
                        format!(" · env {}", keys.join(", "))
                    };
                    format!(
                        "- **{}** `{target}`{env}{}",
                        server.name,
                        auth_note(auth.get(&server.name))
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "mcp_server_add" => {
            #[derive(Deserialize)]
            struct Add {
                name: String,
                #[serde(default)]
                command: String,
                #[serde(default)]
                args: Vec<String>,
                #[serde(default)]
                env: BTreeMap<String, String>,
                url: Option<String>,
                #[serde(default)]
                navigate_user: bool,
            }
            let add: Add = args(arguments)?;
            let (added, navigate_user) = (add.name.clone(), add.navigate_user);
            if OWN_SERVERS.contains(&add.name.as_str()) {
                return Err(
                    Refused("That name is one of Artisan's own servers.".to_owned()).into(),
                );
            }
            if add.command.is_empty() == add.url.is_none() {
                return Err(
                    Refused("An MCP server has either a command or a url.".to_owned()).into(),
                );
            }
            let what = add
                .url
                .clone()
                .unwrap_or_else(|| format!("{} {}", add.command, add.args.join(" ")));
            approve(
                hub,
                caller,
                name,
                format!("Give every harness the MCP server {} ({what})", add.name),
            )
            .await?;
            let previous = mcp_servers(hub)
                .await?
                .into_iter()
                .find(|server| server.name == add.name);
            let servers = store
                .mutate_mcp(McpMutation::Upsert {
                    previous,
                    server: McpServer {
                        name: add.name,
                        command: add.command,
                        args: add.args,
                        env: add.env,
                        url: add.url,
                        ..McpServer::default()
                    },
                })
                .await?;
            // Find out now whether it needs a sign-in, so the answer can say.
            hub.check_mcp_auth().await;
            let sign_in = match hub.mcp_auth().await?.get(&added) {
                Some(artisan_api::McpAuthStatus::NeedsSignIn) => {
                    " It needs a sign-in first: mcp_server_sign_in."
                }
                Some(artisan_api::McpAuthStatus::NeedsToken) => {
                    " It refuses without a token: the user adds an Authorization header on its card."
                }
                _ => "",
            };
            let note = navigate_if_asked(hub, navigate_user, "extensions").await;
            Ok(format!(
                "{} MCP servers; new threads get them.{sign_in}{note}",
                servers.len()
            ))
        }
        "mcp_server_remove" => {
            #[derive(Deserialize)]
            struct Remove {
                name: String,
            }
            let remove: Remove = args(arguments)?;
            let previous = mcp_servers(hub)
                .await?
                .into_iter()
                .find(|server| server.name == remove.name)
                .ok_or_else(|| Refused(format!("There is no MCP server {}.", remove.name)))?;
            approve(
                hub,
                caller,
                name,
                format!("Remove the MCP server {} from every harness", remove.name),
            )
            .await?;
            store.mutate_mcp(McpMutation::Remove { previous }).await?;
            hub.check_mcp_auth().await;
            Ok(format!("Removed {}.", remove.name))
        }
        "mcp_server_sign_in" => {
            #[derive(Deserialize)]
            struct SignIn {
                name: String,
                #[serde(default)]
                navigate_user: bool,
            }
            let sign_in: SignIn = args(arguments)?;
            let started = hub
                .call_editor(EditorCall::McpSignIn {
                    server: sign_in.name.clone(),
                    navigate_user: sign_in.navigate_user,
                })
                .await?;
            let url = started
                .get("authorize_url")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Ok(if sign_in.navigate_user {
                format!(
                    "Opened {}'s sign-in page in the user's browser and its card in Settings → Extensions. Once they approve, new threads get its tools. If the page didn't open, the link is {url}",
                    sign_in.name
                )
            } else {
                format!(
                    "Give the user this link to sign in to {}: {url}\nOnce they approve it in their browser, new threads get its tools. The Sign in button on its card in Settings → Extensions does the same.",
                    sign_in.name
                )
            })
        }
        "skills_list" => {
            let skills = hub.skill_catalog().await?;
            if skills.is_empty() {
                return Ok("No skills.".to_owned());
            }
            Ok(skills
                .iter()
                .map(|skill| {
                    format!(
                        "- **{}** · {} `{}`{}",
                        skill.name,
                        if skill.enabled { "on" } else { "off" },
                        skill.id,
                        if skill.description.is_empty() {
                            String::new()
                        } else {
                            format!("\n  {}", format::cell(&skill.description))
                        },
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "skill_set_enabled" => {
            #[derive(Deserialize)]
            struct Enable {
                id: String,
                enabled: bool,
            }
            let enable: Enable = args(arguments)?;
            hub.set_skill_enabled(&enable.id, enable.enabled).await?;
            Ok(format!(
                "{} is {}.",
                enable.id,
                if enable.enabled { "on" } else { "off" }
            ))
        }
        "skill_install" => {
            #[derive(Deserialize)]
            struct Install {
                path: String,
                #[serde(default)]
                replace: bool,
            }
            let install: Install = args(arguments)?;
            if install.replace {
                approve(
                    hub,
                    caller,
                    name,
                    format!("Replace a skill with the one in {}", install.path),
                )
                .await?;
            }
            let skill = hub
                .install_skill(install.path.into(), install.replace)
                .await?;
            Ok(format!(
                "Installed {skill} for every harness. New threads get it."
            ))
        }
        "global_prompt_get" => {
            let prompt = hub.global_prompt().await?;
            Ok(if prompt.text.trim().is_empty() {
                "The global prompt is empty.".to_owned()
            } else {
                prompt.text
            })
        }
        "global_prompt_set" => {
            #[derive(Deserialize)]
            struct Set {
                text: String,
                #[serde(default)]
                navigate_user: bool,
            }
            let set: Set = args(arguments)?;
            let navigate_user = set.navigate_user;
            let current = hub.global_prompt().await?;
            approve(
                hub,
                caller,
                name,
                format!(
                    "Replace the global prompt every harness reads ({} characters)",
                    set.text.chars().count()
                ),
            )
            .await?;
            let saved = hub
                .save_global_prompt(artisan_api::SavePrompt {
                    base_version: current.version,
                    text: set.text,
                })
                .await?;
            let note = navigate_if_asked(hub, navigate_user, "global-prompt").await;
            Ok(format!("Saved version {}.{note}", saved.version))
        }
        "harness_set_default_model" => {
            #[derive(Deserialize)]
            struct Default {
                harness: String,
                model: String,
                #[serde(default)]
                navigate_user: bool,
            }
            let default: Default = args(arguments)?;
            let engine = harness(&default.harness)?;
            let model = hub
                .engines()
                .status(engine)
                .models
                .iter()
                .find(|model| model.id == default.model)
                .cloned()
                .ok_or_else(|| {
                    Refused(format!(
                        "{} has no model {}; see harnesses.",
                        engine.display_name(),
                        default.model
                    ))
                })?;
            store.set_default_model(engine, model).await?;
            let page = format!("engines/{}", engine.as_str());
            let note = navigate_if_asked(hub, default.navigate_user, &page).await;
            Ok(format!(
                "New {} threads start on {}.{note}",
                engine.display_name(),
                default.model
            ))
        }
        "harness_install" => {
            #[derive(Deserialize)]
            struct Install {
                harness: String,
                #[serde(default)]
                navigate_user: bool,
            }
            let install: Install = args(arguments)?;
            let engine = harness(&install.harness)?;
            approve(
                hub,
                caller,
                name,
                format!("Install or update {}", engine.display_name()),
            )
            .await?;
            let background = hub.clone();
            tokio::spawn(async move {
                let engines = background.engines().clone();
                match engines.install(engine).await {
                    Ok(_) => {
                        if let Err(error) = background.reconcile_global_prompt().await {
                            tracing::warn!(%engine, "reconcile prompt after install: {error:#}");
                        }
                        if let Err(error) = background.reconcile_skills().await {
                            tracing::warn!(%engine, "reconcile skills after install: {error:#}");
                        }
                        engines.refresh(engine).await;
                    }
                    Err(error) => tracing::warn!(%engine, "installing for an agent: {error:#}"),
                }
            });
            let page = format!("engines/{}", engine.as_str());
            let note = navigate_if_asked(hub, install.navigate_user, &page).await;
            Ok(format!(
                "Installing {}; harnesses shows when it's ready.{note}",
                engine.display_name()
            ))
        }
        "open_thread" => {
            #[derive(Deserialize)]
            struct Open {
                thread: String,
            }
            let open: Open = args(arguments)?;
            let thread = thread_id(&open.thread);
            hub.call_editor(EditorCall::OpenThread {
                thread: thread.clone(),
            })
            .await?;
            let title = store.thread(&thread).await?.and_then(|thread| thread.title);
            Ok(format!(
                "Opened **{}**.",
                title.as_deref().unwrap_or("the thread")
            ))
        }
        "open_settings" => {
            #[derive(Deserialize)]
            struct Open {
                page: String,
            }
            let open: Open = args(arguments)?;
            hub.call_editor(EditorCall::OpenSettings {
                page: open.page.clone(),
            })
            .await?;
            Ok(format!("Opened settings: {}.", open.page))
        }
        _ => Err(Refused(format!("Unknown tool {name}")).into()),
    }
}

async fn mcp_servers(hub: &Hub) -> Result<Vec<McpServer>> {
    Ok(hub
        .store()
        .setting("mcp_servers")
        .await?
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default())
}
