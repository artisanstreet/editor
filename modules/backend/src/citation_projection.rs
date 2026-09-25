//! Resolve Codex citation references from the exact bound provider session.
//! This is a read-only display projection: stored messages and revisions remain intact.
use artisan_database::{Repository, SessionContinuationLookup, SessionContinuationQuery};
use artisan_domain::{
    AssistantBody, ConversationItem, ConversationSnapshot, EngineId, EngineSelection,
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    io::Read as _,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

pub(crate) const OPEN: &str = "\u{e200}cite\u{e202}";
const CLOSE: char = '\u{e201}';
const SEP: char = '\u{e202}';
const MAX_BYTES: u64 = 16 * 1024 * 1024;
type Replacements = HashMap<String, String>;
struct Cached {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
    replacements: Replacements,
}
static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();

pub(crate) async fn resolve_snapshot(
    repository: &Repository,
    snapshot: ConversationSnapshot,
) -> ConversationSnapshot {
    if !snapshot.items().iter().any(|item| matches!(item, ConversationItem::AssistantMessage(message) if message.body.as_str().contains(OPEN))) { return snapshot; }
    let Some(settings) = repository
        .read_thread_engine_settings(snapshot.thread_id())
        .await
        .ok()
        .flatten()
    else {
        return snapshot;
    };
    let EngineSelection::Codex(selection) = settings.config().selection() else {
        return snapshot;
    };
    let Ok(SessionContinuationLookup::Usable(session)) = repository
        .read_session_continuation(SessionContinuationQuery {
            thread_id: snapshot.thread_id().clone(),
            engine_id: EngineId::Codex,
            profile_id: selection.profile_id().clone(),
            exclude_run_id: None,
        })
        .await
    else {
        return snapshot;
    };
    let id = session.session_id.as_str().to_owned();
    let replacements = tokio::task::spawn_blocking(move || session_replacements(&id))
        .await
        .unwrap_or_default();
    if replacements.is_empty() {
        return snapshot;
    }
    let items = snapshot
        .items()
        .iter()
        .cloned()
        .map(|mut item| {
            if let ConversationItem::AssistantMessage(message) = &mut item
                && let Some(body) = replacements.get(message.body.as_str())
                && let Ok(body) = AssistantBody::parse(body.clone())
            {
                message.body = body;
            }
            item
        })
        .collect();
    ConversationSnapshot::new(
        snapshot.thread_id().clone(),
        snapshot.cursor(),
        snapshot.turns().to_vec(),
        items,
        snapshot.updated_at(),
    )
    .unwrap_or(snapshot)
}

pub(crate) async fn resolve_message(session_id: &str, body: String) -> String {
    if !body.contains(OPEN) {
        return body;
    }
    let id = session_id.to_owned();
    let original = body.clone();
    tokio::task::spawn_blocking(move || {
        session_replacements(&id)
            .remove(&body)
            .filter(|resolved| resolved.len() <= AssistantBody::MAX_BYTES)
            .unwrap_or(body)
    })
    .await
    .unwrap_or(original)
}

fn session_replacements(id: &str) -> Replacements {
    // Provider ids must never become arbitrary filesystem paths.
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return HashMap::new();
    }
    let root = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|home| PathBuf::from(home).join(".codex"))
        });
    let Some(root) = root else {
        return HashMap::new();
    };
    let root = root.join("sessions");
    let key = format!("{}:{id}", root.display());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut cache) = cache.lock() else {
        return HashMap::new();
    };
    let path = cache
        .get(&key)
        .map(|cached| cached.path.clone())
        .or_else(|| find_rollout(&root, id, 4, &mut 8192));
    let Some(path) = path else {
        return HashMap::new();
    };
    let Ok(metadata) = std::fs::metadata(&path) else {
        return HashMap::new();
    };
    if metadata.len() > MAX_BYTES {
        return HashMap::new();
    }
    let modified = metadata.modified().ok();
    if let Some(cached) = cache.get(&key)
        && cached.len == metadata.len()
        && cached.modified == modified
    {
        return cached.replacements.clone();
    }
    let Ok(file) = std::fs::File::open(&path) else {
        return HashMap::new();
    };
    let mut text = String::new();
    if file.take(MAX_BYTES + 1).read_to_string(&mut text).is_err() || text.len() as u64 > MAX_BYTES
    {
        return HashMap::new();
    }
    let replacements = parse_rollout(&text, id);
    if cache.len() >= 16 {
        cache.clear();
    }
    cache.insert(
        key,
        Cached {
            path,
            modified,
            len: metadata.len(),
            replacements: replacements.clone(),
        },
    );
    replacements
}

fn find_rollout(root: &Path, id: &str, depth: usize, remaining: &mut usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        if *remaining == 0 {
            return None;
        }
        *remaining -= 1;
        let kind = entry.file_type().ok()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_file()
            && entry
                .file_name()
                .to_str()?
                .ends_with(&format!("-{id}.jsonl"))
        {
            return Some(entry.path());
        }
        if kind.is_dir()
            && let Some(found) = find_rollout(&entry.path(), id, depth - 1, remaining)
        {
            return Some(found);
        }
    }
    None
}

fn parse_rollout(text: &str, session_id: &str) -> Replacements {
    let mut sources = HashMap::new();
    let mut replacements = HashMap::new();
    let mut authorized = false;
    let mut ambiguous = HashSet::new();
    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let payload = &row["payload"];
        if row["type"] == "session_meta" {
            authorized = payload["id"].as_str() == Some(session_id);
        }
        if !authorized {
            continue;
        }
        if payload["type"] == "task_started" {
            sources.clear();
        }
        if payload["type"] == "custom_tool_call_output" || payload["type"] == "function_call_output"
        {
            collect_output(&payload["output"], &mut sources);
        }
        if payload["type"] == "message" && payload["role"] == "assistant" {
            for part in payload["content"].as_array().into_iter().flatten() {
                if let Some(body) = part["text"].as_str()
                    && body.contains(OPEN)
                {
                    let resolved = resolve_body(body, &sources);
                    // Identical reply text can occur in different native turns.
                    // Without a unique message match, never assign another turn's sources.
                    if ambiguous.contains(body) {
                        continue;
                    }
                    if replacements
                        .get(body)
                        .is_some_and(|previous| previous != &resolved)
                    {
                        replacements.remove(body);
                        ambiguous.insert(body.to_owned());
                    } else {
                        replacements.insert(body.to_owned(), resolved);
                    }
                }
            }
        }
    }
    replacements
}

fn collect_output(value: &Value, sources: &mut HashMap<String, (String, String)>) {
    if let Some(text) = value.as_str() {
        collect_sources(text, sources);
    }
    if let Some(parts) = value.as_array() {
        for part in parts {
            if let Some(text) = part["text"].as_str() {
                collect_sources(text, sources);
            }
        }
    }
}

fn collect_sources(text: &str, sources: &mut HashMap<String, (String, String)>) {
    let mut previous = "";
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(OPEN)
            && let Some(end) = rest.find(CLOSE)
            && let Some((title, url)) = previous.rsplit_once(" (")
            && let Some(url) = url.strip_suffix(')')
            && let Ok(parsed) = reqwest::Url::parse(url)
            && matches!(parsed.scheme(), "http" | "https")
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
        {
            for id in rest[..end].split(SEP).take(32) {
                if sources.len() < 512 {
                    sources.insert(
                        id.to_owned(),
                        (
                            title.chars().take(200).collect(),
                            parsed.as_str().to_owned(),
                        ),
                    );
                }
            }
        }
        if !line.trim().is_empty() {
            previous = line.trim();
        }
    }
}

fn resolve_body(body: &str, sources: &HashMap<String, (String, String)>) -> String {
    let mut output = String::new();
    let mut rest = body;
    while let Some(start) = rest.find(OPEN) {
        output.push_str(&rest[..start]);
        let citation = &rest[start + OPEN.len()..];
        let Some(end) = citation.find(CLOSE) else {
            output.push_str(&rest[start..]);
            return output;
        };
        for id in citation[..end].split(SEP).take(32) {
            if let Some((title, url)) = sources.get(id) {
                let label = title
                    .replace('\\', "\\\\")
                    .replace('[', "\\[")
                    .replace(']', "\\]");
                let _ = write!(output, " [{label}](<{url}>)");
            } else {
                output.push_str(" [source unavailable]");
            }
        }
        rest = &citation[end + CLOSE.len_utf8()..];
    }
    output.push_str(rest);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_replies_with_different_sources_are_not_misattributed() {
        let mut rows = vec![serde_json::json!({"type":"session_meta","payload":{"id":"test"}})];
        for url in ["https://example.com/first", "https://example.com/second"] {
            rows.push(serde_json::json!({"payload":{"type":"task_started"}}));
            rows.push(serde_json::json!({"payload":{"type":"custom_tool_call_output","output":format!("Title ({url})\n\u{e200}cite\u{e202}turn0search0\u{e201}")}}));
            rows.push(serde_json::json!({"payload":{"type":"message","role":"assistant","content":[{"text":"Fact. \u{e200}cite\u{e202}turn0search0\u{e201}"}]}}));
        }
        let text = rows
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse_rollout(&text, "test").is_empty());
    }

    #[test]
    fn citations_resolve_only_inside_the_bound_session_and_turn() {
        let text = [
            serde_json::json!({"type":"session_meta","payload":{"id":"test"}}),
            serde_json::json!({"payload":{"type":"task_started"}}),
            serde_json::json!({"payload":{"type":"custom_tool_call_output","output":[{"text":"Example (https://example.com/a)\n\u{e200}cite\u{e202}turn0search0\u{e201} excerpt"}]}}),
            serde_json::json!({"payload":{"type":"message","role":"assistant","content":[{"text":"Answer. \u{e200}cite\u{e202}turn0search0\u{e201}"}]}}),
            serde_json::json!({"payload":{"type":"task_started"}}),
            serde_json::json!({"payload":{"type":"message","role":"assistant","content":[{"text":"Other. \u{e200}cite\u{e202}turn0search0\u{e201}"}]}}),
        ].iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        assert!(parse_rollout(&text, "wrong").is_empty());
        let parsed = parse_rollout(&text, "test");
        assert!(
            parsed
                .values()
                .any(|body| body.contains("[Example](<https://example.com/a>)"))
        );
        assert!(
            parsed
                .values()
                .any(|body| body == "Other.  [source unavailable]")
        );
    }
}

#[cfg(test)]
mod saved_session_test {
    use super::*;
    #[test]
    #[ignore = "requires a local Codex session supplied by CITATION_TEST_SESSION_ID"]
    fn recover_saved_session_citations() {
        let id = std::env::var("CITATION_TEST_SESSION_ID").expect("session id");
        let replacements = session_replacements(&id);
        assert!(!replacements.is_empty());
        for body in replacements.values() {
            assert!(!body.contains(OPEN), "internal markers must be resolved");
            assert!(
                !body.contains("source unavailable"),
                "all sources in this fixture exist"
            );
            assert!(body.contains("](<https://"));
        }
        println!(
            "Recovered {} cited messages from the exact provider session",
            replacements.len()
        );
    }
}

#[cfg(test)]
mod snapshot_recovery_test {
    use super::*;
    #[tokio::test]
    #[ignore = "read-only probe using CITATION_TEST_DATABASE and CITATION_TEST_THREAD"]
    async fn recover_existing_snapshot_without_mutating_stored_text() {
        use artisan_domain::{
            ConversationQuery, ConversationQueryBounds, QueryTurnCount, ThreadId,
        };
        let path = std::env::var("CITATION_TEST_DATABASE").expect("database path");
        let id = std::env::var("CITATION_TEST_THREAD").expect("thread id");
        let database = sea_orm::Database::connect(format!("sqlite://{path}?mode=ro"))
            .await
            .expect("read-only database");
        let repository = Repository::new(database);
        let query = ConversationQuery {
            thread_id: ThreadId::parse(id).unwrap(),
            bounds: ConversationQueryBounds::Window {
                maximum_turn_count: QueryTurnCount::new(32).unwrap(),
            },
        };
        let original = repository.read_conversation_snapshot(&query).await.unwrap();
        assert!(original.items().iter().any(|item| matches!(item, ConversationItem::AssistantMessage(message) if message.body.as_str().contains(OPEN))));
        let projected = resolve_snapshot(&repository, original.clone()).await;
        assert_eq!(projected.cursor(), original.cursor());
        assert!(projected.items().iter().any(|item| matches!(item, ConversationItem::AssistantMessage(message) if message.body.as_str().contains("](<https://") && !message.body.as_str().contains(OPEN))));
        assert_eq!(
            repository.read_conversation_snapshot(&query).await.unwrap(),
            original
        );
    }
}
