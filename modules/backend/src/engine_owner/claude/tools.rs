//! Claude tool calls onto the shared work vocabulary.
//!
//! Mirrors the Codex tool projection (`codex/adapter.rs`): a buffered
//! `assistant` frame's `tool_use` block starts one step and the matching
//! `tool_result` in the next `user` frame settles it. Shell commands become
//! terminal activity (command, bounded output, failure); every other tool
//! becomes a tool step under a canonical kind the Editor already groups
//! (`read`, `edit`, `grep`, `web_search`, ...) with its target as detail.
//! Tool input and output never cross this boundary beyond those bounded
//! fields.

use std::collections::HashMap;

use artisan_domain::{
    OBSERVATION_COMMAND_MAX_BYTES, OBSERVATION_LABEL_MAX_BYTES, OBSERVATION_OUTPUT_MAX_BYTES,
    OBSERVATION_TEXT_MAX_BYTES, Observation, ObservationId, ObservationSequence, RunId,
    TerminalActivityInput, TerminalActivityObservation, TerminalActivityState, ToolAction,
    ToolObservation,
};
use serde_json::Value;

/// One `tool_use` block of a buffered assistant frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeToolUse {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) input: Value,
}

/// One `tool_result` block of a root `user` frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeToolResult {
    pub(crate) tool_use_id: String,
    pub(crate) is_error: bool,
    /// The result's text content, joined; empty when it carried none.
    pub(crate) output: String,
}

/// Decodes the `tool_result` blocks of one root `user` message.
pub(crate) fn tool_results(content: &[Value]) -> Vec<ClaudeToolResult> {
    content
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("tool_result"))
        .filter_map(|item| {
            let tool_use_id = item.get("tool_use_id")?.as_str()?.to_owned();
            let output = match item.get("content") {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            Some(ClaudeToolResult {
                tool_use_id,
                is_error: item.get("is_error").and_then(Value::as_bool) == Some(true),
                output,
            })
        })
        .collect()
}

/// How one started tool call projects.
#[derive(Clone, Debug)]
enum OpenTool {
    /// A shell command: terminal activity.
    Command,
    /// Any other tool: a step under this canonical kind and detail.
    Step {
        kind: String,
        detail: Option<String>,
    },
}

/// Run-local tool-call tracker for one Claude turn.
#[derive(Debug, Default)]
pub(crate) struct ClaudeToolTracker {
    open: HashMap<String, OpenTool>,
}

impl ClaudeToolTracker {
    /// Starts one tool call; a repeated `tool_use` id projects nothing.
    pub(crate) fn started(
        &mut self,
        run_id: &RunId,
        frame_sequence: u64,
        tool: &ClaudeToolUse,
    ) -> Vec<Observation> {
        if self.open.contains_key(&tool.id) {
            return Vec::new();
        }
        let open = classify(tool);
        let rows = match &open {
            OpenTool::Command => terminal_row(
                run_id,
                frame_sequence,
                &tool.id,
                TerminalActivityState::Started,
                string_field(&tool.input, "command"),
                None,
            ),
            OpenTool::Step { kind, detail } => step_row(
                run_id,
                frame_sequence,
                &tool.id,
                kind,
                ToolAction::Started,
                detail.as_deref(),
            ),
        };
        self.open.insert(tool.id.clone(), open);
        rows
    }

    /// Settles one tool call with its result; unknown ids project nothing.
    pub(crate) fn finished(
        &mut self,
        run_id: &RunId,
        frame_sequence: u64,
        result: &ClaudeToolResult,
    ) -> Vec<Observation> {
        let Some(open) = self.open.remove(&result.tool_use_id) else {
            return Vec::new();
        };
        match open {
            OpenTool::Command => terminal_row(
                run_id,
                frame_sequence,
                &result.tool_use_id,
                if result.is_error {
                    TerminalActivityState::Failed
                } else {
                    TerminalActivityState::Completed
                },
                None,
                Some(&result.output),
            ),
            OpenTool::Step { kind, detail } => step_row(
                run_id,
                frame_sequence,
                &result.tool_use_id,
                &kind,
                if result.is_error {
                    ToolAction::Failed
                } else {
                    ToolAction::Completed
                },
                detail.as_deref(),
            ),
        }
    }
}

fn string_field<'a>(input: &'a Value, field: &str) -> Option<&'a str> {
    input
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Maps Claude Code's tool names onto the canonical kinds and targets.
fn classify(tool: &ClaudeToolUse) -> OpenTool {
    let input = &tool.input;
    let (kind, detail) = match tool.name.as_str() {
        "Bash" => return OpenTool::Command,
        "Read" => ("read", string_field(input, "file_path")),
        "Write" => ("write", string_field(input, "file_path")),
        "Edit" | "MultiEdit" => ("edit", string_field(input, "file_path")),
        "NotebookEdit" => ("edit", string_field(input, "notebook_path")),
        "Grep" => ("grep", string_field(input, "pattern")),
        "Glob" => ("glob", string_field(input, "pattern")),
        "WebSearch" => ("web_search", string_field(input, "query")),
        "WebFetch" => ("web_fetch", string_field(input, "url")),
        "Task" | "Agent" => ("subagent", string_field(input, "description")),
        name if name.starts_with("mcp__") => ("mcp", Some(name)),
        name => (name, None),
    };
    OpenTool::Step {
        kind: kind.to_owned(),
        detail: detail.map(str::to_owned),
    }
}

/// Keeps at most `max_bytes` of `text`, cut on a character boundary.
fn bounded(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn row_scope(
    run_id: &RunId,
    frame_sequence: u64,
    tool_id: &str,
    slug: &str,
) -> Option<(ObservationId, ObservationSequence, ObservationId)> {
    Some((
        ObservationId::parse(format!(
            "{}:claude:{frame_sequence}:{slug}:{tool_id}",
            run_id.as_str()
        ))
        .ok()?,
        ObservationSequence::new(frame_sequence).ok()?,
        ObservationId::parse(tool_id.to_owned()).ok()?,
    ))
}

fn terminal_row(
    run_id: &RunId,
    frame_sequence: u64,
    tool_id: &str,
    state: TerminalActivityState,
    command: Option<&str>,
    output: Option<&str>,
) -> Vec<Observation> {
    let Some((id, sequence, activity_id)) = row_scope(
        run_id,
        frame_sequence,
        tool_id,
        &format!("term:{}", state.as_str()),
    ) else {
        return Vec::new();
    };
    // An oversize command is omitted while the step is preserved; output is
    // evidence, so it keeps its bounded head.
    let command = command
        .filter(|command| command.len() <= OBSERVATION_COMMAND_MAX_BYTES)
        .map(str::to_owned);
    let output = output
        .filter(|output| !output.is_empty())
        .map(|output| bounded(output, OBSERVATION_OUTPUT_MAX_BYTES).to_owned());
    TerminalActivityObservation::new(
        id,
        sequence,
        TerminalActivityInput {
            activity_id,
            channel: None,
            command,
            shell: None,
            output,
            exit_code: None,
            state,
        },
    )
    .ok()
    .map(Observation::TerminalActivity)
    .into_iter()
    .collect()
}

fn step_row(
    run_id: &RunId,
    frame_sequence: u64,
    tool_id: &str,
    kind: &str,
    action: ToolAction,
    detail: Option<&str>,
) -> Vec<Observation> {
    let Some((id, sequence, tool_id)) = row_scope(
        run_id,
        frame_sequence,
        tool_id,
        &format!("tool:{}", action.as_str()),
    ) else {
        return Vec::new();
    };
    let kind = bounded(kind, OBSERVATION_LABEL_MAX_BYTES).to_owned();
    let detail = detail.map(|detail| bounded(detail, OBSERVATION_TEXT_MAX_BYTES).to_owned());
    ToolObservation::new(id, sequence, tool_id, kind, action, detail)
        .ok()
        .map(Observation::Tool)
        .into_iter()
        .collect()
}
