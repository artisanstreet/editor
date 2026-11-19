use crate::transcript::time;
use artisan_api::{Thread, ThreadId};
use forge_runs::{AgentInfo, AgentState, Hub};
use forge_store::{GrepPage, RecallKind, SearchPage, Touch};
use std::fmt::Write as _;

pub(crate) fn models(models: &[artisan_api::Model]) -> Result<String, serde_json::Error> {
    Ok(format!(
        "Use effort, speed and variant ids exactly as listed. OpenCode reasoning choices use variant. \
         Defaults are default_effort, default_variant and speeds[].default. \
         context_window is the standard token count; context_windows[].override_tokens is the \
         value to request an alternate window. Omitted optional fields mean none are advertised.\n\n```json\n{}\n```\n\n",
        serde_json::to_string_pretty(models)?
    ))
}

const fn who(kind: RecallKind) -> &'static str {
    match kind {
        RecallKind::User => "You",
        RecallKind::Assistant => "Reply",
        RecallKind::Tool => "Tool",
        RecallKind::Agent => "Agent",
        RecallKind::Plan => "Plan",
        RecallKind::Question => "Question",
        RecallKind::Notice => "Note",
        RecallKind::Error => "Error",
        RecallKind::Image => "Image",
        RecallKind::Compaction => "Checkpoint",
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn title(title: Option<&str>) -> &str {
    title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("Untitled thread")
}

fn group(out: &mut String, last: &mut Option<ThreadId>, thread: &ThreadId, name: Option<&str>) {
    if last.as_ref() != Some(thread) {
        writeln!(out, "\n### {}\n", title(name)).expect("invariant: writing to a String cannot fail");
        *last = Some(thread.clone());
    }
}

pub(crate) fn search(page: &SearchPage, continued: bool) -> String {
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
            who(hit.kind),
            time(hit.created_at),
            one_line(&hit.snippet),
            hit.item_id,
        )
        .expect("invariant: writing to a String cannot fail");
    }
    if let Some(next) = page.next {
        writeln!(out, "\nMore hits: call again with cursor {next}.").expect("invariant: writing to a String cannot fail");
    } else if continued {
        out.push_str("\nNo more hits.\n");
    }
    out.push_str("\nRead around a hit with thread_read and its item id.\n");
    out.trim_start().to_owned()
}

pub(crate) fn grep(page: &GrepPage) -> String {
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
            who(found.kind),
            time(found.created_at),
            found.line,
            found.item_id,
        )
        .expect("invariant: writing to a String cannot fail");
        for line in found.before.iter().chain([&found.text]).chain(&found.after) {
            writeln!(out, "> {line}").expect("invariant: writing to a String cannot fail");
        }
        out.push('\n');
    }
    if let Some(next) = page.next {
        writeln!(out, "More matches: call again with cursor {next}.").expect("invariant: writing to a String cannot fail");
    }
    out.trim().to_owned()
}

pub(crate) fn touches(touches: &[Touch]) -> String {
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
        )
        .expect("invariant: writing to a String cannot fail");
    }
    out.trim_start().to_owned()
}

pub(crate) fn thread(hub: &Hub, thread: &Thread) -> String {
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

pub(crate) fn warnings(out: &mut String, warnings: &[String]) {
    for warning in warnings {
        writeln!(out, "\nNote: {warning}").expect("invariant: writing to a String cannot fail");
    }
}

pub(crate) fn agent(info: &AgentInfo) -> String {
    let state = match info.state {
        AgentState::Working => "working",
        AgentState::WaitingForUser => "waiting for the user",
        AgentState::Done => "done",
        AgentState::Failed => "failed",
        AgentState::Cancelled => "stopped",
    };
    let reasoning = if info.harness == artisan_api::EngineId::OpenCode {
        info.variant.as_deref()
    } else {
        info.effort.as_deref()
    };
    let setup: Vec<&str> = [info.model.as_deref(), reasoning]
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
        writeln!(out, "\nError: {error}").expect("invariant: writing to a String cannot fail");
    }
    if let Some(result) = &info.result {
        writeln!(out, "\n{}", result.trim()).expect("invariant: writing to a String cannot fail");
    }
    warnings(&mut out, &info.warnings);
    out
}

pub(crate) fn agents(infos: &[AgentInfo]) -> String {
    infos
        .iter()
        .map(agent)
        .collect::<Vec<_>>()
        .join("\n---\n\n")
}

pub(crate) fn cell(text: &str) -> String {
    one_line(text).replace('|', "\\|")
}

#[cfg(test)]
#[path = "../tests/format/unit.rs"]
mod tests;
