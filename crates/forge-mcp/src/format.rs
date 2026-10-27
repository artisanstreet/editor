//! Tool results as short markdown, which the calling model reads and the
//! user sees when they open the call: titles and words first, ids in code
//! spans where a follow-up call needs them.

use crate::transcript::time;
use artisan_api::{Thread, ThreadId};
use forge_runs::{AgentInfo, AgentState, Hub};
use forge_store::{GrepPage, SearchPage, Touch};
use std::fmt::Write as _;

/// Serialize the shared catalog directly so new model settings are exposed
/// without maintaining a second, incomplete representation for MCP.
pub fn models(models: &[artisan_api::Model]) -> anyhow::Result<String> {
    Ok(format!(
        "Use effort, speed and variant ids exactly as listed. OpenCode reasoning choices use variant. \
         Defaults are default_effort, default_variant and speeds[].default. \
         context_window is the standard token count; context_windows[].override_tokens is the \
         value to request an alternate window. Omitted optional fields mean none are advertised.\n\n```json\n{}\n```\n\n",
        serde_json::to_string_pretty(models)?
    ))
}

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
        )
        .ok();
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
        )
        .ok();
        for line in found.before.iter().chain([&found.text]).chain(&found.after) {
            writeln!(out, "> {line}").ok();
        }
        out.push('\n');
    }
    if let Some(next) = page.next {
        writeln!(
            out,
            "Older items not searched yet: call again with cursor {next}."
        )
        .ok();
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
        )
        .ok();
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

#[cfg(test)]
mod tests {
    #[test]
    fn opencode_agent_reports_variant_even_with_legacy_effort() {
        let mut info = forge_runs::AgentInfo {
            agent: artisan_api::ItemId::new(),
            thread: artisan_api::ThreadId::new(),
            task: "Review".into(),
            harness: artisan_api::EngineId::OpenCode,
            model: Some("opencode-go/muse".into()),
            effort: Some("medium".into()),
            variant: Some("xhigh".into()),
            permissions: artisan_api::Permissions::Auto,
            state: forge_runs::AgentState::Working,
            result: None,
            error: None,
            warnings: Vec::new(),
        };
        let output = super::agent(&info);
        assert!(output.contains("opencode-go/muse, xhigh"));
        assert!(!output.contains("medium"));
        info.variant = None;
        assert!(!super::agent(&info).contains("medium"));
        info.harness = artisan_api::EngineId::Codex;
        assert!(super::agent(&info).contains("medium"));
    }

    #[test]
    fn catalog_exposes_variant_ids_defaults_and_exact_context_choices() {
        let models: Vec<artisan_api::Model> = serde_json::from_value(serde_json::json!([{
            "id": "opencode/muse", "name": "Muse", "efforts": [], "default_effort": null,
            "variants": [{"id": "xhigh", "label": "Extra high"}], "default_variant": "xhigh",
            "context_window": 262144,
            "context_windows": [{"tokens": 262144}, {"tokens": 1048576, "override_tokens": 1000000}],
            "supports_images": true, "description": "Contributor", "lab": "Meta", "group": "OpenCode",
            "speeds": [{"id": "fast", "label": "Fast", "description": "Lower latency", "default": true}],
            "effort_notes": [{"effort": "max", "special": true, "advisory": "More usage", "description": "Maximum effort"}]
        }])).unwrap();
        let output = super::models(&models).unwrap();
        assert!(output.contains("OpenCode reasoning choices use variant"));
        let json = output
            .split("```json\n")
            .nth(1)
            .unwrap()
            .split("\n```")
            .next()
            .unwrap();
        let catalog: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(catalog[0]["variants"][0]["id"], "xhigh");
        assert_eq!(catalog[0]["default_variant"], "xhigh");
        assert_eq!(catalog[0]["context_window"], 262144);
        assert_eq!(catalog[0]["context_windows"][1]["override_tokens"], 1000000);
        assert_eq!(catalog[0]["speeds"][0]["default"], true);
        assert_eq!(catalog[0]["effort_notes"][0]["advisory"], "More usage");
        assert_eq!(catalog[0]["supports_images"], true);
        assert_eq!(catalog[0]["description"], "Contributor");
        assert_eq!(catalog[0]["lab"], "Meta");
        assert_eq!(catalog[0]["group"], "OpenCode");
    }
}
