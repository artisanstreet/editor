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

    #[cfg(test)]
    pub(crate) const fn from_raw_for_test(value: u64) -> Self {
        Self(value)
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
        .filter(|window| window.has_valid_percentage())
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
mod tests {
    use super::*;

    fn window(
        id: &str,
        cadence: NativeUsageCadence,
        label: Option<&str>,
        percent_used: f64,
    ) -> NativeUsageWindow {
        NativeUsageWindow {
            id: id.to_owned(),
            cadence,
            label: label.map(str::to_owned),
            percent_used,
            resets_at: None,
            window_minutes: None,
        }
    }

    #[test]
    fn groups_follow_cadence_and_put_account_window_first() {
        let groups = group_usage_windows(&[
            window(
                "weekly-model",
                NativeUsageCadence::Weekly,
                Some("Model"),
                25.0,
            ),
            window("monthly", NativeUsageCadence::Monthly, None, 10.0),
            window("weekly-all", NativeUsageCadence::Weekly, None, 20.0),
            window(
                "weekly-model",
                NativeUsageCadence::Weekly,
                Some("duplicate"),
                99.0,
            ),
        ]);

        assert_eq!(
            groups.iter().map(|group| group.cadence).collect::<Vec<_>>(),
            vec![NativeUsageCadence::Weekly, NativeUsageCadence::Monthly]
        );
        assert_eq!(groups[0].windows[0].scope_label(), "All models");
        assert_eq!(groups[0].windows[1].scope_label(), "duplicate");
        assert_eq!(groups[0].windows[1].percent_used, 99.0);
        assert_eq!(groups[0].windows.len(), 2);
    }

    #[test]
    fn invalid_percentages_are_not_painted_as_zero() {
        let groups = group_usage_windows(&[
            window("nan", NativeUsageCadence::Session, None, f64::NAN),
            window("too-high", NativeUsageCadence::Session, None, 101.0),
        ]);
        assert!(groups.is_empty());
    }

    fn report(
        engine_id: &str,
        authentication: NativeUsageAuthentication,
        windows: Vec<NativeUsageWindow>,
    ) -> NativeUsageReport {
        NativeUsageReport {
            engine_id: engine_id.to_owned(),
            display_name: profile_usage_display_name(engine_id).to_owned(),
            authentication,
            account_email: None,
            quota_surface: NativeUsageQuotaSurface::Supported,
            windows,
            failure: None,
        }
    }

    fn presented_entry(
        engine_id: &str,
        report: Option<NativeUsageReport>,
        failure: Option<&str>,
    ) -> NativeUsageEntry {
        NativeUsageEntry {
            engine_id: engine_id.to_owned(),
            display_name: profile_usage_display_name(engine_id).to_owned(),
            report,
            failure: failure.map(str::to_owned),
            fetched_at_ms: Some(1_000_000),
        }
    }

    fn visible_ids(entries: &[NativeUsageEntry], refreshing: &[&str]) -> Vec<String> {
        NativeProfileUsageState {
            entries: entries.to_vec(),
            refreshing_engine_ids: refreshing.iter().map(|id| (*id).to_owned()).collect(),
            pending_read_seq: Vec::new(),
        }
        .visible_usage_entries()
        .iter()
        .map(|entry| entry.engine_id.clone())
        .collect()
    }

    #[test]
    fn dropdown_hides_providers_without_renderable_data() {
        let zero = presented_entry(
            "zero",
            Some(report(
                "zero",
                NativeUsageAuthentication::Authenticated,
                vec![window("session", NativeUsageCadence::Session, None, 0.0)],
            )),
            None,
        );
        let empty = presented_entry(
            "empty",
            Some(report(
                "empty",
                NativeUsageAuthentication::Authenticated,
                Vec::new(),
            )),
            None,
        );
        let invalid = presented_entry(
            "invalid",
            Some(report(
                "invalid",
                NativeUsageAuthentication::Authenticated,
                vec![
                    window("nan", NativeUsageCadence::Session, None, f64::NAN),
                    window("high", NativeUsageCadence::Session, None, 120.0),
                ],
            )),
            None,
        );
        let unauthenticated = presented_entry(
            "unauthenticated",
            Some(report(
                "unauthenticated",
                NativeUsageAuthentication::Unauthenticated,
                vec![window("session", NativeUsageCadence::Session, None, 40.0)],
            )),
            None,
        );
        let failed = presented_entry("failed", None, Some("transport"));
        let pending = NativeUsageEntry::pending("pending", "Pending");
        // A zero percentage is real data and stays visible; everything
        // without a renderable authenticated window is hidden.
        assert_eq!(
            visible_ids(
                &[zero, empty, invalid, unauthenticated, failed, pending],
                &[]
            ),
            vec!["zero"]
        );
    }

    #[test]
    fn dropdown_keeps_last_good_windows_while_refreshing_or_failed() {
        let windows = vec![window("session", NativeUsageCadence::Session, None, 62.0)];
        let refreshing = presented_entry(
            "refreshing",
            Some(report(
                "refreshing",
                NativeUsageAuthentication::Authenticated,
                windows.clone(),
            )),
            None,
        );
        let mut failed = report("failed", NativeUsageAuthentication::Authenticated, windows);
        failed.failure = Some("stale".to_owned());
        let failed = presented_entry("failed", Some(failed), Some("transport"));
        assert_eq!(
            visible_ids(&[refreshing, failed], &["refreshing"]),
            vec!["refreshing", "failed"]
        );
    }

    #[test]
    fn reset_duration_requires_every_window_to_have_a_future_reset() {
        let mut first = window("first", NativeUsageCadence::Session, None, 25.0);
        first.resets_at = Some("2030-01-01T00:00:00Z".to_owned());
        let mut second = window("second", NativeUsageCadence::Session, Some("model"), 50.0);
        second.resets_at = Some("2030-01-01T00:30:00Z".to_owned());

        assert_eq!(
            reset_duration(&[first.clone(), second.clone()], 1_893_454_200_000),
            Some("1 hour".to_owned())
        );
        second.resets_at = None;
        assert_eq!(reset_duration(&[first, second], 1_893_454_200_000), None);
    }

    #[test]
    fn state_keeps_newer_provider_reading_and_separate_refresh_state() {
        let mut state = NativeProfileUsageState {
            entries: vec![NativeUsageEntry {
                engine_id: "claude".to_owned(),
                display_name: "Claude".to_owned(),
                report: None,
                failure: Some("temporary".to_owned()),
                fetched_at_ms: Some(20),
            }],
            refreshing_engine_ids: vec!["claude".to_owned()],
            pending_read_seq: vec![("claude".to_owned(), 4)],
        };
        state.accept(NativeUsageEntry {
            engine_id: "claude".to_owned(),
            display_name: "Claude".to_owned(),
            report: None,
            failure: Some("stale".to_owned()),
            fetched_at_ms: Some(10),
        });
        assert_eq!(
            state.entry("claude").and_then(|entry| entry.fetched_at_ms),
            Some(20)
        );
        assert!(state.refreshing_engine_ids.is_empty());

        state.finish_refresh("claude");
        assert!(state.refreshing_engine_ids.is_empty());
    }

    #[test]
    fn checked_labels_use_each_provider_timestamp() {
        assert_eq!(checked_label(None, 10_000), None);
        assert_eq!(
            checked_label(Some(10_000), 10_000),
            Some("last checked now".to_owned())
        );
        assert_eq!(
            checked_label(Some(0), 3_600_000),
            Some("last checked 1 hr ago".to_owned())
        );
    }

    #[test]
    fn remaining_percent_rounds_like_the_tooltip() {
        assert_eq!(usage_remaining_percent(62.4), 38);
        assert_eq!(usage_remaining_percent(0.0), 100);
        assert_eq!(usage_remaining_percent(100.0), 0);
        assert_eq!(usage_remaining_percent(120.0), 0);
        assert_eq!(usage_remaining_percent(f64::NAN), 0);
    }

    #[test]
    fn tip_run_up_starts_just_short_of_its_target() {
        assert_eq!(tip_run_up_from(38.0), 35.0);
        assert_eq!(tip_run_up_from(100.0), 92.0);
        assert_eq!(tip_run_up_from(0.0), 0.0);
    }

    fn entry_with_time(engine_id: &str, fetched_at_ms: Option<i64>) -> NativeUsageEntry {
        NativeUsageEntry {
            engine_id: engine_id.to_owned(),
            display_name: profile_usage_display_name(engine_id).to_owned(),
            report: None,
            failure: Some("transport".to_owned()),
            fetched_at_ms,
        }
    }

    #[test]
    fn opening_dispatches_missing_rows_and_freshness_avoids_repeats() {
        let mut state = NativeProfileUsageState::default();
        let now_ms = 1_000_000;
        let first = plan_profile_usage_loads(&state, now_ms, false, None);
        assert_eq!(first.len(), PROFILE_USAGE_ROSTER.len());

        for engine_id in &first {
            state.entries.push(entry_with_time(engine_id, Some(now_ms)));
        }
        assert!(plan_profile_usage_loads(&state, now_ms, false, None).is_empty());

        let stale_ms = now_ms - 181_000;
        state.entries[0].fetched_at_ms = Some(stale_ms);
        assert_eq!(
            plan_profile_usage_loads(&state, now_ms, false, None),
            vec![state.entries[0].engine_id.clone()]
        );
    }

    #[test]
    fn forced_refresh_loads_everything_and_keeps_menu_scope() {
        let mut state = NativeProfileUsageState::default();
        let now_ms = 2_000_000;
        for (engine_id, _) in PROFILE_USAGE_ROSTER {
            state.entries.push(entry_with_time(engine_id, Some(now_ms)));
        }
        let forced = plan_profile_usage_loads(&state, now_ms, true, None);
        assert_eq!(forced.len(), PROFILE_USAGE_ROSTER.len());

        let single = plan_profile_usage_loads(&state, now_ms, true, Some("claude"));
        assert_eq!(single, vec!["claude".to_owned()]);
    }

    #[test]
    fn inflight_reads_deduplicate_until_forced() {
        let mut state = NativeProfileUsageState::default();
        state.begin_refresh("codex");
        let now_ms = 3_000_000;
        assert!(plan_profile_usage_loads(&state, now_ms, false, Some("codex")).is_empty());
        assert_eq!(
            plan_profile_usage_loads(&state, now_ms, true, Some("codex")),
            vec!["codex".to_owned()]
        );
    }

    #[test]
    fn response_pairing_rejects_wrong_generation_engine_or_sequence() {
        let mut state = NativeProfileUsageState::default();
        state.begin_refresh_seq("codex", 11);
        let current = ProfileUsageGeneration::first();
        let next = current.checked_next().expect("next generation");
        assert!(account_usage_response_current(
            &state, current, current, "codex", 11
        ));
        assert!(!account_usage_response_current(
            &state, current, next, "codex", 11
        ));
        assert!(!account_usage_response_current(
            &state, current, current, "claude", 11
        ));
        assert!(!account_usage_response_current(
            &state,
            current,
            current,
            "unknown-engine",
            11
        ));
        assert!(!account_usage_response_current(
            &state, current, current, "codex", 10
        ));
    }

    #[test]
    fn old_same_engine_reply_after_force_cannot_settle_or_replace() {
        let mut state = NativeProfileUsageState::default();
        state.begin_refresh_seq("codex", 1);
        // A forced refresh while the first read is pending supersedes it.
        state.begin_refresh_seq("codex", 2);
        let current = ProfileUsageGeneration::first();
        assert!(!account_usage_response_current(
            &state, current, current, "codex", 1
        ));
        assert!(account_usage_response_current(
            &state, current, current, "codex", 2
        ));

        // The older reply is dropped: pending stays armed and no row appears.
        assert!(!state.try_accept(
            NativeUsageEntry {
                engine_id: "codex".to_owned(),
                display_name: "Codex".to_owned(),
                report: None,
                failure: Some("old".to_owned()),
                fetched_at_ms: Some(10),
            },
            1
        ));
        assert_eq!(state.pending_seq("codex"), Some(2));
        assert!(state.entry("codex").is_none());

        // A stale failure is dropped the same way.
        assert!(!state.try_accept_failure("codex", "Codex", "old".to_owned(), Some(11), 1));
        assert_eq!(state.pending_seq("codex"), Some(2));

        // The newer reply settles and replaces.
        assert!(state.try_accept(
            NativeUsageEntry {
                engine_id: "codex".to_owned(),
                display_name: "Codex".to_owned(),
                report: None,
                failure: Some("new".to_owned()),
                fetched_at_ms: Some(20),
            },
            2
        ));
        assert_eq!(
            state.entry("codex").and_then(|entry| entry.fetched_at_ms),
            Some(20)
        );
        assert!(state.pending_seq("codex").is_none());
    }

    #[test]
    fn one_failure_preserves_siblings_and_stale_cannot_clear_refresh() {
        let mut state = NativeProfileUsageState::default();
        state.accept(NativeUsageEntry {
            engine_id: "codex".to_owned(),
            display_name: "Codex".to_owned(),
            report: Some(NativeUsageReport {
                engine_id: "codex".to_owned(),
                display_name: "Codex".to_owned(),
                authentication: NativeUsageAuthentication::Authenticated,
                account_email: None,
                quota_surface: NativeUsageQuotaSurface::Supported,
                windows: Vec::new(),
                failure: None,
            }),
            failure: None,
            fetched_at_ms: Some(100),
        });
        state.begin_refresh("codex");
        state.begin_refresh("claude");

        state.accept_failure("claude", "Claude", "busy".to_owned(), Some(50));
        assert!(
            state
                .entry("codex")
                .is_some_and(|entry| entry.report.is_some())
        );
        assert!(
            state
                .entry("claude")
                .is_some_and(|entry| entry.has_response())
        );

        state.accept(NativeUsageEntry {
            engine_id: "codex".to_owned(),
            display_name: "Codex".to_owned(),
            report: None,
            failure: Some("stale".to_owned()),
            fetched_at_ms: Some(10),
        });
        assert_eq!(
            state.entry("codex").and_then(|entry| entry.fetched_at_ms),
            Some(100)
        );
    }

    #[test]
    fn refresh_failure_keeps_last_good_meters_and_stays_visible() {
        let mut state = NativeProfileUsageState::default();
        state.accept(NativeUsageEntry {
            engine_id: "cursor".to_owned(),
            display_name: "Cursor".to_owned(),
            report: Some(NativeUsageReport {
                engine_id: "cursor".to_owned(),
                display_name: "Cursor".to_owned(),
                authentication: NativeUsageAuthentication::Authenticated,
                account_email: None,
                quota_surface: NativeUsageQuotaSurface::Supported,
                windows: vec![window("five_hour", NativeUsageCadence::Session, None, 42.0)],
                failure: None,
            }),
            failure: None,
            fetched_at_ms: Some(77),
        });
        state.begin_refresh_seq("cursor", 9);
        state.accept_failure("cursor", "Cursor", "peer".to_owned(), Some(78));
        let entry = state.entry("cursor").expect("cursor row");
        // Meters stay: the last-good report and its original observation time
        // are preserved, while the refresh failure is stored alongside.
        assert_eq!(
            entry.report.as_ref().map(|report| report.windows.len()),
            Some(1)
        );
        assert_eq!(entry.failure.as_deref(), Some("peer"));
        assert_eq!(entry.fetched_at_ms, Some(77));
        assert!(!state.refreshing_engine_ids.contains(&"cursor".to_owned()));
    }

    #[test]
    fn provider_failure_with_windows_keeps_both_visible() {
        let report = NativeUsageReport {
            engine_id: "codex".to_owned(),
            display_name: "Codex".to_owned(),
            authentication: NativeUsageAuthentication::Authenticated,
            account_email: None,
            quota_surface: NativeUsageQuotaSurface::Supported,
            windows: vec![window("seven_day", NativeUsageCadence::Weekly, None, 10.0)],
            failure: Some("partial read".to_owned()),
        };
        assert_eq!(report.renderable_windows().len(), 1);
        assert_eq!(report.failure.as_deref(), Some("partial read"));
    }

    #[test]
    fn narrowed_pairing_accepts_exactly_one_matching_report() {
        assert_eq!(narrowed_report_position(&["codex"], "codex"), Some(0));
        assert_eq!(narrowed_report_position(&[], "codex"), None);
        assert_eq!(narrowed_report_position(&["claude"], "codex"), None);
        assert_eq!(
            narrowed_report_position(&["codex", "claude"], "codex"),
            None
        );
        assert_eq!(
            narrowed_report_position(&["unknown-engine"], "unknown-engine"),
            Some(0)
        );
    }

    #[test]
    fn disconnect_clears_incompatible_cache_and_pending() {
        let mut state = NativeProfileUsageState::default();
        state.entries.push(entry_with_time("codex", Some(10)));
        state.begin_refresh_seq("claude", 3);
        state.clear_for_connection();
        assert!(state.entries.is_empty());
        assert!(state.refreshing_engine_ids.is_empty());
        assert_eq!(state.pending_seq("claude"), None);
    }
}
