//! Pure, bounded extraction of provider token usage from `OpenCode2` envelopes.
//!
//! The parser follows the Electron normalizer's authenticated event shape:
//! only `session.step.ended` and `session.step.failed` carry usage, the
//! session is `data.sessionID`, and the monotonic source cursor is
//! `durable.seq`. `OpenCode2` does not put an Artisan run or model in that
//! envelope, so the owner supplies immutable launch-context attribution.
//! No provider payload is retained in the result or in an error.

use artisan_domain::{
    EngineModelId, EngineRouteId, EngineVariantId, RUN_USAGE_MAX_SOURCE_SEQUENCE,
    RUN_USAGE_MAX_TOKEN_COUNT, RUN_USAGE_PROVIDER_SESSION_MAX_BYTES, RunId, RunUsageReport,
    RunUsageReportError, RunUsageReportInput, ThreadId, UnixMillis,
};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::engine_owner::framing::SseEvent;

/// A usage event is expected to be small; the SSE framer separately enforces
/// the configured stream-event ceiling. This parser rejects oversized input
/// before asking `serde_json` to allocate a value tree.
pub(crate) const OPENCODE2_USAGE_EVENT_MAX_BYTES: usize = 64 * 1024;

/// Immutable owner context needed to attribute an `OpenCode2` usage envelope.
///
/// `model_id`, `provider_route_id`, and `variant_id` must come from the
/// persisted run launch snapshot. In particular, they must not be read from
/// the current composer selection while a run is in flight.
pub(crate) struct OpenCode2UsageContext<'a> {
    pub(crate) run_id: &'a RunId,
    pub(crate) thread_id: &'a ThreadId,
    pub(crate) provider_session_id: &'a str,
    pub(crate) model_id: &'a EngineModelId,
    pub(crate) provider_route_id: &'a EngineRouteId,
    pub(crate) variant_id: Option<&'a EngineVariantId>,
    pub(crate) observed_at: UnixMillis,
}

impl<'a> OpenCode2UsageContext<'a> {
    /// Creates attribution context from the owner-held immutable run values.
    #[must_use]
    pub(crate) const fn new(
        run_id: &'a RunId,
        thread_id: &'a ThreadId,
        provider_session_id: &'a str,
        model_id: &'a EngineModelId,
        provider_route_id: &'a EngineRouteId,
        variant_id: Option<&'a EngineVariantId>,
        observed_at: UnixMillis,
    ) -> Self {
        Self {
            run_id,
            thread_id,
            provider_session_id,
            model_id,
            provider_route_id,
            variant_id,
            observed_at,
        }
    }
}

/// Payload-free failure while extracting one `OpenCode2` usage observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum UsageParseError {
    #[error("usage event exceeds its bounded size")]
    EventTooLarge,
    #[error("usage event is not valid json")]
    InvalidJson,
    #[error("usage event envelope is not an object")]
    InvalidEnvelope,
    #[error("usage event type is not valid")]
    InvalidEventType,
    #[error("usage event data is not an object")]
    InvalidData,
    #[error("usage event does not contain a provider session id")]
    MissingSessionId,
    #[error("usage event provider session id is not valid")]
    InvalidSessionId,
    #[error("usage event provider session does not match the active run")]
    SessionMismatch,
    #[error("usage event does not contain a durable source sequence")]
    MissingSourceSequence,
    #[error("usage event durable source sequence is not a bounded integer")]
    InvalidSourceSequence,
    #[error("usage event tokens value is not an object")]
    InvalidTokens,
    #[error("usage event cache value is not an object")]
    InvalidCache,
    #[error("usage event token field `{field}` is not a bounded integer")]
    InvalidToken { field: &'static str },
    #[error("usage event cost is not a finite number")]
    InvalidCost,
    #[error("usage event assistant message id is not a string")]
    InvalidAssistantMessageId,
    #[error("usage report failed domain validation: {0}")]
    Report(#[from] RunUsageReportError),
}

/// Extracts one usage report from a complete framed SSE event.
///
/// Unrelated valid `OpenCode2` event types return `Ok(None)`. A recognized
/// usage event must contain the actual durable/session fields; projected
/// recovery envelopes without `durable.seq` return
/// [`UsageParseError::MissingSourceSequence`] rather than receiving an
/// invented local cursor.
pub(crate) fn parse_opencode2_usage(
    event: &SseEvent,
    context: &OpenCode2UsageContext<'_>,
) -> Result<Option<RunUsageReport>, UsageParseError> {
    parse_opencode2_usage_json(event.data(), context)
}

/// Pure JSON form of [`parse_opencode2_usage`], kept separate so focused
/// parser tests do not need to manufacture private framing state.
pub(crate) fn parse_opencode2_usage_json(
    payload: &str,
    context: &OpenCode2UsageContext<'_>,
) -> Result<Option<RunUsageReport>, UsageParseError> {
    if payload.len() > OPENCODE2_USAGE_EVENT_MAX_BYTES {
        return Err(UsageParseError::EventTooLarge);
    }

    let envelope: Value =
        serde_json::from_str(payload).map_err(|_| UsageParseError::InvalidJson)?;
    let envelope = envelope
        .as_object()
        .ok_or(UsageParseError::InvalidEnvelope)?;
    let event_type = envelope
        .get("type")
        .and_then(Value::as_str)
        .ok_or(UsageParseError::InvalidEventType)?;
    if !matches!(event_type, "session.step.ended" | "session.step.failed") {
        return Ok(None);
    }

    let data = envelope
        .get("data")
        .and_then(Value::as_object)
        .ok_or(UsageParseError::InvalidData)?;
    let provider_session_id = match data.get("sessionID") {
        None => return Err(UsageParseError::MissingSessionId),
        Some(value) => value.as_str().ok_or(UsageParseError::InvalidSessionId)?,
    };
    validate_session_id(provider_session_id)?;
    if provider_session_id != context.provider_session_id {
        return Err(UsageParseError::SessionMismatch);
    }

    let durable = envelope
        .get("durable")
        .and_then(Value::as_object)
        .ok_or(UsageParseError::MissingSourceSequence)?;
    let source_sequence = durable
        .get("seq")
        .and_then(Value::as_u64)
        .filter(|sequence| *sequence <= RUN_USAGE_MAX_SOURCE_SEQUENCE)
        .ok_or(UsageParseError::InvalidSourceSequence)?;

    let tokens = match data.get("tokens") {
        None => None,
        Some(value) => Some(value.as_object().ok_or(UsageParseError::InvalidTokens)?),
    };
    let cost_present = match data.get("cost") {
        None => false,
        Some(value) => {
            if value.as_f64().is_none() {
                return Err(UsageParseError::InvalidCost);
            }
            true
        }
    };
    if tokens.is_none() && !cost_present {
        return Ok(None);
    }

    let (input_tokens, cached_input_tokens, output_tokens) = match tokens {
        None => (None, None, None),
        Some(tokens) => {
            let input_tokens = optional_token(tokens, "input")?;
            let output_tokens = optional_token(tokens, "output")?;
            let cache = match tokens.get("cache") {
                None => None,
                Some(value) => Some(value.as_object().ok_or(UsageParseError::InvalidCache)?),
            };
            let cached_input_tokens = match cache {
                None => None,
                Some(cache) => {
                    let cached = optional_token(cache, "read")?;
                    // These fields are part of the actual OpenCode2 shape and
                    // are validated even though this packet persists only the
                    // Electron normalizer's cached-read projection.
                    let _ = optional_token(cache, "write")?;
                    cached
                }
            };
            // Reasoning is also a genuine provider token field. Validate it,
            // but do not silently relabel it as output or context usage.
            let _ = optional_token(tokens, "reasoning")?;
            (input_tokens, cached_input_tokens, output_tokens)
        }
    };

    let provider_turn_id = match data.get("assistantMessageID") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or(UsageParseError::InvalidAssistantMessageId)?
                .to_owned(),
        ),
    };

    Ok(Some(RunUsageReport::new(RunUsageReportInput {
        run_id: context.run_id.clone(),
        thread_id: context.thread_id.clone(),
        provider_session_id: provider_session_id.to_owned(),
        source_sequence,
        model_id: context.model_id.clone(),
        provider_route_id: context.provider_route_id.clone(),
        variant_id: context.variant_id.cloned(),
        basis: artisan_domain::RunUsageBasis::Delta,
        provider_turn_id,
        input_tokens,
        cached_input_tokens,
        output_tokens,
        // Current OpenCode2 events do not report either context measure. The
        // model catalog's limit is not a usage observation and is not copied.
        context_tokens: None,
        context_window_tokens: None,
        observed_at: context.observed_at,
    })?))
}

fn validate_session_id(value: &str) -> Result<(), UsageParseError> {
    if value.is_empty()
        || value.len() > RUN_USAGE_PROVIDER_SESSION_MAX_BYTES
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(UsageParseError::InvalidSessionId);
    }
    Ok(())
}

fn optional_token(
    object: &Map<String, Value>,
    key: &'static str,
) -> Result<Option<u64>, UsageParseError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let token = value
        .as_u64()
        .filter(|token| *token <= RUN_USAGE_MAX_TOKEN_COUNT)
        .ok_or(UsageParseError::InvalidToken { field: key })?;
    Ok(Some(token))
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use super::*;

    fn context() -> OpenCode2UsageContext<'static> {
        static RUN_ID: OnceLock<RunId> = OnceLock::new();
        static THREAD_ID: OnceLock<ThreadId> = OnceLock::new();
        static MODEL_ID: OnceLock<EngineModelId> = OnceLock::new();
        static ROUTE_ID: OnceLock<EngineRouteId> = OnceLock::new();
        let run_id = RUN_ID.get_or_init(|| RunId::parse("run-usage-1").expect("run id"));
        let thread_id =
            THREAD_ID.get_or_init(|| ThreadId::parse("thread-usage-1").expect("thread id"));
        let model_id =
            MODEL_ID.get_or_init(|| EngineModelId::parse("model-usage-1").expect("model id"));
        let route_id =
            ROUTE_ID.get_or_init(|| EngineRouteId::parse("route-usage-1").expect("route id"));
        OpenCode2UsageContext {
            run_id,
            thread_id,
            provider_session_id: "provider-session-1",
            model_id,
            provider_route_id: route_id,
            variant_id: None,
            observed_at: UnixMillis::from_millis(42),
        }
    }

    const FIXTURE: &str = r#"{
        "type":"session.step.ended",
        "data":{
            "sessionID":"provider-session-1",
            "assistantMessageID":"assistant-message-1",
            "tokens":{
                "cache":{"read":12,"write":3},
                "input":40,
                "output":9,
                "reasoning":2
            },
            "cost":0.001
        },
        "durable":{"seq":17}
    }"#;

    #[test]
    fn parses_the_actual_step_usage_envelope_and_preserves_attribution() {
        let report = parse_opencode2_usage_json(FIXTURE, &context())
            .expect("fixture is valid")
            .expect("step carries usage");
        assert_eq!(report.source_sequence(), 17);
        assert_eq!(report.provider_session_id(), "provider-session-1");
        assert_eq!(report.provider_turn_id(), Some("assistant-message-1"));
        assert_eq!(report.input_tokens(), Some(40));
        assert_eq!(report.cached_input_tokens(), Some(12));
        assert_eq!(report.output_tokens(), Some(9));
        assert_eq!(report.context_tokens(), None);
        assert_eq!(report.context_window_tokens(), None);
    }

    #[test]
    fn preserves_zero_and_absent_fields() {
        let payload = r#"{
            "type":"session.step.failed",
            "data":{
                "sessionID":"provider-session-1",
                "tokens":{
                    "cache":{"read":0,"write":0},
                    "input":0,
                    "output":0,
                    "reasoning":0
                }
            },
            "durable":{"seq":0}
        }"#;
        let report = parse_opencode2_usage_json(payload, &context())
            .expect("zero-valued usage is valid")
            .expect("tokens are present");
        assert_eq!(report.source_sequence(), 0);
        assert_eq!(report.input_tokens(), Some(0));
        assert_eq!(report.cached_input_tokens(), Some(0));
        assert_eq!(report.output_tokens(), Some(0));

        let absent = r#"{
            "type":"session.step.ended",
            "data":{"sessionID":"provider-session-1","tokens":{}},
            "durable":{"seq":1}
        }"#;
        let report = parse_opencode2_usage_json(absent, &context())
            .expect("absent token fields are valid")
            .expect("tokens object is a usage observation");
        assert_eq!(report.input_tokens(), None);
        assert_eq!(report.cached_input_tokens(), None);
        assert_eq!(report.output_tokens(), None);
    }

    #[test]
    fn rejects_invalid_numeric_and_missing_source_values() {
        let fractional = FIXTURE.replace("\"input\":40", "\"input\":40.5");
        assert!(matches!(
            parse_opencode2_usage_json(&fractional, &context()),
            Err(UsageParseError::InvalidToken { field: "input" })
        ));

        let negative = FIXTURE.replace("\"output\":9", "\"output\":-1");
        assert!(matches!(
            parse_opencode2_usage_json(&negative, &context()),
            Err(UsageParseError::InvalidToken { field: "output" })
        ));

        let oversized = FIXTURE.replace("\"seq\":17", "\"seq\":9223372036854775808");
        assert!(matches!(
            parse_opencode2_usage_json(&oversized, &context()),
            Err(UsageParseError::InvalidSourceSequence)
        ));

        let no_durable = FIXTURE.replace("\"durable\":{\"seq\":17}", "\"durable\":null");
        assert!(matches!(
            parse_opencode2_usage_json(&no_durable, &context()),
            Err(UsageParseError::MissingSourceSequence)
        ));

        let invalid_cost = FIXTURE.replace("\"cost\":0.001", "\"cost\":\"0.001\"");
        assert!(matches!(
            parse_opencode2_usage_json(&invalid_cost, &context()),
            Err(UsageParseError::InvalidCost)
        ));

        let oversized = "{".repeat(OPENCODE2_USAGE_EVENT_MAX_BYTES + 1);
        assert!(matches!(
            parse_opencode2_usage_json(&oversized, &context()),
            Err(UsageParseError::EventTooLarge)
        ));
    }

    #[test]
    fn does_not_use_an_unrelated_event_or_a_mismatched_session() {
        let unrelated = FIXTURE.replace("session.step.ended", "session.text.delta");
        assert_eq!(
            parse_opencode2_usage_json(&unrelated, &context()).expect("unrelated event"),
            None
        );

        let mismatch = FIXTURE.replace("provider-session-1", "provider-session-2");
        assert!(matches!(
            parse_opencode2_usage_json(&mismatch, &context()),
            Err(UsageParseError::SessionMismatch)
        ));
    }
}
