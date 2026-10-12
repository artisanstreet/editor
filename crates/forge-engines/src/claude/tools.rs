//! What Claude's tool inputs mean as items.

use super::decoder::str_at;
use artisan_api::{
    FileChange, FileOp, PlanStep, Question, QuestionOption, SearchMode, StepStatus, ToolKind,
};
use serde_json::Value;

pub(super) fn tool_kind(name: &str, input: &Value) -> ToolKind {
    let text = |field: &str| str_at(input, field).unwrap_or_default().to_owned();
    let optional = |field: &str| str_at(input, field).map(str::to_owned);
    let edit = |path: String, op: FileOp, diff: Option<String>| ToolKind::Edit {
        changes: vec![FileChange { path, op, diff }],
    };
    match name {
        "Bash" | "PowerShell" => ToolKind::Shell {
            command: text("command"),
            cwd: None,
            background: input["run_in_background"].as_bool() == Some(true),
        },
        "Read" => {
            let start = input["offset"].as_u64().unwrap_or(1);
            ToolKind::Read {
                path: text("file_path"),
                range: input["limit"]
                    .as_u64()
                    .map(|limit| (start, start + limit.saturating_sub(1))),
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
                        str_at(edit, "old_string").unwrap_or_default(),
                        str_at(edit, "new_string").unwrap_or_default(),
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
        _ => {
            let input = (!input.is_null()).then(|| input.to_string());
            match name
                .strip_prefix("mcp__")
                .and_then(|rest| rest.split_once("__"))
            {
                Some((server, tool)) => ToolKind::Mcp {
                    server: server.to_owned(),
                    tool: tool.to_owned(),
                    input,
                },
                None => ToolKind::Other {
                    name: name.to_owned(),
                    input,
                },
            }
        }
    }
}

/// A plain line diff: what Claude replaced, then what it wrote.
fn diff(old: &str, new: &str) -> String {
    let removed = old.lines().map(|line| format!("-{line}"));
    let added = new.lines().map(|line| format!("+{line}"));
    removed.chain(added).collect::<Vec<_>>().join("\n")
}

pub(super) fn todo_steps(input: &Value) -> Vec<PlanStep> {
    input["todos"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|todo| PlanStep {
            text: str_at(todo, "content").unwrap_or_default().to_owned(),
            status: match str_at(todo, "status") {
                Some("in_progress") => StepStatus::InProgress,
                Some("completed") => StepStatus::Done,
                Some("cancelled") => StepStatus::Cancelled,
                _ => StepStatus::Pending,
            },
        })
        .collect()
}

pub(super) fn questions(input: &Value) -> Vec<Question> {
    input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, question)| Question {
            id: index.to_string(),
            header: str_at(question, "header").map(str::to_owned),
            text: str_at(question, "question").unwrap_or_default().to_owned(),
            multi_select: question["multiSelect"].as_bool() == Some(true),
            options: question["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|option| QuestionOption {
                    label: str_at(option, "label").unwrap_or_default().to_owned(),
                    description: str_at(option, "description").map(str::to_owned),
                })
                .collect(),
        })
        .collect()
}
