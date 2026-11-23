use serde::Deserialize;
use std::collections::HashMap;
use std::io::{ErrorKind, SeekFrom};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

const PROFILE: &str = "antigravity-cli";
const TRANSCRIPT_TAIL: u64 = 256 * 1024;
const MAX_MESSAGES: usize = 256;
const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;
const RECEIPTS: &str = "read.json";
const HIGH_MARK: &str = "priority=MESSAGE_PRIORITY_HIGH";
const LOW_MARK: &str = "priority=MESSAGE_PRIORITY_LOW";

#[derive(Debug, thiserror::Error)]
pub(super) enum PendingWakeError {
    #[error("Antigravity session id {session} is not a plain id")]
    SessionId { session: String },
    #[error("list Antigravity's queued messages in {}", path.display())]
    ListMessages {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Antigravity's queued messages in {} exceed {MAX_MESSAGES}", path.display())]
    TooManyMessages { path: PathBuf },
    #[error("read Antigravity's record {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decode Antigravity's record {}", path.display())]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("the last line of Antigravity's transcript {} is longer than {TRANSCRIPT_TAIL} bytes", path.display())]
    TranscriptLineTooLong { path: PathBuf },
}

#[derive(Debug, Deserialize)]
struct Message {
    priority: Priority,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
enum Priority {
    #[serde(rename = "MESSAGE_PRIORITY_HIGH")]
    High,
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct Step {
    #[serde(rename = "type")]
    kind: StepKind,
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum StepKind {
    SystemMessage,
    #[serde(other)]
    Other,
}

pub(super) async fn pending_wake(
    gemini_dir: &Path,
    session: &str,
) -> Result<bool, PendingWakeError> {
    if session.is_empty()
        || !session
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || char == '-')
    {
        return Err(PendingWakeError::SessionId {
            session: session.to_owned(),
        });
    }
    let generated = gemini_dir
        .join(PROFILE)
        .join("brain")
        .join(session)
        .join(".system_generated");
    if unread_high_priority(&generated.join("messages")).await? {
        return Ok(true);
    }
    trailing_system_message(&generated.join("logs").join("transcript.jsonl")).await
}

async fn unread_high_priority(inbox: &Path) -> Result<bool, PendingWakeError> {
    let mut entries = match tokio::fs::read_dir(inbox).await {
        Ok(entries) => entries,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(PendingWakeError::ListMessages {
                path: inbox.to_owned(),
                source,
            });
        }
    };
    let listed = |source| PendingWakeError::ListMessages {
        path: inbox.to_owned(),
        source,
    };
    let mut messages = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(listed)? {
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
            continue;
        };
        if name != RECEIPTS && entry.file_type().await.map_err(listed)?.is_file() {
            messages.push(id.to_owned());
        }
        if messages.len() > MAX_MESSAGES {
            return Err(PendingWakeError::TooManyMessages {
                path: inbox.to_owned(),
            });
        }
    }
    if messages.is_empty() {
        return Ok(false);
    }
    let receipts_path = inbox.join(RECEIPTS);
    let receipts: HashMap<String, bool> = match read_bounded(&receipts_path).await {
        Ok(bytes) => decode(&receipts_path, &bytes)?,
        Err(PendingWakeError::Read { source, .. }) if source.kind() == ErrorKind::NotFound => {
            HashMap::new()
        }
        Err(error) => return Err(error),
    };
    for id in messages {
        if receipts.get(&id) == Some(&true) {
            continue;
        }
        let path = inbox.join(format!("{id}.json"));
        let message: Message = decode(&path, &read_bounded(&path).await?)?;
        if message.priority == Priority::High {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn trailing_system_message(path: &Path) -> Result<bool, PendingWakeError> {
    let failed = |source| PendingWakeError::Read {
        path: path.to_owned(),
        source,
    };
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(failed(source)),
    };
    let size = file.metadata().await.map_err(failed)?.len();
    let start = size.saturating_sub(TRANSCRIPT_TAIL);
    file.seek(SeekFrom::Start(start)).await.map_err(failed)?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).await.map_err(failed)?;
    let Some(line) = last_line(&tail, start > 0) else {
        return if start > 0 {
            Err(PendingWakeError::TranscriptLineTooLong {
                path: path.to_owned(),
            })
        } else {
            Ok(false)
        };
    };
    let step: Step = decode(path, line)?;
    Ok(wakes(&step))
}

fn last_line(tail: &[u8], cut: bool) -> Option<&[u8]> {
    let trimmed = tail.trim_ascii_end();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.iter().rposition(|byte| *byte == b'\n') {
        Some(newline) => Some(&trimmed[newline + 1..]),
        None if cut => None,
        None => Some(trimmed),
    }
}

fn wakes(step: &Step) -> bool {
    step.kind == StepKind::SystemMessage
        && step
            .content
            .as_deref()
            .is_none_or(|content| content.contains(HIGH_MARK) || !content.contains(LOW_MARK))
}

async fn read_bounded(path: &Path) -> Result<Vec<u8>, PendingWakeError> {
    let failed = |source| PendingWakeError::Read {
        path: path.to_owned(),
        source,
    };
    let file = tokio::fs::File::open(path).await.map_err(failed)?;
    let mut bytes = Vec::new();
    file.take(MAX_MESSAGE_BYTES)
        .read_to_end(&mut bytes)
        .await
        .map_err(failed)?;
    Ok(bytes)
}

fn decode<T: serde::de::DeserializeOwned>(
    path: &Path,
    bytes: &[u8],
) -> Result<T, PendingWakeError> {
    serde_json::from_slice(bytes).map_err(|source| PendingWakeError::Decode {
        path: path.to_owned(),
        source,
    })
}
