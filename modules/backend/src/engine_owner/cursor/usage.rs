#![allow(clippy::module_name_repetitions)]
#![allow(dead_code)]

use artisan_domain::{
    EngineModelId, EngineRouteId, RunId, RunUsageBasis, RunUsageReport, RunUsageReportInput,
    ThreadId, UnixMillis,
};
use serde_json::Value;
use tokio::sync::mpsc;

use super::super::observation::{EngineObservation, TerminalState, UsageObservation};

/// Cumulative token sample from one ACP usage disclosure.
///
/// Mirrors the TypeScript ACP evidence: prompt-result `usage`
/// (`inputTokens`/`outputTokens`/`cachedReadTokens`, `basis: "cumulative"` in
/// `modules/engines/src/acp/engine.ts`) carries the running counters, while
/// a streaming `usage_update` (`used`/`size`, `basis: "cumulative"` in
/// `modules/engines/src/acp/normalizer.ts`) gauges the current window.
/// Absent stays absent rather than becoming a wrong zero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CursorUsageSample {
    pub input: Option<u64>,
    pub cached_input: Option<u64>,
    pub output: Option<u64>,
    /// Window gauge from a `usage_update` only — never a sum. Every turn
    /// resends its context, so prompt totals keep counting while the gauge
    /// measures what actually occupies the window right now. Absent stays
    /// absent rather than becoming a wrong zero.
    pub context: Option<u64>,
    pub context_window: Option<u64>,
}

/// Extracts the cumulative sample from a prompt-result `usage` object.
///
/// Non-`u64` numerics (negatives, fractions) fail closed to absent for that
/// field; absent stays absent rather than becoming zero. Returns `None` when
/// no counter carries a value: an empty measurement is not a report.
pub(crate) fn parse_cursor_prompt_usage(usage: &Value) -> Option<CursorUsageSample> {
    let object = usage.as_object()?;
    let sample = CursorUsageSample {
        input: object.get("inputTokens").and_then(Value::as_u64),
        cached_input: object.get("cachedReadTokens").and_then(Value::as_u64),
        output: object.get("outputTokens").and_then(Value::as_u64),
        context: None,
        context_window: None,
    };
    if sample.input.is_none() && sample.cached_input.is_none() && sample.output.is_none() {
        return None;
    }
    Some(sample)
}

/// Extracts the window gauge from a streaming `usage_update` wire update.
///
/// Mirrors `usage_update` in `modules/engines/src/acp/normalizer.ts` (`used`
/// gauges the window, `size` its capacity, `basis: "cumulative"`). The gauge
/// always replaces the previous report regardless of basis — the codec
/// performs no arithmetic at all. Returns `None` when neither gauge carries
/// a value: an empty measurement is not a report.
pub(crate) fn parse_cursor_usage_update(update: &Value) -> Option<CursorUsageSample> {
    let object = update.as_object()?;
    let sample = CursorUsageSample {
        input: None,
        cached_input: None,
        output: None,
        context: object.get("used").and_then(Value::as_u64),
        context_window: object.get("size").and_then(Value::as_u64),
    };
    if sample.context.is_none() && sample.context_window.is_none() {
        return None;
    }
    Some(sample)
}

/// Immutable attribution for one cursor usage report.
///
/// Model and thread come from the immutable launch snapshot; the provider
/// session is the authenticated ACP session, never an envelope claim.
pub(crate) struct CursorUsageContext<'a> {
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
pub(crate) struct CursorUsageAttribution {
    pub thread_id: ThreadId,
    pub model_id: EngineModelId,
}

/// Borrowed usage scope for one pump loop.
#[expect(
    clippy::struct_field_names,
    reason = "fields mirror CursorUsageAttribution and the wire usage vocabulary; renaming would obscure the mapping"
)]
pub(crate) struct CursorUsageScope<'a> {
    pub thread_id: &'a ThreadId,
    pub model_id: &'a EngineModelId,
    pub provider_session_id: &'a str,
}

/// Builds the cumulative usage report for one sample.
///
/// Fails closed (`None`) when identities or bounds reject: usage is never
/// synthesized from partial identities. The context gauge always replaces
/// the previous report regardless of basis — the codec performs no
/// arithmetic at all. Cursor names no provider route, so reports attribute
/// to the engine's own `cursor` route namespace; dashboard quota windows are
/// never copied here, so no quota is invented. This follows the TypeScript
/// ACP disclosure: prompt-result and `usage_update` usage is cumulative.
pub(crate) fn cursor_usage_report(
    context: &CursorUsageContext<'_>,
    provider_turn_id: Option<String>,
    source_sequence: u64,
    sample: &CursorUsageSample,
) -> Option<RunUsageReport> {
    let provider_route_id = EngineRouteId::parse("cursor").ok()?;
    RunUsageReport::new(RunUsageReportInput {
        run_id: context.run_id.clone(),
        thread_id: context.thread_id.clone(),
        provider_session_id: context.provider_session_id.to_owned(),
        source_sequence,
        model_id: context.model_id.clone(),
        provider_route_id,
        variant_id: None,
        basis: RunUsageBasis::Cumulative,
        provider_turn_id,
        input_tokens: sample.input,
        cached_input_tokens: sample.cached_input,
        output_tokens: sample.output,
        context_tokens: sample.context,
        context_window_tokens: sample.context_window,
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
pub(crate) async fn project_cursor_usage_sample(
    observations: &mpsc::Sender<EngineObservation>,
    run_id: &RunId,
    scope: Option<&CursorUsageScope<'_>>,
    provider_turn_id: Option<String>,
    source_sequence: u64,
    sample: &CursorUsageSample,
) -> Option<TerminalState> {
    let scope = scope?;
    let observed_at = current_unix_millis()?;
    let report = cursor_usage_report(
        &CursorUsageContext {
            run_id,
            thread_id: scope.thread_id,
            provider_session_id: scope.provider_session_id,
            model_id: scope.model_id,
            observed_at,
        },
        provider_turn_id,
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

/// Kind of one cursor quota window.
///
/// Mirrors `map_cursor_period_usage_to_quota_windows` in
/// `modules/engines/src/cursor/usage.ts`: the dashboard discloses monthly
/// billing-cycle pools only (plan pools plus on-demand). Anything outside
/// the cursor surface is unknown rather than guessed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CursorQuotaWindowKind {
    Monthly,
    Unknown,
}

/// Classifies one quota window id.
///
/// Ids emitted by [`map_cursor_quota_windows`] are monthly billing-cycle
/// pools; anything else is unknown rather than guessed.
pub(crate) fn classify_cursor_quota_window_kind(window_id: &str) -> CursorQuotaWindowKind {
    match window_id {
        "cursor:cursor-models"
        | "cursor:other-models"
        | "cursor:included-usage"
        | "cursor:on-demand" => CursorQuotaWindowKind::Monthly,
        _ => CursorQuotaWindowKind::Unknown,
    }
}

/// Clamps one percent reading into `0..=100`.
///
/// Absent or non-finite readings become `0`: usage display never blocks on a
/// corrupt gauge and never invents quota from it.
pub(crate) fn clamp_cursor_percent_used(used_percent: Option<f64>) -> f64 {
    match used_percent {
        Some(value) if value.is_finite() => value.clamp(0.0, 100.0),
        _ => 0.0,
    }
}

/// Formats whole provider millisecond instants as an ISO-8601 UTC instant.
///
/// Billing-cycle bounds arrive as whole provider milliseconds and the reset
/// instant is display-only diagnostics, never turn input. Returns `None` for
/// negative or unrepresentable instants rather than inventing a date.
/// Computed without a date library so the owner keeps no new dependency for
/// one diagnostic string.
pub(crate) fn cursor_reset_at_iso(millis: i64) -> Option<String> {
    if millis < 0 {
        return None;
    }
    let secs = millis.checked_div(1_000)?;
    let days = secs.checked_div(86_400)?;
    let clock = secs.checked_rem(86_400)?;
    let shifted = days.checked_add(719_468)?;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_pair = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_pair + 2) / 5 + 1;
    let month = if month_pair < 10 {
        month_pair + 3
    } else {
        month_pair - 9
    };
    let display_year = if month <= 2 { year + 1 } else { year };
    if !(0..10_000).contains(&display_year) {
        return None;
    }
    Some(format!(
        "{display_year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        clock / 3_600,
        (clock % 3_600) / 60,
        clock % 60
    ))
}

/// One provider-neutral cursor quota window: diagnostics only, never quota.
///
/// Quota windows are read through the non-billable dashboard surface and
/// classified here; they are never copied into [`RunUsageReport`] and never
/// gate a turn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CursorQuotaWindow {
    pub id: String,
    pub kind: CursorQuotaWindowKind,
    pub label: Option<String>,
    pub percent_used: f64,
    pub resets_at: Option<String>,
    pub window_minutes: Option<u64>,
    /// Always `"shared"`: the dashboard reports account pools shared across
    /// models, never per-model attribution. Mirrors the TypeScript scope
    /// without inventing quota attribution.
    pub scope: &'static str,
}

/// Extracts the first `N%` gauge from a provider display message.
///
/// Mirrors `percentage_from_display_message` in
/// `modules/engines/src/cursor/usage.ts` (`\b(\d+(?:\.\d+)?)%`): digits must
/// start on a word boundary and the value must be finite.
fn cursor_percentage_from_display_message(message: &Value) -> Option<f64> {
    let text = message.as_str()?;
    let bytes = text.as_bytes();
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut index = 0;
    while index < bytes.len() {
        let is_boundary_start =
            bytes[index].is_ascii_digit() && (index == 0 || !is_word(bytes[index - 1]));
        if is_boundary_start {
            let mut end = index;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end < bytes.len() && bytes[end] == b'.' {
                let mut fraction = end + 1;
                while fraction < bytes.len() && bytes[fraction].is_ascii_digit() {
                    fraction += 1;
                }
                if fraction > end + 1 {
                    end = fraction;
                }
            }
            if end < bytes.len() && bytes[end] == b'%' {
                let parsed: f64 = text[index..end].parse().ok()?;
                if parsed.is_finite() {
                    return Some(parsed);
                }
            }
        }
        index += 1;
    }
    None
}

/// Reads one optional provider number, mirroring `optional_number` in
/// `modules/engines/src/cursor/usage.ts`: absent and null stay absent,
/// numbers and numeric strings parse when finite, anything else fails the
/// whole mapping (best-effort: the caller yields no windows).
fn cursor_optional_number(
    record: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<f64>, ()> {
    let Some(value) = record.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    if let Some(number) = value.as_f64() {
        return if number.is_finite() {
            Ok(Some(number))
        } else {
            Err(())
        };
    }
    if let Some(text) = value.as_str() {
        let parsed: f64 = text.parse().map_err(|_| ())?;
        return if parsed.is_finite() {
            Ok(Some(parsed))
        } else {
            Err(())
        };
    }
    Err(())
}

/// Maps a decoded dashboard `GetCurrentPeriodUsage` response to
/// provider-neutral quota windows.
///
/// Mirrors `map_cursor_period_usage_to_quota_windows` in
/// `modules/engines/src/cursor/usage.ts`: split plan pools when the provider
/// discloses them (`autoPercentUsed`/`apiPercentUsed`/`autoBucketModels`),
/// else one included-usage pool; then the first capped on-demand pool
/// (overall, individual, pooled). Malformed input yields no windows:
/// collection is best-effort diagnostics and never blocks a turn.
#[expect(
    clippy::too_many_lines,
    reason = "quota mapping is one linear best-effort projection with ordered fallbacks; extraction would fragment the precedence rules"
)]
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "JSON numbers are range-checked immediately before each narrowing cast; the f64 bounds only reject out-of-range values"
)]
pub(crate) fn map_cursor_quota_windows(response: &Value) -> Vec<CursorQuotaWindow> {
    let Some(root) = response.as_object() else {
        return Vec::new();
    };
    let Some(plan) = root.get("planUsage").and_then(Value::as_object) else {
        return Vec::new();
    };
    let Ok(start_ms) = cursor_optional_number(root, "billingCycleStart") else {
        return Vec::new();
    };
    let Ok(end_ms) = cursor_optional_number(root, "billingCycleEnd") else {
        return Vec::new();
    };
    let resets_at = end_ms.and_then(|ms| {
        if ms < 0.0 || !ms.is_finite() || ms > i64::MAX as f64 {
            return None;
        }
        cursor_reset_at_iso(ms as i64)
    });
    let window_minutes = match (start_ms, end_ms) {
        (Some(start), Some(end)) if end > start => {
            let minutes = ((end - start) / 60_000.0).round();
            if minutes > 0.0 && minutes <= u64::MAX as f64 {
                Some(minutes as u64)
            } else {
                None
            }
        }
        _ => None,
    };
    let Ok(total_spend) = cursor_optional_number(plan, "totalSpend") else {
        return Vec::new();
    };
    let Ok(included_limit) = cursor_optional_number(plan, "limit") else {
        return Vec::new();
    };
    let Ok(provider_percent) = cursor_optional_number(plan, "totalPercentUsed") else {
        return Vec::new();
    };
    let total_spend = total_spend.unwrap_or(0.0);
    let included_limit = included_limit.unwrap_or(0.0);
    let plan_percent = if included_limit > 0.0 {
        (total_spend / included_limit) * 100.0
    } else {
        provider_percent
            .or_else(|| {
                root.get("displayMessage")
                    .and_then(cursor_percentage_from_display_message)
            })
            .unwrap_or(0.0)
    };
    let Ok(cursor_models_percent) = cursor_optional_number(plan, "autoPercentUsed") else {
        return Vec::new();
    };
    let Ok(other_models_percent) = cursor_optional_number(plan, "apiPercentUsed") else {
        return Vec::new();
    };
    // The dashboard reports two independent plan pools. Repeated protobuf
    // fields survive at zero more reliably than scalar percentages, so
    // `autoBucketModels` is also the compatibility discriminator when an
    // unused pool's zero-valued percentage is omitted from protobuf JSON.
    let has_split_plan_usage = cursor_models_percent.is_some()
        || other_models_percent.is_some()
        || root.get("autoBucketModels").is_some_and(Value::is_array);
    let mut plan_windows: Vec<(String, String, f64)> = Vec::new();
    if has_split_plan_usage {
        plan_windows.push((
            "cursor:cursor-models".to_owned(),
            "Cursor models".to_owned(),
            clamp_cursor_percent_used(cursor_models_percent),
        ));
        plan_windows.push((
            "cursor:other-models".to_owned(),
            "Other models".to_owned(),
            clamp_cursor_percent_used(other_models_percent),
        ));
    } else {
        plan_windows.push((
            "cursor:included-usage".to_owned(),
            "Included usage".to_owned(),
            clamp_cursor_percent_used(Some(plan_percent)),
        ));
    }
    let mut windows: Vec<CursorQuotaWindow> = plan_windows
        .into_iter()
        .map(|(id, label, percent_used)| CursorQuotaWindow {
            kind: classify_cursor_quota_window_kind(&id),
            id,
            label: Some(label),
            percent_used,
            resets_at: resets_at.clone(),
            window_minutes,
            scope: "shared",
        })
        .collect();

    let Some(spend_limit) = root.get("spendLimitUsage").and_then(Value::as_object) else {
        return windows;
    };
    for (limit_key, used_key, remaining_key) in [
        ("overallLimit", "overallUsed", "overallRemaining"),
        ("individualLimit", "individualUsed", "individualRemaining"),
        ("pooledLimit", "pooledUsed", "pooledRemaining"),
    ] {
        let Ok(limit) = cursor_optional_number(spend_limit, limit_key) else {
            return Vec::new();
        };
        let limit = limit.unwrap_or(0.0);
        if limit <= 0.0 {
            continue;
        }
        let Ok(remaining) = cursor_optional_number(spend_limit, remaining_key) else {
            return Vec::new();
        };
        let Ok(used) = cursor_optional_number(spend_limit, used_key) else {
            return Vec::new();
        };
        let used = used.unwrap_or_else(|| remaining.map_or(0.0, |left| (limit - left).max(0.0)));
        let id = "cursor:on-demand".to_owned();
        windows.push(CursorQuotaWindow {
            kind: classify_cursor_quota_window_kind(&id),
            id,
            label: Some("On-demand".to_owned()),
            percent_used: clamp_cursor_percent_used(Some((used / limit) * 100.0)),
            resets_at: resets_at.clone(),
            window_minutes,
            scope: "shared",
        });
        break;
    }
    windows
}
