use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct EditTotals {
    pub(super) added: u64,
    pub(super) removed: u64,
}

fn diff_totals(diff: &str) -> EditTotals {
    let mut totals = EditTotals::default();
    let mut in_hunk = false;
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            in_hunk = false;
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if !in_hunk && (line.starts_with("+++") || line.starts_with("---")) {
            continue;
        }
        if line.starts_with('+') {
            totals.added = totals.added.saturating_add(1);
        } else if line.starts_with('-') {
            totals.removed = totals.removed.saturating_add(1);
        }
    }
    totals
}

pub(super) fn edit_totals(tool: &Value) -> Option<EditTotals> {
    if tool.get("kind").and_then(Value::as_str) != Some("edit") {
        return None;
    }
    let mut totals = EditTotals::default();
    for change in tool.get("changes")?.as_array()? {
        let diff = match change.get("diff") {
            None | Some(Value::Null) => continue,
            Some(Value::String(diff)) => diff,
            Some(_stored) => return None,
        };
        let count = diff_totals(diff);
        totals.added = totals.added.saturating_add(count.added);
        totals.removed = totals.removed.saturating_add(count.removed);
    }
    Some(totals)
}

fn kind_type(item: &Value) -> Option<&str> {
    item.pointer("/kind/type").and_then(Value::as_str)
}

fn present(value: Option<&Value>) -> bool {
    value.is_some_and(|value| !value.is_null())
}

pub(super) fn folds_into_work(item: &Value) -> bool {
    let kind = &item["kind"];
    match kind_type(item) {
        Some("user_message") => {
            (kind.get("steer").and_then(Value::as_bool) == Some(true)
                && present(item.get("run_id")))
                || present(kind.get("from_agent"))
        }
        Some("assistant_message") => kind.get("phase").and_then(Value::as_str) != Some("final"),
        Some("tool" | "plan") => {
            item.get("status").and_then(Value::as_str) != Some("awaiting_user")
        }
        Some("notice") => !matches!(
            kind.pointer("/kind/type").and_then(Value::as_str),
            Some("goal" | "you_should_know")
        ),
        Some("agent") => true,
        Some(_) | None => false,
    }
}

fn outline_label(text: &str) -> String {
    let prefix: String = text.chars().take(1024).collect();
    let line = prefix
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Image");
    let mut label: String = line.chars().take(160).collect();
    if line.chars().nth(160).is_some() {
        label.push('…');
    }
    label
}

#[derive(Debug, thiserror::Error)]
pub enum OutlineError {
    #[error("read the start of stored text {content}")]
    Read {
        content: String,
        #[source]
        source: rusqlite::Error,
    },
    #[error("stored text {content} is not stored")]
    Missing { content: String },
    #[error("the start of stored text {content} is not UTF-8")]
    Text {
        content: String,
        #[source]
        source: std::string::FromUtf8Error,
    },
}

fn text_start(conn: &Connection, text: &Value) -> Result<Option<String>, OutlineError> {
    match text {
        Value::String(text) => Ok(Some(text.clone())),
        Value::Object(stored) => {
            let Some(content) = stored.get("id").and_then(Value::as_str) else {
                return Ok(None);
            };
            let data: Vec<u8> = conn
                .query_row(
                    include_str!("../sql/migrations/first_chunk.sql"),
                    [content],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|source| OutlineError::Read {
                    content: content.to_owned(),
                    source,
                })?
                .ok_or_else(|| OutlineError::Missing {
                    content: content.to_owned(),
                })?;
            String::from_utf8(data)
                .map(Some)
                .map_err(|source| OutlineError::Text {
                    content: content.to_owned(),
                    source,
                })
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => Ok(None),
    }
}

pub(super) fn outline_entry(
    conn: &Connection,
    item: &Value,
) -> Result<Option<String>, OutlineError> {
    let kind = &item["kind"];
    let steered =
        kind.get("steer").and_then(Value::as_bool) == Some(true) && present(item.get("run_id"));
    if kind_type(item) != Some("user_message")
        || present(item.get("parent_id"))
        || present(kind.get("from_agent"))
        || steered
    {
        return Ok(None);
    }
    Ok(text_start(conn, &kind["text"])?.map(|text| outline_label(&text)))
}
