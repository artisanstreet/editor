use artisan_domain::{
    EngineModelId, EngineRouteId, RootPath, RunId, RunUsageBasis, RunUsageReport,
    RunUsageReportInput, ThreadId, UnixMillis,
};
use serde_json::Value;

#[cfg(test)]
use super::protocol::request_line;
use super::protocol::{CodexSettings, CodexTokenUsageSample};

// ---------------------------------------------------------------------------
// X3: native continuation gate, resume, usage, and teardown contract
// ---------------------------------------------------------------------------

/// Minimum Codex CLI for native continuation.
///
/// The transport floor stays `0.142.5`; continuation additionally requires
/// `0.145.0`, mirroring `continuation_cli_version` in
/// `modules/engines/src/codex/protocol.ts` and the `native_continuation`
/// capability note in `modules/engines/src/codex/engine.ts`.
pub(crate) const CODEX_CONTINUATION_MINIMUM_CLI_VERSION: &str = "0.145.0";

/// Returns whether Codex teardown must terminate the whole process group.
///
/// Always true: the owner spawns Codex with whole-group custody (Job Object
/// on Windows), so teardown kills codex grandchildren that still hold pipes
/// instead of orphaning them. Unobserved reaps quarantine through the shared
/// `cleanup_after_abort` / `finish_turn_result` path.
#[cfg(test)]
pub(crate) const fn codex_requires_group_termination() -> bool {
    true
}

/// Compares two `X.Y.Z` CLI spellings by their numeric core.
///
/// A leading `v` and any trailing pre-release/build suffix are ignored, so
/// `0.145.0-alpha` compares equal to `0.145.0`. Returns `None` when either
/// side has no parseable triple; callers fail closed on `None`.
pub(crate) fn compare_codex_cli_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    Some(parse_cli_triple(left)?.cmp(&parse_cli_triple(right)?))
}

/// Returns whether a probed CLI version meets a minimum floor.
pub(crate) fn codex_cli_meets_minimum(version: &str, minimum: &str) -> bool {
    matches!(
        compare_codex_cli_versions(version, minimum),
        Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater)
    )
}

fn parse_cli_triple(text: &str) -> Option<[u64; 3]> {
    let start = text.find(|character: char| character.is_ascii_digit())?;
    let run: String = text[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    let mut parts = run.split('.');
    Some([
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ])
}

/// Native-continuation decision for one Codex turn.
///
/// `Compatible` authorizes `thread/resume` against the stored provider
/// thread; `Incompatible` carries the stable reason the dispatcher surfaces
/// instead of silently starting fresh or resuming across engines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexContinuationDecision {
    Compatible,
    Incompatible { reason: &'static str },
}

/// Bounded input for [`check_codex_native_continuation`].
pub(crate) struct CodexContinuationGateInput<'a> {
    /// Probed CLI version (`VerifiedCodexLaunch::version`).
    pub cli_version: &'a str,
    /// Explicit target model from the current selection; `None` fails closed.
    pub target_model: Option<&'a str>,
    /// Advertised models when a `model/list` inventory was read; `None`
    /// skips advertisement validation (live inventory is deferred) but never
    /// skips the explicit-model or CLI gates.
    pub advertised_models: Option<&'a [&'a str]>,
    /// Whether the stored binding names the same `codex` engine. The
    /// dispatcher scopes continuation reads to `EngineId::Codex`, so a false
    /// value fails closed instead of resuming across engines.
    pub same_engine: bool,
}

/// Gates one Codex native continuation without touching provider state.
///
/// Order is contractual: same-engine first, then the explicit target model
/// (pre-validated before resume), then the `0.145.0` CLI floor, then model
/// advertisement when an inventory is supplied. Any failure is a typed
/// incompatible — never a silent fresh start and never a cross-engine resume.
pub(crate) fn check_codex_native_continuation(
    input: &CodexContinuationGateInput<'_>,
) -> CodexContinuationDecision {
    if !input.same_engine {
        return CodexContinuationDecision::Incompatible {
            reason: "Codex native continuation cannot resume across engines",
        };
    }
    let Some(target) = input.target_model.filter(|model| !model.is_empty()) else {
        return CodexContinuationDecision::Incompatible {
            reason: "Codex native continuation requires an explicit target model",
        };
    };
    if !codex_cli_meets_minimum(input.cli_version, CODEX_CONTINUATION_MINIMUM_CLI_VERSION) {
        return CodexContinuationDecision::Incompatible {
            reason: "Codex native continuation requires CLI 0.145.0 or newer",
        };
    }
    if let Some(advertised) = input.advertised_models
        && !advertised.contains(&target)
    {
        return CodexContinuationDecision::Incompatible {
            reason: "Codex does not currently advertise the target model",
        };
    }
    CodexContinuationDecision::Compatible
}

/// Builds the `thread/resume` params for one authorized continuation.
///
/// Mirrors the TypeScript open path (`thread/resume` with the stored thread
/// id over the same thread options a fresh start would use): the resume
/// reopens provider-owned state only and never invents checkpoints. Returns
/// `None` when the stored thread id is outside its bounded route-segment
/// grammar so the caller fails closed instead of resuming a corrupt session.
pub(crate) fn thread_resume_params(
    settings: &CodexSettings,
    project_root: &RootPath,
    stored_thread_id: &str,
) -> Option<Value> {
    if stored_thread_id.is_empty() || stored_thread_id.len() > 256 {
        return None;
    }
    let mut params = settings.thread_params(project_root);
    let object = params.as_object_mut()?;
    object.insert(
        "threadId".to_owned(),
        Value::String(stored_thread_id.to_owned()),
    );
    Some(params)
}

/// Immutable attribution for one Codex usage report.
///
/// Model and thread come from the immutable launch snapshot; the provider
/// session is the authenticated native thread, never an envelope claim.
pub(crate) struct CodexUsageContext<'a> {
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
pub(crate) struct CodexUsageAttribution {
    pub thread_id: ThreadId,
    pub model_id: EngineModelId,
}

/// Borrowed usage scope for one pump loop.
#[expect(
    clippy::struct_field_names,
    reason = "fields mirror CodexUsageAttribution and the wire usage vocabulary; renaming would obscure the mapping"
)]
pub(crate) struct CodexUsageScope<'a> {
    pub thread_id: &'a ThreadId,
    pub model_id: &'a EngineModelId,
    pub provider_session_id: &'a str,
}

/// Builds the cumulative usage report for one sample.
///
/// Fails closed (`None`) when identities or bounds reject: usage is never
/// synthesized from partial identities. The context gauge always replaces
/// the previous report regardless of basis — the codec performs no
/// arithmetic at all. Codex names no provider route, so reports attribute to
/// the engine's own `codex` route namespace; rate-limit windows are never
/// copied here, so no quota is invented.
pub(crate) fn codex_usage_report(
    context: &CodexUsageContext<'_>,
    provider_turn_id: Option<String>,
    source_sequence: u64,
    sample: &CodexTokenUsageSample,
) -> Option<RunUsageReport> {
    let provider_route_id = EngineRouteId::parse("codex").ok()?;
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

pub(crate) fn current_unix_millis() -> Option<UnixMillis> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    let millis = i64::try_from(duration.as_millis()).ok()?;
    Some(UnixMillis::from_millis(millis))
}

/// Kind of one Codex rate-limit window, classified from
/// `windowDurationMins`.
///
/// Mirrors `classify_codex_quota_window_kind` in
/// `modules/engines/src/codex/usage.ts`: 300 minutes is a session window,
/// 10,080 a weekly window, 40,000–45,000 a monthly window; anything else
/// (including absent) is unknown rather than guessed.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexQuotaWindowKind {
    Session,
    Weekly,
    Monthly,
    Unknown,
}

/// Classifies one rate-limit window duration.
#[cfg(test)]
pub(crate) fn classify_codex_quota_window_kind(
    window_minutes: Option<u64>,
) -> CodexQuotaWindowKind {
    match window_minutes {
        Some(300) => CodexQuotaWindowKind::Session,
        Some(10_080) => CodexQuotaWindowKind::Weekly,
        Some(minutes) if (40_000..=45_000).contains(&minutes) => CodexQuotaWindowKind::Monthly,
        _ => CodexQuotaWindowKind::Unknown,
    }
}

/// Clamps one `usedPercent` reading into `0..=100`.
///
/// Absent or non-finite readings become `0`: usage display never blocks on a
/// corrupt gauge and never invents quota from it.
#[cfg(test)]
pub(crate) fn clamp_codex_percent_used(used_percent: Option<f64>) -> f64 {
    match used_percent {
        Some(value) if value.is_finite() => value.clamp(0.0, 100.0),
        _ => 0.0,
    }
}

/// Formats whole `resetsAt` provider seconds as an ISO-8601 UTC instant.
///
/// Sub-second precision is truncated: rate-limit resets arrive as whole
/// provider seconds and the reset instant is display-only diagnostics, never
/// turn input. Computed without a date library so the owner keeps no new
/// dependency for one diagnostic string.
#[cfg(test)]
pub(crate) fn codex_reset_at_iso(resets_at_secs: u64) -> String {
    let days = i64::try_from(resets_at_secs / 86_400).unwrap_or(i64::MAX);
    let clock = i64::try_from(resets_at_secs % 86_400).unwrap_or(0);
    let shifted = days + 719_468;
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
    format!(
        "{display_year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        clock / 3_600,
        (clock % 3_600) / 60,
        clock % 60
    )
}

/// One provider-neutral Codex quota window: diagnostics only, never quota.
///
/// Rate-limit windows are read through the non-billable
/// `account/rateLimits/read` surface and classified here; they are never
/// copied into [`RunUsageReport`] and never gate a turn.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CodexQuotaWindow {
    pub id: String,
    pub kind: CodexQuotaWindowKind,
    pub label: Option<String>,
    pub percent_used: f64,
    pub resets_at: Option<String>,
    pub window_minutes: Option<u64>,
    /// `"model"` when the bucket names a model limit, else `"unknown"`.
    /// Mirrors the TypeScript scope rule without inventing quota attribution.
    pub scope: &'static str,
}

/// Maps a decoded `account/rateLimits/read` result to provider-neutral quota
/// windows.
///
/// Iterates `rateLimitsByLimitId` when present, else falls back to the single
/// `rateLimits` snapshot; emits one window per non-null
/// `primary`/`secondary` slot in bucket-then-slot order. Malformed input
/// yields no windows: collection is best-effort diagnostics and never blocks
/// a turn.
#[cfg(test)]
pub(crate) fn map_codex_rate_limit_windows(result: &Value) -> Vec<CodexQuotaWindow> {
    let mut buckets: Vec<(String, Value)> = Vec::new();
    if let Some(map) = result.get("rateLimitsByLimitId").and_then(Value::as_object) {
        for (bucket_id, snapshot) in map {
            buckets.push((bucket_id.clone(), snapshot.clone()));
        }
    } else if let Some(snapshot) = result.get("rateLimits") {
        let bucket_id = snapshot
            .get("limitId")
            .and_then(Value::as_str)
            .unwrap_or("codex")
            .to_owned();
        buckets.push((bucket_id, snapshot.clone()));
    }
    let mut windows = Vec::new();
    for (bucket_id, snapshot) in &buckets {
        let label = snapshot
            .get("limitName")
            .and_then(Value::as_str)
            .map(str::to_owned);
        for slot in ["primary", "secondary"] {
            let Some(window) = snapshot.get(slot) else {
                continue;
            };
            if window.is_null() {
                continue;
            }
            let Some(object) = window.as_object() else {
                continue;
            };
            let minutes = object.get("windowDurationMins").and_then(Value::as_u64);
            windows.push(CodexQuotaWindow {
                id: format!("{bucket_id}:{slot}"),
                kind: classify_codex_quota_window_kind(minutes),
                label: label.clone(),
                percent_used: clamp_codex_percent_used(
                    object.get("usedPercent").and_then(Value::as_f64),
                ),
                resets_at: object
                    .get("resetsAt")
                    .and_then(Value::as_u64)
                    .map(codex_reset_at_iso),
                window_minutes: minutes,
                scope: if bucket_id == "codex" || label.is_none() {
                    "unknown"
                } else {
                    "model"
                },
            });
        }
    }
    windows
}

/// Builds the non-billable `account/read` request line for usage collection.
///
/// Usage reads travel the same owned app-server session as the turn but never
/// start a run: they authenticate and classify quota without provider
/// effects.
#[cfg(test)]
pub(crate) fn codex_account_read_line(id: u64) -> String {
    request_line(id, "account/read", &Value::Object(serde_json::Map::new()))
}

/// Builds the non-billable `account/rateLimits/read` request line for usage
/// collection. Same non-billable contract as [`codex_account_read_line`].
#[cfg(test)]
pub(crate) fn codex_rate_limits_read_line(id: u64) -> String {
    request_line(
        id,
        "account/rateLimits/read",
        &Value::Object(serde_json::Map::new()),
    )
}
