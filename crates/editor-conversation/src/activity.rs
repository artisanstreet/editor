//! The activity vocabulary of work rows and trace chains, ported from the
//! old Editor's `conversation_scene` and `transcript_helpers`: the category
//! each tool counts under, its foreground label, the counted chain clause
//! ("Ran 2 commands, read a file"), the category glyphs, and the muted
//! detail text (the normalized shell command for terminal rows).

use artisan_api::{FileOp, Item, ItemKind, SearchMode, Status, ToolKind};
use editor_ui::AssetId;

/// The semantic bucket one activity falls in.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ActivityCategory {
    AppInspect,
    Command,
    Database,
    Diff,
    FileDelete,
    FileEdit,
    FileRead,
    FileSearch,
    GitStatus,
    Integration,
    Other,
    Subagent,
    Test,
    Tool,
    Typecheck,
    WebSearch,
}

impl ActivityCategory {
    /// The foreground label shared by row and header.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::AppInspect => "App",
            Self::Command => "Command",
            Self::Database => "Database",
            Self::Diff => "Changes",
            Self::FileDelete | Self::FileEdit | Self::FileRead | Self::FileSearch => "Files",
            Self::GitStatus => "Git",
            Self::Integration => "Integrations",
            Self::Other | Self::Tool => "Tools",
            Self::Subagent => "Agents",
            Self::Test => "Tests",
            Self::Typecheck => "Types",
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
            Self::AppInspect => counted("inspected the app", "ran {n} app inspections"),
            Self::Command => format!("ran {}", plural("a command", "commands")),
            Self::Database => counted("inspected the database", "ran {n} database inspections"),
            Self::Diff => counted("reviewed changes", "reviewed {n} diffs"),
            Self::FileDelete => format!("deleted {}", plural("a file", "files")),
            Self::FileEdit => format!("edited {}", plural("a file", "files")),
            Self::FileRead => format!("read {}", plural("a file", "files")),
            Self::FileSearch => counted("searched files", "searched {n} files"),
            Self::GitStatus => counted("checked Git status", "ran {n} Git status checks"),
            Self::Integration => format!("used {}", plural("an integration", "integrations")),
            Self::Other | Self::Tool => format!("used {}", plural("a tool", "tools")),
            Self::Subagent => format!("talked to {}", plural("an agent", "agents")),
            Self::Test => counted("ran tests", "ran {n} test runs"),
            Self::Typecheck => counted("checked types", "ran {n} type checks"),
            Self::WebSearch => counted("searched the web", "ran {n} web searches"),
        }
    }

    /// The Tabler glyph naming the category.
    pub(crate) const fn icon(self) -> AssetId {
        match self {
            Self::Command | Self::Test | Self::Typecheck => AssetId::TABLER_TERMINAL_2,
            Self::FileRead => AssetId::TABLER_FILE_TEXT,
            Self::FileEdit => AssetId::TABLER_FILE_PENCIL,
            Self::FileDelete => AssetId::TABLER_FILE_X,
            Self::FileSearch => AssetId::TABLER_FILE_SEARCH,
            Self::WebSearch => AssetId::TABLER_WORLD_SEARCH,
            Self::Subagent => AssetId::TABLER_BOT_ID,
            _ => AssetId::TABLER_TOOL,
        }
    }

    /// Unrecognised kinds and generic tools read identically, so they share
    /// one clause and one glyph in a chain header.
    const fn in_chain(self) -> Self {
        match self {
            Self::Other => Self::Tool,
            category => category,
        }
    }
}

/// Classifies a tool name exactly like the old reference helper: the
/// narrower semantics are tested before the broad `tool` catch.
pub(crate) fn category_of_name(kind: &str) -> ActivityCategory {
    let value = kind.to_lowercase().replace(['-', '_'], ".");
    let has = |needle: &str| value.contains(needle);
    if has("terminal") || has("command") || has("shell") || has("bash") || has("exec") {
        return ActivityCategory::Command;
    }
    if value == "file"
        || value == "read"
        || value == "read.file"
        || has("file.read")
        || has("workspace.read")
        || value
            .rsplit_once('.')
            .is_some_and(|(_, last)| last == "read")
    {
        return ActivityCategory::FileRead;
    }
    if has("file.delete") {
        return ActivityCategory::FileDelete;
    }
    if value == "write"
        || value == "edit"
        || value == "apply"
        || has("file.edit")
        || has("file.write")
        || has("workspace.edit")
        || has("workspace.write")
        || has("apply.patch")
    {
        return ActivityCategory::FileEdit;
    }
    if has("workspace.search")
        || has("file.list")
        || has("grep")
        || has("glob")
        || has("find")
        || has("ripgrep")
    {
        return ActivityCategory::FileSearch;
    }
    if value == "search" || has("web.search") || has("fetch") {
        return ActivityCategory::WebSearch;
    }
    if has("test") {
        return ActivityCategory::Test;
    }
    if has("typescript") || has("typecheck") {
        return ActivityCategory::Typecheck;
    }
    if has("git.status") {
        return ActivityCategory::GitStatus;
    }
    if has("diff") {
        return ActivityCategory::Diff;
    }
    if has("database") {
        return ActivityCategory::Database;
    }
    if has("preview") || has("browser") || has("ui.inspect") || has("accessibility") {
        return ActivityCategory::AppInspect;
    }
    if has("subagent") || has("agent.activity") {
        return ActivityCategory::Subagent;
    }
    if has("mcp") || has("integration") {
        return ActivityCategory::Integration;
    }
    if value == "tool" || has("tool") || has("plugin") {
        return ActivityCategory::Tool;
    }
    ActivityCategory::Other
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
                    && changes.iter().all(|change| change.op == FileOp::Delete) =>
            {
                ActivityCategory::FileDelete
            }
            ToolKind::Edit { .. } => ActivityCategory::FileEdit,
            ToolKind::Search { .. } => ActivityCategory::FileSearch,
            ToolKind::WebFetch { .. } | ToolKind::WebSearch { .. } => ActivityCategory::WebSearch,
            ToolKind::Mcp { .. } => ActivityCategory::Integration,
            ToolKind::Other { name, .. } if name == SEND_MESSAGE => ActivityCategory::Subagent,
            ToolKind::Other { name, .. } => category_of_name(name),
        }),
        _ => None,
    }
}

/// Claude's tool for messaging an agent it started, or another session.
const SEND_MESSAGE: &str = "SendMessage";

/// One message sent to an agent (or another session) with `SendMessage`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentMessage {
    /// Where it went: an agent's id (`Agent { agent_id }`), or a session.
    pub to: String,
    pub summary: Option<String>,
    pub text: String,
}

/// The message `item` sent, when it is a `SendMessage` call.
pub(crate) fn agent_message(item: &Item) -> Option<AgentMessage> {
    let ItemKind::Tool {
        tool: ToolKind::Other {
            name,
            input: Some(input),
        },
        ..
    } = &item.kind
    else {
        return None;
    };
    if name != SEND_MESSAGE {
        return None;
    }
    let input: serde_json::Value = serde_json::from_str(input).ok()?;
    let field = |key: &str| {
        input[key]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
    };
    Some(AgentMessage {
        to: field("to")?,
        summary: field("summary"),
        text: field("message").unwrap_or_default(),
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

/// Who a message went to, for its row: the agent's name when it is one of
/// this thread's agents, the session's name for another session.
pub(crate) fn recipient(to: &str, agents: &std::collections::HashMap<String, String>) -> String {
    if let Some(name) = agents.get(to) {
        name.clone()
    } else if to.starts_with("uds:") || to.contains('/') {
        "another session".to_owned()
    } else if to.len() >= 16 && to.chars().all(|c| c.is_ascii_alphanumeric()) {
        "an agent".to_owned()
    } else {
        to.to_owned()
    }
}

/// The chain header's sentence: composition counted in first-appearance
/// order, the first clause capitalized ("Ran 1 command, read 2 files").
pub(crate) fn chain_clause(categories: impl Iterator<Item = ActivityCategory>) -> String {
    let mut composition: Vec<(ActivityCategory, usize)> = Vec::new();
    for category in categories.map(ActivityCategory::in_chain) {
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

/// The chain header's glyph: a homogeneous chain shows its category, mixed
/// work the group glyph.
pub(crate) fn chain_icon(categories: impl Iterator<Item = ActivityCategory>) -> AssetId {
    let mut seen: Vec<ActivityCategory> = Vec::new();
    for category in categories.map(ActivityCategory::in_chain) {
        if !seen.contains(&category) {
            seen.push(category);
        }
    }
    match seen.as_slice() {
        [only] => only.icon(),
        _ => AssetId::TABLER_LIST_DETAILS,
    }
}

/// The fallback words for an activity without a detail of its own.
fn presentation_label(category: ActivityCategory, status: Status) -> &'static str {
    use ActivityCategory as C;
    match status {
        Status::Failed | Status::Cancelled | Status::Declined => match category {
            C::Command => "Command failed",
            C::Test => "Tests failed",
            C::Typecheck => "Type check failed",
            C::FileRead => "File read failed",
            C::FileEdit => "File edit failed",
            C::FileDelete => "File delete failed",
            C::FileSearch => "File search failed",
            C::WebSearch => "Web search failed",
            C::Diff => "Change review failed",
            C::GitStatus => "Git status failed",
            C::Database => "Database inspection failed",
            C::AppInspect => "App inspection failed",
            C::Integration => "Integration failed",
            C::Subagent => "Agent failed",
            C::Other | C::Tool => "Tool failed",
        },
        Status::Running | Status::AwaitingUser => match category {
            C::Command => "Running a command",
            C::Test => "Running tests",
            C::Typecheck => "Checking types",
            C::FileRead => "Reading a file",
            C::FileEdit => "Editing a file",
            C::FileDelete => "Deleting a file",
            C::FileSearch => "Searching files",
            C::WebSearch => "Searching the web",
            C::Diff => "Reviewing changes",
            C::GitStatus => "Checking Git status",
            C::Database => "Inspecting the database",
            C::AppInspect => "Inspecting the app",
            C::Integration => "Using an integration",
            C::Subagent => "Talking to an agent",
            C::Other | C::Tool => "Using a tool",
        },
        Status::Done => match category {
            C::Command => "Ran a command",
            C::Test => "Ran tests",
            C::Typecheck => "Checked types",
            C::FileRead => "Read a file",
            C::FileEdit => "Edited a file",
            C::FileDelete => "Deleted a file",
            C::FileSearch => "Searched files",
            C::WebSearch => "Searched the web",
            C::Diff => "Reviewed changes",
            C::GitStatus => "Checked Git status",
            C::Database => "Inspected the database",
            C::AppInspect => "Inspected the app",
            C::Integration => "Used an integration",
            C::Subagent => "Talked to an agent",
            C::Other | C::Tool => "Used a tool",
        },
    }
}

/// The muted row text for one activity: the normalized shell command for a
/// terminal row, the raw detail otherwise, else the presentation label.
pub(crate) fn detail_text(item: &Item, category: ActivityCategory) -> String {
    let detail: Option<String> = match &item.kind {
        // Artisan's name for it, never the engine's type or id.
        ItemKind::Agent { description, .. } => item.agent_label().map(|label| {
            if description.is_empty() {
                label
            } else {
                format!("{label}: {description}")
            }
        }),
        ItemKind::Tool { tool, title, .. } => match tool {
            ToolKind::Shell { command, .. } => Some(present_shell_command(command)),
            ToolKind::Read { path, range } => Some(match range {
                Some((from, to)) => format!("{path}:{from}-{to}"),
                None => path.clone(),
            }),
            ToolKind::Edit { changes } => match changes.as_slice() {
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
            ToolKind::Other { name, .. } => title.clone().or_else(|| Some(name.clone())),
        },
        _ => None,
    };
    detail
        .filter(|detail| !detail.trim().is_empty())
        .unwrap_or_else(|| presentation_label(category, item.status).to_owned())
}

/// One recognised shell wrapper: its executable names and command flags.
struct ShellWrapper {
    flags: &'static [&'static str],
    names: &'static [&'static str],
}

const SHELL_WRAPPERS: &[ShellWrapper] = &[
    ShellWrapper {
        flags: &["-command", "-c"],
        names: &["pwsh", "powershell"],
    },
    ShellWrapper {
        flags: &["-c", "-lc", "-ic", "-lic"],
        names: &["bash", "sh", "zsh", "dash"],
    },
    ShellWrapper {
        flags: &["/c", "/k"],
        names: &["cmd"],
    },
];

/// Splits one leading argument, honouring the quotes a path with spaces
/// needs; returns `(rest, value)`.
fn take_shell_argument(input: &str) -> (&str, &str) {
    let text = input.trim_start();
    let quote = match text.chars().next() {
        Some(quote @ ('"' | '\'')) => Some(quote),
        _ => None,
    };
    if let Some(quote) = quote
        && let Some(offset) = text[1..].find(quote)
    {
        let close = 1 + offset;
        return (&text[close + 1..], &text[1..close]);
    }
    match text.find(char::is_whitespace) {
        Some(break_at) => (&text[break_at..], &text[..break_at]),
        None => ("", text),
    }
}

fn shell_executable_name(path: &str) -> String {
    let file = path.rfind(['/', '\\']).map_or(path, |at| &path[at + 1..]);
    let lowered = file.to_lowercase();
    lowered.strip_suffix(".exe").unwrap_or(&lowered).to_owned()
}

fn unquote_shell_argument(text: &str) -> &str {
    match text.chars().next() {
        Some(first @ ('"' | '\'')) if text.len() > 1 && text.ends_with(first) => {
            &text[1..text.len() - 1]
        }
        _ => text,
    }
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The command a reader means, recovered from the invocation an engine
/// reports: `bash -lc "cargo test"` reads `cargo test`. Anything not
/// recognised comes back as it came, whitespace collapsed.
pub(crate) fn present_shell_command(command: &str) -> String {
    let collapsed = collapse_whitespace(command);
    let (rest, executable) = take_shell_argument(&collapsed);
    let name = shell_executable_name(executable);
    let Some(wrapper) = SHELL_WRAPPERS
        .iter()
        .find(|candidate| candidate.names.contains(&name.as_str()))
    else {
        return collapsed;
    };
    let (body, flag) = take_shell_argument(rest);
    if !wrapper.flags.contains(&flag.to_lowercase().as_str()) {
        return collapsed;
    }
    let body = unquote_shell_argument(body.trim());
    if body.is_empty() {
        collapsed
    } else {
        body.to_owned()
    }
}
