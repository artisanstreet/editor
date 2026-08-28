//! Dependency-free conversation activity classification and presentation.
//!
//! This module is the native counterpart of
//! `modules/protocol/src/conversation-activity.ts`. It deliberately stops at
//! the deterministic mapper boundary: protocol decoding, provider adapters,
//! and rendering remain outside this module. All caller text retained by an
//! input or returned by a presentation is owned, so a projection can outlive
//! the decoded row that supplied it.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

/// The renderer-visible lifecycle values accepted by conversation activities.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConversationLifecycle {
    /// The activity exists but work has not started.
    Pending,
    /// Text or reasoning is arriving incrementally.
    Streaming,
    /// Work is actively progressing.
    Active,
    /// Work is waiting for input or another dependency.
    Waiting,
    /// Work completed successfully.
    Completed,
    /// Work ended because of a failure.
    Failed,
    /// Work was externally stopped and may be resumed.
    Interrupted,
    /// Work was deliberately cancelled.
    Cancelled,
}

/// The normalized semantic bucket assigned to one activity kind.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConversationActivityCategory {
    /// Inspecting the application or its accessibility surface.
    AppInspect,
    /// Running a shell or terminal command.
    Command,
    /// Inspecting a database.
    Database,
    /// Reviewing a diff or changes.
    Diff,
    /// Deleting a file.
    FileDelete,
    /// Editing or writing a file.
    FileEdit,
    /// Reading a file.
    FileRead,
    /// Searching or listing files.
    FileSearch,
    /// Checking Git status.
    GitStatus,
    /// Using an external integration.
    Integration,
    /// An unrecognized activity kind.
    Other,
    /// Talking to an Artisan-owned subagent.
    Subagent,
    /// Running tests.
    Test,
    /// Using a generic tool.
    Tool,
    /// Checking types.
    Typecheck,
    /// Searching the web.
    WebSearch,
}

/// The Artisan-owned identity attached to a delegated activity.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationActivitySubagent {
    /// Stable identity used to combine lifecycle rows for one worker.
    pub agent_id: String,
    /// Reader-facing name used in singular subagent copy.
    pub display_name: String,
}

impl ConversationActivitySubagent {
    /// Creates an owned subagent identity without normalizing either value.
    #[must_use]
    pub fn new(agent_id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            display_name: display_name.into(),
        }
    }
}

/// The small owned activity projection consumed by the presentation mapper.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationActivityPresentationInput {
    /// Provider-neutral or provider-specific activity kind.
    pub kind: String,
    /// Caller-facing activity label, retained verbatim for fallback copy.
    pub label: String,
    /// Current lifecycle of the activity row.
    pub status: ConversationLifecycle,
    /// Delegated worker identity, when this row represents a subagent.
    pub subagent: Option<ConversationActivitySubagent>,
}

impl ConversationActivityPresentationInput {
    /// Creates an owned mapper input without validating or changing its text.
    #[must_use]
    pub fn new(
        kind: impl Into<String>,
        label: impl Into<String>,
        status: ConversationLifecycle,
        subagent: Option<ConversationActivitySubagent>,
    ) -> Self {
        Self {
            kind: kind.into(),
            label: label.into(),
            status,
            subagent,
        }
    }
}

/// The subset of an activity row needed to describe one grouped trace clause.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationActivityGroupMember {
    /// Activity kind retained with the grouped member for the caller boundary.
    pub kind: String,
    /// Delegated worker identity, when this row represents a subagent.
    pub subagent: Option<ConversationActivitySubagent>,
}

impl ConversationActivityGroupMember {
    /// Creates an owned grouped member without normalizing its kind.
    #[must_use]
    pub fn new(kind: impl Into<String>, subagent: Option<ConversationActivitySubagent>) -> Self {
        Self {
            kind: kind.into(),
            subagent,
        }
    }
}

/// The exact foreground label returned for one activity row.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationActivityPresentation {
    /// Reader-facing copy for the activity.
    pub label: String,
}

/// The exact lowercase clause and count returned for one grouped category.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationActivityGroupPresentation {
    /// Number of grouped activities, after subagent identity coalescing.
    pub count: usize,
    /// Lowercase clause; a renderer capitalizes only the first clause in a chain.
    pub label: String,
}

/// Returns the stable foreground label for a normalized category.
#[must_use]
pub const fn get_conversation_activity_category_label(
    category: ConversationActivityCategory,
) -> &'static str {
    match category {
        ConversationActivityCategory::AppInspect => "App",
        ConversationActivityCategory::Command => "Command",
        ConversationActivityCategory::Database => "Database",
        ConversationActivityCategory::Diff => "Changes",
        ConversationActivityCategory::FileDelete
        | ConversationActivityCategory::FileEdit
        | ConversationActivityCategory::FileRead
        | ConversationActivityCategory::FileSearch => "Files",
        ConversationActivityCategory::GitStatus => "Git",
        ConversationActivityCategory::Integration => "Integrations",
        ConversationActivityCategory::Other | ConversationActivityCategory::Tool => "Tools",
        ConversationActivityCategory::Subagent => "Subagents",
        ConversationActivityCategory::Test => "Tests",
        ConversationActivityCategory::Typecheck => "Types",
        ConversationActivityCategory::WebSearch => "Web",
    }
}

/// Classifies one activity kind after the TypeScript mapper's exact
/// lowercasing and separator normalization.
///
/// The condition order is part of the contract. In particular, command and
/// file-read semantics are considered before broader or later categories.
#[must_use]
pub fn get_conversation_activity_category(kind: &str) -> ConversationActivityCategory {
    let value = normalize_kind(kind);

    if value.contains("terminal")
        || value.contains("command")
        || value.contains("shell")
        || value.contains("bash")
        || value.contains("exec")
    {
        return ConversationActivityCategory::Command;
    }
    if value == "file"
        || value == "read"
        || value == "read.file"
        || value.contains("file.read")
        || value.contains("workspace.read")
        || value.strip_suffix(".read").is_some()
    {
        return ConversationActivityCategory::FileRead;
    }
    if value.contains("file.delete") {
        return ConversationActivityCategory::FileDelete;
    }
    if value == "write"
        || value == "edit"
        || value == "apply"
        || value.contains("file.edit")
        || value.contains("file.write")
        || value.contains("workspace.edit")
        || value.contains("workspace.write")
        || value.contains("apply.patch")
    {
        return ConversationActivityCategory::FileEdit;
    }
    if value.contains("workspace.search")
        || value.contains("file.list")
        || value.contains("grep")
        || value.contains("glob")
        || value.contains("find")
        || value.contains("ripgrep")
    {
        return ConversationActivityCategory::FileSearch;
    }
    if value == "search" || value.contains("web.search") || value.contains("fetch") {
        return ConversationActivityCategory::WebSearch;
    }
    if value.contains("test") {
        return ConversationActivityCategory::Test;
    }
    if value.contains("typescript") || value.contains("typecheck") {
        return ConversationActivityCategory::Typecheck;
    }
    if value.contains("git.status") {
        return ConversationActivityCategory::GitStatus;
    }
    if value.contains("diff") {
        return ConversationActivityCategory::Diff;
    }
    if value.contains("database") {
        return ConversationActivityCategory::Database;
    }
    if value.contains("preview")
        || value.contains("browser")
        || value.contains("ui.inspect")
        || value.contains("accessibility")
    {
        return ConversationActivityCategory::AppInspect;
    }
    if value.contains("subagent") || value.contains("agent.activity") {
        return ConversationActivityCategory::Subagent;
    }
    if value.contains("mcp") || value.contains("integration") {
        return ConversationActivityCategory::Integration;
    }
    if value == "tool" || value.contains("tool") || value.contains("plugin") {
        return ConversationActivityCategory::Tool;
    }

    ConversationActivityCategory::Other
}

/// Returns the lowercase count clause for one category and count.
#[must_use]
pub fn get_conversation_activity_count_label(
    category: ConversationActivityCategory,
    count: usize,
) -> String {
    match category {
        ConversationActivityCategory::AppInspect => {
            if count == 1 {
                String::from("inspected the app")
            } else {
                format!("ran {count} app inspections")
            }
        }
        ConversationActivityCategory::Command => {
            format!("ran {}", plural(count, "a command", "commands"))
        }
        ConversationActivityCategory::Database => {
            if count == 1 {
                String::from("inspected the database")
            } else {
                format!("ran {count} database inspections")
            }
        }
        ConversationActivityCategory::Diff => {
            if count == 1 {
                String::from("reviewed changes")
            } else {
                format!("reviewed {count} diffs")
            }
        }
        ConversationActivityCategory::FileDelete => {
            format!("deleted {}", plural(count, "a file", "files"))
        }
        ConversationActivityCategory::FileEdit => {
            format!("edited {}", plural(count, "a file", "files"))
        }
        ConversationActivityCategory::FileRead => {
            format!("read {}", plural(count, "a file", "files"))
        }
        ConversationActivityCategory::FileSearch => {
            if count == 1 {
                String::from("searched files")
            } else {
                format!("searched {count} files")
            }
        }
        ConversationActivityCategory::GitStatus => {
            if count == 1 {
                String::from("checked Git status")
            } else {
                format!("ran {count} Git status checks")
            }
        }
        ConversationActivityCategory::Integration => {
            format!("used {}", plural(count, "an integration", "integrations"))
        }
        ConversationActivityCategory::Other | ConversationActivityCategory::Tool => {
            format!("used {}", plural(count, "a tool", "tools"))
        }
        ConversationActivityCategory::Subagent => {
            format!("talked to {}", plural(count, "a subagent", "subagents"))
        }
        ConversationActivityCategory::Test => {
            if count == 1 {
                String::from("ran tests")
            } else {
                format!("ran {count} test runs")
            }
        }
        ConversationActivityCategory::Typecheck => {
            if count == 1 {
                String::from("checked types")
            } else {
                format!("ran {count} type checks")
            }
        }
        ConversationActivityCategory::WebSearch => {
            if count == 1 {
                String::from("searched the web")
            } else {
                format!("ran {count} web searches")
            }
        }
    }
}

/// Describes one category inside an adjacent activity chain.
///
/// Non-subagent categories count rows directly. Subagent rows are coalesced by
/// `agent_id`, with a repeated identity retaining its latest display name and
/// its first insertion position, exactly like JavaScript `Map`.
#[must_use]
pub fn get_conversation_activity_group_presentation(
    category: ConversationActivityCategory,
    activities: &[ConversationActivityGroupMember],
) -> ConversationActivityGroupPresentation {
    if category != ConversationActivityCategory::Subagent {
        return ConversationActivityGroupPresentation {
            count: activities.len(),
            label: get_conversation_activity_count_label(category, activities.len()),
        };
    }

    let mut named_agents = Vec::<(String, String)>::new();
    let mut anonymous_count = 0;
    for activity in activities {
        let Some(subagent) = activity.subagent.as_ref() else {
            anonymous_count += 1;
            continue;
        };

        if let Some((_, display_name)) = named_agents
            .iter_mut()
            .find(|(agent_id, _)| agent_id == &subagent.agent_id)
        {
            display_name.clone_from(&subagent.display_name);
        } else {
            named_agents.push((subagent.agent_id.clone(), subagent.display_name.clone()));
        }
    }

    let count = named_agents.len() + anonymous_count;
    let label = if count == 1 && anonymous_count == 0 {
        // The count guarantees that the map contains one value. An empty
        // display name remains Some in JavaScript and therefore intentionally
        // produces the trailing space in `talked to `.
        format!("talked to {}", named_agents[0].1)
    } else {
        get_conversation_activity_count_label(category, count)
    };

    ConversationActivityGroupPresentation { count, label }
}

/// Maps one provider-neutral activity row to stable human copy.
///
/// Older `OpenCode` rows may carry `tool` as their kind and the real tool name
/// as `label`; that label is reclassified before generic-tool presentation.
/// Unknown kinds return the original label verbatim. All dynamic output is
/// owned by the returned value.
#[must_use]
pub fn get_conversation_activity_presentation(
    activity: &ConversationActivityPresentationInput,
) -> ConversationActivityPresentation {
    let mut category = get_conversation_activity_category(&activity.kind);

    if matches!(
        category,
        ConversationActivityCategory::Tool | ConversationActivityCategory::Other
    ) && activity.label != "Tool"
        && activity.label != "Tools"
    {
        let label_category = get_conversation_activity_category(&activity.label);
        if !matches!(
            label_category,
            ConversationActivityCategory::Tool | ConversationActivityCategory::Other
        ) {
            category = label_category;
        }
    }

    if category == ConversationActivityCategory::Other {
        return ConversationActivityPresentation {
            label: activity.label.clone(),
        };
    }

    if category == ConversationActivityCategory::Tool
        && activity.label != "Tool"
        && activity.label != "Tools"
    {
        let name = normalize_tool_name(&activity.label);
        if !name.is_empty() {
            let label = match activity_state(activity.status) {
                ActivityState::Active => format!("Using {name}"),
                ActivityState::Completed => format!("Used {name}"),
                ActivityState::Failed => {
                    format!("{} failed", capitalize_first_javascript(&name))
                }
            };
            return ConversationActivityPresentation { label };
        }
    }

    if category == ConversationActivityCategory::Subagent
        && let Some(subagent) = activity.subagent.as_ref()
    {
        let label = if matches!(
            activity.status,
            ConversationLifecycle::Failed | ConversationLifecycle::Cancelled
        ) {
            format!("{}'s work failed", subagent.display_name)
        } else if activity.status == ConversationLifecycle::Completed {
            format!("Talked to {}", subagent.display_name)
        } else {
            format!("Talking to {}", subagent.display_name)
        };
        return ConversationActivityPresentation { label };
    }

    ConversationActivityPresentation {
        label: activity_copy(category, activity_state(activity.status)),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActivityState {
    Active,
    Completed,
    Failed,
}

fn activity_state(status: ConversationLifecycle) -> ActivityState {
    match status {
        ConversationLifecycle::Failed
        | ConversationLifecycle::Cancelled
        | ConversationLifecycle::Interrupted => ActivityState::Failed,
        ConversationLifecycle::Completed => ActivityState::Completed,
        ConversationLifecycle::Pending
        | ConversationLifecycle::Streaming
        | ConversationLifecycle::Active
        | ConversationLifecycle::Waiting => ActivityState::Active,
    }
}

#[allow(clippy::too_many_lines)]
fn activity_copy(category: ConversationActivityCategory, state: ActivityState) -> String {
    match (category, state) {
        (ConversationActivityCategory::AppInspect, ActivityState::Active) => {
            String::from("Inspecting the app")
        }
        (ConversationActivityCategory::AppInspect, ActivityState::Completed) => {
            String::from("Inspected the app")
        }
        (ConversationActivityCategory::AppInspect, ActivityState::Failed) => {
            String::from("App inspection failed")
        }
        (ConversationActivityCategory::Command, ActivityState::Active) => {
            String::from("Running a command")
        }
        (ConversationActivityCategory::Command, ActivityState::Completed) => {
            String::from("Ran a command")
        }
        (ConversationActivityCategory::Command, ActivityState::Failed) => {
            String::from("Command failed")
        }
        (ConversationActivityCategory::Database, ActivityState::Active) => {
            String::from("Inspecting the database")
        }
        (ConversationActivityCategory::Database, ActivityState::Completed) => {
            String::from("Inspected the database")
        }
        (ConversationActivityCategory::Database, ActivityState::Failed) => {
            String::from("Database inspection failed")
        }
        (ConversationActivityCategory::Diff, ActivityState::Active) => {
            String::from("Reviewing changes")
        }
        (ConversationActivityCategory::Diff, ActivityState::Completed) => {
            String::from("Reviewed changes")
        }
        (ConversationActivityCategory::Diff, ActivityState::Failed) => {
            String::from("Change review failed")
        }
        (ConversationActivityCategory::FileDelete, ActivityState::Active) => {
            String::from("Deleting a file")
        }
        (ConversationActivityCategory::FileDelete, ActivityState::Completed) => {
            String::from("Deleted a file")
        }
        (ConversationActivityCategory::FileDelete, ActivityState::Failed) => {
            String::from("File delete failed")
        }
        (ConversationActivityCategory::FileEdit, ActivityState::Active) => {
            String::from("Editing a file")
        }
        (ConversationActivityCategory::FileEdit, ActivityState::Completed) => {
            String::from("Edited a file")
        }
        (ConversationActivityCategory::FileEdit, ActivityState::Failed) => {
            String::from("File edit failed")
        }
        (ConversationActivityCategory::FileRead, ActivityState::Active) => {
            String::from("Reading a file")
        }
        (ConversationActivityCategory::FileRead, ActivityState::Completed) => {
            String::from("Read a file")
        }
        (ConversationActivityCategory::FileRead, ActivityState::Failed) => {
            String::from("File read failed")
        }
        (ConversationActivityCategory::FileSearch, ActivityState::Active) => {
            String::from("Searching files")
        }
        (ConversationActivityCategory::FileSearch, ActivityState::Completed) => {
            String::from("Searched files")
        }
        (ConversationActivityCategory::FileSearch, ActivityState::Failed) => {
            String::from("File search failed")
        }
        (ConversationActivityCategory::GitStatus, ActivityState::Active) => {
            String::from("Checking Git status")
        }
        (ConversationActivityCategory::GitStatus, ActivityState::Completed) => {
            String::from("Checked Git status")
        }
        (ConversationActivityCategory::GitStatus, ActivityState::Failed) => {
            String::from("Git status failed")
        }
        (ConversationActivityCategory::Integration, ActivityState::Active) => {
            String::from("Using an integration")
        }
        (ConversationActivityCategory::Integration, ActivityState::Completed) => {
            String::from("Used an integration")
        }
        (ConversationActivityCategory::Integration, ActivityState::Failed) => {
            String::from("Integration failed")
        }
        (ConversationActivityCategory::Other, ActivityState::Active) => String::from("Working"),
        (ConversationActivityCategory::Other, ActivityState::Completed) => String::from("Worked"),
        (ConversationActivityCategory::Other, ActivityState::Failed) => String::from("Work failed"),
        (ConversationActivityCategory::Subagent, ActivityState::Active) => {
            String::from("Talking to a subagent")
        }
        (ConversationActivityCategory::Subagent, ActivityState::Completed) => {
            String::from("Talked to a subagent")
        }
        (ConversationActivityCategory::Subagent, ActivityState::Failed) => {
            String::from("Subagent work failed")
        }
        (ConversationActivityCategory::Test, ActivityState::Active) => {
            String::from("Running tests")
        }
        (ConversationActivityCategory::Test, ActivityState::Completed) => String::from("Ran tests"),
        (ConversationActivityCategory::Test, ActivityState::Failed) => String::from("Tests failed"),
        (ConversationActivityCategory::Tool, ActivityState::Active) => String::from("Using a tool"),
        (ConversationActivityCategory::Tool, ActivityState::Completed) => {
            String::from("Used a tool")
        }
        (ConversationActivityCategory::Tool, ActivityState::Failed) => String::from("Tool failed"),
        (ConversationActivityCategory::Typecheck, ActivityState::Active) => {
            String::from("Checking types")
        }
        (ConversationActivityCategory::Typecheck, ActivityState::Completed) => {
            String::from("Checked types")
        }
        (ConversationActivityCategory::Typecheck, ActivityState::Failed) => {
            String::from("Type check failed")
        }
        (ConversationActivityCategory::WebSearch, ActivityState::Active) => {
            String::from("Searching the web")
        }
        (ConversationActivityCategory::WebSearch, ActivityState::Completed) => {
            String::from("Searched the web")
        }
        (ConversationActivityCategory::WebSearch, ActivityState::Failed) => {
            String::from("Web search failed")
        }
    }
}

fn plural(count: usize, singular: &str, many: &str) -> String {
    if count == 1 {
        singular.to_owned()
    } else {
        format!("{count} {many}")
    }
}

fn normalize_kind(kind: &str) -> String {
    kind.to_lowercase().replace(['-', '_'], ".")
}

/// JavaScript's `trim()` and `\s` use this exact `WhiteSpace` set in the
/// regular expressions used by the source mapper. Rust's `char::is_whitespace`
/// additionally accepts U+0085, while JavaScript does not, so it cannot be
/// substituted here.
const fn is_ecmascript_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

fn normalize_tool_name(label: &str) -> String {
    let trimmed = label.trim_matches(is_ecmascript_whitespace);
    let separators_replaced = replace_separator_runs(trimmed);
    collapse_ecmascript_whitespace(&separators_replaced)
}

fn replace_separator_runs(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_separator_run = false;
    for character in input.chars() {
        if matches!(character, '.' | '_' | '-') {
            if !in_separator_run {
                output.push(' ');
                in_separator_run = true;
            }
        } else {
            output.push(character);
            in_separator_run = false;
        }
    }
    output
}

fn collapse_ecmascript_whitespace(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_whitespace_run = false;
    for character in input.chars() {
        if is_ecmascript_whitespace(character) {
            if !in_whitespace_run {
                output.push(' ');
                in_whitespace_run = true;
            }
        } else {
            output.push(character);
            in_whitespace_run = false;
        }
    }
    output
}

fn capitalize_first_javascript(value: &str) -> String {
    let Some(first) = value.chars().next() else {
        return String::new();
    };

    // JavaScript indexes strings by UTF-16 code units. For a supplementary
    // scalar, `name[0]` is an unmatched high surrogate and uppercasing it is
    // a no-op; concatenating it with `slice(1)` returns the original scalar.
    if first.len_utf16() == 2 {
        return value.to_owned();
    }

    let mut output = first.to_uppercase().collect::<String>();
    output.push_str(&value[first.len_utf8()..]);
    output
}
