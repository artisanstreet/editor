//! Explicit offline history normalization; never used by production reads.
use super::{IMPORT_BYTES, stamp};
use crate::Store;
use anyhow::Result;
use artisan_api::{ItemId, ItemKind, ToolKind};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::Path;

/// Read-only count for the CLI's existing dry-run mode. Opening Store would
/// recover runs and mutate the database, so inspection uses a readonly handle.
pub fn inspect_agent_messages(data_dir: &Path) -> Result<usize> {
    let conn = Connection::open_with_flags(
        data_dir.join("forge.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut statement = conn.prepare("SELECT length(CAST(body AS BLOB)) + COALESCE(json_extract(body,'$.kind.tool.input.bytes'),0), body FROM items WHERE json_extract(body, '$.kind.tool.kind') = 'other' AND json_extract(body, '$.kind.tool.name') = 'SendMessage' AND coalesce((SELECT json_extract(r.body,'$.engine') FROM runs r WHERE r.id=items.run_id),(SELECT json_extract(t.body,'$.config.engine') FROM threads t WHERE t.id=items.thread_id))='claude'")?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let size: usize = row.get(0)?;
        anyhow::ensure!(
            size <= 32 * 1024 * 1024,
            "history item exceeds 32 MiB normalization byte limit"
        );
        let body: String = row.get(1)?;
        let item: artisan_api::Item = serde_json::from_str(&body)?;
        count += usize::from(legacy_agent_message(&conn, &item)?.is_some());
    }
    Ok(count)
}

/// Explicit compatibility repair for databases imported before AgentMessage
/// became canonical. It never runs when the production Store opens.
pub async fn normalize_agent_messages(store: &Store) -> Result<usize> {
    let mut cursor = String::new();
    let mut normalized = 0;
    loop {
        let after = cursor.clone();
        let ids: Vec<ItemId> = store.read(move |conn| {
            let mut statement = conn.prepare_cached("SELECT id, length(CAST(body AS BLOB)) + COALESCE(json_extract(body,'$.kind.tool.input.bytes'),0) FROM items WHERE id > ?1 AND json_extract(body, '$.kind.tool.kind') = 'other' AND json_extract(body, '$.kind.tool.name') = 'SendMessage' AND coalesce((SELECT json_extract(r.body,'$.engine') FROM runs r WHERE r.id=items.run_id),(SELECT json_extract(t.body,'$.config.engine') FROM threads t WHERE t.id=items.thread_id))='claude' ORDER BY id LIMIT 256")?;
            let mut rows = statement.query([after])?;
            let mut ids = Vec::new();
            let mut bytes = 0usize;
            while let Some(row) = rows.next()? {
                let size: usize = row.get(1)?;
                anyhow::ensure!(size<=32*1024*1024,"history item exceeds 32 MiB normalization byte limit");
                if !ids.is_empty() && bytes.saturating_add(size) > IMPORT_BYTES { break; }
                bytes = bytes.saturating_add(size);
                ids.push(ItemId(row.get(0)?));
            }
            Ok(ids)
        }).await?;
        let Some(last) = ids.last() else {
            break;
        };
        cursor = last.to_string();
        normalized += store
            .write(move |tx| {
                let mut changed = 0;
                for id in ids {
                    let Some(old) = crate::read::item(tx, &id)? else {
                        continue;
                    };
                    let Some(tool) = legacy_agent_message(tx, &old)? else {
                        continue;
                    };
                    let mut item = old.clone();
                    if let ItemKind::Tool { tool: slot, .. } = &mut item.kind {
                        *slot = tool;
                    }
                    let stamp = stamp(item.created_at, item.updated_at);
                    tx.with_stamp(stamp, |tx| tx.save_item(item, Some(old)))?;
                    changed += 1;
                }
                Ok(changed)
            })
            .await?;
    }
    Ok(normalized)
}

fn legacy_agent_message(conn: &Connection, item: &artisan_api::Item) -> Result<Option<ToolKind>> {
    let input = match &item.kind {
        ItemKind::Tool {
            tool:
                ToolKind::Other {
                    name,
                    input: Some(input),
                },
            ..
        } if name == "SendMessage" => input,
        _ => return Ok(None),
    };
    anyhow::ensure!(
        input.byte_len() <= 32 * 1024 * 1024,
        "legacy agent-message input exceeds 32 MiB normalization byte limit"
    );
    let input = match input {
        artisan_api::TextContent::Inline(text) => std::borrow::Cow::Borrowed(text.as_str()),
        artisan_api::TextContent::Stored(_) => {
            std::borrow::Cow::Owned(crate::content::resolve(conn, input)?)
        }
    };
    let Some(mut tool) = parse_legacy_agent_message(&input) else {
        return Ok(None);
    };
    if let ToolKind::AgentMessage {
        recipient, target, ..
    } = &mut tool
    {
        *target = crate::read::agent_by_address(conn, &item.thread_id, recipient)?;
    }
    Ok(Some(tool))
}

fn parse_legacy_agent_message(input: &str) -> Option<ToolKind> {
    let input: Value = serde_json::from_str(input).ok()?;
    let field = |key| {
        input
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
    };
    Some(ToolKind::AgentMessage {
        recipient: field("to")?,
        target: None,
        summary: field("summary"),
        text: (field("message").unwrap_or_default()).into(),
    })
}

#[cfg(test)]
mod byte_admission_tests {
    use super::*;
    #[test]
    fn inspect_rejects_multibyte_history_above_byte_limit_before_item_decode() {
        let folder = tempfile::tempdir().unwrap();
        let conn = Connection::open(folder.path().join("forge.sqlite3")).unwrap();
        conn.execute_batch("CREATE TABLE items(id TEXT,thread_id TEXT,run_id TEXT,body TEXT);CREATE TABLE runs(id TEXT,body TEXT);CREATE TABLE threads(id TEXT,body TEXT);INSERT INTO threads VALUES('thread','{\"config\":{\"engine\":\"claude\"}}');").unwrap();
        let text = "€".repeat(32 * 1024 * 1024 / 3 + 1);
        let body=serde_json::json!({"kind":{"type":"tool","tool":{"kind":"other","name":"SendMessage","input":text}}}).to_string();
        assert!(body.chars().count() < 32 * 1024 * 1024 && body.len() > 32 * 1024 * 1024);
        conn.execute("INSERT INTO items VALUES('item','thread',NULL,?1)", [body])
            .unwrap();
        let error = inspect_agent_messages(folder.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("32 MiB normalization byte limit")
        );
        let bytes: usize = conn
            .query_row("SELECT length(CAST(body AS BLOB)) FROM items", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(
            bytes > 32 * 1024 * 1024,
            "readonly inspection must preserve original bytes"
        );
    }
}
