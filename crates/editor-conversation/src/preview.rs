//! What an activity shows when opened, as markdown: the markdown renderer
//! highlights fenced code off the main thread, so a preview is only text.

use artisan_api::{FileOp, Item, ItemKind, SearchMode, Status, TextContent, ToolKind};
use std::fmt::Write as _;

/// Lines one code block shows before it says how many more there are.
const MAX_LINES: usize = 400;
/// Matches a grep could list before the rest are counted.
const MAX_MATCHES: usize = 200;

/// The opened activity's markdown; `None` when it has nothing to show beyond
/// its row, or its text is stored rather than inline.
pub fn markdown(item: &Item) -> Option<String> {
    let ItemKind::Tool { tool, output, .. } = &item.kind else {
        return None;
    };
    // A failed call's output is its reason, drawn red under its row.
    if item.status == Status::Failed {
        return None;
    }
    let output = output
        .as_ref()
        .and_then(|output| output.text.inline())
        .map(str::trim_end)
        .filter(|output| !output.trim().is_empty());
    let mut text = String::new();
    match tool {
        ToolKind::Read { path, .. } => fence(&mut text, language(path), &unnumbered(output?)),
        ToolKind::Edit { changes } => {
            let changes = changes.inline()?;
            for change in changes {
                if changes.len() > 1 || matches!(change.op, FileOp::Move { .. }) {
                    writeln!(text, "{}\n", change_heading(&change.op, &change.path)).ok();
                }
                let Some(diff) = change.diff.as_ref().and_then(TextContent::inline) else {
                    continue;
                };
                match (&change.op, added_only(diff)) {
                    (FileOp::Add, Some(file)) => fence(&mut text, language(&change.path), &file),
                    _ => fence(&mut text, Some("diff"), &hunks(diff)),
                }
            }
        }
        ToolKind::Search { mode, .. } => match mode {
            SearchMode::Grep => grep(&mut text, output?),
            SearchMode::Glob | SearchMode::List => paths(&mut text, output?),
        },
        ToolKind::WebFetch { url } => {
            writeln!(text, "<{url}>\n").ok();
            if let Some(output) = output {
                text.push_str(output);
            }
        }
        ToolKind::WebSearch { .. } => text.push_str(output?),
        // The row already says what was asked; opened, a call shows what
        // came back.
        ToolKind::Mcp { .. } | ToolKind::Other { .. } => result(&mut text, output?),
        ToolKind::Agents { .. } => fence(&mut text, None, output?),
        // A command opens on its terminal, a message on its own text; an
        // image has no text.
        ToolKind::Shell { .. } | ToolKind::AgentMessage { .. } | ToolKind::ViewImage { .. } => {
            return None;
        }
    }
    let text = text.trim_end().to_owned();
    (!text.is_empty()).then_some(text)
}

/// `code` in a fence long enough that nothing inside closes it, cut at
/// [`MAX_LINES`].
fn fence(text: &mut String, language: Option<&str>, code: &str) {
    let longest = code
        .lines()
        .map(|line| line.chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or_default();
    let ticks = "`".repeat(longest.max(2) + 1);
    let total = code.lines().count();
    writeln!(text, "{ticks}{}", language.unwrap_or_default()).ok();
    for line in code.lines().take(MAX_LINES) {
        text.push_str(line);
        text.push('\n');
    }
    writeln!(text, "{ticks}\n").ok();
    if total > MAX_LINES {
        writeln!(text, "{} more lines\n", total - MAX_LINES).ok();
    }
}

/// The fence language for a path: its extension, which the highlighter
/// resolves; a few names it doesn't know by extension.
fn language(path: &str) -> Option<&str> {
    let name = path.rsplit(['/', '\\']).next()?;
    match name {
        "Makefile" | "makefile" => return Some("make"),
        "Dockerfile" => return Some("dockerfile"),
        _ => {}
    }
    let (_, extension) = name.rsplit_once('.')?;
    Some(match extension {
        "tsx" | "ts" | "mts" | "cts" => "js",
        "jsonl" => "json",
        "svelte" | "vue" => "html",
        "nix" => "nix",
        extension => extension,
    })
}

/// File text without the line numbers `cat -n` style reads put in front
/// (`    12→`, `    12\t`, `00012| `), when nearly every line has one.
fn unnumbered(text: &str) -> String {
    let strip = |line: &str| -> Option<String> {
        let trimmed = line.trim_start();
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        let rest = trimmed.get(digits..).filter(|_| digits > 0)?;
        ["→", "\t", "| ", "|"]
            .into_iter()
            .find_map(|mark| rest.strip_prefix(mark))
            .map(str::to_owned)
    };
    // Claude may append a note after the file; it isn't the file.
    let total = text.lines().count();
    let stripped: Vec<String> = text.lines().filter_map(strip).collect();
    if stripped.is_empty() || stripped.len() * 10 < total * 9 {
        return text.to_owned();
    }
    stripped.join("\n")
}

/// A new file's text, when its diff only adds lines.
fn added_only(diff: &str) -> Option<String> {
    let mut file = Vec::new();
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
            continue;
        }
        file.push(line.strip_prefix('+')?);
    }
    Some(file.join("\n"))
}

/// A diff without its `---`/`+++` file header lines, which the row names.
fn hunks(diff: &str) -> String {
    diff.lines()
        .filter(|line| !line.starts_with("+++") && !line.starts_with("---"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn change_heading(op: &FileOp, path: &str) -> String {
    match op {
        FileOp::Add => format!("Created `{path}`"),
        FileOp::Modify => format!("Edited `{path}`"),
        FileOp::Delete => format!("Deleted `{path}`"),
        FileOp::Move { from } => format!("Moved `{from}` to `{path}`"),
    }
}

/// Grep output grouped by file: `path:line:text` lines become one
/// highlighted block per file; a bare list of paths stays a list.
fn grep(text: &mut String, output: &str) {
    let lines: Vec<&str> = output.lines().filter(|line| !line.is_empty()).collect();
    let found: Option<Vec<(&str, &str, &str)>> = lines.iter().map(|line| grep_line(line)).collect();
    let Some(found) = found else {
        if lines.iter().all(|line| !line.contains(':')) {
            paths(text, output);
        } else {
            fence(text, None, output);
        }
        return;
    };
    let mut files: Vec<(&str, Vec<String>)> = Vec::new();
    for (path, number, line) in found.iter().take(MAX_MATCHES) {
        let entry = format!("{number:>5}  {line}");
        match files.last_mut() {
            Some((last, lines)) if last == path => lines.push(entry),
            _ => files.push((path, vec![entry])),
        }
    }
    for (path, lines) in files {
        writeln!(text, "`{path}`\n").ok();
        fence(text, language(path), &lines.join("\n"));
    }
    if found.len() > MAX_MATCHES {
        writeln!(text, "{} more matches", found.len() - MAX_MATCHES).ok();
    }
}

/// One grep line's path, line number and text: `path:12:text` for a match,
/// `path-12-text` for context around it.
fn grep_line(line: &str) -> Option<(&str, &str, &str)> {
    line.char_indices()
        .filter(|(_, c)| matches!(c, ':' | '-'))
        .find_map(|(at, separator)| {
            let rest = &line[at + 1..];
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            let found = rest[digits..].strip_prefix(separator)?;
            (at > 0 && digits > 0).then(|| (&line[..at], &rest[..digits], found))
        })
}

/// Found paths as a list.
fn paths(text: &mut String, output: &str) {
    let lines: Vec<&str> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    for line in lines.iter().take(MAX_MATCHES) {
        writeln!(text, "- `{}`", line.trim()).ok();
    }
    if lines.len() > MAX_MATCHES {
        writeln!(text, "\n{} more", lines.len() - MAX_MATCHES).ok();
    }
    text.push('\n');
}

/// A tool's result: JSON pretty-printed, anything else as markdown.
fn result(text: &mut String, output: &str) {
    if let Ok(value @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) =
        serde_json::from_str::<serde_json::Value>(output)
    {
        let pretty = serde_json::to_string_pretty(&value).unwrap_or_default();
        fence(text, Some("json"), &pretty);
    } else {
        text.push_str(output);
        text.push('\n');
    }
}
