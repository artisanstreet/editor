//! Tool results as short markdown, which the calling model reads and the
//! user sees when they open the call: titles and words first, ids in code
//! spans where a follow-up call needs them.

use crate::transcript::time;
use artisan_api::{Thread, ThreadId};
use forge_runs::{AgentInfo, AgentState, Hub};
use forge_store::{GrepPage, SearchPage, Touch};
use std::fmt::Write as _;

/// Who an indexed item's text is from.
fn who(kind: &str) -> &'static str {
    match kind {
        "user" => "You",
        "assistant" => "Reply",
        "tool" => "Tool",
        "agent" => "Agent",
        "plan" => "Plan",
        "question" => "Question",
        "notice" => "Note",
        "error" => "Error",
        "image" => "Image",
        _ => "Item",
    }
}

/// `text` on one line, whitespace collapsed.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn title(title: Option<&str>) -> &str {
    title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("Untitled thread")
}

/// Starts a group under its thread's title when the thread changes.
fn group(out: &mut String, last: &mut Option<ThreadId>, thread: &ThreadId, name: Option<&str>) {
    if last.as_ref() != Some(thread) {
        writeln!(out, "\n### {}\n", title(name)).ok();
        *last = Some(thread.clone());
    }
}

pub fn search(page: &SearchPage, offset: u32) -> String {
    let mut out = String::new();
    let mut last = None;
    for hit in &page.hits {
        group(
            &mut out,
            &mut last,
            &hit.thread_id,
            hit.thread_title.as_deref(),
        );
        writeln!(
            out,
            "- **{}**, {}: {} `{}`",
            who(&hit.kind),
            time(hit.created_at),
            one_line(&hit.snippet),
            hit.item_id,
        ).ok();
    }
    if let Some(next) = page.next {
        writeln!(out, "\nMore hits: call again with offset {next}.").ok();
    } else if offset > 0 {
        out.push_str("\nNo more hits.\n");
    }
    out.push_str("\nRead around a hit with thread_read and its item id.\n");
    out.trim_start().to_owned()
}

pub fn grep(page: &GrepPage) -> String {
    let mut out = String::new();
    let mut last = None;
    for found in &page.matches {
        group(
            &mut out,
            &mut last,
            &found.thread_id,
            found.thread_title.as_deref(),
        );
        writeln!(
            out,
            "**{}**, {}, line {} `{}`\n",
            who(&found.kind),
            time(found.created_at),
            found.line,
            found.item_id,
        ).ok();
        for line in found.before.iter().chain([&found.text]).chain(&found.after) {
            writeln!(out, "> {line}").ok();
        }
        out.push('\n');
    }
    if let Some(next) = page.next {
        writeln!(
            out,
            "Older items not searched yet: call again with cursor {next}."
        ).ok();
    }
    out.trim().to_owned()
}

pub fn touches(touches: &[Touch]) -> String {
    let mut out = String::new();
    let mut last = None;
    for touch in touches {
        group(
            &mut out,
            &mut last,
            &touch.thread_id,
            touch.thread_title.as_deref(),
        );
        let verb = match touch.op.as_str() {
            "read" => "Read",
            "create" => "Created",
            "delete" => "Deleted",
            _ => "Edited",
        };
        writeln!(
            out,
            "- {verb} `{}`, {} `{}`",
            touch.path,
            time(touch.created_at),
            touch.item_id
        ).ok();
    }
    out.trim_start().to_owned()
}

/// One thread as a list line: title, harness and model, state, id.
pub fn thread(hub: &Hub, thread: &Thread) -> String {
    let mut states = Vec::new();
    if hub.is_live(&thread.id) {
        states.push("working");
    }
    if thread.is_unread() {
        states.push("unread");
    }
    if thread.archived {
        states.push("archived");
    }
    if thread.spawned_by.is_some() {
        states.push("agent");
    }
    format!(
        "**{}** · {}{}{} `{}`",
        title(thread.title.as_deref()),
        thread.config.engine.display_name(),
        thread
            .config
            .model
            .as_deref()
            .map(|model| format!(" ({model})"))
            .unwrap_or_default(),
        if states.is_empty() {
            String::new()
        } else {
            format!(" · {}", states.join(", "))
        },
        thread.id,
    )
}

pub fn threads<'a>(hub: &Hub, threads: impl IntoIterator<Item = &'a Thread>) -> String {
    let lines: Vec<String> = threads
        .into_iter()
        .map(|found| format!("- {}", thread(hub, found)))
        .collect();
    if lines.is_empty() {
        "No threads.".to_owned()
    } else {
        lines.join("\n")
    }
}

pub fn warnings(out: &mut String, warnings: &[String]) {
    for warning in warnings {
        writeln!(out, "\nNote: {warning}").ok();
    }
}

/// One agent: what it works on, where, how it stands, and its reply.
pub fn agent(info: &AgentInfo) -> String {
    let state = match info.state {
        AgentState::Working => "working",
        AgentState::WaitingForUser => "waiting for the user",
        AgentState::Done => "done",
        AgentState::Failed => "failed",
        AgentState::Cancelled => "stopped",
    };
    let setup: Vec<&str> = [info.model.as_deref(), info.effort.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let mut out = format!(
        "**{}** · {}{} · {state} `{}`\n",
        info.task,
        info.harness.display_name(),
        if setup.is_empty() {
            String::new()
        } else {
            format!(" ({})", setup.join(", "))
        },
        info.agent,
    );
    if let Some(error) = &info.error {
        writeln!(out, "\nError: {error}").ok();
    }
    if let Some(result) = &info.result {
        writeln!(out, "\n{}", result.trim()).ok();
    }
    warnings(&mut out, &info.warnings);
    out
}

pub fn agents(infos: &[AgentInfo]) -> String {
    infos
        .iter()
        .map(agent)
        .collect::<Vec<_>>()
        .join("\n---\n\n")
}

/// A markdown table cell: one line, pipes escaped.
pub fn cell(text: &str) -> String {
    one_line(text).replace('|', "\\|")
}
