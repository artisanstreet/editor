//! Native profile-menu usage state and Electron-compatible presentation policy.
//!
//! The Electron menu receives account quota reports from the TypeScript
//! backend. This module owns the validated native data seam and its projection
//! rules. It never creates a provider row, percentage, reset time, or
//! authentication result on its own. The native adapter feeds
//! [`NativeProfileUsageState::accept`] with provider-owned facts; while that
//! exchange is pending, the profile menu renders a generic connection state.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use crate::usage_reset_duration::{UsageResetWindow, usage_reset_duration};

/// Stable selector for the profile usage section.
pub const PROFILE_USAGE_SELECTOR: &str = "artisan-native-profile-usage";

/// Electron's usage-window cadence vocabulary, kept separate from provider
/// labels so identical model buckets in different cadences remain distinct.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NativeUsageCadence {
    /// A short-lived provider session window.
    Session,
    /// A provider weekly window.
    Weekly,
    /// A provider monthly window.
    Monthly,
    /// A provider window whose cadence was not recognized.
    Unknown,
}

impl NativeUsageCadence {
    /// Returns the Electron section heading for this cadence.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
            Self::Unknown => "Usage",
        }
    }
}

/// Authentication state reported by one provider adapter.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NativeUsageAuthentication {
    /// The provider accepted the account and returned its quota surface.
    Authenticated,
    /// The provider explicitly reported that no account is signed in.
    Unauthenticated,
    /// The adapter could not establish an authentication state.
    Unknown,
}

/// Whether the provider disclosed a quota surface for this account.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NativeUsageQuotaSurface {
    /// Quota windows are supported by the provider adapter.
    Supported,
    /// The provider answered but does not expose quota windows.
    Unsupported,
    /// The adapter did not establish support.
    Unknown,
}

/// One provider-owned quota window.
///
/// The native adapter must copy these fields from its provider response.  In
/// particular, `percent_used` is not derived from token counts or the model
/// catalog, and `resets_at` is not guessed from the cadence.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeUsageWindow {
    /// Stable provider bucket identity.
    pub id: String,
    /// Provider-disclosed cadence.
    pub cadence: NativeUsageCadence,
    /// Optional provider/model scope label.
    pub label: Option<String>,
    /// Provider-reported percentage in the inclusive `0..=100` range.
    pub percent_used: f64,
    /// Optional provider-disclosed ISO-8601 reset instant.
    pub resets_at: Option<String>,
    /// Optional provider-disclosed duration in minutes.
    pub window_minutes: Option<u32>,
}

impl NativeUsageWindow {
    /// Returns whether this is safe to paint as a meter.
    ///
    /// Invalid provider values are omitted rather than silently becoming a
    /// zero reading, which would look like a real unused quota.
    #[must_use]
    pub fn has_valid_percentage(&self) -> bool {
        self.percent_used.is_finite() && (0.0..=100.0).contains(&self.percent_used)
    }

    /// Returns the Electron scope label for a window.
    #[must_use]
    pub fn scope_label(&self) -> &str {
        self.label.as_deref().unwrap_or("All models")
    }
}

/// One provider's complete account-usage report.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeUsageReport {
    /// Stable provider/engine identity.
    pub engine_id: String,
    /// Provider-owned display name.
    pub display_name: String,
    /// Authentication fact reported by the provider adapter.
    pub authentication: NativeUsageAuthentication,
    /// Provider account email, when the provider disclosed one.
    pub account_email: Option<String>,
    /// Provider quota capability.
    pub quota_surface: NativeUsageQuotaSurface,
    /// Provider-owned quota windows.
    pub windows: Vec<NativeUsageWindow>,
    /// Provider or transport failure text, already redacted by the adapter.
    pub failure: Option<String>,
}

impl NativeUsageReport {
    /// Returns the windows that can be rendered without inventing a reading.
    #[must_use]
    pub fn renderable_windows(&self) -> Vec<NativeUsageWindow> {
        self.windows
            .iter()
            .filter(|window| window.has_valid_percentage())
            .cloned()
            .collect()
    }
}

/// One per-provider state entry, matching the Electron controller's split
/// between a provider report and a transport failure.
///
/// A refresh failure is stored alongside a last-good report instead of
/// replacing it: `report` keeps the meters while `failure` keeps the refresh
/// condition visible. The menu renders both.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeUsageEntry {
    /// Stable provider/engine identity.
    pub engine_id: String,
    /// Known display name, including while a refresh is pending.
    pub display_name: String,
    /// The provider's last response, if one was received.
    pub report: Option<NativeUsageReport>,
    /// Redacted transport failure when no report was received.
    pub failure: Option<String>,
    /// Timestamp belonging to this provider's response.
    pub fetched_at_ms: Option<i64>,
}

impl NativeUsageEntry {
    /// Creates a named pending row without claiming account state.
    #[must_use]
    pub fn pending(engine_id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            engine_id: engine_id.into(),
            display_name: display_name.into(),
            report: None,
            failure: None,
            fetched_at_ms: None,
        }
    }

    /// Returns whether a real provider response has been received.
    #[must_use]
    pub fn has_response(&self) -> bool {
        self.report.is_some() || self.failure.is_some()
    }
}

/// State for one profile-menu usage section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NativeProfileUsageState {
    /// Provider rows in stable adapter/catalog order.
    pub entries: Vec<NativeUsageEntry>,
    /// Providers with an admitted refresh that have not settled yet.
    pub refreshing_engine_ids: Vec<String>,
    /// Latest admitted per-engine read sequence; a stale same-engine reply
    /// carrying an older sequence must never settle or replace the newer
    /// request.
    pending_read_seq: Vec<(String, u64)>,
}

impl NativeProfileUsageState {
    /// Returns the current provider entry, if present.
    #[must_use]
    pub fn entry(&self, engine_id: &str) -> Option<&NativeUsageEntry> {
        self.entries
            .iter()
            .find(|entry| entry.engine_id == engine_id)
    }

    /// Marks one provider refresh as pending without creating a provider row.
    pub fn begin_refresh(&mut self, engine_id: &str) {
        if !self
            .refreshing_engine_ids
            .iter()
            .any(|current| current == engine_id)
        {
            self.refreshing_engine_ids.push(engine_id.to_owned());
        }
    }

    /// Settles a provider refresh.
    pub fn finish_refresh(&mut self, engine_id: &str) {
        self.refreshing_engine_ids
            .retain(|current| current != engine_id);
        self.pending_read_seq
            .retain(|(pending, _)| pending != engine_id);
    }

    /// Marks one provider refresh pending under an exact per-engine request
    /// sequence. A forced second read while the first is pending replaces the
    /// expected sequence so the older reply can no longer settle the newer
    /// request.
    pub fn begin_refresh_seq(&mut self, engine_id: &str, request_seq: u64) {
        self.begin_refresh(engine_id);
        if let Some(slot) = self
            .pending_read_seq
            .iter_mut()
            .find(|(pending, _)| pending == engine_id)
        {
            slot.1 = request_seq;
        } else {
            self.pending_read_seq
                .push((engine_id.to_owned(), request_seq));
        }
    }

    /// Returns the latest admitted read sequence for one engine, if pending.
    #[must_use]
    pub fn pending_seq(&self, engine_id: &str) -> Option<u64> {
        self.pending_read_seq
            .iter()
            .find(|(pending, _)| pending == engine_id)
            .map(|(_, seq)| *seq)
    }

    /// Settles a provider refresh only when `request_seq` is still the latest
    /// admitted sequence. A stale same-engine reply returns `false` and leaves
    /// the newer pending request untouched.
    #[must_use]
    pub fn finish_refresh_seq(&mut self, engine_id: &str, request_seq: u64) -> bool {
        if self.pending_seq(engine_id) != Some(request_seq) {
            return false;
        }
        self.finish_refresh(engine_id);
        true
    }

    /// Accepts one provider response, retaining a newer reading over a late
    /// stale response exactly like the Electron controller.
    pub fn accept(&mut self, entry: NativeUsageEntry) {
        let engine_id = entry.engine_id.clone();
        let existing = self
            .entries
            .iter()
            .position(|current| current.engine_id == engine_id);
        match existing {
            Some(index)
                if entry.fetched_at_ms.is_none() && self.entries[index].fetched_at_ms.is_some() => {
            }
            Some(index)
                if self.entries[index]
                    .fetched_at_ms
                    .zip(entry.fetched_at_ms)
                    .is_some_and(|(current, incoming)| current > incoming) => {}
            Some(index) => self.entries[index] = entry,
            None => self.entries.push(entry),
        }
        self.finish_refresh(&engine_id);
    }

    /// Accepts one provider response only when `request_seq` is still the
    /// latest admitted sequence for its engine. A stale same-engine reply
    /// arriving after a forced refresh returns `false` and changes nothing:
    /// it neither settles the newer pending request nor replaces its reading.
    pub fn try_accept(&mut self, entry: NativeUsageEntry, request_seq: u64) -> bool {
        let engine_id = entry.engine_id.clone();
        if self.pending_seq(&engine_id) != Some(request_seq) {
            return false;
        }
        self.accept(entry);
        true
    }

    /// Returns whether at least one genuine provider response is available.
    #[must_use]
    pub fn has_real_data(&self) -> bool {
        self.entries.iter().any(NativeUsageEntry::has_response)
    }

    /// Returns the entries the dropdown may paint, in adapter order.
    ///
    /// This mirrors the Electron menu's per-row facts but applies the
    /// requested presentation filter: only an authenticated report carrying
    /// at least one renderable window is visible. Unsupported,
    /// unauthenticated, failed, empty, and pending-without-data providers
    /// are hidden; a last-good report stays visible while its refresh is
    /// pending or has settled a failure. A zero percentage is real data and
    /// is never filtered. Data acquisition is untouched — this only decides
    /// dropdown presentation.
    #[must_use]
    pub fn visible_usage_entries(&self) -> Vec<&NativeUsageEntry> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.report.as_ref().is_some_and(|report| {
                    report.authentication == NativeUsageAuthentication::Authenticated
                        && !report.renderable_windows().is_empty()
                })
            })
            .collect()
    }

    /// Clears incompatible cache and pending state on connection changes.
    ///
    /// Entries belong to the previous Forge connection scope and must not be
    /// paired with a newer generation. Refreshing flags and pending read
    /// sequences are dropped without touching any other application state.
    pub fn clear_for_connection(&mut self) {
        self.entries.clear();
        self.refreshing_engine_ids.clear();
        self.pending_read_seq.clear();
    }

    /// Records a redacted refresh failure while keeping last-good meters.
    ///
    /// When a last-good report exists its windows and original observation
    /// time are preserved and the failure is stored alongside it so the menu
    /// shows both. Otherwise a named failure row is stored so one provider's
    /// error never erases siblings.
    pub fn accept_failure(
        &mut self,
        engine_id: &str,
        display_name: &str,
        failure: String,
        fetched_at_ms: Option<i64>,
    ) {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.engine_id == engine_id)
            && self.entries[index].report.is_some()
        {
            self.entries[index].failure = Some(failure);
            self.finish_refresh(engine_id);
            return;
        }
        self.accept(NativeUsageEntry {
            engine_id: engine_id.to_owned(),
            display_name: display_name.to_owned(),
            report: None,
            failure: Some(failure),
            fetched_at_ms,
        });
    }

    /// Seq-aware refresh-failure settlement. A stale same-engine failure
    /// returns `false` and changes nothing; the current failure keeps the
    /// last-good meters and records the failure alongside them.
    pub fn try_accept_failure(
        &mut self,
        engine_id: &str,
        display_name: &str,
        failure: String,
        fetched_at_ms: Option<i64>,
        request_seq: u64,
    ) -> bool {
        if self.pending_seq(engine_id) != Some(request_seq) {
            return false;
        }
        self.accept_failure(engine_id, display_name, failure, fetched_at_ms);
        true
    }
}

/// Application-minted monotonic identity for one Forge connection scope.
///
/// Every account-usage request and reply carries this fence so a late reply
/// from a previous connection can never populate a newer menu.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProfileUsageGeneration(u64);

impl ProfileUsageGeneration {
    /// Returns the first valid connection generation.
    #[must_use]
    pub const fn first() -> Self {
        Self(1)
    }

    /// Returns the next generation, or `None` when the counter is exhausted.
    #[must_use]
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }

    /// Returns the finite generation number for test and correlation checks.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

}

/// Stable per-engine roster in backend order.
///
/// Display names are the provider-owned names from the engine-usage adapter
/// contract. The adapter never invents rows outside this roster.
pub const PROFILE_USAGE_ROSTER: [(&str, &str); 6] = [
    ("codex", "Codex"),
    ("claude", "Claude"),
    ("cursor", "Cursor"),
    ("grok", "Grok Build"),
    ("hermes", "Hermes"),
    ("opencode2", "OpenCode"),
];

/// Returns the provider-owned display name for one roster engine.
#[must_use]
pub fn profile_usage_display_name(engine_id: &str) -> &str {
    PROFILE_USAGE_ROSTER
        .iter()
        .find(|(id, _)| *id == engine_id)
        .map_or(engine_id, |(_, display)| *display)
}

/// Returns whether one provider reading is still inside the shared 180s
/// freshness window.
#[must_use]
pub fn profile_usage_is_fresh(fetched_at_ms: Option<i64>, now_ms: i64) -> bool {
    !crate::engine_usage_cache::engine_usage_refresh_is_due(fetched_at_ms, now_ms)
}

/// Pure open/refresh plan for the profile menu.
///
/// Returns the roster engine ids that need a new read: missing rows, stale
/// rows outside the 180s window, or every row when `force` is set. Engines
/// with an admitted in-flight read are deduplicated unless `force` supersedes
/// the preceding non-forced flight. The caller owns dispatch and pending-row
/// creation; this function never invents provider data.
#[must_use]
pub fn plan_profile_usage_loads(
    state: &NativeProfileUsageState,
    now_ms: i64,
    force: bool,
    only_engine_id: Option<&str>,
) -> Vec<String> {
    let mut wanted: Vec<String> = Vec::new();
    for (engine_id, _) in PROFILE_USAGE_ROSTER {
        if let Some(only) = only_engine_id
            && engine_id != only
        {
            continue;
        }
        let refreshing = state
            .refreshing_engine_ids
            .iter()
            .any(|current| current == engine_id);
        if refreshing && !force {
            continue;
        }
        if !force
            && let Some(entry) = state.entry(engine_id)
            && profile_usage_is_fresh(entry.fetched_at_ms, now_ms)
        {
            continue;
        }
        wanted.push(engine_id.to_owned());
    }
    wanted
}

/// Returns whether one account-usage reply belongs to the active scope.
///
/// All four must match exactly: connection generation, engine id, the engine
/// still being marked refreshing, and the per-engine request sequence still
/// being the latest admitted one. A stale generation, an unknown engine, a
/// reply for an engine that is no longer refreshing, or an older same-engine
/// sequence superseded by a forced refresh is dropped without touching
/// sibling rows and without settling the newer pending request.
#[must_use]
pub fn account_usage_response_current(
    state: &NativeProfileUsageState,
    generation: ProfileUsageGeneration,
    current_generation: ProfileUsageGeneration,
    engine_id: &str,
    request_seq: u64,
) -> bool {
    generation == current_generation
        && PROFILE_USAGE_ROSTER.iter().any(|(id, _)| *id == engine_id)
        && state
            .refreshing_engine_ids
            .iter()
            .any(|current| current == engine_id)
        && state.pending_seq(engine_id) == Some(request_seq)
}

/// Returns the position of the exactly one narrowed report, if the snapshot
/// carries precisely one report and it belongs to the requested engine.
///
/// A narrowed `ReadAccountUsage` read must answer exactly one report echoing
/// the requested id (including the unknown-id failure row). Extra reports
/// are never scanned through: a multi-report snapshot cannot settle a
/// narrowed read.
#[must_use]
pub fn narrowed_report_position(engine_ids: &[&str], requested_engine_id: &str) -> Option<usize> {
    if engine_ids.len() != 1 {
        return None;
    }
    (engine_ids[0] == requested_engine_id).then_some(0)
}

/// Roster engines whose adapters expose a real account-usage surface.
///
/// These engines get a usage verdict for account state: `codex` reads
/// `account/rateLimits/read`, `claude` parses `claude -p /usage`, and
/// `cursor` posts its dashboard endpoint. This mirrors the backend roster
/// contract in `modules/backend/src/account_usage_service.rs`: `grok`,
/// `hermes`, and `opencode2` expose no account-usage surface. Admission to
/// run is narrower (see [`CLI_PROBED_ENGINES`]): the dashboard read proves
/// a Cursor account, never a local CLI installation.
pub const ACCOUNT_GATED_ENGINES: [&str; 3] = ["codex", "claude", "cursor"];

/// Roster engines whose usage read proves a responding local CLI.
///
/// Only `codex` (`account/rateLimits/read`) and `claude` (`claude -p
/// /usage`) probe an installed executable, so only their authenticated
/// usage admits static models to run. `cursor` posts an HTTP dashboard
/// endpoint: its account verdict stays visible, but dashboard auth never
/// proves a local CLI installation, so Cursor runtime admission follows
/// the backend catalog marking instead of the usage overlay.
pub const CLI_PROBED_ENGINES: [&str; 2] = ["codex", "claude"];

/// Backend-probed account readiness for one native engine.
///
/// Derived only from the provider-owned usage rows the backend probed with
/// real non-billable reads. A fresh `Authenticated` report proves the
/// installed executable and the shared ambient account at once, so the
/// static catalog models for that engine are admittable without any managed
/// `OpenCode` profile or registry. Anything else stays unrunnable with an
/// honest reason; readiness is never synthesized from a missing row, and a
/// stale last-good report never counts as fresh readiness indefinitely.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EngineReadiness {
    /// The provider authenticated this account inside the freshness window;
    /// static models may run.
    Ready,
    /// The provider reports no signed-in account.
    NeedsSignIn,
    /// The provider is unreachable, failing, stale, or has no account data yet.
    NotReady,
    /// A usage read is in flight; the verdict is pending.
    Checking,
}

/// Derives one engine's account readiness from its probed usage row.
///
/// `now_ms` bounds last-good optimism with the same 180-second freshness
/// window both layers share ([`profile_usage_is_fresh`]): an authenticated
/// report older than that is `NotReady` (or `Checking` while its refresh is
/// admitted), never `Ready`, so a signed-out or uninstalled engine cannot
/// ride a stale report indefinitely. A fresh authenticated report with a
/// refresh failure alongside stays `Ready` — the backend deliberately serves
/// last-good on transient refresh failures — while the failure itself stays
/// visible through [`engine_refresh_failure`] for actionable Settings
/// status. Unknown engine ids and rows without a usable report are never
/// ready: a missing row settles to `Checking` only while its refresh is
/// admitted, otherwise `NotReady`, so the composer cannot mistake an
/// unprobed engine for a runnable one.
#[must_use]
pub fn engine_readiness(
    state: &NativeProfileUsageState,
    engine_id: &str,
    now_ms: i64,
) -> EngineReadiness {
    let refreshing = state
        .refreshing_engine_ids
        .iter()
        .any(|current| current == engine_id);
    match state.entry(engine_id) {
        Some(entry) => match entry.report.as_ref().map(|report| report.authentication) {
            Some(NativeUsageAuthentication::Authenticated) => {
                if profile_usage_is_fresh(entry.fetched_at_ms, now_ms) {
                    EngineReadiness::Ready
                } else if refreshing {
                    EngineReadiness::Checking
                } else {
                    EngineReadiness::NotReady
                }
            }
            Some(NativeUsageAuthentication::Unauthenticated) => EngineReadiness::NeedsSignIn,
            _ => {
                if entry.failure.is_some() || !refreshing {
                    EngineReadiness::NotReady
                } else {
                    EngineReadiness::Checking
                }
            }
        },
        None => {
            if refreshing {
                EngineReadiness::Checking
            } else {
                EngineReadiness::NotReady
            }
        }
    }
}

/// Returns the actionable refresh failure for one engine, if any.
///
/// This is the report-level provider failure (for example a timed-out
/// refresh served over last-good meters) or the transport failure recorded
/// when no report arrived. Callers paint it next to the last-good state with
/// a retry action; it never clears the stored account verdict on its own.
#[must_use]
pub fn engine_refresh_failure(state: &NativeProfileUsageState, engine_id: &str) -> Option<String> {
    let entry = state.entry(engine_id)?;
    entry
        .report
        .as_ref()
        .and_then(|report| report.failure.clone())
        .or_else(|| entry.failure.clone())
}

/// Returns whether one CLI-probed engine's static catalog models may run.
///
/// Only a fresh backend-authenticated usage report admits them, and only
/// for the CLI-probed subset ([`CLI_PROBED_ENGINES`]): a dashboard read
/// (Cursor) never proves a local installation. Engines without an account
/// surface (`grok`, `hermes`, `opencode2`) never qualify here either; the
/// overlay preserves their snapshot marking instead.
#[must_use]
pub fn engine_static_models_admittable(
    state: &NativeProfileUsageState,
    engine_id: &str,
    now_ms: i64,
) -> bool {
    CLI_PROBED_ENGINES.contains(&engine_id)
        && matches!(
            engine_readiness(state, engine_id, now_ms),
            EngineReadiness::Ready
        )
}

/// Overlays backend-probed readiness onto a catalog snapshot.
///
/// The CLI-probed native subset ([`CLI_PROBED_ENGINES`]) is recomputed from
/// scratch on every overlay: a freshly authenticated engine joins
/// `runnable_harness_ids` once, and an engine whose latest verdict is
/// anything else leaves it — so a previously admitted engine never survives
/// a later signed-out, failed, or stale report when overlaid on the current
/// snapshot. Every other runnable id is preserved verbatim: genuine managed
/// `OpenCode` readiness from backend discovery, harness support for
/// surfaceless engines, and the backend catalog marking for the
/// dashboard-read `cursor` engine (whose usage auth never proves a local
/// CLI). The shared admission policy (`admit_policy`, `validate_policy`)
/// then treats the probed static models as runnable without inventing
/// routes, versions, or account facts.
#[must_use]
pub fn catalog_with_usage_readiness(
    catalog: crate::native_model_catalog::NativeModelCatalog,
    usage: &NativeProfileUsageState,
    now_ms: i64,
) -> crate::native_model_catalog::NativeModelCatalog {
    let mut catalog = catalog;
    let mut runnable: Vec<String> = catalog
        .runnable_harness_ids
        .iter()
        .filter(|id| !CLI_PROBED_ENGINES.contains(&id.as_str()))
        .cloned()
        .collect();
    for engine_id in CLI_PROBED_ENGINES {
        if engine_static_models_admittable(usage, engine_id, now_ms)
            && !runnable.iter().any(|ready| ready == engine_id)
        {
            runnable.push(engine_id.to_owned());
        }
    }
    catalog.runnable_harness_ids = runnable;
    catalog
}

/// One cadence group in Electron's display order.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeUsageWindowGroup {
    /// The group's cadence heading.
    pub cadence: NativeUsageCadence,
    /// Unique provider windows in account-wide-then-labeled order.
    pub windows: Vec<NativeUsageWindow>,
}

/// Groups windows using the Electron menu's ordering and duplicate policy.
#[must_use]
pub fn group_usage_windows(windows: &[NativeUsageWindow]) -> Vec<NativeUsageWindowGroup> {
    let mut unique = Vec::new();
    for window in windows {
        if let Some(index) = unique
            .iter()
            .position(|current: &NativeUsageWindow| current.id == window.id)
        {
            // This is the Vec equivalent of the Electron `new Map(...)`
            // projection: the value is replaced, while the first key
            // position remains stable.
            unique[index] = window.clone();
        } else {
            unique.push(window.clone());
        }
    }
    let unique = unique
        .into_iter()
        .filter(NativeUsageWindow::has_valid_percentage)
        .collect::<Vec<_>>();

    let mut groups = Vec::new();
    for cadence in [
        NativeUsageCadence::Session,
        NativeUsageCadence::Weekly,
        NativeUsageCadence::Monthly,
        NativeUsageCadence::Unknown,
    ] {
        let mut group_windows = Vec::new();
        group_windows.extend(
            unique
                .iter()
                .filter(|window| window.cadence == cadence && window.label.is_none())
                .cloned(),
        );
        group_windows.extend(
            unique
                .iter()
                .filter(|window| window.cadence == cadence && window.label.is_some())
                .cloned(),
        );
        if !group_windows.is_empty() {
            groups.push(NativeUsageWindowGroup {
                cadence,
                windows: group_windows,
            });
        }
    }
    groups
}

/// Returns the Electron reset sentence fragment for one complete group.
#[must_use]
pub fn reset_duration(windows: &[NativeUsageWindow], at_ms: i64) -> Option<String> {
    let adapters = windows
        .iter()
        .map(|window| UsageResetWindow::new(window.resets_at.as_deref()))
        .collect::<Vec<_>>();
    usage_reset_duration(&adapters, at_ms)
}

/// Returns the tooltip's remaining quota for one meter reading.
///
/// This mirrors the Electron tooltip's `Math.max(0, 100 -
/// Math.round(percent_used))`: the exact remaining percentage, while the
/// meter itself stays ceil-quantized. Non-finite input cannot reach a meter
/// and deterministically reads as fully used.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "the clamped percentage is converted to the integer remaining-percent count the reference tooltip rounds to"
)]
pub fn usage_remaining_percent(percent_used: f64) -> i64 {
    if !percent_used.is_finite() {
        return 0;
    }
    (100.0 - percent_used.round()).clamp(0.0, 100.0) as i64
}

/// First-reading start for the shared remaining tween: just short of the
/// target so the number is legible the whole way rather than spinning up
/// from zero. Mirrors `RunUpFrom` in `usage-window-motion.ts`.
#[must_use]
pub fn tip_run_up_from(target: f64) -> f64 {
    (target - 1.0f64.max((target * 0.08).round())).max(0.0)
}

/// Returns the provider-specific “last checked” label used by Electron.
#[must_use]
pub fn checked_label(fetched_at_ms: Option<i64>, now_ms: i64) -> Option<String> {
    let fetched_at_ms = fetched_at_ms?;
    let minutes = (now_ms.saturating_sub(fetched_at_ms) / 60_000).max(0);
    if minutes < 1 {
        return Some("last checked now".to_owned());
    }
    if minutes < 60 {
        return Some(format!("last checked {minutes} min ago"));
    }
    let hours = minutes / 60;
    if hours < 24 {
        return Some(format!("last checked {hours} hr ago"));
    }
    Some(format!("last checked {} d ago", hours / 24))
}

#[cfg(test)]
#[path = "native_profile_usage/tests.rs"]
mod tests;


