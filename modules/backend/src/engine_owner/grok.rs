//! Finite G1 Grok runtime definition on the shared ACP core.
//!
//! This leaf owns the Grok-specific interpretation of the ACP transport core
//! (`super::acp`) plus the A2 bridges (`super::acp_bridges`): typed
//! [`GrokSettings`] derived from the durable [`GrokSelection`], the launch
//! args mapping with plan-mode forcing for read-only policies, the
//! definition-row accessor over the existing [`GROK_ACP`](super::acp::GROK_ACP)
//! row (args builder, version parser, auth classifier, embedded image mode),
//! the pending-launch capability ([`GrokLaunch`]), and the G1 command policy
//! (steer-while-active rejection, gated native continuation, best-effort
//! provider usage, no provider-owned startup classifier).
//!
//! Behavior mirrors `modules/engines/src/grok/engine.ts` over the shared
//! `MakeAcpEngine` core: default executable `"grok"`, `--no-auto-update`
//! first, optional `--model` / `--reasoning-effort`, plan mode when the
//! canonical policy denies writes, `auto` / `always-approve` permission
//! mapping, then `agent stdio`; `xai.api_key` when `XAI_API_KEY` is present
//! else `cached_token`; embedded `artisan://attachment` image blocks; a
//! waiting session accepts a follow-up as a new prompt while an active prompt
//! cannot be steered in place (`EngineUnsupportedCommandError` in
//! TypeScript).
//!
//! G3 notes: continuation resumes provider-owned state only (never invented
//! checkpoints) through `session/load` behind
//! [`check_grok_native_continuation`] (same engine, explicit target model
//! pre-validated before resume, recorded CLI version gate); usage is
//! collected best-effort alongside the turn (per-round ACP counters project
//! to [`RunUsageBasis::Delta`] [`RunUsageReport`] rows, quota-budget windows
//! classify into diagnostics with clamped percents and kind classification)
//! and never blocks it; teardown terminates the whole process group so no
//! grok grandchild holding a pipe is orphaned; interrupted runs replay the
//! durable prefix on resume without duplicating provider effects.
//!
//! Explicit non-goals for G3: catalog support (the catalog leaf stays
//! OpenCode2-only), frontend selection, a verified-launch authority in
//! `native_engine` (the dispatcher probes with the existing discovery plus
//! version parse and seats the resolved path here), a TypeScript grok
//! quota/budget surface to mirror exactly (`modules/engines/src/grok/`
//! carries only `engine.ts`, so quota shapes mirror the X3 codex precedent),
//! live model inventory (advertisement is enforced only when supplied),
//! streaming text projection onto the S1a vocabulary (a later packet), and
//! live answer delivery into the provider session (same as X1: tracked,
//! never auto-answered).
//!
//! Native continuation stays same-model-only: ACP `session/load` reopens the
//! stored conversation and the follow-up prompt reuses the launch's selected
//! model, so a loaded session never changes model identity and a
//! cross-engine resume never proceeds.
//!
//! [`GrokSelection`]: artisan_domain::GrokSelection

#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};

use artisan_domain::{
    EngineModelId, EngineProfileId, EngineRouteId, FilesystemAccess, GrokSelection, RunId,
    RunUsageBasis, RunUsageReport, RunUsageReportInput, ThreadId, UnixMillis,
};
#[cfg(test)]
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;

use super::acp::{AcpDefinition, GROK_ACP, LaunchArgs, TokenUsage};
use super::observation::{EngineObservation, TerminalState, UsageObservation};

/// Structural ceiling for one resumed Grok session identity, mirroring the
/// owner continuation bound the ACP core already enforces.
pub(crate) const GROK_MAX_SESSION_ID_BYTES: usize = 256;

/// Ceiling for tracked permission/elicitation requests on one live Grok turn.
/// Matches the per-frame question ceiling the sibling runtimes use.
pub(crate) const GROK_MAX_PENDING_REQUESTS: usize = 32;

/// Agent method carrying one Grok permission request on the shared wire,
/// mirroring the TypeScript `session/requestPermission` evidence.
pub(crate) const GROK_PERMISSION_METHOD: &str = "session/requestPermission";

/// Agent method carrying one Grok elicitation request on the shared wire,
/// mirroring the TypeScript `elicitation/create` evidence.
pub(crate) const GROK_ELICITATION_METHOD: &str = "elicitation/create";

/// Typed Grok settings derived from the durable selection.
///
/// Mirrors `GrokAcpArgs` in `modules/engines/src/grok/engine.ts`: the model
/// and reasoning effort pass through verbatim when selected, the permission
/// mode passes through as its adapter spelling, and write access derives
/// from the canonical filesystem policy (anything but `None` may write, so
/// only `None` forces plan mode at argument-building time). The adapter
/// imposes no construction-time permission rejections beyond the typed
/// options, so construction here is infallible.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GrokSettings {
    profile_id: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    permission_mode: Option<String>,
    write_access: bool,
}

impl GrokSettings {
    /// Derives typed ACP settings from the durable selection.
    #[must_use = "settings must reach the definition row"]
    pub(crate) fn from_selection(selection: &GrokSelection) -> Self {
        Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model: selection.model_id().map(|model| model.as_str().to_owned()),
            reasoning_effort: selection
                .reasoning_effort()
                .map(|effort| effort.as_str().to_owned()),
            permission_mode: selection
                .permission_mode()
                .map(|mode| mode.as_str().to_owned()),
            write_access: selection.permission().filesystem() != FilesystemAccess::None,
        }
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Builds the explicit launch params the row's arg builder interprets.
    /// Cursor-only speed forcing stays off; the row maps the stored
    /// permission spelling and forces plan mode when writes are denied.
    #[must_use = "launch args must reach the spawn call"]
    pub(crate) fn launch_args(&self) -> LaunchArgs {
        LaunchArgs {
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            speed_fast: false,
            permission: self.permission_mode.clone(),
            write_access: self.write_access,
        }
    }

    /// Returns the shared-core definition row for Grok Build: `grok`
    /// executable, embedded image blocks, the TypeScript version and auth
    /// classifiers, and the arg builder above.
    #[must_use = "definition rows must drive the dispatch arm"]
    pub(crate) fn definition() -> AcpDefinition {
        GROK_ACP
    }
}

/// Pending Grok launch capability seated by the dispatcher probe.
///
/// Carries the resolved executable path, the exact selected profile, and the
/// probed CLI version. There is no verified-launch authority for Grok yet:
/// the dispatcher certifies the path as a regular file plus a parsed
/// `--version` at claim time, and the dispatch arm rechecks the profile
/// fence before spawn. Revalidation at spawn time is a later packet.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct GrokLaunch {
    executable: PathBuf,
    profile_id: EngineProfileId,
    version: String,
}

impl GrokLaunch {
    /// Seats a probe-certified launch capability.
    #[must_use = "launch capabilities must reach the owner queue"]
    pub(crate) fn new(executable: PathBuf, profile_id: EngineProfileId, version: String) -> Self {
        Self {
            executable,
            profile_id,
            version,
        }
    }

    /// Returns the exact selected profile identity.
    #[must_use]
    pub(crate) fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }

    /// Returns the resolved executable path.
    #[must_use]
    pub(crate) fn executable_path(&self) -> &Path {
        &self.executable
    }

    /// Returns the probed CLI version.
    #[must_use]
    pub(crate) fn version(&self) -> &str {
        &self.version
    }
}

impl std::fmt::Debug for GrokLaunch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GrokLaunch { <redacted> }")
    }
}

/// Returns whether ambient `XAI_API_KEY` credentials are present for the
/// row's auth classifier, mirroring the TypeScript `AuthMethod` evidence
/// reading `process.env`. The value itself is never read here.
#[must_use = "auth presence must gate the handshake"]
pub(crate) fn api_key_present() -> bool {
    std::env::var_os(artisan_native_engine::grok::XAI_API_KEY_ENV)
        .is_some_and(|value| !value.is_empty())
}

/// Typed, payload-free failure for Grok commands outside the G1 vocabulary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[allow(dead_code)]
pub(crate) enum GrokCommandError {
    /// A steer arrived while a prompt was active. Waiting-only follow-up is
    /// a new prompt, never a fake in-place steer.
    #[error("grok command `{command}` is unsupported while a prompt is active")]
    UnsupportedCommand { command: &'static str },
}

/// How a waiting Grok session accepts follow-up text: exactly one new
/// prompt round, mirroring the TypeScript waiting-only branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum GrokFollowUp {
    /// Start a new prompt round on the waiting session.
    NewPrompt,
}

/// Decides whether follow-up text may be sent.
///
/// An active prompt rejects with typed [`GrokCommandError::UnsupportedCommand`];
/// a waiting session accepts exactly [`GrokFollowUp::NewPrompt`]. Live steer
/// delivery wiring is a later packet; this decision is exercised by tests.
///
/// # Errors
///
/// Returns [`GrokCommandError::UnsupportedCommand`] when `prompt_active`.
#[allow(dead_code)]
pub(crate) fn follow_up(prompt_active: bool) -> Result<GrokFollowUp, GrokCommandError> {
    if prompt_active {
        return Err(GrokCommandError::UnsupportedCommand { command: "steer" });
    }
    Ok(GrokFollowUp::NewPrompt)
}

// ---------------------------------------------------------------------------
// G3: native continuation gate, resume, usage, and teardown contract
// ---------------------------------------------------------------------------

/// Returns whether Grok teardown must terminate the whole process group.
///
/// Always true: the owner spawns Grok with whole-group custody (Job Object
/// on Windows via [`super::acp::spawn_acp_child`]), so teardown kills grok
/// grandchildren that still hold pipes instead of orphaning them. An
/// unobserved ACP reap surfaces as `UnresolvedReapDuring` through the
/// executor's finish path (the retained ACP handle carries no observable
/// wait and drops rather than quarantining the owner).
#[cfg(test)]
pub(crate) const fn grok_requires_group_termination() -> bool {
    true
}

/// Compares two `X.Y.Z` CLI spellings by their numeric core.
///
/// A leading name and any trailing pre-release/build suffix are ignored, so
/// `grok 1.2.3-beta.1` compares equal to `1.2.3`. Returns `None` when either
/// side has no parseable triple; callers fail closed on `None`.
#[cfg(test)]
pub(crate) fn compare_grok_cli_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    Some(parse_cli_triple(left)?.cmp(&parse_cli_triple(right)?))
}

#[cfg(test)]
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

/// Returns whether a recorded CLI version is present and parseable.
///
/// Grok carries no TypeScript continuation floor (the dispatcher seats any
/// version the shared ACP row parses): the gate requires the recorded probe
/// version seated in [`GrokLaunch`] — the bare triple the row extracts, such
/// as `"1.2.3"` — to round-trip through that same row parser instead of
/// inventing a minimum. Empty and foreign-engine spellings fail closed.
pub(crate) fn grok_cli_version_recorded(version: &str) -> bool {
    super::acp::parse_grok_version(&format!("grok {version}")).is_some()
}

/// Native-continuation decision for one Grok turn.
///
/// `Compatible` authorizes `session/load` against the stored provider
/// conversation; `Incompatible` carries the stable reason the dispatcher
/// surfaces instead of silently starting fresh or resuming across engines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GrokContinuationDecision {
    Compatible,
    Incompatible { reason: &'static str },
}

/// Bounded input for [`check_grok_native_continuation`].
pub(crate) struct GrokContinuationGateInput<'a> {
    /// Recorded CLI version (`GrokLaunch::version`).
    pub cli_version: &'a str,
    /// Explicit target model from the current selection; `None` fails closed.
    pub target_model: Option<&'a str>,
    /// Advertised models when a `model/list` inventory was read; `None`
    /// skips advertisement validation (live inventory is deferred) but never
    /// skips the explicit-model or CLI gates.
    pub advertised_models: Option<&'a [&'a str]>,
    /// Whether the stored binding names the same `grok` engine. The
    /// dispatcher scopes continuation reads to `EngineId::Grok`, so a false
    /// value fails closed instead of resuming across engines.
    pub same_engine: bool,
}

/// Gates one Grok native continuation without touching provider state.
///
/// Order is contractual: same-engine first, then the explicit target model
/// (pre-validated before resume), then the recorded CLI version gate, then
/// model advertisement when an inventory is supplied. Any failure is a typed
/// incompatible — never a silent fresh start and never a cross-engine resume.
pub(crate) fn check_grok_native_continuation(
    input: &GrokContinuationGateInput<'_>,
) -> GrokContinuationDecision {
    if !input.same_engine {
        return GrokContinuationDecision::Incompatible {
            reason: "Grok native continuation cannot resume across engines",
        };
    }
    let Some(target) = input.target_model.filter(|model| !model.is_empty()) else {
        return GrokContinuationDecision::Incompatible {
            reason: "Grok native continuation requires an explicit target model",
        };
    };
    if !grok_cli_version_recorded(input.cli_version) {
        return GrokContinuationDecision::Incompatible {
            reason: "Grok native continuation requires a recorded CLI version",
        };
    }
    if let Some(advertised) = input.advertised_models
        && !advertised.contains(&target)
    {
        return GrokContinuationDecision::Incompatible {
            reason: "Grok does not currently advertise the target model",
        };
    }
    GrokContinuationDecision::Compatible
}

/// Validates one stored Grok conversation identity for `session/load`.
///
/// The resume reopens provider-owned state only and never invents
/// checkpoints. Returns `None` when the stored id is outside the bounded
/// session grammar so the caller fails closed instead of resuming a corrupt
/// session; the transport then reopens exactly this id.
pub(crate) fn grok_resume_session_id(stored_session_id: &str) -> Option<String> {
    if stored_session_id.is_empty() || stored_session_id.len() > GROK_MAX_SESSION_ID_BYTES {
        return None;
    }
    Some(stored_session_id.to_owned())
}

/// Returns whether a loaded ACP session reopened the stored conversation.
///
/// `session/load` reopens provider-owned state only: the prepared identity
/// must equal the stored one, so resume reopens the same conversation id
/// and a restart replays the durable prefix without duplicating provider
/// effects.
pub(crate) fn grok_loaded_session_is_stored(loaded: &str, stored: &str) -> bool {
    !loaded.is_empty() && loaded == stored
}

/// Per-round usage sample from one ACP prompt result.
///
/// Mirrors the provider disclosure: the ACP `usage` object carries the
/// counters for exactly the prompt round that just settled, so reports use
/// the [`RunUsageBasis::Delta`] basis. The agent discloses no context gauge
/// and no turn identity, so neither is synthesized.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct GrokUsageSample {
    pub input: Option<u64>,
    pub cached_input: Option<u64>,
    pub output: Option<u64>,
}

/// Extracts the per-round sample from one ACP [`TokenUsage`].
///
/// A zero round (no counters) is not a report (`None`): an empty
/// measurement stays a diagnostic and never becomes usage.
pub(crate) fn grok_sample_from_token_usage(usage: &TokenUsage) -> Option<GrokUsageSample> {
    if usage.input == 0 && usage.output == 0 && usage.cached_input.unwrap_or(0) == 0 {
        return None;
    }
    Some(GrokUsageSample {
        input: Some(usage.input),
        cached_input: usage.cached_input,
        output: Some(usage.output),
    })
}

/// Immutable attribution for one Grok usage report.
///
/// Model and thread come from the immutable launch snapshot; the provider
/// session is the authenticated native conversation, never an envelope claim.
pub(crate) struct GrokUsageContext<'a> {
    pub run_id: &'a RunId,
    pub thread_id: &'a ThreadId,
    pub provider_session_id: &'a str,
    pub model_id: &'a EngineModelId,
    pub observed_at: UnixMillis,
}

/// Best-effort usage scope carried beside the update channel.
///
/// `None` (no explicit model or no thread scope) skips usage projection
/// without disturbing the turn: usage never blocks turns.
#[derive(Clone, Debug)]
pub(crate) struct GrokUsageAttribution {
    pub thread_id: ThreadId,
    pub model_id: EngineModelId,
}

/// Borrowed usage scope for one pump loop.
#[expect(
    clippy::struct_field_names,
    reason = "fields mirror GrokUsageAttribution and the wire usage vocabulary; renaming would obscure the mapping"
)]
pub(crate) struct GrokUsageScope<'a> {
    pub thread_id: &'a ThreadId,
    pub model_id: &'a EngineModelId,
    pub provider_session_id: &'a str,
}

/// Builds the per-round usage report for one sample.
///
/// Fails closed (`None`) when identities or bounds reject: usage is never
/// synthesized from partial identities. Grok names no provider route, so
/// reports attribute to the engine's own `grok` route namespace; quota
/// windows are never copied here, so no quota is invented. Grok discloses
/// no provider turn identity on usage results, so none is attributed rather
/// than synthesized.
pub(crate) fn grok_usage_report(
    context: &GrokUsageContext<'_>,
    source_sequence: u64,
    sample: &GrokUsageSample,
) -> Option<RunUsageReport> {
    let provider_route_id = EngineRouteId::parse("grok").ok()?;
    RunUsageReport::new(RunUsageReportInput {
        run_id: context.run_id.clone(),
        thread_id: context.thread_id.clone(),
        provider_session_id: context.provider_session_id.to_owned(),
        source_sequence,
        model_id: context.model_id.clone(),
        provider_route_id,
        variant_id: None,
        basis: RunUsageBasis::Delta,
        provider_turn_id: None,
        input_tokens: sample.input,
        cached_input_tokens: sample.cached_input,
        output_tokens: sample.output,
        context_tokens: None,
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
pub(crate) async fn project_grok_usage_sample(
    observations: &mpsc::Sender<EngineObservation>,
    run_id: &RunId,
    scope: Option<&GrokUsageScope<'_>>,
    source_sequence: u64,
    sample: &GrokUsageSample,
) -> Option<TerminalState> {
    let scope = scope?;
    let observed_at = current_unix_millis()?;
    let report = grok_usage_report(
        &GrokUsageContext {
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

/// Kind of one Grok quota-budget window, classified from
/// `windowDurationMins`.
///
/// Mirrors the X3 codex precedent (`modules/engines/src/codex/usage.ts`):
/// `modules/engines/src/grok/` carries only `engine.ts` with no
/// quota/budget surface, so 300 minutes is a session window, 10,080 a
/// weekly window, 40,000–45,000 a monthly window; anything else (including
/// absent) is unknown rather than guessed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(test)]
pub(crate) enum GrokQuotaWindowKind {
    Session,
    Weekly,
    Monthly,
    Unknown,
}

/// Classifies one quota-budget window duration.
#[cfg(test)]
pub(crate) fn classify_grok_quota_window_kind(window_minutes: Option<u64>) -> GrokQuotaWindowKind {
    match window_minutes {
        Some(300) => GrokQuotaWindowKind::Session,
        Some(10_080) => GrokQuotaWindowKind::Weekly,
        Some(minutes) if (40_000..=45_000).contains(&minutes) => GrokQuotaWindowKind::Monthly,
        _ => GrokQuotaWindowKind::Unknown,
    }
}

/// Clamps one `usedPercent` reading into `0..=100`.
///
/// Absent or non-finite readings become `0`: usage display never blocks on a
/// corrupt gauge and never invents quota from it.
#[cfg(test)]
pub(crate) fn clamp_grok_percent_used(used_percent: Option<f64>) -> f64 {
    match used_percent {
        Some(value) if value.is_finite() => value.clamp(0.0, 100.0),
        _ => 0.0,
    }
}

/// Formats whole `resetsAt` provider seconds as an ISO-8601 UTC instant.
///
/// Sub-second precision is truncated: budget resets arrive as whole provider
/// seconds and the reset instant is display-only diagnostics, never turn
/// input. Computed without a date library so the owner keeps no new
/// dependency for one diagnostic string.
#[cfg(test)]
pub(crate) fn grok_reset_at_iso(resets_at_secs: u64) -> String {
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

/// One provider-neutral Grok quota-budget window: diagnostics only, never quota.
///
/// Budget windows are read through a non-billable surface and classified
/// here; they are never copied into [`RunUsageReport`] and never gate a turn.
#[derive(Clone, Debug, PartialEq)]
#[cfg(test)]
pub(crate) struct GrokQuotaWindow {
    pub id: String,
    pub kind: GrokQuotaWindowKind,
    pub label: Option<String>,
    pub percent_used: f64,
    pub resets_at: Option<String>,
    pub window_minutes: Option<u64>,
    /// `"model"` when the bucket names a model limit, else `"unknown"`.
    /// Mirrors the X3 scope rule without inventing quota attribution.
    pub scope: &'static str,
}

/// Maps a decoded quota-budget result to provider-neutral windows.
///
/// Iterates `rateLimitsByLimitId` when present, else falls back to the single
/// `rateLimits` snapshot; emits one window per non-null
/// `primary`/`secondary` slot in bucket-then-slot order. Malformed input
/// yields no windows: collection is best-effort diagnostics and never blocks
/// a turn.
#[cfg(test)]
pub(crate) fn map_grok_quota_windows(result: &Value) -> Vec<GrokQuotaWindow> {
    let mut buckets: Vec<(String, Value)> = Vec::new();
    if let Some(map) = result.get("rateLimitsByLimitId").and_then(Value::as_object) {
        for (bucket_id, snapshot) in map {
            buckets.push((bucket_id.clone(), snapshot.clone()));
        }
    } else if let Some(snapshot) = result.get("rateLimits") {
        let bucket_id = snapshot
            .get("limitId")
            .and_then(Value::as_str)
            .unwrap_or("grok")
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
            windows.push(GrokQuotaWindow {
                id: format!("{bucket_id}:{slot}"),
                kind: classify_grok_quota_window_kind(minutes),
                label: label.clone(),
                percent_used: clamp_grok_percent_used(
                    object.get("usedPercent").and_then(Value::as_f64),
                ),
                resets_at: object
                    .get("resetsAt")
                    .and_then(Value::as_u64)
                    .map(grok_reset_at_iso),
                window_minutes: minutes,
                scope: if bucket_id == "grok" || label.is_none() {
                    "unknown"
                } else {
                    "model"
                },
            });
        }
    }
    windows
}

/// Classifies a bounded provider diagnostic emitted while Grok ACP is
/// becoming ready.
///
/// The TypeScript Grok definition carries no `ClassifyStartupFailure`, so
/// this always declines: the dispatch arm maps startup failures onto the
/// generic owner vocabulary instead of inventing provider-owned cases.
#[allow(dead_code)]
pub(crate) fn classify_startup_failure(
    _operation: &'static str,
    _stderr_tail: &str,
) -> Option<super::operation::EngineOperationError> {
    None
}
