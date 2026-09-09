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

    /// Returns whether at least one genuine provider response is available.
    #[must_use]
    pub fn has_real_data(&self) -> bool {
        self.entries.iter().any(NativeUsageEntry::has_response)
    }
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
}
