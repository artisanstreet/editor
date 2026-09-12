//! Shared Codex app-server JSONL wire helpers.
//!
//! One bounded, payload-free implementation of the line reads/writes and
//! request-id extraction used by both the socket open phase
//! (`engine_owner::socket::codex_session`) and the configured turn drive
//! phase (`engine_owner::operation::codex`). Provider bytes never leave this
//! boundary; only typed identities and text do.

#![forbid(unsafe_code)]

use std::sync::Arc;

use artisan_transport::CancelHandle;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
use tokio::process::{ChildStdin, ChildStdout};

use super::protocol::CODEX_MAX_FRAME_BYTES;

/// Why one bounded wire interaction failed.
///
/// The open and drive phases map these control outcomes onto their own typed
/// error vocabularies; the variants carry no payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexLineError {
    /// The owner shut down.
    Shutdown,
    /// The caller cancelled the turn.
    Cancelled,
    /// The absolute phase deadline elapsed.
    Deadline,
    /// The transport failed or ended.
    StreamFailed,
}

/// Writes one newline-terminated JSONL request to the owned stdin.
///
/// # Errors
///
/// Returns [`CodexLineError::StreamFailed`] when the write or flush fails.
pub(crate) async fn write_codex_line(
    stdin: &mut ChildStdin,
    line: &str,
) -> Result<(), CodexLineError> {
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|_| CodexLineError::StreamFailed)?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|_| CodexLineError::StreamFailed)?;
    stdin
        .flush()
        .await
        .map_err(|_| CodexLineError::StreamFailed)?;
    Ok(())
}

/// Reads one line while racing shutdown, cancellation, and the deadline.
///
/// # Errors
///
/// Returns the winning [`CodexLineError`]: shutdown, cancellation, deadline,
/// or a failed/ended transport.
pub(crate) async fn read_codex_line(
    reader: &mut tokio::io::BufReader<ChildStdout>,
    line: &mut String,
    deadline: tokio::time::Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
) -> Result<(), CodexLineError> {
    line.clear();
    tokio::select! {
        biased;
        () = shutdown.wait() => Err(CodexLineError::Shutdown),
        () = control.wait() => Err(CodexLineError::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(CodexLineError::Deadline),
        read = reader.read_line(line) => match read {
            Ok(0) | Err(_) => Err(CodexLineError::StreamFailed),
            Ok(_) => Ok(()),
        },
    }
}

/// Returns whether one handshake line is the result for the request id.
///
/// Bounds the line before parsing and requires a `result` member; anything
/// else fails the handshake closed without spawning further phases.
pub(crate) fn is_codex_result_for(line: &str, id: u64) -> bool {
    if line.len() > CODEX_MAX_FRAME_BYTES {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    let matches_id = value.get("id").is_some_and(|candidate| {
        candidate.as_u64() == Some(id)
            || candidate
                .as_str()
                .is_some_and(|text| text == id.to_string())
    });
    matches_id && value.get("result").is_some()
}

/// Extracts the exact native thread identity from a `thread/start` result.
///
/// Returns `None` on id mismatch, missing thread, or out-of-bound identity
/// so the dispatcher never binds a corrupt session.
pub(crate) fn codex_thread_id(line: &str, id: u64) -> Option<String> {
    if line.len() > CODEX_MAX_FRAME_BYTES {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    let matches_id = value.get("id").is_some_and(|candidate| {
        candidate.as_u64() == Some(id)
            || candidate
                .as_str()
                .is_some_and(|text| text == id.to_string())
    });
    if !matches_id {
        return None;
    }
    let thread = value.get("result")?.get("thread")?;
    let id = thread.get("id")?.as_str()?;
    if id.is_empty() || id.len() > 256 {
        return None;
    }
    Some(id.to_owned())
}

/// Extracts the exact native turn identity from a `turn/start` result.
///
/// Returns `None` on id mismatch, on a JSON-RPC error envelope (for example
/// `-32600` for a missing `threadId`), or on a missing/out-of-bound turn
/// identity, so the dispatcher fails the turn fast instead of pumping a turn
/// that the server never started.
pub(crate) fn codex_turn_id(line: &str, id: u64) -> Option<String> {
    if line.len() > CODEX_MAX_FRAME_BYTES {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    let matches_id = value.get("id").is_some_and(|candidate| {
        candidate.as_u64() == Some(id)
            || candidate
                .as_str()
                .is_some_and(|text| text == id.to_string())
    });
    if !matches_id {
        return None;
    }
    if value.get("error").is_some() {
        return None;
    }
    let turn = value.get("result")?.get("turn")?;
    let id = turn.get("id")?.as_str()?;
    if id.is_empty() || id.len() > 256 {
        return None;
    }
    Some(id.to_owned())
}

/// Returns whether one inbound line carries a JSON-RPC response id equal to
/// the supplied request id (numeric or string form).
///
/// Used to correlate error envelopes with their pending request: only the
/// matching reply fails its phase fast, while uncorrelated lines keep the
/// bounded wait alive.
pub(crate) fn codex_response_id_matches(line: &str, id: u64) -> bool {
    if line.len() > CODEX_MAX_FRAME_BYTES {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    value.get("id").is_some_and(|candidate| {
        candidate.as_u64() == Some(id)
            || candidate
                .as_str()
                .is_some_and(|text| text == id.to_string())
    })
}

/// Extracts the JSON-RPC response id of one inbound line, if it carries one.
///
/// Method envelopes (notifications and server requests) carry no id and
/// yield `None`; numeric and string id forms both correlate. Used to route
/// correlated `turn/steer` replies to their pending delivery instead of
/// misparsing a steer response as turn completion.
pub(crate) fn codex_response_id(line: &str) -> Option<u64> {
    if line.len() > CODEX_MAX_FRAME_BYTES {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("method").is_some() {
        return None;
    }
    let id = value.get("id")?;
    if let Some(numeric) = id.as_u64() {
        return Some(numeric);
    }
    id.as_str()?.parse::<u64>().ok()
}

/// Extracts the resumed native thread identity, requiring the same thread.
///
/// `thread/resume` reopens provider-owned state only: a result naming any
/// other thread fails closed (`None`) instead of adopting a foreign session,
/// so resume reopens the same thread id and a restart replays the durable
/// prefix without duplicating provider effects.
pub(crate) fn codex_resumed_thread_id(
    line: &str,
    id: u64,
    stored_thread_id: &str,
) -> Option<String> {
    let resumed = codex_thread_id(line, id)?;
    (resumed.as_str() == stored_thread_id).then_some(resumed)
}
