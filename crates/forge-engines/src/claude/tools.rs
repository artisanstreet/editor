//! What Claude's tool inputs mean as items.

use super::decoder::str_at;
use artisan_api::{
    AgentAction, FileChange, FileOp, MessageDirection, PlanStep, Question, QuestionOption,
    SearchMode, StepStatus, ToolKind,
};
use serde_json::Value;

#[expect(clippy::too_many_lines, reason = "one arm per Claude tool")]
#[expect(
    clippy::expect_used,
    reason = "validate_input has checked every field read here"
)]
pub fn tool_kind(name: &str, input: &Value) -> Result<ToolKind, String> {
    validate_input(name, input)?;
    let text = |field: &str| {
        str_at(input, field)
            .expect("validated native tool field")
            .to_owned()
    };
    let optional = |field: &str| str_at(input, field).map(str::to_owned);
    let edit = |path: String, op: FileOp, diff: Option<String>| ToolKind::Edit {
        changes: (vec![FileChange {
            path,
            op,
            diff: diff.map(Into::into),
        }])
        .into(),
    };
    Ok(match name {
        "Bash" | "PowerShell" => ToolKind::Shell {
            command: (text("command")).into(),
            cwd: None,
            background: input["run_in_background"].as_bool() == Some(true),
            watch: false,
        },
        "Read" => {
            let start = input["offset"].as_u64().unwrap_or(1);
            ToolKind::Read {
                path: text("file_path"),
                range: input["limit"]
                    .as_u64()
                    .filter(|limit| *limit > 0)
                    .map(|limit| (start, start.saturating_add(limit - 1))),
            }
        }
        "Edit" => edit(
            text("file_path"),
            FileOp::Modify,
            Some(diff(&text("old_string"), &text("new_string"))),
        ),
        "MultiEdit" => {
            let edits = input["edits"].as_array().into_iter().flatten();
            let diff = edits
                .map(|edit| {
                    diff(
                        str_at(edit, "old_string").expect("validated old_string"),
                        str_at(edit, "new_string").expect("validated new_string"),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            edit(text("file_path"), FileOp::Modify, Some(diff))
        }
        "Write" => edit(
            text("file_path"),
            FileOp::Add,
            Some(diff("", &text("content"))),
        ),
        "NotebookEdit" => edit(text("notebook_path"), FileOp::Modify, None),
        "Grep" => ToolKind::Search {
            pattern: text("pattern"),
            path: optional("path"),
            mode: SearchMode::Grep,
        },
        "Glob" => ToolKind::Search {
            pattern: text("pattern"),
            path: optional("path"),
            mode: SearchMode::Glob,
        },
        "WebFetch" => ToolKind::WebFetch { url: text("url") },
        "WebSearch" => ToolKind::WebSearch {
            query: text("query"),
        },
        // Forge resolves `to` to the Agent item it names.
        "SendMessage" if optional("to").is_some_and(|to| !to.trim().is_empty()) => {
            ToolKind::AgentMessage {
                recipient: text("to"),
                target: None,
                summary: optional("summary").filter(|summary| !summary.trim().is_empty()),
                text: (text("message")).into(),
                direction: MessageDirection::Sent,
            }
        }
        "SubagentHandback" => ToolKind::AgentMessage {
            recipient: "main".to_owned(),
            target: None,
            summary: None,
            text: (text("message")).into(),
            direction: MessageDirection::Report,
        },
        "ListAgents" => ToolKind::Agents {
            action: AgentAction::List,
            addresses: Vec::new(),
            targets: Vec::new(),
        },
        // An agent's id or a background shell's; Forge finds the agent.
        "TaskStop" | "KillShell" => ToolKind::Agents {
            action: AgentAction::Stop,
            addresses: optional("task_id")
                .or_else(|| optional("shell_id"))
                .into_iter()
                .collect(),
            targets: Vec::new(),
        },
        "Monitor" => ToolKind::Shell {
            command: (text("command")).into(),
            cwd: None,
            background: true,
            watch: true,
        },
        _ => {
            let input = (!input.is_null()).then(|| input.to_string());
            match name
                .strip_prefix("mcp__")
                .and_then(|rest| rest.split_once("__"))
            {
                Some((server, tool)) => ToolKind::Mcp {
                    server: server.to_owned(),
                    tool: tool.to_owned(),
                    input: input.map(Into::into),
                },
                None => ToolKind::Other {
                    name: name.to_owned(),
                    input: input.map(Into::into),
                },
            }
        }
    })
}

pub fn validate_input(name: &str, input: &Value) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Claude tool omitted its name".into());
    }
    let fields: &[(&str, bool)] = match name {
        "Bash" | "PowerShell" | "Monitor" => &[("command", false)],
        "Read" | "MultiEdit" => &[("file_path", false)],
        "Edit" => &[
            ("file_path", false),
            ("old_string", true),
            ("new_string", true),
        ],
        "Write" => &[("file_path", false), ("content", true)],
        "NotebookEdit" => &[("notebook_path", false)],
        "Grep" | "Glob" => &[("pattern", false)],
        "WebFetch" => &[("url", false)],
        "WebSearch" => &[("query", false)],
        "SendMessage" => &[("to", false), ("message", false)],
        "SubagentHandback" => &[("message", false)],
        "Task" | "Agent" => &[("prompt", false), ("description", false)],
        _ => &[],
    };
    for &(field, allow_empty) in fields {
        if str_at(input, field).is_none_or(|value| !allow_empty && value.trim().is_empty()) {
            return Err(format!("Claude {name} omitted a valid {field}"));
        }
    }
    if name == "MultiEdit" {
        let Some(edits) = input["edits"].as_array() else {
            return Err("Claude MultiEdit omitted edits".into());
        };
        if edits.iter().any(|edit| {
            str_at(edit, "old_string").is_none() || str_at(edit, "new_string").is_none()
        }) {
            return Err("Claude MultiEdit omitted a replacement string".into());
        }
    }
    if name == "TodoWrite" {
        let Some(todos) = input["todos"].as_array() else {
            return Err("Claude TodoWrite omitted todos".into());
        };
        if todos.iter().any(|todo| {
            str_at(todo, "content").is_none_or(|value| value.trim().is_empty())
                || !matches!(
                    str_at(todo, "status"),
                    Some("pending" | "in_progress" | "completed" | "cancelled")
                )
        }) {
            return Err("Claude TodoWrite supplied a malformed todo".into());
        }
    }
    if name == "AskUserQuestion" {
        let Some(questions) = input["questions"]
            .as_array()
            .filter(|questions| !questions.is_empty())
        else {
            return Err("Claude AskUserQuestion omitted questions".into());
        };
        if questions.iter().any(|question| {
            str_at(question, "question").is_none_or(|value| value.trim().is_empty())
                || question["options"].as_array().is_none_or(|options| {
                    options.iter().any(|option| {
                        str_at(option, "label").is_none_or(|value| value.trim().is_empty())
                    })
                })
        }) {
            return Err("Claude AskUserQuestion supplied a malformed prompt or option".into());
        }
    }
    Ok(())
}

/// The same unified diff representation used by the other adapters.
fn diff(old: &str, new: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .to_string()
}

#[expect(
    clippy::expect_used,
    reason = "callers run validate_input first, which checks these fields"
)]
pub fn todo_steps(input: &Value) -> Vec<PlanStep> {
    input["todos"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|todo| PlanStep {
            text: (str_at(todo, "content")
                .expect("validated todo content")
                .to_owned())
            .into(),
            status: match str_at(todo, "status") {
                Some("in_progress") => StepStatus::InProgress,
                Some("completed") => StepStatus::Done,
                Some("cancelled") => StepStatus::Cancelled,
                _ => StepStatus::Pending,
            },
        })
        .collect()
}

#[expect(
    clippy::expect_used,
    reason = "callers run validate_input first, which checks these fields"
)]
pub(super) fn questions(input: &Value) -> Vec<Question> {
    input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, question)| Question {
            id: index.to_string(),
            header: str_at(question, "header").map(str::to_owned),
            text: (str_at(question, "question")
                .expect("validated question prompt")
                .to_owned())
            .into(),
            multi_select: question["multiSelect"].as_bool() == Some(true),
            options: question["options"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, option)| QuestionOption {
                    id: format!("option-{index}"),
                    label: str_at(option, "label")
                        .expect("validated option label")
                        .to_owned(),
                    description: (str_at(option, "description").map(str::to_owned)).map(Into::into),
                })
                .collect::<Vec<_>>()
                .into(),
        })
        .collect()
}

#[cfg(test)]
mod required_input_tests {
    use super::*;
    #[test]
    fn known_native_tools_reject_missing_fields_but_allow_empty_replacement_text() {
        for name in [
            "Bash",
            "Read",
            "Edit",
            "Write",
            "Grep",
            "Glob",
            "WebFetch",
            "WebSearch",
            "SendMessage",
        ] {
            assert!(tool_kind(name, &serde_json::json!({})).is_err(), "{name}");
        }
        assert!(
            tool_kind(
                "Edit",
                &serde_json::json!({"file_path":"file","old_string":"","new_string":""})
            )
            .is_ok()
        );
    }
}
