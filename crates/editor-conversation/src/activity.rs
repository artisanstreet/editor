//! What each activity says: the verb and object of its row, and what it
//! counts as in its work chain's header.

use artisan_api::{
    AgentAction, FileOp, Item, ItemId, ItemKind, MessageDirection, SearchMode, Status, ToolKind,
};
use std::collections::HashMap;

/// What one activity counts as in a work chain's header.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ActivityCategory {
    Command,
    FileCreate,
    FileDelete,
    FileEdit,
    FileRead,
    FileSearch,
    Integration,
    Message,
    Subagent,
    Tool,
    WebSearch,
}

impl ActivityCategory {
    /// The lowercase chain clause for `count` members.
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
            Self::FileSearch => counted("searched files", "searched files {n} times"),
            Self::Integration => counted("used an integration", "used {n} integrations"),
            Self::Message => counted("exchanged a message", "exchanged {n} messages"),
            Self::Subagent => counted("delegated a task", "delegated {n} tasks"),
            Self::Tool => counted("used a tool", "used {n} tools"),
            Self::WebSearch => counted("searched the web", "searched the web {n} times"),
        }
    }
}

/// The category one activity item counts under; `None` for items that are
/// not activity (prose, plans, notices).
pub(crate) fn category(item: &Item) -> Option<ActivityCategory> {
    use ActivityCategory as C;
    match &item.kind {
        ItemKind::Agent { .. } => Some(C::Subagent),
        ItemKind::Tool { tool, .. } => Some(match tool {
            ToolKind::Shell { .. } => C::Command,
            ToolKind::Read { .. } | ToolKind::ViewImage { .. } => C::FileRead,
            ToolKind::Edit { changes } => match edit_op(changes) {
                Some(FileOp::Add) => C::FileCreate,
                Some(FileOp::Delete) => C::FileDelete,
                _ => C::FileEdit,
            },
            ToolKind::Search { .. } => C::FileSearch,
            ToolKind::WebFetch { .. } | ToolKind::WebSearch { .. } => C::WebSearch,
            ToolKind::Mcp { .. } => C::Integration,
            ToolKind::AgentMessage { .. } => C::Message,
            ToolKind::Agents { .. } | ToolKind::Other { .. } => C::Tool,
        }),
        _ => None,
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
/// was done to, muted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub verb: String,
    pub object: String,
    /// The object is code (a command) and sets in monospace.
    pub code: bool,
    pub tail: Option<Tail>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Tail {
    /// Lines an edit added and removed.
    Changed { added: usize, removed: usize },
    /// Why it didn't finish: "failed", "exit 101", "declined", "stopped".
    Failed(String),
}

/// The row's words for `item`. `agents` names each agent by its task, the
/// name it has everywhere else in the thread.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per kind of call, each its own words"
)]
pub(crate) fn line(item: &Item, agents: &HashMap<ItemId, String>) -> Line {
    let live = matches!(item.status, Status::Running | Status::AwaitingUser);
    let tense = |done: &str, running: &str| if live { running } else { done }.to_owned();
    let agent = |id: Option<&ItemId>| id.and_then(|id| agents.get(id)).cloned();
    let mut code = false;
    let mut tail = match item.status {
        Status::Failed => Some(Tail::Failed("failed".into())),
        Status::Declined => Some(Tail::Failed("declined".into())),
        Status::Cancelled => Some(Tail::Failed("stopped".into())),
        _ => None,
    };
    let (verb, object) = match &item.kind {
        ItemKind::Agent { description, .. } => (
            "Delegated".to_owned(),
            description
                .inline()
                .map_or_else(|| "a task".to_owned(), first_line),
        ),
        ItemKind::Tool {
            tool,
            title,
            exit_code,
            ..
        } => match tool {
            ToolKind::Shell {
                command,
                background,
                ..
            } => {
                code = true;
                if let Some(code) = exit_code.filter(|code| *code != 0) {
                    tail = Some(Tail::Failed(format!("exit {code}")));
                }
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
                            _ => one.path.clone(),
                        },
                        [first, rest @ ..] => {
                            format!("{} and {} more", first.path, rest.len())
                        }
                    },
                };
                if tail.is_none() {
                    tail = changed(changes);
                }
                (verb, object)
            }
            ToolKind::Search {
                pattern,
                path,
                mode,
            } => {
                let within = |text: &str| match path {
                    Some(path) => format!("{text} in {path}"),
                    None => text.to_owned(),
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
        _ => (String::new(), String::new()),
    };
    Line {
        verb,
        object,
        code,
        tail,
    }
}

/// Lines added and removed across an edit's inline diffs.
fn changed(changes: &artisan_api::Collection<artisan_api::FileChange>) -> Option<Tail> {
    let (mut added, mut removed) = (0, 0);
    for diff in changes
        .inline()?
        .iter()
        .filter_map(|change| change.diff.as_ref()?.inline())
    {
        for line in diff.lines() {
            if line.starts_with('+') && !line.starts_with("+++") {
                added += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                removed += 1;
            }
        }
    }
    (added + removed > 0).then_some(Tail::Changed { added, removed })
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
pub(crate) struct AgentMessage {
    /// The thread's `Agent` item on the other side; `None` for another
    /// session, the main agent or an agent this thread doesn't have.
    pub to: Option<artisan_api::ItemId>,
    pub direction: MessageDirection,
    pub summary: Option<String>,
    pub text: artisan_api::TextContent,
}

/// The message `item` is, when it is one.
pub(crate) fn agent_message(item: &Item) -> Option<AgentMessage> {
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
pub(crate) fn chain_categories<'a>(items: impl Iterator<Item = &'a Item>) -> Vec<ActivityCategory> {
    items.filter_map(category).collect()
}

/// The chain header's sentence: composition counted in first-appearance
/// order, the first clause capitalized ("Ran 1 command, read 2 files").
pub(crate) fn chain_clause(categories: impl Iterator<Item = ActivityCategory>) -> String {
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
pub(crate) fn chain_icon(categories: impl Iterator<Item = ActivityCategory>) -> editor_ui::AssetId {
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
        ActivityCategory::WebSearch => AssetId::TABLER_WORLD_SEARCH,
        ActivityCategory::Subagent => AssetId::TABLER_BOT_ID,
        ActivityCategory::Message => AssetId::TABLER_MESSAGES,
        _ => AssetId::TABLER_TOOL,
    }
}
