//! Finite C1 Cursor runtime on the ACP core: definition plus dispatch arms,
//! not runnable yet.
//!
//! Spawns nothing here. This leaf owns the native Cursor definition row over
//! the shared A1 transport core plus the A2 bridges: typed [`CursorSettings`]
//! derived from the durable [`CursorSelection`](artisan_domain::CursorSelection),
//! the model resolver (effort appended unless suffixed, `-fast` handling,
//! bracket passthrough), the args builder (`--mode ask` on read-only,
//! `--force` mapping, then `acp`), the `AE-PROVIDER-206` startup classifier
//! with model capture, the `cursor/ask_question` and `cursor/create_plan`
//! plan-approval extension mapping, and image-block mode. It extends the ACP
//! core with the cursor definition row and never forks it: spawning,
//! framing, handshake, session, update-loop, and teardown behavior stay in
//! `super::acp`, and permission/elicitation bridging stays in
//! `super::acp_bridges`.
//!
//! One `EngineOwner` task and queue stays the authority for admission,
//! custody, and quarantine; the dispatch arm in `super::operation` and the
//! dispatcher branch in `crate::native_run_dispatch` add only new match arms
//! beside the X1 Codex arm. The actual `cursor-agent` CLI is the only
//! executable this packet names; no fixture binary and no raw JSON cross into
//! the domain.
//!
//! Explicit non-goals for C3: catalog flag flip, frontend selection, the live
//! dashboard usage read and model inventory, the probe/authority launch, and
//! any engine beyond the cursor row.
//!
//! Later packet: cursor-account catalog merge design (recorded, not
//! implemented).
//!
//! The curated Cursor catalog stays unread in C1. A later packet merges the
//! authenticated dashboard account surface (`MakeCursorUsage` in
//! `modules/engines/src/cursor/usage.ts`) with the curated model list the way
//! `crate::native_model_catalog` merges the OpenCode2 runtime result: only
//! rows disclosed for this account become runnable, static rows for other
//! harnesses stay readable but unavailable to new policy admission, and no
//! thinking, speed, cost, or image-input value is inferred when the provider
//! did not report it. Usage stays non-billable and never starts a run.
//!
//! C3 notes: continuation resumes provider-owned state only (never invented
//! checkpoints) through `session/load` behind
//! [`check_cursor_native_continuation`]; usage is collected best-effort
//! alongside the turn (prompt-result and `usage_update` frames project to
//! cumulative [`RunUsageReport`] rows per the TypeScript ACP disclosure,
//! dashboard quota windows classify into diagnostics) and never blocks it;
//! teardown terminates the whole process group so no cursor grandchild
//! holding a pipe is orphaned; interrupted runs replay the durable prefix on
//! resume without duplicating provider effects.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]
// C3 wires the continuation gate into dispatch and the owner fence; the live
// pump, dashboard read, and catalog merge still belong to later packets, so
// the usage projectors await their first live caller. The finite dispatch
// arm fails closed before spawning, and the fixture tests in this module
// plus `tests/backend/engine_owner_cursor.rs` prove the wire shape.
// Every item is covered by those tests.
#![allow(dead_code)]

use std::ffi::OsString;

use artisan_domain::{
    ApprovalRequest, CursorPermissionMode, CursorSelection, CursorSpeed, EngineModelId,
    EngineRouteId, FilesystemAccess, ObservationId, PlanEntry, PlanEntryStatus, QuestionInput,
    QuestionOption, RunId, RunUsageBasis, RunUsageReport, RunUsageReportInput, ThreadId,
    UnixMillis,
};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;

use super::acp::{AcpDefinition, CURSOR_ACP, ImageMode, LaunchArgs, cursor_build_args};
use super::observation::{EngineObservation, TerminalState, UsageObservation};

/// Engine id carried by the C1 cursor definition row.
pub(crate) const CURSOR_ENGINE_ID: &str = "cursor";

/// Sentinel version carried by [`CursorLaunch`] until the probe/authority
/// packet lands. Preflight, catalog, and turn paths reject the cursor launch
/// before any version gate reads it; the value never authorizes execution.
pub(crate) const CURSOR_C1_UNPROBED_VERSION: &str = "0.0.0-cursor-c1-unprobed";

/// Maximum accepted cursor session identity bytes (mirrors the ACP row bound
/// the transport enforces for `session/load`).
pub(crate) const CURSOR_MAX_SESSION_ID_BYTES: usize = 256;

/// Payload-free failure of the cursor definition boundary.
///
/// Transport mistakes (spawn, handshake, prompt, stream, stall, cancel,
/// shutdown, deadline, interruption, exit) surface as the owner
/// [`EngineOperationError`](super::operation::EngineOperationError) at the
/// dispatch arm; only typed-boundary mistakes originate here.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub(crate) enum CursorTurnError {
    /// The durable selection or a provider frame violates the cursor adapter
    /// contract. The turn fails closed instead of seating a wrong session.
    #[error("cursor turn misconfigured")]
    Configuration,
    /// A stdio write over the ACP transport failed.
    #[error("cursor stdio write failed")]
    StreamFailed,
    /// A steer was issued while a prompt round is active. A waiting ACP
    /// session accepts a follow-up; an active prompt cannot be steered in
    /// place, mirroring `EngineUnsupportedCommandError` for `steer`.
    #[error("cursor steer unsupported while a prompt is active")]
    UnsupportedCommand,
}

/// Typed cursor settings derived from the durable selection.
///
/// Mirrors `CursorAcpArgs`/`ResolveCursorModel` in
/// `modules/engines/src/cursor/engine.ts` through the shared [`LaunchArgs`]
/// shape: the model stays optional (the adapter omits `--model` when no model
/// is selected), a read-only canonical policy maps to `--mode ask` at
/// argument-building time, and only the `force` permission mode maps to a CLI
/// flag. Effort/speed suffix resolution lives in the shared
/// [`cursor_build_args`] row builder; the durable selection keeps the raw
/// choices.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorSettings {
    profile_id: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    speed_fast: bool,
    permission_force: bool,
    write_access: bool,
}

impl CursorSettings {
    /// Derives typed ACP settings from the durable selection.
    ///
    /// The cursor adapter imposes no construction-time permission rejections
    /// beyond the typed options: read-only maps to ask mode, never to a
    /// refusal.
    ///
    /// # Errors
    ///
    /// Returns [`CursorTurnError::Configuration`] only when a selected value
    /// cannot be represented for the CLI. The typed selection already bounds
    /// every field, so well-formed selections always succeed.
    pub(crate) fn from_selection(selection: &CursorSelection) -> Result<Self, CursorTurnError> {
        Ok(Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model: selection.model_id().map(|model| model.as_str().to_owned()),
            reasoning_effort: selection
                .reasoning_effort()
                .map(|effort| effort.as_str().to_owned()),
            speed_fast: selection.speed() == Some(CursorSpeed::Fast),
            permission_force: selection.permission_mode() == Some(CursorPermissionMode::Force),
            write_access: selection.permission().filesystem() != FilesystemAccess::None,
        })
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Projects these settings onto the shared ACP row args.
    #[must_use]
    pub(crate) fn launch_args(&self) -> LaunchArgs {
        LaunchArgs {
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            speed_fast: self.speed_fast,
            permission: if self.permission_force {
                Some("force".to_owned())
            } else {
                None
            },
            write_access: self.write_access,
        }
    }

    /// Builds the exact cursor stdio argv tail for one turn: optional
    /// resolved `--model`, ask mode on read-only, `--force` mapping, then
    /// `acp`.
    #[must_use]
    pub(crate) fn build_args(&self) -> Vec<OsString> {
        cursor_build_args(&self.launch_args())
    }

    /// Returns the shared cursor ACP definition row: platform executable,
    /// native image blocks, `status` auth probe, `cursor_login` selection.
    #[must_use]
    pub(crate) fn definition() -> &'static AcpDefinition {
        &CURSOR_ACP
    }

    /// Returns how this row carries image payloads: native image blocks,
    /// never embedded resources and never dropped.
    #[must_use]
    pub(crate) const fn image_mode() -> ImageMode {
        ImageMode::Image
    }
}

/// Finite C1 cursor launch capability.
///
/// Carries the managed profile identity plus the unprobed version sentinel
/// until the probe/authority packet lands. The dispatcher never constructs
/// this from a live probe in C1, and the dispatch arm fails closed before
/// spawning; the value exists so admission, binding (`cursor`, format 1), and
/// quarantine plumbing prove their cursor arms without claiming a runnable
/// engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorLaunch {
    profile_id: String,
    version: String,
}

impl CursorLaunch {
    /// Creates an unprobed launch for exactly one managed profile.
    #[must_use]
    pub(crate) fn unprobed(profile_id: String) -> Self {
        Self {
            profile_id,
            version: CURSOR_C1_UNPROBED_VERSION.to_owned(),
        }
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Returns the version string (the C1 sentinel until probed).
    #[must_use]
    pub(crate) fn version(&self) -> &str {
        &self.version
    }
}

/// Rejects a steer issued while a prompt round is active with the typed
/// unsupported-command failure, mirroring the shared ACP `Send` gate: a
/// waiting session accepts a follow-up, an active prompt cannot be steered in
/// place.
///
/// # Errors
///
/// Returns [`CursorTurnError::UnsupportedCommand`] when `prompt_active` is
/// set.
pub(crate) const fn check_cursor_steer(prompt_active: bool) -> Result<(), CursorTurnError> {
    if prompt_active {
        return Err(CursorTurnError::UnsupportedCommand);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// C3: native continuation gate, resume, usage, and teardown contract
// ---------------------------------------------------------------------------

/// Minimum cursor CLI for native continuation: the certified agent release.
///
/// Mirrors `cursor_certified_version` in
/// `modules/engines/src/toolchain/distribution.ts`. The gate re-checks this
/// recorded constant against the probed launch version so a stale capability
/// can never authorize a resume the installed CLI no longer honors. Cursor
/// versions are dated (`YYYY.M.D-suffix`); comparison uses their numeric
/// core.
pub(crate) const CURSOR_CONTINUATION_MINIMUM_CLI_VERSION: &str = "2026.08.11-e8db854";

/// Returns whether cursor teardown must terminate the whole process group.
///
/// Always true: the cursor turn spawns through
/// [`spawn_acp_child`](super::acp::spawn_acp_child) with whole-group custody
/// (Job Object on Windows), so teardown kills cursor grandchildren that
/// still hold pipes instead of orphaning them. Unobserved reaps surface as
/// [`AcpShutdown::Retained`](super::acp::AcpShutdown) for owner quarantine.
pub(crate) const fn cursor_requires_group_termination() -> bool {
    true
}

/// Compares two dated cursor CLI spellings by their numeric core.
///
/// A leading agent name and any trailing `-suffix` are ignored, so
/// `agent 2026.9.6-stable.1` compares by `[2026, 9, 6]`. Returns `None` when
/// either side has no parseable dated triple; callers fail closed on `None`.
pub(crate) fn compare_cursor_cli_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    Some(parse_cursor_dated_triple(left)?.cmp(&parse_cursor_dated_triple(right)?))
}

/// Returns whether a probed CLI version meets a minimum floor.
pub(crate) fn cursor_cli_meets_minimum(version: &str, minimum: &str) -> bool {
    matches!(
        compare_cursor_cli_versions(version, minimum),
        Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater)
    )
}

fn parse_cursor_dated_triple(text: &str) -> Option<[u64; 3]> {
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

/// Native-continuation decision for one cursor turn.
///
/// `Compatible` authorizes `session/load` against the stored ACP session;
/// `Incompatible` carries the stable reason the dispatcher surfaces instead
/// of silently starting fresh or resuming across engines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CursorContinuationDecision {
    Compatible,
    Incompatible { reason: &'static str },
}

/// Bounded input for [`check_cursor_native_continuation`].
pub(crate) struct CursorContinuationGateInput<'a> {
    /// Probed CLI version (`CursorLaunch::version` once the probe/authority
    /// packet records it; the C1 unprobed sentinel fails the floor).
    pub cli_version: &'a str,
    /// Explicit target model from the current selection; `None` fails closed.
    pub target_model: Option<&'a str>,
    /// Advertised models when a model inventory was read; `None` skips
    /// advertisement validation (live inventory is deferred) but never skips
    /// the explicit-model or CLI gates.
    pub advertised_models: Option<&'a [&'a str]>,
    /// Whether the stored binding names the same `cursor` engine. The
    /// dispatcher scopes continuation reads to `EngineId::Cursor`, so a false
    /// value fails closed instead of resuming across engines.
    pub same_engine: bool,
}

/// Gates one cursor native continuation without touching provider state.
///
/// Order is contractual: same-engine first, then the explicit target model
/// (pre-validated before resume), then the certified CLI floor, then model
/// advertisement when an inventory is supplied. Any failure is a typed
/// incompatible — never a silent fresh start and never a cross-engine resume.
pub(crate) fn check_cursor_native_continuation(
    input: &CursorContinuationGateInput<'_>,
) -> CursorContinuationDecision {
    if !input.same_engine {
        return CursorContinuationDecision::Incompatible {
            reason: "Cursor native continuation cannot resume across engines",
        };
    }
    let Some(target) = input.target_model.filter(|model| !model.is_empty()) else {
        return CursorContinuationDecision::Incompatible {
            reason: "Cursor native continuation requires an explicit target model",
        };
    };
    if !cursor_cli_meets_minimum(input.cli_version, CURSOR_CONTINUATION_MINIMUM_CLI_VERSION) {
        return CursorContinuationDecision::Incompatible {
            reason: "Cursor native continuation requires CLI 2026.08.11-e8db854 or newer",
        };
    }
    if let Some(advertised) = input.advertised_models
        && !advertised.contains(&target)
    {
        return CursorContinuationDecision::Incompatible {
            reason: "Cursor does not currently advertise the target model",
        };
    }
    CursorContinuationDecision::Compatible
}

/// Reopens the stored ACP session for one authorized continuation.
///
/// Mirrors the TypeScript resume path (`session/load` with the stored session
/// id over the same cwd a fresh `session/new` would use): the resume reopens
/// provider-owned state only and never invents checkpoints. Returns `None`
/// when the stored session id is outside its bounded route-segment grammar
/// so the caller fails closed instead of resuming a corrupt session. The
/// load then requires the agent to acknowledge exactly this session.
pub(crate) fn cursor_resume_session_id(stored_session_id: &str) -> Option<String> {
    if stored_session_id.is_empty() || stored_session_id.len() > CURSOR_MAX_SESSION_ID_BYTES {
        return None;
    }
    Some(stored_session_id.to_owned())
}

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

/// Typed `AE-PROVIDER-206` startup failure carrying the rejected model name.
pub(crate) use artisan_native_engine::cursor::CursorStartupFailure;
/// Converts cursor's known pre-session model rejection to Artisan's stable
/// model code, mirroring `ClassifyCursorStartupFailure` in
/// `modules/engines/src/cursor/engine.ts`: `Cannot use this model: X` maps to
/// `AE-PROVIDER-206` carrying the captured model name `X`.
pub(crate) use artisan_native_engine::cursor::classify_cursor_startup_failure;

// ---------------------------------------------------------------------------
// Cursor plan-approval extensions: `cursor/ask_question`, `cursor/create_plan`
// ---------------------------------------------------------------------------

/// One offered answer to a cursor question.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorQuestionOption {
    /// Provider option identity answered back on the wire.
    id: String,
    /// Human label shown beside the question.
    label: String,
}

/// One typed cursor question (`cursor/ask_question` entry).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorQuestion {
    /// Provider question identity.
    id: String,
    /// The question itself.
    prompt: String,
    /// Offered answers.
    options: Vec<CursorQuestionOption>,
    /// Whether more than one option may be chosen at once.
    allow_multiple: bool,
}

/// One typed cursor question request frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorQuestionRequest {
    /// Provider request identity answered by the response.
    tool_call_id: String,
    /// Short category label shown beside the questions, when disclosed.
    title: Option<String>,
    /// The questions in this request group.
    questions: Vec<CursorQuestion>,
}

fn nonempty_string(value: &Value, field: &str) -> Result<String, CursorTurnError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or(CursorTurnError::Configuration)
}

/// Parses one `cursor/ask_question` params object, mirroring
/// `parse_cursor_question` in `modules/engines/src/acp/engine.ts`: a
/// non-empty `toolCallId`, a non-empty `questions` array whose entries carry
/// a non-empty `id`, a non-empty `prompt`, and a non-empty `options` array of
/// `{ id, label }` pairs, with `allowMultiple` defaulting to single select
/// and an optional `title` header.
///
/// # Errors
///
/// Returns [`CursorTurnError::Configuration`] when any identity, prompt, or
/// option violates the extension shape. The turn fails closed instead of
/// rendering a question with nothing to choose.
pub(crate) fn parse_cursor_question_request(
    value: &Value,
) -> Result<CursorQuestionRequest, CursorTurnError> {
    let obj = value.as_object().ok_or(CursorTurnError::Configuration)?;
    let tool_call_id = nonempty_string(value, "toolCallId")?;
    let title = obj
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .map(str::to_owned);
    let raw_questions = obj
        .get("questions")
        .and_then(Value::as_array)
        .ok_or(CursorTurnError::Configuration)?;
    if raw_questions.is_empty() {
        return Err(CursorTurnError::Configuration);
    }
    let mut questions = Vec::new();
    for raw in raw_questions {
        let question = raw.as_object().ok_or(CursorTurnError::Configuration)?;
        let id = nonempty_string(raw, "id")?;
        let prompt = nonempty_string(raw, "prompt")?;
        let allow_multiple = question
            .get("allowMultiple")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let raw_options = question
            .get("options")
            .and_then(Value::as_array)
            .ok_or(CursorTurnError::Configuration)?;
        if raw_options.is_empty() {
            return Err(CursorTurnError::Configuration);
        }
        let mut options = Vec::new();
        for raw_option in raw_options {
            options.push(CursorQuestionOption {
                id: nonempty_string(raw_option, "id")?,
                label: nonempty_string(raw_option, "label")?,
            });
        }
        questions.push(CursorQuestion {
            id,
            prompt,
            options,
            allow_multiple,
        });
    }
    Ok(CursorQuestionRequest {
        tool_call_id,
        title,
        questions,
    })
}

/// Maps one cursor question onto the domain question vocabulary: the prompt
/// becomes the text, the request title becomes the header, `allowMultiple`
/// becomes multi-select, and option labels become the offered choices.
///
/// # Errors
///
/// Returns [`CursorTurnError::Configuration`] when identities, text, or
/// options violate domain ceilings. An out-of-bound provider frame never
/// reaches the durable A-approve rows.
pub(crate) fn cursor_question_to_domain(
    request: &CursorQuestionRequest,
    question: &CursorQuestion,
) -> Result<QuestionInput, CursorTurnError> {
    let question_id =
        ObservationId::parse(question.id.clone()).map_err(|_| CursorTurnError::Configuration)?;
    if question.prompt.trim().is_empty() {
        return Err(CursorTurnError::Configuration);
    }
    let mut options = Vec::new();
    for option in &question.options {
        options.push(
            QuestionOption::new(option.label.clone(), None)
                .map_err(|_| CursorTurnError::Configuration)?,
        );
    }
    if options.is_empty() {
        return Err(CursorTurnError::Configuration);
    }
    Ok(QuestionInput {
        question_id,
        text: question.prompt.clone(),
        header: request.title.clone(),
        multi_select: question.allow_multiple,
        options: Some(options),
    })
}

/// Maps explicit answers for one cursor question back to provider option
/// identities, mirroring the TypeScript answer encoding: an answer matching
/// an option id or label answers that option id, anything else answers
/// verbatim.
#[must_use]
pub(crate) fn cursor_selected_option_ids(
    question: &CursorQuestion,
    answers: &[String],
) -> Vec<String> {
    answers
        .iter()
        .map(|answer| {
            question
                .options
                .iter()
                .find(|option| option.id == *answer || option.label == *answer)
                .map(|option| option.id.clone())
                .unwrap_or_else(|| answer.clone())
        })
        .collect()
}

/// One typed cursor plan step (`cursor/create_plan` todo).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorPlanTodo {
    /// Provider step identity.
    id: String,
    /// Step text.
    content: String,
    /// Provider status spelling (`cancelled`/`completed`/`in_progress`/`pending`).
    status: String,
}

/// One typed cursor plan request frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorPlanRequest {
    /// Provider request identity answered by the approval response.
    tool_call_id: String,
    /// Plan name, when disclosed.
    name: Option<String>,
    /// Plan overview, when disclosed.
    overview: Option<String>,
    /// Full plan text under review.
    plan: String,
    /// Plan steps in provider order, including cancelled ones.
    todos: Vec<CursorPlanTodo>,
}

/// Parses one `cursor/create_plan` params object, mirroring
/// `parse_cursor_plan` in `modules/engines/src/acp/engine.ts`: a non-empty
/// `toolCallId`, a string `plan`, and a `todos` array whose malformed entries
/// are dropped the way the TypeScript `flatMap` evidence does. Only the four
/// provider statuses survive; anything else is not a step.
///
/// # Errors
///
/// Returns [`CursorTurnError::Configuration`] when the tool-call identity,
/// plan text, or todo list violates the extension shape.
pub(crate) fn parse_cursor_plan_request(
    value: &Value,
) -> Result<CursorPlanRequest, CursorTurnError> {
    let obj = value.as_object().ok_or(CursorTurnError::Configuration)?;
    let tool_call_id = nonempty_string(value, "toolCallId")?;
    let plan = obj
        .get("plan")
        .and_then(Value::as_str)
        .ok_or(CursorTurnError::Configuration)?
        .to_owned();
    let raw_todos = obj
        .get("todos")
        .and_then(Value::as_array)
        .ok_or(CursorTurnError::Configuration)?;
    let mut todos = Vec::new();
    for raw in raw_todos {
        let Some(todo) = raw.as_object() else {
            continue;
        };
        let (Some(id), Some(content), Some(status)) = (
            todo.get("id").and_then(Value::as_str),
            todo.get("content").and_then(Value::as_str),
            todo.get("status").and_then(Value::as_str),
        ) else {
            continue;
        };
        if id.is_empty()
            || !matches!(
                status,
                "cancelled" | "completed" | "in_progress" | "pending"
            )
        {
            continue;
        }
        todos.push(CursorPlanTodo {
            id: id.to_owned(),
            content: content.to_owned(),
            status: status.to_owned(),
        });
    }
    let optional_string = |field: &str| {
        obj.get(field)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    Ok(CursorPlanRequest {
        tool_call_id,
        name: optional_string("name"),
        overview: optional_string("overview"),
        plan,
        todos,
    })
}

/// Returns the human-readable plan approval description: the overview, then
/// the name, then the shared fallback, mirroring the TypeScript evidence.
#[must_use]
pub(crate) fn cursor_plan_description(request: &CursorPlanRequest) -> String {
    request
        .overview
        .clone()
        .or_else(|| request.name.clone())
        .unwrap_or_else(|| "Approve this plan?".to_owned())
}

/// Maps one cursor plan request onto the domain approval vocabulary: a
/// generic action carrying the full plan text as its reason. An empty plan
/// still gates as a reason-free action rather than vanishing.
///
/// # Errors
///
/// Returns [`CursorTurnError::Configuration`] when the plan text violates
/// domain ceilings.
pub(crate) fn cursor_plan_approval(
    request: &CursorPlanRequest,
) -> Result<(String, ApprovalRequest), CursorTurnError> {
    let description = cursor_plan_description(request);
    let reason = if request.plan.is_empty() {
        None
    } else {
        Some(request.plan.clone())
    };
    let approval = ApprovalRequest::action(reason).map_err(|_| CursorTurnError::Configuration)?;
    Ok((description, approval))
}

/// Projects one cursor plan request onto provider-neutral plan entries,
/// mirroring the TypeScript emission: cancelled steps are filtered, and the
/// surviving statuses map verbatim (`pending`, `in_progress`, `completed`).
/// Steps without renderable text are skipped so an out-of-bound provider
/// frame never reaches the durable rows.
///
/// # Errors
///
/// Returns [`CursorTurnError::Configuration`] when an identity violates
/// domain ceilings.
pub(crate) fn cursor_plan_entries(
    request: &CursorPlanRequest,
) -> Result<Vec<PlanEntry>, CursorTurnError> {
    let mut entries = Vec::new();
    for todo in &request.todos {
        if todo.status == "cancelled" || todo.content.is_empty() {
            continue;
        }
        let status = PlanEntryStatus::parse(todo.status.as_str())
            .map_err(|_| CursorTurnError::Configuration)?;
        let id =
            ObservationId::parse(todo.id.clone()).map_err(|_| CursorTurnError::Configuration)?;
        entries.push(
            PlanEntry::new(id, status, todo.content.clone())
                .map_err(|_| CursorTurnError::Configuration)?,
        );
    }
    Ok(entries)
}

/// Builds the explicit `cursor/create_plan` wire outcome: an approved plan is
/// `accepted`, a denied plan is `rejected`. The turn continues either way.
#[must_use]
pub(crate) fn answer_cursor_plan(approved: bool) -> Value {
    serde_json::json!({
        "outcome": { "outcome": if approved { "accepted" } else { "rejected" } },
    })
}

// ---------------------------------------------------------------------------
// Fixture tests: cursor-shaped ACP args over the shared transport core
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::time::Duration;

    use artisan_domain::{
        ApprovalKind, ApprovalMode, CursorPermissionMode, CursorReasoningEffort, CursorSelection,
        CursorSpeed, EngineAgentId, EngineModelId, EnginePermissionPolicy, EngineProfileId,
        FilesystemAccess, NetworkAccess, PermissionId, PlanEntryStatus, WebSearchAccess,
    };
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, BufReader, split};

    use super::{
        CURSOR_C1_UNPROBED_VERSION, CURSOR_ENGINE_ID, CursorLaunch, CursorSettings,
        CursorTurnError, answer_cursor_plan, check_cursor_steer, classify_cursor_startup_failure,
        cursor_plan_approval, cursor_plan_description, cursor_plan_entries,
        cursor_question_to_domain, cursor_selected_option_ids, parse_cursor_plan_request,
        parse_cursor_question_request,
    };
    use crate::engine_owner::acp::{
        AcpBounds, AcpTransport, ImageBlock, ImageMode, PromptPart, UpdateEvent,
        build_prompt_content,
    };
    use crate::engine_owner::acp_bridges::{
        PermissionOutcome, answer_permission, normalize_permission_request,
    };
    use crate::native_run_dispatch::{binding_bytes_vec, binding_matches_bytes};

    fn permission(filesystem: FilesystemAccess) -> EnginePermissionPolicy {
        EnginePermissionPolicy::new(
            PermissionId::parse("permission-cursor").expect("permission id"),
            EngineAgentId::parse("agent-cursor").expect("agent id"),
            ApprovalMode::OnRequest,
            filesystem,
            NetworkAccess::Enabled,
            WebSearchAccess::Disabled,
        )
    }

    fn cursor_selection(
        model: Option<&str>,
        effort: Option<&str>,
        speed: Option<CursorSpeed>,
        permission_mode: Option<CursorPermissionMode>,
        filesystem: FilesystemAccess,
    ) -> CursorSelection {
        CursorSelection::new(
            EngineProfileId::parse("cursor-fixture").expect("profile id"),
            model.map(|model| EngineModelId::parse(model).expect("model id")),
            permission(filesystem),
            effort.map(|effort| CursorReasoningEffort::parse(effort).expect("reasoning effort")),
            speed,
            permission_mode,
        )
    }

    fn settings(
        model: Option<&str>,
        effort: Option<&str>,
        speed: Option<CursorSpeed>,
        permission_mode: Option<CursorPermissionMode>,
        filesystem: FilesystemAccess,
    ) -> CursorSettings {
        CursorSettings::from_selection(&cursor_selection(
            model,
            effort,
            speed,
            permission_mode,
            filesystem,
        ))
        .expect("cursor settings stay representable")
    }

    fn strict_bounds() -> AcpBounds {
        AcpBounds::new(
            4096,
            16_384,
            256,
            Duration::from_secs(5),
            Duration::from_secs(5),
            Duration::from_secs(2),
        )
        .expect("test bounds hold")
    }

    async fn agent_read_value(
        reader: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    ) -> Option<Value> {
        let mut line = String::new();
        let count = reader.read_line(&mut line).await.expect("agent reads");
        if count == 0 {
            return None;
        }
        Some(serde_json::from_str(line.trim_end()).expect("driver frames stay valid json"))
    }

    async fn agent_write_line(
        writer: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>,
        line: &str,
    ) {
        use tokio::io::AsyncWriteExt as _;
        writer
            .write_all(line.as_bytes())
            .await
            .expect("agent writes");
        writer.write_all(b"\n").await.expect("agent writes");
        writer.flush().await.expect("agent flushes");
    }

    fn update_line(session: &str, index: u32) -> String {
        json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "sessionId": session, "update": { "kind": "delta", "index": index } },
        })
        .to_string()
    }

    fn permission_params(tool_call_id: &str, command: &str) -> Value {
        json!({
            "toolCall": {
                "toolCallId": tool_call_id,
                "kind": "execute",
                "title": "Run tests",
                "rawInput": { "command": command, "cwd": "C:\\work" },
            },
            "options": [
                { "kind": "allow_once", "optionId": "allow-1", "label": "Allow" },
                { "kind": "reject_once", "optionId": "reject-1", "label": "Deny" },
            ],
        })
    }

    // -----------------------------------------------------------------------
    // Definition row: model resolution, args, classifier, image-block mode
    // -----------------------------------------------------------------------

    #[test]
    fn model_resolution_matrix() {
        // No model stays absent; effort and speed never invent one. The raw
        // selection rides `launch_args` untouched.
        assert_eq!(
            settings(
                None,
                Some("high"),
                Some(CursorSpeed::Fast),
                None,
                FilesystemAccess::Workspace
            )
            .launch_args()
            .model,
            None
        );

        // Resolution lives in `cursor_build_args` (mirroring TS
        // `ResolveCursorModel` inside `CursorAcpArgs`), so the matrix asserts
        // through the resolved `--model` argv value, never the raw selection.
        fn resolved_model(settings: &CursorSettings) -> Option<String> {
            let args = settings.build_args();
            let position = args
                .iter()
                .position(|arg| arg.to_str() == Some("--model"))?;
            args.get(position + 1)
                .and_then(|model| model.to_str())
                .map(str::to_owned)
        }

        // Effort appends unless the base already carries a suffix.
        assert_eq!(
            resolved_model(&settings(
                Some("composer-1"),
                Some("high"),
                None,
                None,
                FilesystemAccess::Workspace
            )),
            Some("composer-1-high".to_owned())
        );
        assert_eq!(
            resolved_model(&settings(
                Some("composer-1-high"),
                Some("low"),
                None,
                None,
                FilesystemAccess::Workspace
            )),
            Some("composer-1-high".to_owned())
        );
        assert_eq!(
            resolved_model(&settings(
                Some("composer-1-high-fast"),
                Some("low"),
                None,
                None,
                FilesystemAccess::Workspace
            )),
            Some("composer-1-high-fast".to_owned())
        );

        // Fast appends unless already present.
        assert_eq!(
            resolved_model(&settings(
                Some("composer-1"),
                Some("high"),
                Some(CursorSpeed::Fast),
                None,
                FilesystemAccess::Workspace
            )),
            Some("composer-1-high-fast".to_owned())
        );
        assert_eq!(
            resolved_model(&settings(
                Some("composer-1-fast"),
                None,
                Some(CursorSpeed::Fast),
                None,
                FilesystemAccess::Workspace
            )),
            Some("composer-1-fast".to_owned())
        );

        // Bracket models pass through untouched.
        assert_eq!(
            resolved_model(&settings(
                Some("cursor[fast]"),
                Some("high"),
                Some(CursorSpeed::Fast),
                None,
                FilesystemAccess::Workspace
            )),
            Some("cursor[fast]".to_owned())
        );
    }

    #[test]
    fn args_matrix() {
        // Bare writable session is just the subcommand.
        assert_eq!(
            settings(None, None, None, None, FilesystemAccess::Workspace).build_args(),
            vec![OsString::from("acp")]
        );

        // Resolved model leads.
        assert_eq!(
            settings(
                Some("composer-1"),
                Some("high"),
                None,
                None,
                FilesystemAccess::Workspace
            )
            .build_args(),
            vec![
                OsString::from("--model"),
                OsString::from("composer-1-high"),
                OsString::from("acp"),
            ]
        );

        // Read-only maps to ask mode and wins over force.
        assert_eq!(
            settings(None, None, None, None, FilesystemAccess::None).build_args(),
            vec![
                OsString::from("--mode"),
                OsString::from("ask"),
                OsString::from("acp"),
            ]
        );
        assert_eq!(
            settings(
                Some("composer-1"),
                None,
                None,
                Some(CursorPermissionMode::Force),
                FilesystemAccess::None
            )
            .build_args(),
            vec![
                OsString::from("--model"),
                OsString::from("composer-1"),
                OsString::from("--mode"),
                OsString::from("ask"),
                OsString::from("acp"),
            ]
        );

        // Force maps only when writes are allowed.
        assert!(
            settings(
                None,
                None,
                None,
                Some(CursorPermissionMode::Force),
                FilesystemAccess::Workspace
            )
            .build_args()
            .contains(&OsString::from("--force"))
        );
    }

    #[test]
    fn startup_rejection_captures_model_with_stable_code() {
        let failure = classify_cursor_startup_failure(
            "Cannot use this model: composer-1. Valid models: composer-1, composer-2",
        )
        .expect("known rejection classifies");
        assert_eq!(failure.model(), "composer-1");
        assert_eq!(failure.artisan_code(), "AE-PROVIDER-206");
        assert_eq!(failure.engine_id(), CURSOR_ENGINE_ID);
        assert_eq!(
            failure.message(),
            "Cursor does not make model composer-1 available to this account."
        );

        // Case-insensitive prefix with a line-break terminator.
        let newline = classify_cursor_startup_failure("cANNOT USE THIS MODEL:  sonar\nretry later")
            .expect("newline terminator classifies");
        assert_eq!(newline.model(), "sonar");

        // End-of-input terminator.
        let trailing = classify_cursor_startup_failure("Cannot use this model: nightly-x")
            .expect("trailing model classifies");
        assert_eq!(trailing.model(), "nightly-x");

        for silent in [
            "",
            "agent 2026.9.6-stable.1",
            "Error: not authenticated",
            "Cannot use this model:",
        ] {
            assert!(
                classify_cursor_startup_failure(silent).is_none(),
                "no rejection without a captured model: {silent:?}"
            );
        }
    }

    #[test]
    fn definition_row_carries_cursor_shape() {
        let row = CursorSettings::definition();
        assert_eq!(row.engine_id, CURSOR_ENGINE_ID);
        assert!(row.executable.contains("agent"));
        assert_eq!(row.version_args, &["--version"][..]);
        assert_eq!(row.auth_probe_args, &["status"][..]);
        assert_eq!(row.image_mode, ImageMode::Image);
        assert_eq!(CursorSettings::image_mode(), ImageMode::Image);

        let available = ["cursor_login"];
        assert_eq!(
            (row.select_auth_method)(&available, false),
            Some("cursor_login")
        );
        assert_eq!((row.select_auth_method)(&[], false), None);
        assert!((row.is_authenticated_output)("signed in as s"));
        assert!(!(row.is_authenticated_output)("Error: not logged in"));

        // The settings project onto the same row builder the core spawns.
        let resolved = settings(
            Some("composer-1"),
            Some("high"),
            None,
            Some(CursorPermissionMode::Force),
            FilesystemAccess::Workspace,
        );
        assert_eq!(
            (row.build_args)(&resolved.launch_args()),
            resolved.build_args()
        );
    }

    // -----------------------------------------------------------------------
    // Steer, continuation, usage honesty
    // -----------------------------------------------------------------------

    #[test]
    fn active_steer_rejects_with_typed_unsupported_command() {
        assert_eq!(check_cursor_steer(false), Ok(()));
        assert_eq!(
            check_cursor_steer(true),
            Err(CursorTurnError::UnsupportedCommand)
        );
    }

    // -----------------------------------------------------------------------
    // Launch identity (the C3 gate matrix lives in
    // `tests/backend/engine_owner_cursor.rs`)
    // -----------------------------------------------------------------------

    #[test]
    fn unprobed_launch_carries_profile_and_sentinel() {
        // The C1 launch carries no usage scope: profile plus sentinel only.
        // The sentinel fails the C3 certified floor, so any continuation
        // through an unprobed launch gates incompatible.
        let launch = CursorLaunch::unprobed("cursor-fixture".to_owned());
        assert_eq!(launch.profile_id(), "cursor-fixture");
        assert_eq!(launch.version(), CURSOR_C1_UNPROBED_VERSION);
    }

    // -----------------------------------------------------------------------
    // Plan-approval extensions
    // -----------------------------------------------------------------------

    fn question_fixture() -> Value {
        json!({
            "toolCallId": "cursor-q-1",
            "title": "Pick",
            "questions": [
                {
                    "id": "q1",
                    "prompt": "Which?",
                    "allowMultiple": false,
                    "options": [
                        { "id": "o1", "label": "First" },
                        { "id": "o2", "label": "Second" },
                    ],
                },
                {
                    "id": "q2",
                    "prompt": "Which tags?",
                    "allowMultiple": true,
                    "options": [{ "id": "t1", "label": "Tag" }],
                },
            ],
        })
    }

    #[test]
    fn cursor_questions_map_to_domain_with_answer_identity() {
        let request = parse_cursor_question_request(&question_fixture()).expect("questions parse");
        assert_eq!(request.tool_call_id, "cursor-q-1");
        assert_eq!(request.title.as_deref(), Some("Pick"));
        assert_eq!(request.questions.len(), 2);

        let first = cursor_question_to_domain(&request, &request.questions[0]).expect("domain");
        assert_eq!(first.text, "Which?");
        assert_eq!(first.header.as_deref(), Some("Pick"));
        assert!(!first.multi_select);
        assert_eq!(first.options.as_ref().expect("options").len(), 2);

        let second = cursor_question_to_domain(&request, &request.questions[1]).expect("domain");
        assert!(second.multi_select);

        // Answers resolve by option id or label, verbatim otherwise.
        assert_eq!(
            cursor_selected_option_ids(
                &request.questions[0],
                &["o2".to_owned(), "First".to_owned(), "custom".to_owned()]
            ),
            vec!["o2".to_owned(), "o1".to_owned(), "custom".to_owned()]
        );

        for bad in [
            json!(null),
            json!({}),
            json!({ "toolCallId": "", "questions": [] }),
            json!({ "toolCallId": "x" }),
            json!({ "toolCallId": "x", "questions": [] }),
            json!({
                "toolCallId": "x",
                "questions": [{ "id": "q", "prompt": "p", "options": [] }],
            }),
            json!({
                "toolCallId": "x",
                "questions": [{ "id": "", "prompt": "p", "options": [{ "id": "o", "label": "l" }] }],
            }),
        ] {
            assert_eq!(
                parse_cursor_question_request(&bad),
                Err(CursorTurnError::Configuration),
                "question shape must fail closed: {bad}"
            );
        }
    }

    fn plan_fixture() -> Value {
        json!({
            "toolCallId": "cursor-plan-1",
            "name": "Plan",
            "overview": "Do things",
            "plan": "Steps to finish.",
            "todos": [
                { "id": "t1", "content": "Step one", "status": "pending" },
                { "id": "t2", "content": "Step two", "status": "in_progress" },
                { "id": "t3", "content": "Step three", "status": "completed" },
                { "id": "t4", "content": "Dropped", "status": "cancelled" },
                { "id": "", "content": "Junk", "status": "pending" },
                { "id": "t5", "content": "Weird", "status": "unknown" },
            ],
        })
    }

    #[test]
    fn cursor_plan_maps_entries_and_action_approval() {
        let request = parse_cursor_plan_request(&plan_fixture()).expect("plan parses");
        assert_eq!(request.tool_call_id, "cursor-plan-1");
        // Malformed todos drop the TypeScript flatMap way; cancelled stays
        // parsed here and filters at emission.
        assert_eq!(request.todos.len(), 4);

        let entries = cursor_plan_entries(&request).expect("entries project");
        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .iter()
                .all(|entry| entry.status() != PlanEntryStatus::Pending
                    || entry.text() == "Step one")
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.status())
                .collect::<Vec<_>>(),
            vec![
                PlanEntryStatus::Pending,
                PlanEntryStatus::InProgress,
                PlanEntryStatus::Completed,
            ]
        );

        assert_eq!(cursor_plan_description(&request), "Do things");
        let (description, approval) = cursor_plan_approval(&request).expect("approval maps");
        assert_eq!(description, "Do things");
        assert_eq!(approval.reason(), Some("Steps to finish."));

        // Overview falls back to name, then to the shared prompt.
        let mut nameless = request.clone();
        nameless.overview = None;
        assert_eq!(cursor_plan_description(&nameless), "Plan");
        nameless.name = None;
        assert_eq!(cursor_plan_description(&nameless), "Approve this plan?");

        // The explicit wire outcome accepts or rejects; the turn continues.
        assert_eq!(
            answer_cursor_plan(true),
            json!({ "outcome": { "outcome": "accepted" } })
        );
        assert_eq!(
            answer_cursor_plan(false),
            json!({ "outcome": { "outcome": "rejected" } })
        );

        for bad in [
            json!(null),
            json!({}),
            json!({ "toolCallId": "x", "plan": "p" }),
            json!({ "toolCallId": "", "plan": "p", "todos": [] }),
            json!({ "toolCallId": "x", "plan": 42, "todos": [] }),
        ] {
            assert_eq!(
                parse_cursor_plan_request(&bad),
                Err(CursorTurnError::Configuration),
                "plan shape must fail closed: {bad}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Binding tag/format round trip
    // -----------------------------------------------------------------------

    #[test]
    fn cursor_binding_round_trip_and_mismatch() {
        let raw =
            binding_bytes_vec("cursor", "cursor-fixture", "sess-cursor-1").expect("binding builds");
        assert!(binding_matches_bytes(
            &raw,
            "cursor",
            "cursor-fixture",
            "sess-cursor-1"
        ));
        for (engine, profile, session) in [
            ("opencode2", "cursor-fixture", "sess-cursor-1"),
            ("codex", "cursor-fixture", "sess-cursor-1"),
            ("cursor", "other-profile", "sess-cursor-1"),
            ("cursor", "cursor-fixture", "other-session"),
        ] {
            assert!(
                !binding_matches_bytes(&raw, engine, profile, session),
                "mismatch must requeue"
            );
        }
        assert!(binding_bytes_vec("", "cursor-fixture", "sess-cursor-1").is_none());
        assert!(binding_bytes_vec("cursor", "", "sess-cursor-1").is_none());
        assert!(binding_bytes_vec("cursor", "cursor-fixture", "").is_none());
    }

    // -----------------------------------------------------------------------
    // Fixture ACP turns: cursor-shaped args over the shared transport core
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn fixture_cursor_start_deltas_approval_close() {
        let bounds = strict_bounds();
        let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, agent_write_half) = split(agent_io);
        let mut agent_read = BufReader::new(agent_read_half);
        let mut agent_write = agent_write_half;

        let agent = tokio::spawn(async move {
            let init = agent_read_value(&mut agent_read)
                .await
                .expect("initialize request");
            assert_eq!(
                init.get("method").and_then(Value::as_str),
                Some("initialize")
            );
            let init_id = init.get("id").cloned().expect("initialize id");
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": init_id,
                    "result": { "protocolVersion": 1, "authMethods": [{ "id": "cursor_login" }] },
                })
                .to_string(),
            )
            .await;

            let auth = agent_read_value(&mut agent_read)
                .await
                .expect("authenticate request");
            assert_eq!(
                auth.get("method").and_then(Value::as_str),
                Some("authenticate")
            );
            let auth_id = auth.get("id").cloned().expect("authenticate id");
            agent_write_line(
                &mut agent_write,
                &json!({ "jsonrpc": "2.0", "id": auth_id, "result": {} }).to_string(),
            )
            .await;

            let new = agent_read_value(&mut agent_read)
                .await
                .expect("session/new request");
            assert_eq!(
                new.get("method").and_then(Value::as_str),
                Some("session/new")
            );
            let new_id = new.get("id").cloned().expect("session/new id");
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": new_id,
                    "result": { "sessionId": "sess-cursor-1" },
                })
                .to_string(),
            )
            .await;

            let prompt = agent_read_value(&mut agent_read)
                .await
                .expect("session/prompt request");
            assert_eq!(
                prompt.get("method").and_then(Value::as_str),
                Some("session/prompt")
            );
            // Cursor carries images as native image blocks, never resources.
            let content = prompt
                .get("params")
                .and_then(|params| params.get("prompt"))
                .and_then(Value::as_array)
                .expect("prompt content");
            assert!(
                content
                    .iter()
                    .any(
                        |part| part.get("type").and_then(Value::as_str) == Some("image")
                            && part.get("mimeType").and_then(Value::as_str) == Some("image/png")
                    ),
                "image-block mode must cross the wire"
            );
            let prompt_id = prompt.get("id").cloned().expect("prompt id");

            agent_write_line(&mut agent_write, &update_line("sess-cursor-1", 1)).await;
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 50,
                    "method": "session/requestPermission",
                    "params": permission_params("cursor-tool-1", "cargo test"),
                })
                .to_string(),
            )
            .await;
            agent_write_line(&mut agent_write, &update_line("sess-cursor-1", 2)).await;
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 51,
                    "method": "session/requestPermission",
                    "params": permission_params("cursor-tool-2", "cargo test"),
                })
                .to_string(),
            )
            .await;
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": prompt_id,
                    "result": {
                        "stopReason": "completed",
                        "usage": { "inputTokens": 7, "outputTokens": 9 },
                    },
                })
                .to_string(),
            )
            .await;

            let mut saw_cancel = false;
            while let Some(frame) = agent_read_value(&mut agent_read).await {
                if frame.get("method").and_then(Value::as_str) == Some("session/cancel") {
                    saw_cancel = true;
                }
            }
            saw_cancel
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        let init = driver.initialize().await.expect("handshake");
        assert_eq!(init.protocol_version, 1);
        let available: Vec<&str> = init.auth_methods.iter().map(String::as_str).collect();
        assert_eq!(
            (CursorSettings::definition().select_auth_method)(&available, false),
            Some("cursor_login")
        );
        driver
            .authenticate("cursor_login")
            .await
            .expect("authenticate");
        let session = driver.new_session("C:\\work").await.expect("session/new");
        assert_eq!(session.as_str(), "sess-cursor-1");

        // Cursor-shaped args: resolved model plus force, then `acp`.
        let shaped = settings(
            Some("composer-1"),
            Some("high"),
            None,
            Some(CursorPermissionMode::Force),
            FilesystemAccess::Workspace,
        );
        assert_eq!(
            shaped.build_args(),
            vec![
                OsString::from("--model"),
                OsString::from("composer-1-high"),
                OsString::from("--force"),
                OsString::from("acp"),
            ]
        );

        let image = PromptPart::Image(ImageBlock {
            id: "attach-1".to_owned(),
            name: "shot.png".to_owned(),
            media_type: "image/png".to_owned(),
            bytes: vec![1, 2, 3],
        });
        let content =
            build_prompt_content(ImageMode::Image, "hello", &[image], None).expect("content");
        let prompt_id = driver.prompt(&session, content).await.expect("prompt");

        let mut deltas = 0_u32;
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("first delta")
        {
            UpdateEvent::SessionUpdate(_) => deltas += 1,
            other => panic!("expected session update, got {other:?}"),
        }

        // Deny lands with no side effect while the turn continues.
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("first approval")
        {
            UpdateEvent::AgentRequest { method, params, .. } => {
                assert_eq!(method, "session/requestPermission");
                let pending = normalize_permission_request(&params).expect("normalize");
                assert_eq!(pending.provider_id(), "cursor-tool-1");
                assert_eq!(
                    answer_permission(&pending, false),
                    PermissionOutcome::Selected {
                        option_id: "reject-1".to_owned(),
                    }
                );
            }
            other => panic!("expected agent request, got {other:?}"),
        }
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("second delta")
        {
            UpdateEvent::SessionUpdate(_) => deltas += 1,
            other => panic!("expected session update, got {other:?}"),
        }

        // Allow answers through the same durable path.
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("second approval")
        {
            UpdateEvent::AgentRequest { params, .. } => {
                let pending = normalize_permission_request(&params).expect("normalize");
                assert_eq!(pending.provider_id(), "cursor-tool-2");
                assert_eq!(
                    answer_permission(&pending, true),
                    PermissionOutcome::Selected {
                        option_id: "allow-1".to_owned(),
                    }
                );
            }
            other => panic!("expected agent request, got {other:?}"),
        }
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("prompt result")
        {
            UpdateEvent::PromptResult(outcome) => {
                assert!(!outcome.cancelled);
                let usage = outcome.usage.expect("usage reported");
                assert_eq!(usage.input, 7);
                assert_eq!(usage.output, 9);
            }
            other => panic!("expected prompt result, got {other:?}"),
        }
        assert_eq!(deltas, 2);

        driver.cancel(&session).await.expect("cancel notify");
        driver.shutdown_writer().await.expect("lifeline close");
        drop(driver);
        assert!(agent.await.expect("agent joins"), "cancel must be observed");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fixture_cursor_plan_and_question_extensions_surface() {
        let bounds = strict_bounds();
        let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, mut agent_write) = split(agent_io);
        drop(agent_read_half);

        let agent = tokio::spawn(async move {
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 60,
                    "method": "cursor/ask_question",
                    "params": question_fixture(),
                })
                .to_string(),
            )
            .await;
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 61,
                    "method": "cursor/create_plan",
                    "params": plan_fixture(),
                })
                .to_string(),
            )
            .await;
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        let session =
            crate::engine_owner::acp::SessionId::parse("sess-cursor-1", 256).expect("session");
        let prompt_id = crate::engine_owner::acp::AcpId::Number(9);

        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("question request")
        {
            UpdateEvent::AgentRequest { method, params, .. } => {
                assert_eq!(method, "cursor/ask_question");
                let request = parse_cursor_question_request(&params).expect("questions parse");
                let domain =
                    cursor_question_to_domain(&request, &request.questions[0]).expect("domain");
                assert_eq!(domain.text, "Which?");
                assert_eq!(
                    cursor_selected_option_ids(&request.questions[0], &["Second".to_owned()]),
                    vec!["o2".to_owned()]
                );
            }
            other => panic!("expected agent request, got {other:?}"),
        }
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("plan request")
        {
            UpdateEvent::AgentRequest { method, params, .. } => {
                assert_eq!(method, "cursor/create_plan");
                let request = parse_cursor_plan_request(&params).expect("plan parses");
                assert_eq!(cursor_plan_entries(&request).expect("entries").len(), 3);
                let (description, approval) =
                    cursor_plan_approval(&request).expect("approval maps");
                assert_eq!(description, "Do things");
                assert_eq!(approval.kind(), ApprovalKind::Action);
                assert_eq!(
                    answer_cursor_plan(false),
                    json!({ "outcome": { "outcome": "rejected" } })
                );
            }
            other => panic!("expected agent request, got {other:?}"),
        }
        agent.await.expect("agent joins");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fixture_cursor_resume_round_trip() {
        let bounds = strict_bounds();
        let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, mut agent_write) = split(agent_io);
        let mut agent_read = BufReader::new(agent_read_half);

        let agent = tokio::spawn(async move {
            let first = agent_read_value(&mut agent_read)
                .await
                .expect("session/load request");
            assert_eq!(
                first.get("method").and_then(Value::as_str),
                Some("session/load")
            );
            assert_eq!(
                first
                    .get("params")
                    .and_then(|params| params.get("sessionId"))
                    .and_then(Value::as_str),
                Some("sess-cursor-9")
            );
            let first_id = first.get("id").cloned().expect("load id");
            agent_write_line(
                &mut agent_write,
                &json!({ "jsonrpc": "2.0", "id": first_id, "result": {} }).to_string(),
            )
            .await;

            let second = agent_read_value(&mut agent_read)
                .await
                .expect("second session/load request");
            let second_id = second.get("id").cloned().expect("load id");
            agent_write_line(
                &mut agent_write,
                &json!({ "jsonrpc": "2.0", "id": second_id, "error": { "code": -32_000 } })
                    .to_string(),
            )
            .await;
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        let session =
            crate::engine_owner::acp::SessionId::parse("sess-cursor-9", 256).expect("session");
        driver
            .load_session(&session, "C:\\work")
            .await
            .expect("resume");
        let error = driver
            .load_session(&session, "C:\\work")
            .await
            .expect_err("rejected resume");
        assert_eq!(error, crate::engine_owner::acp::AcpError::ChildFailed);
        agent.await.expect("agent joins");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fixture_cursor_malformed_frames_reject_without_shell() {
        use crate::engine_owner::acp::{AcpError, parse_envelope};

        assert_eq!(
            parse_envelope("not json", 4096).expect_err("malformed"),
            AcpError::MalformedEnvelope
        );
        assert_eq!(
            parse_envelope("{\"jsonrpc\":\"2.0\",\"id\":1}", 4096).expect_err("malformed"),
            AcpError::MalformedEnvelope
        );

        // A foreign session frame is skipped; the owned update still lands.
        let bounds = strict_bounds();
        let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, mut agent_write) = split(agent_io);
        drop(agent_read_half);
        let agent = tokio::spawn(async move {
            agent_write_line(&mut agent_write, &update_line("other-sess", 9)).await;
            agent_write_line(&mut agent_write, &update_line("sess-cursor-1", 1)).await;
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        let session =
            crate::engine_owner::acp::SessionId::parse("sess-cursor-1", 256).expect("session");
        let prompt_id = crate::engine_owner::acp::AcpId::Number(4);
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("own update")
        {
            UpdateEvent::SessionUpdate(update) => {
                assert_eq!(update.session.as_str(), "sess-cursor-1")
            }
            other => panic!("expected session update, got {other:?}"),
        }
        agent.await.expect("agent joins");
    }

    #[test]
    fn unavailable_model_startup_rejection_marks_unrunnable_claim() {
        // The dispatcher requeues the claim; the stable code travels with the
        // transcript diagnostic instead of a raw provider string.
        let failure = classify_cursor_startup_failure(
            "Cannot use this model: composer-1. Valid models: composer-1",
        )
        .expect("rejection classifies");
        assert_eq!(failure.model(), "composer-1");
        assert_eq!(failure.artisan_code(), "AE-PROVIDER-206");
    }
}
