//! Crate-private pure decoder bridging `SseEvent` and `EngineObservation`.

use artisan_domain::RunId;
use thiserror::Error;

use crate::engine_owner::framing::SseEvent;
use crate::engine_owner::observation::{
    EngineObservation, TerminalObservation, TerminalState, chunk_text,
};

/// Typed, payload-free failure of SSE event decoding.
///
/// `Debug` and `Display` are constant strings; no raw event data, text,
/// reason, error reference, or identifier is ever embedded. All variants are
/// `Copy` and `Eq` to keep error handling free of allocations.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum EventDecodeError {
    #[error("event data is not valid json")]
    InvalidJson,
    #[error("run id is not valid")]
    InvalidRunId,
    #[error("sequence is not valid")]
    InvalidSequence,
    #[error("event contains both delta and state")]
    BothShapes,
    #[error("event contains neither delta nor state")]
    NeitherShape,
    #[error("delta is not valid")]
    InvalidDelta,
    #[error("state is not valid")]
    InvalidState,
    #[error("terminal state is unknown")]
    UnknownState,
    #[error("reason is not valid")]
    InvalidReason,
    #[error("error reference is not valid")]
    InvalidErrorRef,
}

/// Decodes a single framed `SseEvent` into zero or more `EngineObservation`s.
///
/// Only `event.data()` is interpreted as JSON; callers must not feed raw
/// transport bytes. Validation and mapping are pure and without side effects.
///
/// Contract:
/// - `data()` must be a JSON object.
/// - `run_id` must be a non-empty domain-validated string (`RunId`).
/// - `sequence` must be a JSON integer that fits `u64`.
/// - Exactly one of `delta` (string) or `state` (string) must be present.
/// - `delta` yields lossless chunks through `chunk_text` using the provided
///   sequence unchanged; empty delta yields zero observations.
/// - `state` maps exactly `succeeded -> Completed`, `failed -> Failed`,
///   `cancelled -> Cancelled`, `interrupted -> Interrupted` with optional
///   string-or-null `reason` and `error_ref`.
/// - Stable text chunk IDs use the SSE `id` when non-empty, otherwise the
///   validated run ID string.
/// - Extra unrelated JSON fields are ignored.
/// - Typed errors never retain raw payload bytes.
pub(crate) fn decode_sse_event(
    event: &SseEvent,
) -> Result<Vec<EngineObservation>, EventDecodeError> {
    let data = event.data();
    let value: serde_json::Value =
        serde_json::from_str(data).map_err(|_| EventDecodeError::InvalidJson)?;
    let object = value.as_object().ok_or(EventDecodeError::InvalidJson)?;

    let run_id_value = object.get("run_id").ok_or(EventDecodeError::InvalidRunId)?;
    let run_id_str = run_id_value
        .as_str()
        .ok_or(EventDecodeError::InvalidRunId)?;
    let run_id = RunId::parse(run_id_str).map_err(|_| EventDecodeError::InvalidRunId)?;

    let sequence_value = object
        .get("sequence")
        .ok_or(EventDecodeError::InvalidSequence)?;
    let sequence = sequence_value
        .as_u64()
        .ok_or(EventDecodeError::InvalidSequence)?;

    let has_delta = object.contains_key("delta");
    let has_state = object.contains_key("state");

    match (has_delta, has_state) {
        (true, true) => return Err(EventDecodeError::BothShapes),
        (false, false) => return Err(EventDecodeError::NeitherShape),
        _ => {}
    }

    if has_delta {
        let delta_value = object.get("delta").expect("delta present");
        let delta_str = delta_value.as_str().ok_or(EventDecodeError::InvalidDelta)?;
        let native_id = match event.id() {
            Some(id) if !id.is_empty() => id,
            _ => run_id.as_str(),
        };
        let deltas = chunk_text(&run_id, sequence, native_id, delta_str);
        let observations = deltas
            .into_iter()
            .map(EngineObservation::TextDelta)
            .collect();
        return Ok(observations);
    }

    // Terminal shape.
    let state_value = object.get("state").expect("state present");
    let state_str = state_value.as_str().ok_or(EventDecodeError::InvalidState)?;
    let state = match state_str {
        "succeeded" => TerminalState::Completed,
        "failed" => TerminalState::Failed,
        "cancelled" => TerminalState::Cancelled,
        "interrupted" => TerminalState::Interrupted,
        _ => return Err(EventDecodeError::UnknownState),
    };

    let reason = match object.get("reason") {
        None => None,
        Some(value) if value.is_null() => None,
        Some(value) => {
            let text = value.as_str().ok_or(EventDecodeError::InvalidReason)?;
            Some(text.to_owned())
        }
    };

    let error_ref = match object.get("error_ref") {
        None => None,
        Some(value) if value.is_null() => None,
        Some(value) => {
            let text = value.as_str().ok_or(EventDecodeError::InvalidErrorRef)?;
            Some(text.to_owned())
        }
    };

    let terminal = TerminalObservation::new(run_id, sequence, state, reason, error_ref);
    Ok(vec![EngineObservation::Terminal(terminal)])
}
