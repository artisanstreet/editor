//! Claude usage samples and their best-effort report projection.
//!
//! Frame decoding extracts [`ClaudeUsageSample`] values from assistant and
//! result `usage` objects; the pump projects them onto the shared usage
//! vocabulary without ever blocking a turn.

use artisan_domain::{
    EngineModelId, EngineRouteId, RunId, RunUsageBasis, RunUsageReport, RunUsageReportInput,
    ThreadId, UnixMillis,
};
use serde_json::Value;
use tokio::sync::mpsc;

use super::super::observation::{EngineObservation, TerminalState, UsageObservation};

/// Cumulative usage sample from one frame's `usage` object.
///
/// Mirrors the TypeScript `UsageSchema` shape: terminal `result.usage`
/// totals carry the running counters, while an assistant frame's
/// per-response usage gauges the current window (see
/// [`parse_claude_assistant_usage`]).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ClaudeUsageSample {
    pub input: Option<u64>,
    pub cached_input: Option<u64>,
    pub output: Option<u64>,
    /// Window gauge from one assistant response only — never a sum and never
    /// taken from terminal totals, which re-count the context on every model
    /// call. Absent stays absent rather than becoming a wrong zero.
    pub context: Option<u64>,
}

fn claude_token_field(
    usage: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<u64>, ClaudeUsageCorrupt> {
    match usage.get(field) {
        None => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or(ClaudeUsageCorrupt),
    }
}

/// Marker: one frame's `usage` value was present but uninterpretable.
///
/// A present-but-corrupt usage object poisons its whole frame (which then
/// decodes `Unknown` at every entry point): the TypeScript adapter fails the
/// frame's schema decode the same way, so no text, gauge, or terminal ever
/// settles on corrupt provider numbers. Absent or empty usage stays
/// `Ok(None)` and never disturbs its frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeUsageCorrupt;

/// Extracts terminal usage totals from a `result` frame's `usage` object.
///
/// A non-object value or a present field outside `u64` (string, negative,
/// fraction, boolean, null, or container) fails the whole sample closed:
/// corrupt provider numbers never become a report and the frame carries
/// none. Absent stays absent rather than becoming zero, and an empty
/// measurement (`Ok(None)`) is not a report. The terminal totals never
/// become a context gauge: they accumulate input across every model call in
/// the turn, re-counting the context each call resent.
///
/// # Errors
///
/// Returns [`ClaudeUsageCorrupt`] when the usage value is present but
/// uninterpretable.
pub(crate) fn parse_claude_result_usage(
    usage: &Value,
) -> Result<Option<ClaudeUsageSample>, ClaudeUsageCorrupt> {
    let object = usage.as_object().ok_or(ClaudeUsageCorrupt)?;
    let sample = ClaudeUsageSample {
        input: claude_token_field(object, "input_tokens")?,
        cached_input: claude_token_field(object, "cache_read_input_tokens")?,
        output: claude_token_field(object, "output_tokens")?,
        context: None,
    };
    Ok(
        if sample.input.is_none() && sample.cached_input.is_none() && sample.output.is_none() {
            None
        } else {
            Some(sample)
        },
    )
}

/// Extracts the per-response sample from an `assistant` frame's `usage`
/// object, including the context-window gauge.
///
/// The gauge is the response's input plus the cache reads and writes that
/// carried the prior conversation — what actually occupies the window right
/// now. Corruption rules match [`parse_claude_result_usage`]: a present but
/// uninterpretable value fails the whole sample closed.
///
/// # Errors
///
/// Returns [`ClaudeUsageCorrupt`] when the usage value is present but
/// uninterpretable.
pub(crate) fn parse_claude_assistant_usage(
    usage: &Value,
) -> Result<Option<ClaudeUsageSample>, ClaudeUsageCorrupt> {
    let object = usage.as_object().ok_or(ClaudeUsageCorrupt)?;
    let input = claude_token_field(object, "input_tokens")?;
    let creation = claude_token_field(object, "cache_creation_input_tokens")?;
    let read = claude_token_field(object, "cache_read_input_tokens")?;
    let context = input.and_then(|tokens| {
        tokens
            .checked_add(creation.unwrap_or(0))?
            .checked_add(read.unwrap_or(0))
    });
    let sample = ClaudeUsageSample {
        input,
        cached_input: read,
        output: claude_token_field(object, "output_tokens")?,
        context,
    };
    Ok(
        if sample.input.is_none()
            && sample.cached_input.is_none()
            && sample.output.is_none()
            && sample.context.is_none()
        {
            None
        } else {
            Some(sample)
        },
    )
}

/// Immutable attribution for one Claude usage report.
///
/// Model and thread come from the immutable launch snapshot; the provider
/// session is the authenticated native session, never an envelope claim.
pub(crate) struct ClaudeUsageContext<'a> {
    pub run_id: &'a RunId,
    pub thread_id: &'a ThreadId,
    pub provider_session_id: &'a str,
    pub model_id: &'a EngineModelId,
    pub observed_at: UnixMillis,
}

/// Best-effort usage scope carried beside the text channel.
///
/// `None` (no explicit model or no thread scope) skips usage projection
/// without disturbing the turn: usage never blocks turns.
#[derive(Clone, Debug)]
pub(crate) struct ClaudeUsageAttribution {
    pub thread_id: ThreadId,
    pub model_id: EngineModelId,
}

/// Borrowed usage scope for one pump loop.
#[expect(
    clippy::struct_field_names,
    reason = "fields mirror ClaudeUsageAttribution and the wire usage vocabulary; renaming would obscure the mapping"
)]
pub(crate) struct ClaudeUsageScope<'a> {
    pub thread_id: &'a ThreadId,
    pub model_id: &'a EngineModelId,
    pub provider_session_id: &'a str,
}

/// Builds the cumulative usage report for one sample.
///
/// Fails closed (`None`) when identities or bounds reject: usage is never
/// synthesized from partial identities. The context gauge always replaces
/// the previous report regardless of basis — the codec performs no
/// arithmetic at all. Claude names no provider route, so reports attribute
/// to the engine's own `claude` route namespace; quota windows are never
/// copied here, so no quota is invented. Claude discloses no provider turn
/// identity on usage frames, so none is attributed rather than synthesized.
pub(crate) fn claude_usage_report(
    context: &ClaudeUsageContext<'_>,
    source_sequence: u64,
    sample: &ClaudeUsageSample,
) -> Option<RunUsageReport> {
    let provider_route_id = EngineRouteId::parse("claude").ok()?;
    RunUsageReport::new(RunUsageReportInput {
        run_id: context.run_id.clone(),
        thread_id: context.thread_id.clone(),
        provider_session_id: context.provider_session_id.to_owned(),
        source_sequence,
        model_id: context.model_id.clone(),
        provider_route_id,
        variant_id: None,
        basis: RunUsageBasis::Cumulative,
        provider_turn_id: None,
        input_tokens: sample.input,
        cached_input_tokens: sample.cached_input,
        output_tokens: sample.output,
        context_tokens: sample.context,
        context_window_tokens: None,
        observed_at: context.observed_at,
    })
    .ok()
}

fn current_unix_millis() -> Option<UnixMillis> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    let millis = i64::try_from(duration.as_millis()).ok()?;
    Some(UnixMillis::from_millis(millis))
}

/// Projects one usage sample best-effort onto the shared usage vocabulary.
///
/// Returns `Some(TerminalState::Interrupted)` only when the observation sink
/// closed mid-send. Skipping (no scope, no clock, or unattributable sample)
/// is never terminal: usage never blocks turns.
pub(crate) async fn project_usage_sample(
    observations: &mpsc::Sender<EngineObservation>,
    run_id: &RunId,
    scope: Option<&ClaudeUsageScope<'_>>,
    source_sequence: u64,
    sample: &ClaudeUsageSample,
) -> Option<TerminalState> {
    let scope = scope?;
    let observed_at = current_unix_millis()?;
    let report = claude_usage_report(
        &ClaudeUsageContext {
            run_id,
            thread_id: scope.thread_id,
            provider_session_id: scope.provider_session_id,
            model_id: scope.model_id,
            observed_at,
        },
        source_sequence,
        sample,
    )?;
    if observations
        .send(EngineObservation::Usage(UsageObservation::new(report)))
        .await
        .is_err()
    {
        return Some(TerminalState::Interrupted);
    }
    None
}
