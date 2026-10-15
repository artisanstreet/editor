//! What activity rows say and what they open on: plain words for every
//! kind of call, and previews the markdown renderer highlights.

use super::*;
use artisan_api::{
    AgentAction, FileChange, FileOp, MessageDirection, Output, SearchMode, ToolKind,
};
use std::collections::HashMap;

fn tool(tool: ToolKind, status: Status, output: Option<&str>) -> Item {
    Item {
        id: ItemId::new(),
        thread_id: ThreadId::new(),
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::Tool {
            tool,
            title: None,
            output: output.map(Output::from_text),
            exit_code: None,
            approval: None,
        },
    }
}

fn row(item: &Item, agents: &HashMap<ItemId, String>) -> String {
    let line = crate::activity::line(item, agents);
    let tail = match line.tail {
        Some(crate::activity::Tail::Changed { added, removed }) => format!(" +{added} -{removed}"),
        Some(crate::activity::Tail::Failed(why)) => format!(" ({why})"),
        None => String::new(),
    };
    format!("{} {}{tail}", line.verb, line.object)
        .trim()
        .to_owned()
}

fn change(path: &str, op: FileOp, diff: &str) -> ToolKind {
    ToolKind::Edit {
        changes: vec![FileChange {
            path: path.into(),
            op,
            diff: Some(diff.into()),
        }]
        .into(),
    }
}

fn message(recipient: &str, target: Option<ItemId>, direction: MessageDirection) -> ToolKind {
    ToolKind::AgentMessage {
        recipient: recipient.into(),
        target,
        summary: None,
        text: "## Summary\n- The build is green.".into(),
        direction,
    }
}

/// Every row reads as a sentence: what was done, then to what. Agents go by
/// their task, sessions by their name; nothing shows an engine's tool name.
#[test]
#[expect(clippy::too_many_lines, reason = "one row per kind of call")]
fn rows_say_what_was_done_in_plain_words() {
    let agent = ItemId::new();
    let agents = HashMap::from([(agent.clone(), "Finish conversation rework".to_owned())]);
    let done = Status::Done;
    let cases = [
        (
            tool(
                ToolKind::Shell {
                    command: "cargo test\n--lib".into(),
                    cwd: None,
                    background: false,
                },
                done,
                None,
            ),
            "Ran cargo test",
        ),
        (
            tool(
                ToolKind::Shell {
                    command: "cargo test".into(),
                    cwd: None,
                    background: false,
                },
                Status::Running,
                None,
            ),
            "Running cargo test",
        ),
        (
            tool(
                ToolKind::Shell {
                    command: "tail -f log".into(),
                    cwd: None,
                    background: true,
                },
                done,
                None,
            ),
            "Started tail -f log",
        ),
        (
            tool(
                ToolKind::Read {
                    path: "src/main.rs".into(),
                    range: Some((40, 140)),
                },
                done,
                None,
            ),
            "Read src/main.rs:40-140",
        ),
        (
            tool(
                change("src/new.rs", FileOp::Add, "@@ -0,0 +1,2 @@\n+a\n+b"),
                done,
                None,
            ),
            "Created src/new.rs +2 -0",
        ),
        (
            tool(
                change(
                    "src/old.rs",
                    FileOp::Modify,
                    "--- a\n+++ b\n@@ -1 +1 @@\n-a\n+b",
                ),
                done,
                None,
            ),
            "Edited src/old.rs +1 -1",
        ),
        (
            tool(
                ToolKind::Edit {
                    changes: vec![FileChange {
                        path: "gone.rs".into(),
                        op: FileOp::Delete,
                        diff: None,
                    }]
                    .into(),
                },
                done,
                None,
            ),
            "Deleted gone.rs",
        ),
        (
            tool(
                ToolKind::Search {
                    pattern: "label()".into(),
                    path: Some("crates".into()),
                    mode: SearchMode::Grep,
                },
                done,
                None,
            ),
            "Searched for label() in crates",
        ),
        (
            tool(
                ToolKind::Search {
                    pattern: "**/*.rs".into(),
                    path: None,
                    mode: SearchMode::Glob,
                },
                done,
                None,
            ),
            "Found files **/*.rs",
        ),
        (
            tool(
                ToolKind::WebSearch {
                    query: "gpui text".into(),
                },
                done,
                None,
            ),
            "Searched the web gpui text",
        ),
        (
            tool(
                ToolKind::WebFetch {
                    url: "https://docs.rs/syntect".into(),
                },
                done,
                None,
            ),
            "Read a page docs.rs/syntect",
        ),
        (
            tool(
                ToolKind::Mcp {
                    server: "claude_ai_Linear".into(),
                    tool: "create_issue".into(),
                    input: None,
                },
                done,
                None,
            ),
            "Used Linear create issue",
        ),
        (
            tool(
                message("a15c", Some(agent.clone()), MessageDirection::Sent),
                done,
                None,
            ),
            "Messaged Finish conversation rework",
        ),
        (
            tool(
                message("a15c", Some(agent.clone()), MessageDirection::Received),
                done,
                None,
            ),
            "Reply from Finish conversation rework",
        ),
        (
            tool(
                message("shooter-e7", None, MessageDirection::Sent),
                done,
                None,
            ),
            "Messaged session shooter-e7",
        ),
        (
            tool(
                message(
                    "uds:/run/user/1001/cc-socks/1.sock",
                    None,
                    MessageDirection::Sent,
                ),
                done,
                None,
            ),
            "Messaged session another session",
        ),
        (
            tool(
                message("shooter-e7", None, MessageDirection::Received),
                done,
                None,
            ),
            "Message from session shooter-e7",
        ),
        (
            tool(message("main", None, MessageDirection::Sent), done, None),
            "Messaged the main agent",
        ),
        (
            tool(message("main", None, MessageDirection::Report), done, None),
            "Reported back Summary",
        ),
        (
            tool(
                ToolKind::Agents {
                    action: AgentAction::List,
                    addresses: vec![],
                    targets: vec![],
                },
                done,
                None,
            ),
            "Listed agents and sessions",
        ),
        (
            tool(
                ToolKind::Agents {
                    action: AgentAction::Wait,
                    addresses: vec!["t".into()],
                    targets: vec![agent.clone()],
                },
                Status::Running,
                None,
            ),
            "Waiting for Finish conversation rework",
        ),
        (
            tool(
                ToolKind::Agents {
                    action: AgentAction::Stop,
                    addresses: vec!["bnuubwmtm".into()],
                    targets: vec![],
                },
                done,
                None,
            ),
            "Stopped a background job",
        ),
        (
            tool(
                ToolKind::Other {
                    name: "ToolSearch".into(),
                    input: Some(r#"{"query":"select:Read","max_results":5}"#.into()),
                },
                done,
                None,
            ),
            "Tool search select:Read",
        ),
        (
            tool(
                ToolKind::Shell {
                    command: "false".into(),
                    cwd: None,
                    background: false,
                },
                Status::Failed,
                None,
            ),
            "Ran false (failed)",
        ),
    ];
    for (item, expected) in cases {
        assert_eq!(row(&item, &agents), expected);
    }
}

/// A shell that exits non-zero says its exit code where a failure would.
#[test]
fn a_failed_command_shows_its_exit_code() {
    let mut item = tool(
        ToolKind::Shell {
            command: "cargo test".into(),
            cwd: None,
            background: false,
        },
        Status::Done,
        None,
    );
    if let ItemKind::Tool { exit_code, .. } = &mut item.kind {
        *exit_code = Some(101);
    }
    assert_eq!(row(&item, &HashMap::new()), "Ran cargo test (exit 101)");
}

/// A read opens on the file, highlighted by its extension and without the
/// line numbers the engine put in front of each line.
#[test]
fn a_read_opens_on_the_highlighted_file() {
    let item = tool(
        ToolKind::Read {
            path: "crates/app/src/main.rs".into(),
            range: None,
        },
        Status::Done,
        Some("     1→fn main() {\n     2→    println!(\"hi\");\n     3→}\n"),
    );
    assert_eq!(
        crate::preview::markdown(&item).unwrap(),
        "```rs\nfn main() {\n    println!(\"hi\");\n}\n```"
    );
}

/// A new file opens as the file; an edit as its diff, without file headers.
#[test]
fn edits_open_on_the_file_or_the_diff() {
    let created = tool(
        change(
            "web/app.py",
            FileOp::Add,
            "--- /dev/null\n+++ b\n@@ -0,0 +1,2 @@\n+import os\n+print(os.name)\n",
        ),
        Status::Done,
        None,
    );
    assert_eq!(
        crate::preview::markdown(&created).unwrap(),
        "```py\nimport os\nprint(os.name)\n```"
    );
    let edited = tool(
        change(
            "web/app.py",
            FileOp::Modify,
            "--- a\n+++ b\n@@ -1 +1 @@\n-import os\n+import sys\n",
        ),
        Status::Done,
        None,
    );
    assert_eq!(
        crate::preview::markdown(&edited).unwrap(),
        "```diff\n@@ -1 +1 @@\n-import os\n+import sys\n```"
    );
}

/// Grep results group by file, highlighted, even when paths hold dashes.
#[test]
fn grep_results_group_by_file() {
    let item = tool(
        ToolKind::Search {
            pattern: "label".into(),
            path: None,
            mode: SearchMode::Grep,
        },
        Status::Done,
        Some(
            "crates/editor-conversation/src/a.rs:12:let label = 1;\ncrates/editor-conversation/src/a.rs:14:label();\ncrates/b.rs:3:label",
        ),
    );
    assert_eq!(
        crate::preview::markdown(&item).unwrap(),
        "`crates/editor-conversation/src/a.rs`\n\n```rs\n   12  let label = 1;\n   14  label();\n```\n\n`crates/b.rs`\n\n```rs\n    3  label\n```"
    );
}

/// A command opens on itself as highlighted shell, then its output without
/// terminal escapes.
#[test]
fn a_command_opens_on_itself_and_its_output() {
    let item = tool(
        ToolKind::Shell {
            command: "ls".into(),
            cwd: None,
            background: false,
        },
        Status::Done,
        Some("\u{1b}[34msrc\u{1b}[0m\nCargo.toml"),
    );
    assert_eq!(
        crate::preview::markdown(&item).unwrap(),
        "```bash\nls\n```\n\n```\nsrc\nCargo.toml\n```"
    );
}

/// An integration's input reads as fields and its JSON result pretty-printed.
#[test]
fn an_integration_opens_on_its_fields_and_result() {
    let item = tool(
        ToolKind::Mcp {
            server: "linear".into(),
            tool: "create_issue".into(),
            input: Some(r#"{"title":"Fix rows","priority":2}"#.into()),
        },
        Status::Done,
        Some(r#"{"id":"LIN-1"}"#),
    );
    assert_eq!(
        crate::preview::markdown(&item).unwrap(),
        "**title** Fix rows  \n**priority** 2  \n\n```json\n{\n  \"id\": \"LIN-1\"\n}\n```"
    );
}
