//! Claude tool calls onto the shared work vocabulary.
//!
//! Mirrors the Codex tool projection (`codex/adapter.rs`): a buffered
//! `assistant` frame's `tool_use` block starts one step and the matching
//! `tool_result` in the next `user` frame settles it. Shell commands become
//! terminal activity (command, bounded output, failure); every other tool
//! becomes a tool step under a canonical kind the Editor already groups
//! (`read`, `edit`, `grep`, `web_search`, ...) with its target as detail.
//! Tool input and output never cross this boundary beyond those bounded
//! fields. Image parts of a result (a `Read` on a picture, an MCP
//! screenshot) are what the model looked at, not something it showed: they
//! project nothing. An agent shows an image with an `artisan-image` block.
//!
//! Every shell command is also a job on the thread's terminal board. A
//! command run in the background keeps going after its tool call settles:
//! Claude Code writes its output to a file, which the turn's
//! [`ShellTail`](super::super::shell_tail::ShellTail) follows until the CLI
//! reports the job stopped.

use std::collections::HashMap;

use artisan_domain::{
    OBSERVATION_COMMAND_MAX_BYTES, OBSERVATION_LABEL_MAX_BYTES, OBSERVATION_OUTPUT_MAX_BYTES,
    OBSERVATION_TEXT_MAX_BYTES, Observation, ObservationId, ObservationSequence, RunId,
    TerminalActivityInput, TerminalActivityObservation, TerminalActivityState, ToolAction,
    ToolObservation,
};
use serde_json::Value;

use super::super::observation::{ShellJobChange, ShellJobEnd, ShellJobRow};
use super::super::shell_tail::ShellTail;

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
    /// The background job a `Bash` call started, from the frame's
    /// structured `tool_use_result`.
    pub(crate) background_task_id: Option<String>,
}

/// Decodes the `tool_result` blocks of one root `user` message.
///
/// `tool_use_result` is the frame's structured result. Claude Code sends one
/// per frame, so it belongs to the frame's only tool result.
pub(crate) fn tool_results(
    content: &[Value],
    tool_use_result: Option<&Value>,
) -> Vec<ClaudeToolResult> {
    let background_task_id = tool_use_result
        .and_then(|result| result.get("backgroundTaskId"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= OBSERVATION_LABEL_MAX_BYTES)
        .map(str::to_owned);
    let mut results = content
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
                background_task_id: None,
            })
        })
        .collect::<Vec<_>>();
    if let [only] = results.as_mut_slice() {
        only.background_task_id = background_task_id;
    }
    results
}

/// The output file Claude Code names in a background `Bash` result:
/// `... Output is being written to: <path>. ...`.
pub(crate) fn background_output_path(output: &str, task_id: &str) -> Option<String> {
    const MARKER: &str = "Output is being written to: ";
    let from = output.find(MARKER)? + MARKER.len();
    let rest = &output[from..];
    let suffix = format!("{task_id}.output");
    let end = rest.find(&suffix)? + suffix.len();
    let path = &rest[..end];
    (!path.contains('\n')).then(|| path.to_owned())
}

/// How one Claude Code background task report reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeTaskReport {
    pub(crate) task_id: String,
    pub(crate) tool_use_id: Option<String>,
    /// `local_bash`, `local_agent`, ...; only `task_started` names it.
    pub(crate) task_type: Option<String>,
    /// `completed`, `failed`, `killed`, ... once the task stopped.
    pub(crate) status: Option<String>,
    /// Human summary of a stopped task, e.g. `... failed with exit code 3`.
    pub(crate) summary: Option<String>,
    /// The task's output file, when the report names it.
    pub(crate) output_file: Option<String>,
}

impl ClaudeTaskReport {
    /// How a stopped task ended, when the report says it stopped.
    fn end(&self) -> Option<ShellJobEnd> {
        let code = self.summary.as_deref().and_then(exit_code_in);
        match self.status.as_deref()? {
            "completed" => Some(ShellJobEnd::Exited(code.or(Some(0)))),
            "failed" => {
                Some(code.map_or(ShellJobEnd::Failed, |code| ShellJobEnd::Exited(Some(code))))
            }
            "killed" | "stopped" | "cancelled" => Some(ShellJobEnd::Killed),
            _ => None,
        }
    }
}

/// The exit code in a summary such as `... failed with exit code 3`.
fn exit_code_in(summary: &str) -> Option<i32> {
    const MARKER: &str = "exit code ";
    let from = summary.rfind(MARKER)? + MARKER.len();
    let digits: String = summary[from..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '-')
        .collect();
    digits.parse().ok()
}

/// How one started tool call projects.
#[derive(Clone, Debug)]
enum OpenTool {
    /// A shell command: terminal activity and a job on the terminal board.
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
    /// Shell job changes not yet sent, in order.
    shell_rows: Vec<ShellJobRow>,
    /// Background task id to the `Bash` call that started it.
    shell_tasks: HashMap<String, String>,
    /// Follows background jobs' output files, when the turn runs one.
    tail: Option<ShellTail>,
    /// Follower commands held until the rows queued before them are sent:
    /// the follower runs on its own task, so a settle handed over early
    /// could reach the terminal board ahead of the job's `Backgrounded`.
    tail_commands: Vec<TailCommand>,
}

/// One command for the output follower, in the order it was decided.
#[derive(Debug)]
enum TailCommand {
    Follow {
        run_id: RunId,
        job_id: String,
        path: String,
    },
    Settle {
        run_id: RunId,
        job_id: String,
        end: ShellJobEnd,
    },
}

impl ClaudeToolTracker {
    /// Follows background jobs' output files through `tail`.
    pub(crate) fn attach_shell_tail(&mut self, tail: ShellTail) {
        self.tail = Some(tail);
    }

    /// Releases the turn's output follower, if one is attached.
    pub(crate) fn take_shell_tail(&mut self) -> Option<ShellTail> {
        self.release_tail_commands();
        self.tail.take()
    }

    /// Hands the held follower commands over. Call once the shell rows
    /// taken before them are sent.
    pub(crate) fn release_tail_commands(&mut self) {
        let Some(tail) = &self.tail else {
            self.tail_commands.clear();
            return;
        };
        for command in self.tail_commands.drain(..) {
            match command {
                TailCommand::Follow {
                    run_id,
                    job_id,
                    path,
                } => tail.follow(&run_id, &job_id, &path),
                TailCommand::Settle {
                    run_id,
                    job_id,
                    end,
                } => tail.settle(&run_id, &job_id, end),
            }
        }
    }

    /// Drains the shell job changes not yet sent, in order.
    pub(crate) fn take_shell_rows(&mut self) -> Vec<ShellJobRow> {
        std::mem::take(&mut self.shell_rows)
    }

    /// Applies one background task report that belongs to a shell job.
    /// Returns `false` when the report is not about a shell job.
    pub(crate) fn shell_task(&mut self, run_id: &RunId, report: &ClaudeTaskReport) -> bool {
        let shell = report.task_type.as_deref() == Some("local_bash")
            || self.shell_tasks.contains_key(&report.task_id);
        if !shell {
            return false;
        }
        let Some(job_id) = self
            .shell_tasks
            .get(&report.task_id)
            .cloned()
            .or_else(|| report.tool_use_id.clone())
        else {
            return true;
        };
        if !self.shell_tasks.contains_key(&report.task_id) {
            self.shell_tasks
                .insert(report.task_id.clone(), job_id.clone());
            self.shell_rows.push(ShellJobRow {
                run_id: run_id.clone(),
                job_id: job_id.clone(),
                change: ShellJobChange::Backgrounded,
            });
        }
        if let Some(path) = report.output_file.as_deref() {
            self.follow(run_id, &job_id, path);
        }
        if let Some(end) = report.end() {
            self.settle(run_id, &job_id, end);
        }
        true
    }

    fn follow(&mut self, run_id: &RunId, job_id: &str, path: &str) {
        if self.tail.is_some() {
            self.tail_commands.push(TailCommand::Follow {
                run_id: run_id.clone(),
                job_id: job_id.to_owned(),
                path: path.to_owned(),
            });
        }
    }

    /// Settles a job after the rest of its output, which the follower reads
    /// first when it follows the job.
    fn settle(&mut self, run_id: &RunId, job_id: &str, end: ShellJobEnd) {
        match &self.tail {
            Some(_) => self.tail_commands.push(TailCommand::Settle {
                run_id: run_id.clone(),
                job_id: job_id.to_owned(),
                end,
            }),
            None => self.shell_rows.push(ShellJobRow {
                run_id: run_id.clone(),
                job_id: job_id.to_owned(),
                change: ShellJobChange::Settled(end),
            }),
        }
    }

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
        if matches!(open, OpenTool::Command) {
            self.shell_rows.push(ShellJobRow {
                run_id: run_id.clone(),
                job_id: tool.id.clone(),
                change: ShellJobChange::Opened {
                    command: string_field(&tool.input, "command").map(str::to_owned),
                    background: tool.input.get("run_in_background").and_then(Value::as_bool)
                        == Some(true),
                },
            });
        }
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
            OpenTool::Command => {
                self.shell_result(run_id, result);
                terminal_row(
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
                )
            }
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

impl ClaudeToolTracker {
    /// A `Bash` result: a foreground command stopped, or a background job
    /// started whose output file the follower now reads.
    fn shell_result(&mut self, run_id: &RunId, result: &ClaudeToolResult) {
        let job_id = &result.tool_use_id;
        match result.background_task_id.as_deref() {
            Some(task_id) if !result.is_error => {
                if self
                    .shell_tasks
                    .insert(task_id.to_owned(), job_id.clone())
                    .is_none()
                {
                    self.shell_rows.push(ShellJobRow {
                        run_id: run_id.clone(),
                        job_id: job_id.clone(),
                        change: ShellJobChange::Backgrounded,
                    });
                }
                if let Some(path) = background_output_path(&result.output, task_id) {
                    self.follow(run_id, job_id, &path);
                }
            }
            _ => self.shell_rows.push(ShellJobRow {
                run_id: run_id.clone(),
                job_id: job_id.clone(),
                change: ShellJobChange::Settled(if result.is_error {
                    ShellJobEnd::Failed
                } else {
                    ShellJobEnd::Exited(None)
                }),
            }),
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

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "/tmp/claude-1001/-tmp-bt-probe/ff3b/tasks/b40iqexn3.output";

    fn run() -> RunId {
        RunId::parse("run-1".to_owned()).expect("run id")
    }

    fn changes(tracker: &mut ClaudeToolTracker) -> Vec<(String, ShellJobChange)> {
        tracker
            .take_shell_rows()
            .into_iter()
            .map(|row| (row.job_id, row.change))
            .collect()
    }

    fn bash(background: bool) -> ClaudeToolUse {
        ClaudeToolUse {
            id: "toolu_1".to_owned(),
            name: "Bash".to_owned(),
            input: serde_json::json!({
                "command": "npm run dev",
                "run_in_background": background,
            }),
        }
    }

    /// The `user` frame the pinned Claude Code sends for a background
    /// `Bash` call, as recorded.
    fn background_result() -> Vec<ClaudeToolResult> {
        let content = serde_json::json!([{
            "tool_use_id": "toolu_1",
            "type": "tool_result",
            "content": format!(
                "Command running in background with ID: b40iqexn3. Output is being written to: \
                 {PATH}. You will be notified when it completes. To check interim output, use \
                 Read on that file path."
            ),
            "is_error": false,
        }]);
        let structured = serde_json::json!({
            "stdout": "", "stderr": "", "interrupted": false,
            "backgroundTaskId": "b40iqexn3",
        });
        tool_results(content.as_array().expect("array"), Some(&structured))
    }

    #[test]
    fn reads_the_background_job_and_its_output_file() {
        let results = background_result();
        assert_eq!(results[0].background_task_id.as_deref(), Some("b40iqexn3"));
        assert_eq!(
            background_output_path(&results[0].output, "b40iqexn3").as_deref(),
            Some(PATH)
        );
    }

    #[test]
    fn a_background_bash_call_is_a_job_that_outlives_its_result() {
        let mut tracker = ClaudeToolTracker::default();
        let _ = tracker.started(&run(), 1, &bash(true));
        let _ = tracker.finished(&run(), 2, &background_result()[0]);
        let report = ClaudeTaskReport {
            task_id: "b40iqexn3".to_owned(),
            tool_use_id: Some("toolu_1".to_owned()),
            task_type: None,
            status: Some("failed".to_owned()),
            summary: Some(
                "Background command \"Run tick loop\" failed with exit code 3".to_owned(),
            ),
            output_file: None,
        };
        assert!(tracker.shell_task(&run(), &report));
        assert_eq!(
            changes(&mut tracker),
            vec![
                (
                    "toolu_1".to_owned(),
                    ShellJobChange::Opened {
                        command: Some("npm run dev".to_owned()),
                        background: true,
                    }
                ),
                ("toolu_1".to_owned(), ShellJobChange::Backgrounded),
                (
                    "toolu_1".to_owned(),
                    ShellJobChange::Settled(ShellJobEnd::Exited(Some(3)))
                ),
            ]
        );
    }

    #[test]
    fn a_foreground_command_settles_with_its_result() {
        let mut tracker = ClaudeToolTracker::default();
        let _ = tracker.started(&run(), 1, &bash(false));
        let result = ClaudeToolResult {
            tool_use_id: "toolu_1".to_owned(),
            is_error: true,
            output: "Exit code 1".to_owned(),
            background_task_id: None,
        };
        let _ = tracker.finished(&run(), 2, &result);
        assert_eq!(
            changes(&mut tracker)[1],
            (
                "toolu_1".to_owned(),
                ShellJobChange::Settled(ShellJobEnd::Failed)
            )
        );
    }

    #[test]
    fn a_subagent_task_is_not_a_shell_job() {
        let mut tracker = ClaudeToolTracker::default();
        let report = ClaudeTaskReport {
            task_id: "agent-1".to_owned(),
            tool_use_id: Some("toolu_9".to_owned()),
            task_type: Some("local_agent".to_owned()),
            status: None,
            summary: None,
            output_file: None,
        };
        assert!(!tracker.shell_task(&run(), &report));
        assert!(changes(&mut tracker).is_empty());
    }

    #[test]
    fn a_read_of_a_picture_settles_its_step_and_shows_no_image() {
        let mut tracker = ClaudeToolTracker::default();
        let tool = ClaudeToolUse {
            id: "toolu_2".to_owned(),
            name: "Read".to_owned(),
            input: serde_json::json!({ "file_path": "/work/diagram.png" }),
        };
        let _ = tracker.started(&run(), 1, &tool);
        let content = [serde_json::json!({
            "type": "tool_result",
            "tool_use_id": "toolu_2",
            "content": [
                { "type": "text", "text": "a picture" },
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "aGk=" } },
            ],
        })];
        let results = tool_results(&content, None);
        assert_eq!(results[0].output, "a picture");
        let rows = tracker.finished(&run(), 2, &results[0]);
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0], Observation::Tool(_)));
    }
}
