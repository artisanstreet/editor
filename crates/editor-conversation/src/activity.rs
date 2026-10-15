//! Canonical tool categories and labels for work summaries and individual activities.

use artisan_api::{FileOp, Item, ItemKind, SearchMode, Status, ToolKind};

/// The semantic bucket one activity falls in.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ActivityCategory {
    Command,
    FileDelete,
    FileEdit,
    FileRead,
    FileSearch,
    Integration,
    Subagent,
    Tool,
    WebSearch,
}

impl ActivityCategory {
    /// The foreground label shared by row and header.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Command => "Command",
            Self::FileDelete | Self::FileEdit | Self::FileRead | Self::FileSearch => "Files",
            Self::Integration => "Integrations",
            Self::Tool => "Tools",
            Self::Subagent => "Agents",
            Self::WebSearch => "Web",
        }
    }

    /// The lowercase chain clause for `count` members.
    pub(crate) fn count_label(self, count: usize) -> String {
        let plural = |singular: &str, many: &str| {
            if count == 1 {
                singular.to_owned()
            } else {
                format!("{count} {many}")
            }
        };
        let counted = |one: &str, many: &str| {
            if count == 1 {
                one.to_owned()
            } else {
                many.replace("{n}", &count.to_string())
            }
        };
        match self {
            Self::Command => format!("ran {}", plural("a command", "commands")),
            Self::FileDelete => format!("deleted {}", plural("a file", "files")),
            Self::FileEdit => format!("edited {}", plural("a file", "files")),
            Self::FileRead => format!("read {}", plural("a file", "files")),
            Self::FileSearch => counted("searched files", "searched {n} files"),
            Self::Integration => format!("used {}", plural("an integration", "integrations")),
            Self::Tool => format!("used {}", plural("a tool", "tools")),
            Self::Subagent => format!("talked to {}", plural("an agent", "agents")),
            Self::WebSearch => counted("searched the web", "ran {n} web searches"),
        }
    }
}

/// The category one activity item counts under; `None` for items that are
/// not activity (prose, plans, notices).
pub(crate) fn category(item: &Item) -> Option<ActivityCategory> {
    match &item.kind {
        ItemKind::Agent { .. } => Some(ActivityCategory::Subagent),
        ItemKind::Tool { tool, .. } => Some(match tool {
            ToolKind::Shell { .. } => ActivityCategory::Command,
            ToolKind::Read { .. } | ToolKind::ViewImage { .. } => ActivityCategory::FileRead,
            ToolKind::Edit { changes }
                if !changes.is_empty()
                    && changes.inline().is_some_and(|changes| {
                        changes.iter().all(|change| change.op == FileOp::Delete)
                    }) =>
            {
                ActivityCategory::FileDelete
            }
            ToolKind::Edit { .. } => ActivityCategory::FileEdit,
            ToolKind::Search { .. } => ActivityCategory::FileSearch,
            ToolKind::WebFetch { .. } | ToolKind::WebSearch { .. } => ActivityCategory::WebSearch,
            ToolKind::Mcp { .. } => ActivityCategory::Integration,
            ToolKind::AgentMessage { .. } => ActivityCategory::Subagent,
            ToolKind::Other { .. } => ActivityCategory::Tool,
        }),
        _ => None,
    }
}

/// One message sent to an agent (or another session) with `SendMessage`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentMessage {
    /// Where it went: an agent's id (`Agent { agent_id }`), or a session.
    pub to: String,
    pub label: String,
    pub summary: Option<String>,
    pub text: artisan_api::TextContent,
}

/// The message `item` sent, when it is a `SendMessage` call.
pub(crate) fn agent_message(item: &Item) -> Option<AgentMessage> {
    let ItemKind::Tool {
        tool:
            ToolKind::AgentMessage {
                recipient,
                recipient_label,
                summary,
                text,
            },
        ..
    } = &item.kind
    else {
        return None;
    };
    Some(AgentMessage {
        to: recipient.clone(),
        label: recipient_label.clone(),
        summary: summary.clone(),
        text: text.clone(),
    })
}

/// The categories a chain counts, in order: an agent counts once however
/// often it was started or messaged.
pub(crate) fn chain_categories<'a>(items: impl Iterator<Item = &'a Item>) -> Vec<ActivityCategory> {
    let mut agents = std::collections::HashSet::new();
    items
        .filter(|item| talked_to(item).is_none_or(|agent| agents.insert(agent)))
        .filter_map(category)
        .collect()
}

/// The agent an item talks to, for counting a chain's agents once: the
/// agent it started, or the one it messaged.
fn talked_to(item: &Item) -> Option<String> {
    match &item.kind {
        ItemKind::Agent { agent_id, .. } => {
            Some(agent_id.clone().unwrap_or_else(|| item.id.to_string()))
        }
        _ => agent_message(item).map(|message| message.to),
    }
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

/// The fallback words for an activity without a detail of its own.
fn presentation_label(category: ActivityCategory, status: Status) -> &'static str {
    use ActivityCategory as C;
    match status {
        Status::Failed | Status::Cancelled | Status::Declined => match category {
            C::Command => "Command failed",
            C::FileRead => "File read failed",
            C::FileEdit => "File edit failed",
            C::FileDelete => "File delete failed",
            C::FileSearch => "File search failed",
            C::WebSearch => "Web search failed",
            C::Integration => "Integration failed",
            C::Subagent => "Agent failed",
            C::Tool => "Tool failed",
        },
        Status::Running | Status::AwaitingUser => match category {
            C::Command => "Running a command",
            C::FileRead => "Reading a file",
            C::FileEdit => "Editing a file",
            C::FileDelete => "Deleting a file",
            C::FileSearch => "Searching files",
            C::WebSearch => "Searching the web",
            C::Integration => "Using an integration",
            C::Subagent => "Talking to an agent",
            C::Tool => "Using a tool",
        },
        Status::Done => match category {
            C::Command => "Ran a command",
            C::FileRead => "Read a file",
            C::FileEdit => "Edited a file",
            C::FileDelete => "Deleted a file",
            C::FileSearch => "Searched files",
            C::WebSearch => "Searched the web",
            C::Integration => "Used an integration",
            C::Subagent => "Talked to an agent",
            C::Tool => "Used a tool",
        },
    }
}

/// The muted row text for one activity: the normalized shell command for a
/// terminal row, the raw detail otherwise, else the presentation label.
pub(crate) fn detail_text(item: &Item, category: ActivityCategory) -> String {
    let detail: Option<String> = match &item.kind {
        // Artisan's name for it, never the engine's type or id.
        ItemKind::Agent { description, .. } => item.agent_label().map(|label| {
            if description.byte_len() == 0 {
                label
            } else {
                match description {
                    artisan_api::TextContent::Inline(text) => format!("{label}: {text}"),
                    artisan_api::TextContent::Stored(_) => format!("{label}: stored task"),
                }
            }
        }),
        ItemKind::Tool { tool, title, .. } => match tool {
            ToolKind::Shell { command, .. } => Some(match command {
                artisan_api::TextContent::Inline(text) => text.clone(),
                artisan_api::TextContent::Stored(_) => "Stored shell command".into(),
            }),
            ToolKind::Read { path, range } => Some(match range {
                Some((from, to)) => format!("{path}:{from}-{to}"),
                None => path.clone(),
            }),
            ToolKind::Edit {
                changes: artisan_api::Collection::Stored(source),
            } => Some(format!("{} file changes", source.count)),
            ToolKind::Edit {
                changes: artisan_api::Collection::Inline(changes),
            } => match changes.as_slice() {
                [] => None,
                [one] => Some(one.path.clone()),
                [first, rest @ ..] => Some(format!("{} and {} more", first.path, rest.len())),
            },
            ToolKind::Search {
                pattern,
                path,
                mode,
            } => Some(match (mode, path) {
                (SearchMode::List, Some(path)) => path.clone(),
                (_, Some(path)) => format!("{pattern} in {path}"),
                (_, None) => pattern.clone(),
            }),
            ToolKind::WebFetch { url } => Some(url.clone()),
            ToolKind::WebSearch { query } => Some(query.clone()),
            ToolKind::Mcp { server, tool, .. } => Some(
                title
                    .clone()
                    .unwrap_or_else(|| format!("{server} · {tool}")),
            ),
            ToolKind::ViewImage { path } => Some(path.clone()),
            ToolKind::AgentMessage {
                recipient_label, ..
            } => Some(format!("Talked to {recipient_label}")),
            ToolKind::Other { name, .. } => title.clone().or_else(|| Some(name.clone())),
        },
        _ => None,
    };
    detail
        .filter(|detail| !detail.trim().is_empty())
        .unwrap_or_else(|| presentation_label(category, item.status).to_owned())
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
        ActivityCategory::FileDelete => AssetId::TABLER_FILE_X,
        ActivityCategory::FileSearch => AssetId::TABLER_FILE_SEARCH,
        ActivityCategory::WebSearch => AssetId::TABLER_WORLD_SEARCH,
        ActivityCategory::Subagent => AssetId::TABLER_BOT_ID,
        _ => AssetId::TABLER_TOOL,
    }
}
