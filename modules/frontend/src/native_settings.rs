//! Native settings shell composition over static fixture data (no Forge).
//!
//! # Live vs static inventory
//!
//! LIVE (pure, deterministic, covered by the inline tests below):
//! - Navigation selection state ([`SettingsShell::select`], [`SettingsShell::selected`]).
//! - Route-to-section resolution ([`section_for_href`]) and per-section mount
//!   ([`SettingsShell::outlet`], [`section_snapshot`]).
//! - Fixture constructors ([`fixture_models`], [`fixture_telemetry`],
//!   [`fixture_retention_policy`], [`fixture_retention_state`],
//!   [`fixture_usage_recovery`], [`fixture_notifications`],
//!   [`fixture_engine_template`]) and the policy helpers that read them
//!   ([`thinking_for_fixture_model`], [`models_for_fixture_engine`],
//!   [`resolve_fixture_telemetry`], [`fixture_retention_is_valid`]).
//! - Primitive style resolvers that delegate to the real `artisan-ui`
//!   recipes ([`fixture_card_style`], [`fixture_switch_style`],
//!   [`fixture_tabs_style`], [`fixture_toggle_group_style`],
//!   [`fixture_tooltip_style`], [`fixture_collapsible_state`]) and the sticky
//!   nav tab specs ([`nav_tab_specs`]).
//!
//! STATIC (visually present but non-functional by design; no callbacks):
//! - Every switch, tabs, toggle group, card, collapsible, and tooltip
//!   control. There are no activation handlers, no persistence, and no
//!   transport calls in this module.
//! - All section copy (titles, descriptions, anchors). The strings mirror the
//!   legacy `routes/settings` Svelte copy verbatim so the shell can paint
//!   without a Forge connection.
//! - All policy values. Fixtures never contact Forge, never spawn entities,
//!   and never mutate durable state; the controllers that own admission,
//!   streams, and saves live outside this shell.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_ui::card::CardStyle;
use artisan_ui::collapsible::CollapsibleState;
use artisan_ui::switch::{SwitchSize, SwitchStyle};
use artisan_ui::tabs::{TabSpec, TabsOrientation, TabsStyle, TabsVariant};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use artisan_ui::toggle_group::{ToggleGroupSize, ToggleGroupStyle, ToggleGroupVariant};
use artisan_ui::tooltip::TooltipStyle;

use crate::engine_settings::{EngineSettingsStatus, manual_configuration_template};
use crate::model_selection_presentation::{
    ModelChoice, ModelDefinition, SessionDefaults, ThinkingCapability, ThinkingLevel,
    models_for_engine, thinking_for_defaults,
};
use crate::notification_preferences::{NotificationPreferences, RuntimeSurface};
use crate::telemetry_preferences::{TelemetryPreferences, resolve_get_telemetry_preferences};
use crate::thread_retention_settings_policy::{
    ThreadRetentionPolicy, ThreadRetentionSettingsState,
};
use crate::usage_recovery_settings_policy::{
    UsageRecoveryAuthoritativeState, UsageRecoverySettingsState,
};

/// Fixture engine id used for the static engines outlet.
pub const FIXTURE_ENGINE_ID: &str = "fixture-engine";

/// Fixture engine label used for the static engines outlet.
pub const FIXTURE_ENGINE_LABEL: &str = "Fixture Engine";

/// Fixture retention threshold in days.
pub const FIXTURE_RETENTION_DAYS: u16 = 30;

/// One sticky-nav anchor (`/settings/section#hash`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SettingsAnchor {
    /// Fragment without the leading `#`.
    pub hash: &'static str,
    /// Visible anchor label.
    pub label: &'static str,
}

impl SettingsAnchor {
    /// Creates one anchor from its fragment and visible label.
    #[must_use]
    pub const fn new(hash: &'static str, label: &'static str) -> Self {
        Self { hash, label }
    }
}

/// Visual primitive families painted by the static settings outlets.
///
/// The shell never instantiates interactive GPUI elements here; the style
/// resolvers below prove each recipe resolves for the fixture theme while the
/// rendered controls stay non-functional.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SettingsPrimitive {
    /// Segmented tabs (sticky nav outlet switching).
    Tabs,
    /// On/off switch (availability, retention, telemetry, notifications).
    Switch,
    /// Segmented single-select (appearance formatting, prose width).
    ToggleGroup,
    /// Compact card container (every section body).
    Card,
    /// Expand/collapse (engine installation detail, never-collected note).
    Collapsible,
    /// Hover tooltip (unsupported notification host, model facts).
    Tooltip,
}

impl SettingsPrimitive {
    /// Returns the stable primitive name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tabs => "tabs",
            Self::Switch => "switch",
            Self::ToggleGroup => "toggle_group",
            Self::Card => "card",
            Self::Collapsible => "collapsible",
            Self::Tooltip => "tooltip",
        }
    }
}

/// One settings section, matching the legacy `routes/settings` directories.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SettingsSection {
    /// `/settings/models`.
    Models,
    /// `/settings/appearance`.
    Appearance,
    /// `/settings/engines/[engine]` (fixture engine).
    Engines,
    /// `/settings/notifications`.
    Notifications,
    /// `/settings/privacy`.
    Privacy,
    /// `/settings/threads`.
    Threads,
}

const MODELS_ANCHORS: [SettingsAnchor; 2] = [
    SettingsAnchor::new("compaction", "Compaction"),
    SettingsAnchor::new("favorites", "Favorites"),
];

const APPEARANCE_ANCHORS: [SettingsAnchor; 4] = [
    SettingsAnchor::new("app-icon", "App icon"),
    SettingsAnchor::new("typography", "Typography"),
    SettingsAnchor::new("glass", "Glass"),
    SettingsAnchor::new("reading", "Reading"),
];

const ENGINES_ANCHORS: [SettingsAnchor; 4] = [
    SettingsAnchor::new("availability", "Availability"),
    SettingsAnchor::new("installation", "Installation"),
    SettingsAnchor::new("account", "Account"),
    SettingsAnchor::new("models", "Models"),
];

const NOTIFICATIONS_ANCHORS: [SettingsAnchor; 1] = [SettingsAnchor::new("system", "System")];

const PRIVACY_ANCHORS: [SettingsAnchor; 2] = [
    SettingsAnchor::new("telemetry", "Observability"),
    SettingsAnchor::new("never-collected", "Never collected"),
];

const THREADS_ANCHORS: [SettingsAnchor; 3] = [
    SettingsAnchor::new("retention", "Retention"),
    SettingsAnchor::new("usage-recovery", "Usage recovery"),
    SettingsAnchor::new("agents", "Agents"),
];

const MODELS_PRIMITIVES: [SettingsPrimitive; 3] = [
    SettingsPrimitive::Tabs,
    SettingsPrimitive::Card,
    SettingsPrimitive::Tooltip,
];

const APPEARANCE_PRIMITIVES: [SettingsPrimitive; 4] = [
    SettingsPrimitive::Tabs,
    SettingsPrimitive::Card,
    SettingsPrimitive::Switch,
    SettingsPrimitive::ToggleGroup,
];

const ENGINES_PRIMITIVES: [SettingsPrimitive; 4] = [
    SettingsPrimitive::Tabs,
    SettingsPrimitive::Card,
    SettingsPrimitive::Switch,
    SettingsPrimitive::Collapsible,
];

const NOTIFICATIONS_PRIMITIVES: [SettingsPrimitive; 4] = [
    SettingsPrimitive::Tabs,
    SettingsPrimitive::Card,
    SettingsPrimitive::Switch,
    SettingsPrimitive::Tooltip,
];

const PRIVACY_PRIMITIVES: [SettingsPrimitive; 3] = [
    SettingsPrimitive::Tabs,
    SettingsPrimitive::Card,
    SettingsPrimitive::Switch,
];

const THREADS_PRIMITIVES: [SettingsPrimitive; 3] = [
    SettingsPrimitive::Tabs,
    SettingsPrimitive::Card,
    SettingsPrimitive::Switch,
];

impl SettingsSection {
    /// Every section in sticky-nav order.
    pub const ALL: [Self; 6] = [
        Self::Models,
        Self::Appearance,
        Self::Engines,
        Self::Notifications,
        Self::Privacy,
        Self::Threads,
    ];

    /// Returns the legacy route href for this section.
    #[must_use]
    pub const fn href(self) -> &'static str {
        match self {
            Self::Models => "/settings/models",
            Self::Appearance => "/settings/appearance",
            Self::Engines => "/settings/engines/fixture-engine",
            Self::Notifications => "/settings/notifications",
            Self::Privacy => "/settings/privacy",
            Self::Threads => "/settings/threads",
        }
    }

    /// Returns the sticky-nav label for this section.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Models => "Models",
            Self::Appearance => "Appearance",
            Self::Engines => "Engines",
            Self::Notifications => "Notifications",
            Self::Privacy => "Privacy",
            Self::Threads => "Threads",
        }
    }

    /// Returns the header title painted by the static outlet.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Models => "Models",
            Self::Appearance => "Appearance",
            Self::Engines => "Fixture Engine",
            Self::Notifications => "Notifications",
            Self::Privacy => "Privacy",
            Self::Threads => "Threads",
        }
    }

    /// Returns the header description painted by the static outlet.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Models => "Forge-owned model defaults shared by every paired client.",
            Self::Appearance => "How Artisan's surfaces are drawn.",
            Self::Engines => {
                "Choose where Fixture Engine appears, manage its installation, and inspect its account and models."
            }
            Self::Notifications => "When Artisan is allowed to interrupt you.",
            Self::Privacy => {
                "Two independent choices for anonymous product analytics and sanitized crash reports."
            }
            Self::Threads => "Lifecycle rules the Forge applies to every thread.",
        }
    }

    /// Returns the deep-link anchors painted under this nav row.
    #[must_use]
    pub const fn anchors(self) -> &'static [SettingsAnchor] {
        match self {
            Self::Models => &MODELS_ANCHORS,
            Self::Appearance => &APPEARANCE_ANCHORS,
            Self::Engines => &ENGINES_ANCHORS,
            Self::Notifications => &NOTIFICATIONS_ANCHORS,
            Self::Privacy => &PRIVACY_ANCHORS,
            Self::Threads => &THREADS_ANCHORS,
        }
    }

    /// Returns the primitive families visually present in this outlet.
    #[must_use]
    pub const fn primitives(self) -> &'static [SettingsPrimitive] {
        match self {
            Self::Models => &MODELS_PRIMITIVES,
            Self::Appearance => &APPEARANCE_PRIMITIVES,
            Self::Engines => &ENGINES_PRIMITIVES,
            Self::Notifications => &NOTIFICATIONS_PRIMITIVES,
            Self::Privacy => &PRIVACY_PRIMITIVES,
            Self::Threads => &THREADS_PRIMITIVES,
        }
    }
}

/// Resolves a legacy settings href to its section.
///
/// The engines route carries a dynamic engine id; any href under
/// `/settings/engines` resolves to [`SettingsSection::Engines`]. A trailing
/// slash is accepted. Returns [`None`] for unknown paths.
#[must_use]
pub fn section_for_href(href: &str) -> Option<SettingsSection> {
    let trimmed = href.strip_suffix('/').unwrap_or(href);
    let without_hash = trimmed.split('#').next().unwrap_or(trimmed);
    match without_hash {
        "/settings/models" => Some(SettingsSection::Models),
        "/settings/appearance" => Some(SettingsSection::Appearance),
        "/settings/notifications" => Some(SettingsSection::Notifications),
        "/settings/privacy" => Some(SettingsSection::Privacy),
        "/settings/threads" => Some(SettingsSection::Threads),
        path if path == "/settings/engines" || path.starts_with("/settings/engines/") => {
            Some(SettingsSection::Engines)
        }
        _ => None,
    }
}

/// Static per-section outlet snapshot mounted by the shell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsSectionSnapshot {
    /// Mounted section.
    pub section: SettingsSection,
    /// Header title (static fixture copy).
    pub title: &'static str,
    /// Header description (static fixture copy).
    pub description: &'static str,
    /// Deep-link anchors for the sticky nav.
    pub anchors: &'static [SettingsAnchor],
    /// Primitive families visually present in the outlet.
    pub primitives: &'static [SettingsPrimitive],
}

impl SettingsSectionSnapshot {
    /// Creates a snapshot for one section from its static copy.
    #[must_use]
    pub const fn for_section(section: SettingsSection) -> Self {
        Self {
            section,
            title: section.title(),
            description: section.description(),
            anchors: section.anchors(),
            primitives: section.primitives(),
        }
    }
}

/// Returns the static outlet snapshot for one section.
#[must_use]
pub const fn section_snapshot(section: SettingsSection) -> SettingsSectionSnapshot {
    SettingsSectionSnapshot::for_section(section)
}

/// Sticky-nav shell composition: the selected section plus its outlet.
///
/// Only selection is live. Rendering belongs to a later GPUI slice; this
/// shell hands that slice a section and its static snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsShell {
    selected: SettingsSection,
}

impl SettingsShell {
    /// Creates a shell selecting the Models section.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            selected: SettingsSection::Models,
        }
    }

    /// Returns the currently selected section.
    #[must_use]
    pub const fn selected(self) -> SettingsSection {
        self.selected
    }

    /// Returns whether `section` is currently selected.
    #[must_use]
    pub const fn is_selected(self, section: SettingsSection) -> bool {
        matches!(
            (self.selected, section),
            (SettingsSection::Models, SettingsSection::Models)
                | (SettingsSection::Appearance, SettingsSection::Appearance,)
                | (SettingsSection::Engines, SettingsSection::Engines,)
                | (
                    SettingsSection::Notifications,
                    SettingsSection::Notifications,
                )
                | (SettingsSection::Privacy, SettingsSection::Privacy,)
                | (SettingsSection::Threads, SettingsSection::Threads,)
        )
    }

    /// Selects a section, returning whether the selection changed.
    pub fn select(&mut self, section: SettingsSection) -> bool {
        if self.selected == section {
            false
        } else {
            self.selected = section;
            true
        }
    }

    /// Returns the static outlet snapshot for the selected section.
    #[must_use]
    pub const fn outlet(self) -> SettingsSectionSnapshot {
        SettingsSectionSnapshot::for_section(self.selected)
    }
}

impl Default for SettingsShell {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns the sticky-nav tab specs in [`SettingsSection::ALL`] order.
///
/// The specs reuse the real `artisan-ui` tabs recipe inputs; selection stays
/// owned by [`SettingsShell`].
#[must_use]
pub fn nav_tab_specs() -> Vec<TabSpec> {
    SettingsSection::ALL
        .iter()
        .map(|section| TabSpec::new(section.href(), section.label()))
        .collect()
}

/// Returns the fixture theme used by every style resolver below.
#[must_use]
pub fn fixture_theme() -> ArtisanTheme {
    ArtisanTheme::for_mode(ThemeMode::Dark)
}

/// Resolves the compact card recipe for the fixture theme.
#[must_use]
pub fn fixture_card_style() -> CardStyle {
    CardStyle::resolve(fixture_theme())
}

/// Resolves the switch recipe for the fixture theme and checked value.
#[must_use]
pub fn fixture_switch_style(checked: bool) -> SwitchStyle {
    SwitchStyle::resolve(fixture_theme(), SwitchSize::Default, checked)
}

/// Resolves the segmented-tabs recipe for the fixture theme.
#[must_use]
pub fn fixture_tabs_style() -> TabsStyle {
    TabsStyle::resolve(
        fixture_theme(),
        TabsVariant::Default,
        TabsOrientation::Horizontal,
    )
}

/// Resolves the outline toggle-group recipe for the fixture theme.
#[must_use]
pub fn fixture_toggle_group_style() -> ToggleGroupStyle {
    ToggleGroupStyle::resolve(
        fixture_theme(),
        ToggleGroupVariant::Outline,
        ToggleGroupSize::Default,
        artisan_ui::theme::ArtisanTheme::for_mode(ThemeMode::Dark)
            .spacing
            .steps(0.0),
    )
}

/// Resolves the default tooltip recipe for the fixture theme.
#[must_use]
pub fn fixture_tooltip_style() -> TooltipStyle {
    TooltipStyle::resolve(fixture_theme())
}

/// Returns the controlled collapsible state for a static outlet block.
#[must_use]
pub const fn fixture_collapsible_state(open: bool) -> CollapsibleState {
    CollapsibleState::new(open, false)
}

/// Returns two static fixture models owned by the fixture engine.
#[must_use]
pub fn fixture_models() -> Vec<ModelChoice> {
    vec![
        ModelChoice::new(
            FIXTURE_ENGINE_ID,
            "fixture-model-a",
            ModelDefinition::new(
                ThinkingCapability::Supported {
                    default: ThinkingLevel::Medium,
                },
                None,
            ),
        ),
        ModelChoice::new(
            FIXTURE_ENGINE_ID,
            "fixture-model-b",
            ModelDefinition::new(ThinkingCapability::Unavailable, None),
        ),
    ]
}

/// Returns the static session defaults referenced by the models outlet.
#[must_use]
pub fn fixture_session_defaults() -> SessionDefaults {
    SessionDefaults::new(Vec::new())
}

/// Resolves the thinking level for one fixture model.
#[must_use]
pub fn thinking_for_fixture_model(model: &ModelChoice) -> Option<ThinkingLevel> {
    thinking_for_defaults(&fixture_session_defaults(), model)
}

/// Returns the fixture models for one engine id in catalog order.
#[must_use]
pub fn models_for_fixture_engine(engine: &str) -> Vec<ModelChoice> {
    models_for_engine(&fixture_models(), engine)
}

/// Returns the static telemetry fixture (both categories unset).
#[must_use]
pub const fn fixture_telemetry() -> TelemetryPreferences {
    TelemetryPreferences::initial()
}

/// Resolves the fixture telemetry through the real get-fallback policy.
#[must_use]
pub const fn resolve_fixture_telemetry(
    remote: Option<TelemetryPreferences>,
) -> TelemetryPreferences {
    resolve_get_telemetry_preferences(remote)
}

/// Returns the static retention fixture (enabled, 30 days).
#[must_use]
pub const fn fixture_retention_policy() -> ThreadRetentionPolicy {
    ThreadRetentionPolicy::new(true, FIXTURE_RETENTION_DAYS)
}

/// Returns whether the static retention fixture satisfies the protocol bound.
#[must_use]
pub const fn fixture_retention_is_valid() -> bool {
    fixture_retention_policy().is_valid()
}

/// Returns the static retention settings state derived from the fixture policy.
#[must_use]
pub fn fixture_retention_state() -> ThreadRetentionSettingsState {
    ThreadRetentionSettingsState::from_policy(fixture_retention_policy())
}

/// Returns the static usage-recovery state (available, continuation off).
#[must_use]
pub fn fixture_usage_recovery() -> UsageRecoverySettingsState {
    UsageRecoverySettingsState::new(UsageRecoveryAuthoritativeState::new(true, false))
}

/// Returns the static desktop notification preference (enabled).
#[must_use]
pub const fn fixture_notifications() -> NotificationPreferences {
    NotificationPreferences::new(true)
}

/// Returns the static desktop notification default for the host surface.
#[must_use]
pub const fn fixture_notification_default() -> NotificationPreferences {
    NotificationPreferences::default_for(RuntimeSurface::Desktop)
}

/// Returns the static engine-settings fixture status (authoritatively ready).
#[must_use]
pub const fn fixture_engine_status() -> EngineSettingsStatus {
    EngineSettingsStatus::Ready
}

/// Returns the exact empty-value clipboard document for the engines outlet.
#[must_use]
pub fn fixture_engine_template() -> String {
    manual_configuration_template()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_selects_models() {
        let shell = SettingsShell::new();
        assert_eq!(shell.selected(), SettingsSection::Models);
        assert!(shell.is_selected(SettingsSection::Models));
        assert!(!shell.is_selected(SettingsSection::Threads));
        assert_eq!(SettingsShell::default().selected(), SettingsSection::Models);
    }

    #[test]
    fn nav_selection_reports_changes() {
        let mut shell = SettingsShell::new();
        assert!(!shell.select(SettingsSection::Models));
        assert!(shell.select(SettingsSection::Threads));
        assert_eq!(shell.selected(), SettingsSection::Threads);
        assert!(shell.is_selected(SettingsSection::Threads));
        assert!(shell.select(SettingsSection::Appearance));
        assert_eq!(shell.selected(), SettingsSection::Appearance);
    }

    #[test]
    fn section_hrefs_match_legacy_settings_routes() {
        assert_eq!(SettingsSection::Models.href(), "/settings/models");
        assert_eq!(SettingsSection::Appearance.href(), "/settings/appearance");
        assert_eq!(
            SettingsSection::Engines.href(),
            "/settings/engines/fixture-engine"
        );
        assert_eq!(
            SettingsSection::Notifications.href(),
            "/settings/notifications"
        );
        assert_eq!(SettingsSection::Privacy.href(), "/settings/privacy");
        assert_eq!(SettingsSection::Threads.href(), "/settings/threads");
        assert_eq!(SettingsSection::ALL.len(), 6);
    }

    #[test]
    fn href_resolution_covers_every_section() {
        for section in SettingsSection::ALL {
            assert_eq!(section_for_href(section.href()), Some(section));
        }
        assert_eq!(
            section_for_href("/settings/engines/other-engine"),
            Some(SettingsSection::Engines)
        );
        assert_eq!(
            section_for_href("/settings/models/"),
            Some(SettingsSection::Models)
        );
        assert_eq!(section_for_href("/settings/unknown"), None);
    }

    #[test]
    fn every_section_mounts_with_copy_and_anchors() {
        for section in SettingsSection::ALL {
            let snapshot = section_snapshot(section);
            assert_eq!(snapshot.section, section);
            assert_eq!(snapshot.title, section.title());
            assert_eq!(snapshot.description, section.description());
            assert!(!snapshot.title.is_empty());
            assert!(!snapshot.description.is_empty());
            assert!(!snapshot.anchors.is_empty());
            assert!(!snapshot.primitives.is_empty());
        }
    }

    #[test]
    fn shell_outlet_follows_selection() {
        let mut shell = SettingsShell::new();
        for section in SettingsSection::ALL {
            assert!(shell.select(section) || shell.selected() == section);
            let outlet = shell.outlet();
            assert_eq!(outlet.section, section);
            assert_eq!(outlet, section_snapshot(section));
        }
    }

    #[test]
    fn section_anchors_match_legacy_nav() {
        let hashes = |section: SettingsSection| {
            section
                .anchors()
                .iter()
                .map(|anchor| anchor.hash)
                .collect::<Vec<_>>()
        };
        assert_eq!(hashes(SettingsSection::Models), ["compaction", "favorites"]);
        assert_eq!(
            hashes(SettingsSection::Threads),
            ["retention", "usage-recovery", "agents"]
        );
        assert_eq!(hashes(SettingsSection::Notifications), ["system"]);
        assert_eq!(
            hashes(SettingsSection::Privacy),
            ["telemetry", "never-collected"]
        );
    }

    #[test]
    fn fixture_policies_resolve_through_existing_helpers() {
        let models = fixture_models();
        assert_eq!(models.len(), 2);
        assert_eq!(models_for_fixture_engine(FIXTURE_ENGINE_ID).len(), 2);
        assert!(models_for_fixture_engine("unknown-engine").is_empty());
        assert_eq!(
            thinking_for_fixture_model(&models[0]),
            Some(ThinkingLevel::Medium)
        );
        assert_eq!(thinking_for_fixture_model(&models[1]), None);

        assert_eq!(resolve_fixture_telemetry(None), fixture_telemetry());
        assert!(fixture_retention_is_valid());
        assert_eq!(
            fixture_retention_state().policy(),
            Some(fixture_retention_policy())
        );

        let recovery = fixture_usage_recovery();
        assert!(!recovery.switch_disabled());

        assert!(fixture_notifications().enabled);
        assert_eq!(fixture_notification_default(), fixture_notifications());
        assert_eq!(fixture_engine_status(), EngineSettingsStatus::Ready);
        assert!(fixture_engine_template().contains("profile_id="));
    }

    #[test]
    fn primitive_recipes_resolve_for_fixture_theme() {
        assert_eq!(nav_tab_specs().len(), 6);
        assert_eq!(nav_tab_specs()[0].label(), "Models");
        let _ = fixture_card_style();
        let _ = fixture_switch_style(true);
        let _ = fixture_switch_style(false);
        let _ = fixture_tabs_style();
        let _ = fixture_toggle_group_style();
        let _ = fixture_tooltip_style();
        assert!(fixture_collapsible_state(true).is_open());
        assert!(!fixture_collapsible_state(false).is_open());
        assert!(
            SettingsSection::Appearance
                .primitives()
                .contains(&SettingsPrimitive::ToggleGroup)
        );
    }
}
