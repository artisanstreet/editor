//! What each activity says: the verb and object of its row, and what it
//! counts as in its work chain's header.

use artisan_api::{
    AgentAction, FileOp, Item, ItemId, ItemKind, MessageDirection, SearchMode, Status, ToolKind,
};
use std::collections::HashMap;

/// What one activity counts as in a work chain's header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ActivityCategory {
    Command,
    FileCreate,
    FileDelete,
    FileEdit,
    FileRead,
    /// Grep and glob searches.
    FileSearch,
    FolderList,
    Integration,
    /// Artisan's own tools, other than recall.
    Artisan,
    Message,
    /// Searching or listing other threads through Artisan.
    ThreadSearch,
    /// Reading another thread through Artisan.
    ThreadRead,
    Subagent,
    Tool,
    WebSearch,
    WebPage,
}

impl ActivityCategory {
    /// The lowercase chain clause for `count` members: a verb and a counted
    /// noun ("ran 3 searches"), never "did X 3 times".
    pub(crate) fn count_label(self, count: usize) -> String {
        let counted = |one: &str, many: &str| {
            if count == 1 {
                one.to_owned()
            } else {
                many.replace("{n}", &count.to_string())
            }
        };
        match self {
            Self::Command => counted("ran a command", "ran {n} commands"),
            Self::FileCreate => counted("created a file", "created {n} files"),
            Self::FileDelete => counted("deleted a file", "deleted {n} files"),
            Self::FileEdit => counted("edited a file", "edited {n} files"),
            Self::FileRead => counted("read a file", "read {n} files"),
            Self::FileSearch => counted("searched files", "ran {n} searches"),
            Self::FolderList => counted("listed a folder", "listed {n} folders"),
            Self::Integration => counted("used an integration", "used {n} integrations"),
            Self::Artisan => counted("used Artisan Editor", "took {n} actions in Artisan Editor"),
            Self::ThreadSearch => counted("searched threads", "ran {n} thread searches"),
            Self::ThreadRead => counted("read a thread", "read {n} threads"),
            Self::Message => counted("exchanged a message", "exchanged {n} messages"),
            Self::Subagent => counted("delegated a task", "delegated {n} tasks"),
            Self::Tool => counted("used a tool", "used {n} tools"),
            Self::WebSearch => counted("searched the web", "ran {n} web searches"),
            Self::WebPage => counted("read a web page", "read {n} web pages"),
        }
    }
}

/// The category one activity item counts under; `None` for items that are
/// not activity (prose, plans, notices).
pub fn category(item: &Item) -> Option<ActivityCategory> {
    use ActivityCategory as C;
    match &item.kind {
        ItemKind::Agent { .. } => Some(C::Subagent),
        // An agent's result Forge delivered: its report, not the user's words.
        ItemKind::UserMessage {
            from_agent: Some(_),
            ..
        } => Some(C::Message),
        ItemKind::Tool { tool, .. } => Some(match tool {
            ToolKind::Shell { .. } => C::Command,
            ToolKind::Read { .. } | ToolKind::ViewImage { .. } => C::FileRead,
            ToolKind::Edit { changes } => match edit_op(changes) {
                Some(FileOp::Add) => C::FileCreate,
                Some(FileOp::Delete) => C::FileDelete,
                _ => C::FileEdit,
            },
            ToolKind::Search {
                mode: SearchMode::List,
                ..
            } => C::FolderList,
            ToolKind::Search { .. } => C::FileSearch,
            ToolKind::WebSearch { .. } => C::WebSearch,
            ToolKind::WebFetch { .. } => C::WebPage,
            ToolKind::Mcp { server, tool, .. } if server == ARTISAN => match tool.as_str() {
                "thread_read" => C::ThreadRead,
                tool if THREAD_SEARCH.contains(&tool) => C::ThreadSearch,
                _ => C::Artisan,
            },
            ToolKind::Mcp { .. } => C::Integration,
            ToolKind::AgentMessage { .. } => C::Message,
            ToolKind::Agents { .. } | ToolKind::Other { .. } => C::Tool,
        }),
        ItemKind::UserMessage { .. }
        | ItemKind::AssistantMessage { .. }
        | ItemKind::Question { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => None,
    }
}

/// The one op all of an edit's inline changes share.
fn edit_op(changes: &artisan_api::Collection<artisan_api::FileChange>) -> Option<&FileOp> {
    let changes = changes.inline()?;
    let first = &changes.first()?.op;
    changes
        .iter()
        .all(|change| std::mem::discriminant(&change.op) == std::mem::discriminant(first))
        .then_some(first)
}

/// One activity row's words: what was done, in the foreground, then what it
/// was done to, muted. Nothing follows them: no exit codes or line counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub verb: String,
    pub object: String,
    /// The object is code (a command) and sets in monospace.
    pub code: bool,
}

/// The row's words for `item`. `agents` names each agent by its task, the
/// name it has everywhere else in the thread.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per kind of call, each its own words"
)]
pub fn line(item: &Item, agents: &HashMap<ItemId, String>) -> Line {
    let live = matches!(item.status, Status::Running | Status::AwaitingUser);
    let tense = |done: &str, running: &str| if live { running } else { done }.to_owned();
    let agent = |id: Option<&ItemId>| id.and_then(|id| agents.get(id)).cloned();
    let mut code = false;
    let (verb, object) = match &item.kind {
        ItemKind::UserMessage {
            from_agent: Some(from),
            ..
        } => (
            "Reported back".to_owned(),
            agent(Some(from)).unwrap_or_else(|| "an agent".to_owned()),
        ),
        ItemKind::Agent { description, .. } => (
            "Delegated".to_owned(),
            description
                .inline()
                .map_or_else(|| "a task".to_owned(), first_line),
        ),
        ItemKind::Tool { tool, title, .. } => match tool {
            ToolKind::Shell {
                command,
                background,
                ..
            } => {
                code = true;
                (
                    if *background {
                        tense("Started", "Starting")
                    } else {
                        tense("Ran", "Running")
                    },
                    command
                        .inline()
                        .map_or_else(|| "a command".to_owned(), first_line),
                )
            }
            ToolKind::Read { path, range } => (
                tense("Read", "Reading"),
                match range {
                    Some((from, to)) => format!("{path}:{from}-{to}"),
                    None => path.clone(),
                },
            ),
            ToolKind::ViewImage { path } => (tense("Viewed", "Viewing"), path.clone()),
            ToolKind::Edit { changes } => {
                let verb = match edit_op(changes) {
                    Some(FileOp::Add) => tense("Created", "Creating"),
                    Some(FileOp::Delete) => tense("Deleted", "Deleting"),
                    Some(FileOp::Move { .. }) => tense("Moved", "Moving"),
                    _ => tense("Edited", "Editing"),
                };
                let object = match changes {
                    artisan_api::Collection::Stored(source) => format!("{} files", source.count),
                    artisan_api::Collection::Inline(changes) => match changes.as_slice() {
                        [] => "a file".to_owned(),
                        [one] => match &one.op {
                            FileOp::Move { from } => format!("{from} to {}", one.path),
                            FileOp::Add | FileOp::Modify | FileOp::Delete => one.path.clone(),
                        },
                        [first, rest @ ..] => {
                            format!("{} and {} more", first.path, rest.len())
                        }
                    },
                };
                (verb, object)
            }
            ToolKind::Search {
                pattern,
                path,
                mode,
            } => {
                let within = |text: &str| {
                    path.as_ref()
                        .map_or_else(|| text.to_owned(), |path| format!("{text} in {path}"))
                };
                match mode {
                    SearchMode::Grep => (tense("Searched for", "Searching for"), within(pattern)),
                    SearchMode::Glob => (tense("Found files", "Finding files"), within(pattern)),
                    SearchMode::List => (
                        tense("Listed", "Listing"),
                        path.clone().unwrap_or_else(|| pattern.clone()),
                    ),
                }
            }
            ToolKind::WebSearch { query } => (
                tense("Searched the web", "Searching the web"),
                query.clone(),
            ),
            ToolKind::WebFetch { url } => (
                tense("Read a page", "Reading a page"),
                url.trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .to_owned(),
            ),
            ToolKind::Mcp {
                server,
                tool,
                input,
            } if server == ARTISAN => {
                let input = input
                    .as_ref()
                    .and_then(|input| serde_json::from_str(input.inline()?).ok())
                    .unwrap_or_default();
                let words = artisan(tool, &input, title.as_deref(), live);
                code = words.2;
                (words.0, words.1)
            }
            ToolKind::Other { name, input } if name == "ToolSearch" => {
                let query = input
                    .as_ref()
                    .and_then(|input| first_field(input.inline()?))
                    .unwrap_or_default();
                let loaded = query.strip_prefix("select:").map(tool_names);
                loaded.map_or_else(
                    || (tense("Searched tools for", "Searching tools for"), query),
                    |names| (tense("Loaded tools", "Loading tools"), names),
                )
            }
            ToolKind::Mcp { server, tool, .. } => (
                tense("Used", "Using"),
                // claude.ai connectors are `claude_ai_<Name>`.
                format!(
                    "{} {}",
                    sentence(&words(server.trim_start_matches("claude_ai_"))),
                    words(tool)
                ),
            ),
            ToolKind::AgentMessage {
                recipient,
                target,
                summary,
                text,
                direction,
            } => {
                let peer = agent(target.as_ref());
                let session = || {
                    if recipient.starts_with("uds:") {
                        "another session".to_owned()
                    } else {
                        recipient.clone()
                    }
                };
                let main = recipient == "main";
                match (direction, peer) {
                    (MessageDirection::Report, _) => (
                        tense("Reported back", "Reporting back"),
                        summary
                            .clone()
                            .unwrap_or_else(|| text.inline().map(first_line).unwrap_or_default()),
                    ),
                    (MessageDirection::Sent, Some(task)) => (tense("Messaged", "Messaging"), task),
                    (MessageDirection::Sent, None) if main => {
                        (tense("Messaged", "Messaging"), "the main agent".to_owned())
                    }
                    (MessageDirection::Sent, None) => {
                        (tense("Messaged session", "Messaging session"), session())
                    }
                    (MessageDirection::Received, Some(task)) => ("Reply from".to_owned(), task),
                    (MessageDirection::Received, None) if main => {
                        ("Message from".to_owned(), "the main agent".to_owned())
                    }
                    (MessageDirection::Received, None) => {
                        ("Message from session".to_owned(), session())
                    }
                }
            }
            ToolKind::Agents {
                action,
                addresses,
                targets,
            } => {
                let tasks: Vec<String> = targets.iter().filter_map(|id| agent(Some(id))).collect();
                match action {
                    AgentAction::List => (
                        tense("Listed agents and sessions", "Listing agents and sessions"),
                        String::new(),
                    ),
                    AgentAction::Wait => (
                        tense("Waited for", "Waiting for"),
                        if tasks.is_empty() {
                            counted_agents(addresses.len())
                        } else {
                            tasks.join(", ")
                        },
                    ),
                    AgentAction::Stop => (
                        tense("Stopped", "Stopping"),
                        if tasks.is_empty() {
                            "a background job".to_owned()
                        } else {
                            tasks.join(", ")
                        },
                    ),
                }
            }
            ToolKind::Other { name, input } => (
                sentence(&words(name)),
                title
                    .clone()
                    .or_else(|| {
                        input
                            .as_ref()
                            .and_then(|input| first_field(input.inline()?))
                    })
                    .map(|text| first_line(&text))
                    .unwrap_or_default(),
            ),
        },
        ItemKind::UserMessage { .. }
        | ItemKind::AssistantMessage { .. }
        | ItemKind::Question { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => (String::new(), String::new()),
    };
    Line { verb, object, code }
}

/// The server name engines know Artisan's own tools by.
const ARTISAN: &str = "artisan";
/// Artisan's tools that search or list threads.
const THREAD_SEARCH: [&str; 4] = [
    "threads_search",
    "threads_grep",
    "threads_touching",
    "thread_list",
];

/// An Artisan call's verb and object, and whether the object is code.
/// `named` is what Forge resolved the call's thread or agents to.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per Artisan tool keeps every verb and object side by side"
)]
fn artisan(
    tool: &str,
    input: &serde_json::Map<String, serde_json::Value>,
    named: Option<&str>,
    live: bool,
) -> (String, String, bool) {
    let field = |key: &str| match input.get(key) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(value) => value.to_string(),
    };
    let quoted = |fallback: &str| {
        named.map_or_else(
            || fallback.to_owned(),
            |name| format!("\u{201c}{name}\u{201d}"),
        )
    };
    let agents = || named.unwrap_or("an agent").to_owned();
    let (done, running, object, code): (&str, &str, String, bool) = match tool {
        "threads_search" => (
            "Searched threads for",
            "Searching threads for",
            field("query"),
            false,
        ),
        "threads_grep" => (
            "Searched threads for",
            "Searching threads for",
            field("pattern"),
            true,
        ),
        "threads_touching" => (
            "Found threads that touched",
            "Finding threads that touched",
            field("path"),
            true,
        ),
        "thread_list" => ("Listed threads", "Listing threads", field("title"), false),
        "thread_read" => ("Read", "Reading", quoted("this thread"), false),
        "thread_create" => (
            "Started a thread",
            "Starting a thread",
            field("title"),
            false,
        ),
        "thread_send" => ("Messaged", "Messaging", quoted("a thread"), false),
        "thread_cancel" => ("Stopped", "Stopping", quoted("a thread"), false),
        "thread_update" => ("Updated", "Updating", quoted("this thread"), false),
        "requests_list" => (
            "Checked what waits on you",
            "Checking what waits on you",
            String::new(),
            false,
        ),
        "harnesses" => (
            "Listed harnesses",
            "Listing harnesses",
            String::new(),
            false,
        ),
        "agent_spawn" => ("Delegated", "Delegating", field("task"), false),
        "agent_wait" => ("Waited for", "Waiting for", agents(), false),
        "agent_message" => ("Messaged", "Messaging", agents(), false),
        "agent_read" => ("Read", "Reading", agents(), false),
        "agent_cancel" => ("Stopped", "Stopping", agents(), false),
        "agent_list" => ("Listed agents", "Listing agents", String::new(), false),
        "settings_list" => ("Listed settings", "Listing settings", String::new(), false),
        "settings_get" => (
            "Read the setting",
            "Reading the setting",
            words(&field("key")),
            false,
        ),
        "settings_set" => (
            "Set",
            "Setting",
            format!("{} to {}", words(&field("key")), field("value")),
            false,
        ),
        "hosts_list" => ("Listed hosts", "Listing hosts", String::new(), false),
        "host_add" => (
            "Added a host",
            "Adding a host",
            [field("distro"), field("address")]
                .into_iter()
                .find(|text| !text.is_empty())
                .unwrap_or_else(|| words(&field("kind"))),
            false,
        ),
        "host_remove" => (
            "Removed the host",
            "Removing the host",
            field("host"),
            false,
        ),
        "host_switch" => ("Switched to", "Switching to", field("host"), false),
        "projects_list" => ("Listed projects", "Listing projects", String::new(), false),
        "project_add" => (
            "Added the project",
            "Adding the project",
            field("path"),
            true,
        ),
        "project_remove" => (
            "Removed a project",
            "Removing a project",
            String::new(),
            false,
        ),
        "mcp_servers_list" => (
            "Listed MCP servers",
            "Listing MCP servers",
            String::new(),
            false,
        ),
        "mcp_server_add" => (
            "Added the MCP server",
            "Adding the MCP server",
            field("name"),
            false,
        ),
        "mcp_server_sign_in" => (
            "Started signing in to",
            "Starting to sign in to",
            field("name"),
            false,
        ),
        "mcp_server_remove" => (
            "Removed the MCP server",
            "Removing the MCP server",
            field("name"),
            false,
        ),
        "skills_list" => ("Listed skills", "Listing skills", String::new(), false),
        "skill_set_enabled" => match input.get("enabled") {
            Some(serde_json::Value::Bool(false)) => {
                ("Turned off", "Turning off", field("id"), false)
            }
            _ => ("Turned on", "Turning on", field("id"), false),
        },
        "skill_install" => (
            "Installed the skill",
            "Installing the skill",
            field("path"),
            false,
        ),
        "global_prompt_get" => (
            "Read the global prompt",
            "Reading the global prompt",
            String::new(),
            false,
        ),
        "global_prompt_set" => (
            "Replaced the global prompt",
            "Replacing the global prompt",
            String::new(),
            false,
        ),
        "harness_set_default_model" => (
            "Set the default model",
            "Setting the default model",
            format!("{} to {}", field("harness"), field("model")),
            false,
        ),
        "harness_install" => ("Installed", "Installing", field("harness"), false),
        "open_thread" => ("Opened", "Opening", quoted("a thread"), false),
        "open_settings" => (
            "Opened settings",
            "Opening settings",
            words(&field("page")),
            false,
        ),
        other => (
            "Used Artisan Editor",
            "Using Artisan Editor",
            words(other),
            false,
        ),
    };
    (if live { running } else { done }.to_owned(), object, code)
}

fn counted_agents(count: usize) -> String {
    match count {
        0 => "agents".to_owned(),
        1 => "an agent".to_owned(),
        count => format!("{count} agents"),
    }
}

/// The first non-empty line, without markdown heading marks.
fn first_line(text: &str) -> String {
    text.lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned()
}

/// The tools a `select:` tool search loads, by name.
fn tool_names(names: &str) -> String {
    names
        .split(',')
        .map(|name| {
            // MCP tools are `mcp__<server>__<tool>`.
            let name = name.trim();
            match name.rsplit_once("__") {
                Some((_, tool)) => words(tool),
                None => name.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// An engine identifier as lowercase words: `ToolSearch`, `tool_search` and
/// `tool-search` all read "tool search".
fn words(name: &str) -> String {
    let mut out = String::new();
    let mut previous: Option<char> = None;
    for c in name.chars() {
        if matches!(c, '_' | '-') {
            out.push(' ');
        } else {
            if c.is_uppercase() && previous.is_some_and(char::is_lowercase) {
                out.push(' ');
            }
            out.extend(c.to_lowercase());
        }
        previous = Some(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The first short string field of a JSON input: what the call was about.
fn first_field(input: &str) -> Option<String> {
    let serde_json::Value::Object(map) = serde_json::from_str(input).ok()? else {
        return None;
    };
    map.values()
        .filter_map(serde_json::Value::as_str)
        .find(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

/// One message between agents or sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMessage {
    /// The thread's `Agent` item on the other side; `None` for another
    /// session, the main agent or an agent this thread doesn't have.
    pub to: Option<artisan_api::ItemId>,
    pub direction: MessageDirection,
    pub summary: Option<String>,
    pub text: artisan_api::TextContent,
}

/// The message `item` is, when it is one.
pub fn agent_message(item: &Item) -> Option<AgentMessage> {
    if let ItemKind::UserMessage {
        text,
        from_agent: Some(from),
        ..
    } = &item.kind
    {
        return Some(AgentMessage {
            to: Some(from.clone()),
            direction: MessageDirection::Report,
            summary: None,
            text: text.clone(),
        });
    }
    let ItemKind::Tool {
        tool:
            ToolKind::AgentMessage {
                target,
                summary,
                text,
                direction,
                ..
            },
        ..
    } = &item.kind
    else {
        return None;
    };
    Some(AgentMessage {
        to: target.clone(),
        direction: *direction,
        summary: summary.clone(),
        text: text.clone(),
    })
}

/// The categories a chain counts, in order.
pub fn chain_categories<'a>(items: impl Iterator<Item = &'a Item>) -> Vec<ActivityCategory> {
    items.filter_map(category).collect()
}

/// The chain header's sentence: composition counted in first-appearance
/// order, the first clause capitalized ("Ran 1 command, read 2 files").
pub fn chain_clause(categories: impl Iterator<Item = ActivityCategory>) -> String {
    let mut composition: Vec<(ActivityCategory, usize)> = Vec::new();
    for category in categories {
        match composition.iter_mut().find(|(known, _)| *known == category) {
            Some((_, count)) => *count += 1,
            None => composition.push((category, 1)),
        }
    }
    let mut text = String::new();
    for (index, (category, count)) in composition.iter().enumerate() {
        let clause = category.count_label(*count);
        if index == 0 {
            let mut characters = clause.chars();
            if let Some(first) = characters.next() {
                text.extend(first.to_uppercase());
                text.push_str(characters.as_str());
            }
        } else {
            text.push_str(", ");
            text.push_str(&clause);
        }
    }
    text
}

/// The original category glyph for a chain, or the mixed-activity list glyph.
pub fn chain_icon(categories: impl Iterator<Item = ActivityCategory>) -> editor_ui::AssetId {
    use editor_ui::AssetId;
    let mut categories = categories;
    let Some(first) = categories.next() else {
        return AssetId::TABLER_LIST_DETAILS;
    };
    if categories.any(|category| category != first) {
        return AssetId::TABLER_LIST_DETAILS;
    }
    match first {
        ActivityCategory::Command => AssetId::TABLER_TERMINAL_2,
        ActivityCategory::FileRead => AssetId::TABLER_FILE_TEXT,
        ActivityCategory::FileEdit => AssetId::TABLER_FILE_PENCIL,
        ActivityCategory::FileCreate => AssetId::TABLER_FILE_SPARK,
        ActivityCategory::FileDelete => AssetId::TABLER_FILE_X,
        ActivityCategory::FileSearch => AssetId::TABLER_FILE_SEARCH,
        ActivityCategory::FolderList => AssetId::TABLER_FOLDER,
        ActivityCategory::WebSearch => AssetId::TABLER_WORLD_SEARCH,
        ActivityCategory::WebPage => AssetId::TABLER_WORLD,
        ActivityCategory::Subagent => AssetId::TABLER_BOT_ID,
        ActivityCategory::Message => AssetId::TABLER_MESSAGES,
        ActivityCategory::Integration
        | ActivityCategory::Artisan
        | ActivityCategory::ThreadSearch
        | ActivityCategory::ThreadRead
        | ActivityCategory::Tool => AssetId::TABLER_TOOL,
    }
}
