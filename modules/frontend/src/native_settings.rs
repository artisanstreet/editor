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
    /// Every section in legacy sticky-nav order (`nav.svelte`).
    pub const ALL: [Self; 6] = [
        Self::Models,
        Self::Threads,
        Self::Appearance,
        Self::Notifications,
        Self::Privacy,
        Self::Engines,
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
/// Selection is live and every control below stays a static fixture until its
/// controller is wired. The GPUI surface for one mounted section is
/// [`SettingsScreen`]; this shell hands that surface a section and its static
/// snapshot.
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
        // Legacy sticky-nav order (nav.svelte): Models, Threads,
        // Appearance, Notifications, Privacy, Engines group last.
        let specs = nav_tab_specs();
        let labels: Vec<&str> = specs.iter().map(TabSpec::label).collect();
        assert_eq!(
            labels,
            [
                "Models",
                "Threads",
                "Appearance",
                "Notifications",
                "Privacy",
                "Engines"
            ]
        );
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

// ============================================================================
// Native GPUI settings surface: nav rail plus one active section.
// ============================================================================
//
// Route-port packet 6. Legacy reference (read-only):
// `routes/settings/+layout.svelte` (frame, `/settings/section#header`
// deep-link scroll), `routes/settings/+page.svelte` (landing aliases the
// models section), `routes/components/settings/nav.svelte` (rail, hrefs,
// active treatment, anchors), `routes/components/settings/header.svelte`,
// `section.svelte`, `card.svelte`, `row.svelte` (section chrome), and the
// per-section components (`models`, `compaction-model`, `appearance`,
// `font-picker`, `engine`, `notifications`, `privacy`, `threads`,
// `thread-titles`, `usage-recovery`, `agent-names`).
//
// Every control below is a static fixture: switches, selects, toggle groups,
// and buttons render their fixture values with interaction disabled. No
// callback, persistence, or transport call lives here; the orchestrator
// injects controller snapshots through the `set_*` methods and wires
// navigation once `native_route.rs` grows a per-engine variant.

use artisan_ui::badge::{BadgeStyle, outline_badge};
use artisan_ui::button::{Button, ButtonContent, ButtonSize, ButtonVariant};
use artisan_ui::card::{compact_card, compact_card_content};
use artisan_ui::motion::MotionPolicy;
use artisan_ui::native_select::{NativeSelect, NativeSelectOption};
use artisan_ui::switch::Switch;
use artisan_ui::toggle_group::ToggleGroup;
use gpui::{
    AnyElement, Context, Div, EventEmitter, FocusHandle, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};

use crate::native_route::SettingsRoute;
use crate::notification_contract::{
    SystemNotificationGap, SystemNotificationPermission, SystemNotificationSettings,
    system_notification_gap_for,
};
use crate::shell_layout::ProseWidth;
use crate::telemetry_preferences::TelemetryPreference;
use crate::thread_retention_settings_policy::ThreadRetentionPolicyState;
use crate::thread_title_settings_policy::{
    ThreadTitleMode, ThreadTitleSettingsAuthoritativeState, ThreadTitleSettingsState,
};

/// Selector prefix for the settings surface root.
///
/// The full root selector is [`settings_screen_selector`], which matches
/// [`NativeRoute::selector_suffix`](crate::native_route::NativeRoute::selector_suffix)
/// for the mounted section.
pub const SETTINGS_SCREEN_SELECTOR_PREFIX: &str = "route-settings";

/// Debug selector for the settings nav rail.
pub const SETTINGS_NAV_SELECTOR: &str = "settings-nav";

/// Content width: legacy `max-w-4xl`.
pub const SETTINGS_CONTENT_MAX_WIDTH_PX: f32 = 896.0;

/// Nav rail width: legacy `md:w-44`.
pub const SETTINGS_NAV_RAIL_WIDTH_PX: f32 = 176.0;

/// Frame gap between rail and content: legacy `md:gap-14`.
pub const SETTINGS_NAV_CONTENT_GAP_PX: f32 = 56.0;

/// Title painted when the engine id matches no catalog entry.
pub const SETTINGS_UNKNOWN_ENGINE_TITLE: &str = "Unknown engine";

/// Default text font: `typography.ts` `default_typography_preferences.text`.
pub const APPEARANCE_DEFAULT_TEXT_FONT: &str = "Spline Sans";

/// Default code font: `typography.ts` `default_typography_preferences.code`.
pub const APPEARANCE_DEFAULT_CODE_FONT: &str = "Spline Sans Mono";

/// Default desktop app icon value (`default_desktop_app_icon`).
pub const APPEARANCE_APP_ICON_DEFAULT: &str = "plastic-jaw-shading";

/// Visible label for the default app icon.
pub const APPEARANCE_APP_ICON_DEFAULT_LABEL: &str = "Plastic + jaw shading";

/// Alternate desktop app icon value.
pub const APPEARANCE_APP_ICON_ALTERNATE: &str = "foreground-gradient-symbol";

/// Visible label for the alternate app icon.
pub const APPEARANCE_APP_ICON_ALTERNATE_LABEL: &str = "Foreground plastic + gradient symbol";

/// Default agent-name dataset id (`DefaultAgentNameDatasetId`).
pub const AGENT_NAME_DATASET_DEFAULT: &str = "norwegian";

/// Agent-name datasets: id, label, and description (`AgentNameDatasets`).
pub const AGENT_NAME_DATASETS: [(&str, &str, &str); 2] = [
    ("norwegian", "Norwegian", "Norwegian feminine given names."),
    ("british", "British", "British feminine given names."),
];

/// Clock preference mirror of `display-format.ts` `TimeFormat`.
///
/// No time-format controller exists in Rust yet; this snapshot only selects
/// the painted toggle-group value.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum AppearanceTimeFormat {
    /// 12-hour clock.
    #[default]
    TwelveHour,
    /// 24-hour clock.
    TwentyFourHour,
}

impl AppearanceTimeFormat {
    /// Both values in legacy toggle order.
    pub const ALL: [Self; 2] = [Self::TwelveHour, Self::TwentyFourHour];

    /// Returns the exact durable literal (`"12-hour"` / `"24-hour"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TwelveHour => "12-hour",
            Self::TwentyFourHour => "24-hour",
        }
    }

    /// Returns the visible toggle label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::TwelveHour => "12-hour",
            Self::TwentyFourHour => "24-hour",
        }
    }
}

/// Separator preference mirror of `display-format.ts` `PathSeparator`.
///
/// No display-format controller exists in Rust yet; this snapshot only
/// selects the painted toggle-group value.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum AppearancePathSeparator {
    /// Backslash separator (the Windows-host default).
    #[default]
    Backslash,
    /// Forward-slash separator.
    ForwardSlash,
}

impl AppearancePathSeparator {
    /// Both values in legacy toggle order.
    pub const ALL: [Self; 2] = [Self::Backslash, Self::ForwardSlash];

    /// Returns the exact durable literal.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Backslash => "backslash",
            Self::ForwardSlash => "forward-slash",
        }
    }

    /// Returns the painted separator character (`PathSeparatorCharacter`).
    #[must_use]
    pub const fn character(self) -> &'static str {
        match self {
            Self::Backslash => "\\",
            Self::ForwardSlash => "/",
        }
    }

    /// Returns the visible toggle label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Backslash => "Backslash",
            Self::ForwardSlash => "Forward slash",
        }
    }
}

/// Maps a mounted [`SettingsRoute`] to its static [`SettingsSection`].
#[must_use]
pub const fn settings_section_for_route(route: SettingsRoute) -> SettingsSection {
    match route {
        SettingsRoute::Models => SettingsSection::Models,
        SettingsRoute::Appearance => SettingsSection::Appearance,
        SettingsRoute::Engines => SettingsSection::Engines,
        SettingsRoute::Notifications => SettingsSection::Notifications,
        SettingsRoute::Privacy => SettingsSection::Privacy,
        SettingsRoute::Threads => SettingsSection::Threads,
    }
}

/// Catalog state behind one live engine page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsEngineCatalogState {
    /// No catalog read has settled yet.
    Loading,
    /// A catalog snapshot is loaded.
    Ready,
    /// The catalog read failed; retry from the composer or the engine page.
    Failed,
}

/// Registry state behind one live engine page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsEngineRegistryState {
    /// No registry read has settled yet.
    Loading,
    /// No registry file exists (managed profiles are not set up).
    Missing,
    /// The registry exists but holds no profiles.
    Empty,
    /// The registry holds profiles.
    Present,
}

/// One catalog model row behind a live engine page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsEngineModel {
    /// Stable catalog model id.
    pub id: String,
    /// Whether this row is the thread's saved model.
    pub saved: bool,
    /// Whether this row is the currently displayed choice.
    pub displayed: bool,
    /// Live unavailability reason, when the catalog disabled the row.
    pub disabled_reason: Option<String>,
}

/// Live facts behind one engine settings page, projected by the
/// orchestrator from the actual catalog, account usage, registry, and
/// thread configuration.
///
/// Every state the page paints — loaded, loading, unavailable, sign-in
/// required, save pending, save failed — comes from this snapshot. The page
/// never invents installation facts: availability and installation read out
/// the backend-probed account verdict, and controls without a durable API
/// stay visibly inert with an explicit unavailable note.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsEngineSnapshot {
    /// Engine id this snapshot was built for.
    pub engine_id: String,
    /// Backend-probed account verdict for the engine.
    pub readiness: crate::native_profile_usage::EngineReadiness,
    /// Provider-disclosed account email, when one was reported.
    pub account_email: Option<String>,
    /// Actionable refresh failure, when the latest check failed.
    pub refresh_failure: Option<String>,
    /// Whether an account read is currently admitted.
    pub refreshing: bool,
    /// Runtime catalog state.
    pub catalog: SettingsEngineCatalogState,
    /// Catalog failure copy, when the read failed.
    pub catalog_error: Option<String>,
    /// Managed profile registry state.
    pub registry: SettingsEngineRegistryState,
    /// Selected thread this engine configuration would save to, if any.
    /// Engine model configuration is thread-specific; global pages name
    /// that explicitly instead of pretending to save.
    pub selected_thread: Option<String>,
    /// Saved model id for the selected thread, when configured.
    pub saved_model: Option<String>,
    /// Saved profile id for the selected thread, when configured.
    pub saved_profile: Option<String>,
    /// Currently displayed model choice, when one is selected.
    pub displayed_model: Option<String>,
    /// Whether the displayed choice equals the saved configuration.
    pub displayed_authoritative: bool,
    /// Whether the displayed choice can be saved to the selected thread.
    pub can_save_displayed: bool,
    /// Unsaved-choice notice for the models section, when a choice is held
    /// without a save (no thread selected, or the engine cannot admit it).
    /// Saved, saving, and failed states read out their own rows instead.
    pub choice_notice: Option<String>,
    /// Whether a configuration save is in flight.
    pub pending_save: bool,
    /// Whether the latest save failed.
    pub save_failed: bool,
    /// Catalog model rows for this engine, in catalog order.
    pub models: Vec<SettingsEngineModel>,
}

impl SettingsEngineSnapshot {
    /// Returns the availability badge for the probed account verdict.
    #[must_use]
    pub const fn availability_badge(&self) -> &'static str {
        match self.readiness {
            crate::native_profile_usage::EngineReadiness::Ready => "Available",
            crate::native_profile_usage::EngineReadiness::NeedsSignIn => "Sign-in required",
            crate::native_profile_usage::EngineReadiness::Checking => "Checking",
            crate::native_profile_usage::EngineReadiness::NotReady => "Unavailable",
        }
    }

    /// Returns the installation state copy for the probed verdict.
    ///
    /// Only a responding executable proves installation: an authenticated
    /// or login-gated answer means the binary runs, anything else is a
    /// check status, never an install claim.
    #[must_use]
    pub fn installation_state(&self) -> String {
        match self.readiness {
            crate::native_profile_usage::EngineReadiness::Ready => match &self.account_email {
                Some(email) => format!("Installed and responding as {email}."),
                None => "Installed and responding.".to_owned(),
            },
            crate::native_profile_usage::EngineReadiness::NeedsSignIn => {
                "Installed. Account sign-in is required.".to_owned()
            }
            crate::native_profile_usage::EngineReadiness::Checking => {
                "Checking installation and account status.".to_owned()
            }
            crate::native_profile_usage::EngineReadiness::NotReady => match &self.refresh_failure
            {
                Some(failure) => format!("Status check failed: {failure}."),
                None => "Installation and account status have not been checked yet.".to_owned(),
            },
        }
    }

    /// Returns the account state copy for the probed verdict.
    #[must_use]
    pub fn account_state(&self) -> String {
        match self.readiness {
            crate::native_profile_usage::EngineReadiness::Ready => match &self.account_email {
                Some(email) => format!("Signed in as {email}."),
                None => "Signed in.".to_owned(),
            },
            crate::native_profile_usage::EngineReadiness::NeedsSignIn => {
                "No account is signed in.".to_owned()
            }
            crate::native_profile_usage::EngineReadiness::Checking => {
                "Reading account status.".to_owned()
            }
            crate::native_profile_usage::EngineReadiness::NotReady => match &self.refresh_failure
            {
                Some(failure) => format!("Account status unavailable: {failure}."),
                None => "Sign-in status unknown.".to_owned(),
            },
        }
    }
}

/// One real catalog engine behind the settings nav rail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsEngineNavEntry {
    /// Stable catalog engine id.
    pub id: String,
    /// Provider-owned display label.
    pub label: String,
}

/// Actions a mounted [`SettingsScreen`] emits for its orchestrator.
///
/// The screen owns no transport or navigation: every live control emits one
/// of these and the application performs the command.
#[derive(Clone, Debug)]
pub enum SettingsScreenEvent {
    /// Navigate to a settings section, preserving the engine id when one is
    /// mounted.
    Navigate {
        /// Section to mount.
        section: SettingsRoute,
        /// Engine id to keep mounted, if the target is the engine page.
        engine: Option<String>,
    },
    /// Force a provider-account refresh for one engine.
    RefreshEngine {
        /// Engine id to refresh.
        engine_id: String,
    },
    /// Save the currently displayed model choice for one engine to the
    /// selected thread through the shared direct typed-save path.
    SaveDisplayedModel {
        /// Engine id whose displayed choice is saved.
        engine_id: String,
    },
    /// Choose one catalog model for one engine from the Settings page.
    ///
    /// The orchestrator serves this through the existing `SelectPolicy`
    /// plus shared typed-save flow — the same path as the composer picker —
    /// so a Settings choice saves, acknowledges, and reloads exactly like
    /// one made in the composer.
    SelectEngineModel {
        /// Engine id whose model is chosen.
        engine_id: String,
        /// Stable catalog model id that is chosen.
        model_id: String,
    },
}

impl EventEmitter<SettingsScreenEvent> for SettingsScreen {}

/// Maps a static [`SettingsSection`] back to its mounted [`SettingsRoute`].
///
/// Inverse of [`settings_section_for_route`], used by the live nav rail to
/// emit navigation for the painted row.
#[must_use]
pub const fn section_route(section: SettingsSection) -> SettingsRoute {
    match section {
        SettingsSection::Models => SettingsRoute::Models,
        SettingsSection::Appearance => SettingsRoute::Appearance,
        SettingsSection::Engines => SettingsRoute::Engines,
        SettingsSection::Notifications => SettingsRoute::Notifications,
        SettingsSection::Privacy => SettingsRoute::Privacy,
        SettingsSection::Threads => SettingsRoute::Threads,
    }
}

/// Returns the stable root selector for one mounted section.
///
/// The value matches
/// [`NativeRoute::selector_suffix`](crate::native_route::NativeRoute::selector_suffix)
/// for `Settings(route)`, so the orchestrator can mount the screen under the
/// selector its probes already expect.
#[must_use]
pub fn settings_screen_selector(section: SettingsRoute) -> String {
    format!("{SETTINGS_SCREEN_SELECTOR_PREFIX}-{}", section.as_str())
}

/// Resolves the visible engine label: explicit label, then engine id, then
/// the fixture label.
#[must_use]
pub fn resolve_engine_label(label: Option<&str>, engine_id: Option<&str>) -> String {
    if let Some(label) = label {
        return label.to_owned();
    }
    if let Some(engine_id) = engine_id {
        return engine_id.to_owned();
    }
    FIXTURE_ENGINE_LABEL.to_owned()
}

/// Returns the legacy unknown-engine description for one engine id.
#[must_use]
pub fn unknown_engine_description(engine_id: &str) -> String {
    format!("No engine with id \"{engine_id}\" exists in the catalog.")
}

/// Returns the anchors painted under the active nav row.
///
/// A switched-off engine hides its account and models sections entirely, so
/// only the availability and installation anchors remain (see
/// `engine.svelte`).
#[must_use]
pub const fn visible_anchors(
    section: SettingsSection,
    engine_enabled: bool,
) -> &'static [SettingsAnchor] {
    if matches!(section, SettingsSection::Engines) && !engine_enabled {
        let (visible, _) = section.anchors().split_at(2);
        visible
    } else {
        section.anchors()
    }
}

/// Returns the desktop gap-notice copy for one notification gap.
///
/// `None` means the legacy surface paints no notice. The copy below is the
/// desktop branch; the browser branch needs a runtime-surface input that has
/// no Rust owner yet.
#[must_use]
pub const fn notification_gap_notice(
    gap: SystemNotificationGap,
) -> Option<(&'static str, &'static str)> {
    match gap {
        SystemNotificationGap::Blocked => Some((
            "Blocked by your system",
            "Artisan asked and your system refused. Allow notifications for Artisan in your operating system's notification settings, then check again.",
        )),
        SystemNotificationGap::Unprompted => Some((
            "Not allowed yet",
            "The permission prompt was closed without an answer, so nothing can be posted yet. Asking again is safe.",
        )),
        SystemNotificationGap::None | SystemNotificationGap::Unsupported => None,
    }
}

/// Returns the telemetry caption painted beside one category switch.
#[must_use]
pub const fn telemetry_choice_caption(choice: TelemetryPreference) -> &'static str {
    match choice {
        TelemetryPreference::Unset => "Not decided",
        TelemetryPreference::Enabled => "On",
        TelemetryPreference::Disabled => "Off",
    }
}

/// Focus handles owned by [`SettingsScreen`].
///
/// Every control below is static and disabled, so one shared handle per
/// family is enough; the handles exist only because the `artisan-ui`
/// components require them.
struct SettingsFocus {
    /// Root focus scope.
    root: FocusHandle,
    /// Shared by every switch, button, and select trigger.
    control: FocusHandle,
    /// Shared by every toggle-group item.
    group: FocusHandle,
}

/// Native GPUI port of the legacy settings area (`routes/settings/*`).
///
/// The constructor takes the active [`SettingsRoute`] section plus the
/// optional `[engine]` id for the engines page. All policy snapshots start
/// from the module fixtures; the orchestrator replaces them through the
/// `set_*` methods once the corresponding controllers exist in Rust.
/// Interaction stays disabled throughout: there is no persistence, transport,
/// or navigation call in this surface.
///
/// The orchestrator must extend `native_route.rs` before mounting the engine
/// page for a real id: [`SettingsRoute::Engines`] carries no engine id today,
/// so the id travels through this constructor instead.
pub struct SettingsScreen {
    section: SettingsRoute,
    engine_id: Option<String>,
    engine_label: Option<String>,
    engine_known: bool,
    engine_enabled: bool,
    theme: artisan_ui::theme::ArtisanTheme,
    focus: SettingsFocus,
    notifications: SystemNotificationSettings,
    telemetry: TelemetryPreferences,
    retention: ThreadRetentionSettingsState,
    thread_title: ThreadTitleSettingsState,
    usage_recovery: UsageRecoverySettingsState,
    prose_width: ProseWidth,
    time_format: AppearanceTimeFormat,
    path_separator: AppearancePathSeparator,
    shader_enabled: bool,
    text_font: String,
    code_font: String,
    agent_dataset: String,
    engine_snapshot: Option<SettingsEngineSnapshot>,
    engines: Vec<SettingsEngineNavEntry>,
}

impl SettingsScreen {
    /// Builds the settings surface for one section and optional engine id.
    ///
    /// `engine_id` selects the `/settings/engines/[engine]` page and is
    /// ignored by every other section. Snapshots start from the module
    /// fixtures with every control disabled.
    #[must_use]
    pub fn new(
        section: SettingsRoute,
        engine_id: Option<String>,
        mode: artisan_ui::theme::ThemeMode,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            section,
            engine_id,
            engine_label: None,
            engine_known: true,
            engine_enabled: true,
            theme: artisan_ui::theme::ArtisanTheme::for_mode(mode),
            focus: SettingsFocus {
                root: cx.focus_handle(),
                control: cx.focus_handle(),
                group: cx.focus_handle(),
            },
            notifications: SystemNotificationSettings::new(
                true,
                SystemNotificationPermission::Granted,
            ),
            telemetry: TelemetryPreferences::initial(),
            retention: fixture_retention_state(),
            thread_title: ThreadTitleSettingsState::new(
                ThreadTitleSettingsAuthoritativeState::new(true, ThreadTitleMode::Summary),
            ),
            usage_recovery: UsageRecoverySettingsState::new(UsageRecoveryAuthoritativeState::new(
                true, false,
            )),
            prose_width: ProseWidth::Balanced,
            time_format: AppearanceTimeFormat::TwelveHour,
            path_separator: AppearancePathSeparator::Backslash,
            shader_enabled: true,
            text_font: APPEARANCE_DEFAULT_TEXT_FONT.to_owned(),
            code_font: APPEARANCE_DEFAULT_CODE_FONT.to_owned(),
            agent_dataset: AGENT_NAME_DATASET_DEFAULT.to_owned(),
            engine_snapshot: None,
            engines: Vec::new(),
        }
    }

    /// Replaces the live engine snapshot for the mounted engine page.
    ///
    /// The snapshot is ignored unless it names the mounted engine id, so a
    /// stale reply for a previous engine can never paint this page.
    pub fn set_engine_snapshot(
        &mut self,
        snapshot: SettingsEngineSnapshot,
        cx: &mut Context<Self>,
    ) {
        if self.engine_id.as_deref() == Some(snapshot.engine_id.as_str()) {
            self.engine_snapshot = Some(snapshot);
            cx.notify();
        }
    }

    /// Returns the live engine snapshot, when one names the mounted engine.
    #[must_use]
    pub fn engine_snapshot(&self) -> Option<&SettingsEngineSnapshot> {
        self.engine_snapshot.as_ref().filter(|snapshot| {
            self.engine_id.as_deref() == Some(snapshot.engine_id.as_str())
        })
    }

    /// Replaces the real catalog engines behind the nav rail.
    ///
    /// The orchestrator feeds the manifest harness identities, so the rail
    /// enumerates actual engines and never fixture identities. An empty list
    /// keeps the legacy single engine row.
    pub fn set_engines(
        &mut self,
        engines: Vec<SettingsEngineNavEntry>,
        cx: &mut Context<Self>,
    ) {
        self.engines = engines;
        cx.notify();
    }

    /// Returns the rail engine entries.
    #[must_use]
    pub fn engines(&self) -> &[SettingsEngineNavEntry] {
        &self.engines
    }

    /// Returns the mounted section.
    #[must_use]
    pub const fn section(&self) -> SettingsRoute {
        self.section
    }

    /// Returns the engines-page engine id, if one was supplied.
    #[must_use]
    pub fn engine_id(&self) -> Option<&str> {
        self.engine_id.as_deref()
    }

    /// Returns the visible engine label (explicit, id, then fixture).
    #[must_use]
    pub fn engine_label(&self) -> String {
        resolve_engine_label(self.engine_label.as_deref(), self.engine_id.as_deref())
    }

    /// Returns whether the engine id matched the catalog.
    #[must_use]
    pub const fn is_engine_known(&self) -> bool {
        self.engine_known
    }

    /// Returns the blanket availability switch value.
    #[must_use]
    pub const fn is_engine_enabled(&self) -> bool {
        self.engine_enabled
    }

    /// Returns the static section snapshot for the mounted section.
    #[must_use]
    pub const fn snapshot(&self) -> SettingsSectionSnapshot {
        section_snapshot(settings_section_for_route(self.section))
    }

    /// Mounts another section, replacing the engine id.
    pub fn set_section(
        &mut self,
        section: SettingsRoute,
        engine_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.section = section;
        self.engine_id = engine_id;
        cx.notify();
    }

    /// Overrides the visible engine label (defaults to the engine id).
    pub fn set_engine_label(&mut self, label: Option<String>, cx: &mut Context<Self>) {
        self.engine_label = label;
        cx.notify();
    }

    /// Selects the unknown-engine branch for the current engine id.
    pub fn set_engine_known(&mut self, known: bool, cx: &mut Context<Self>) {
        self.engine_known = known;
        cx.notify();
    }

    /// Sets the blanket availability switch value.
    pub fn set_engine_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.engine_enabled = enabled;
        cx.notify();
    }

    /// Replaces the notification snapshot from its controller.
    pub fn set_notifications(
        &mut self,
        notifications: SystemNotificationSettings,
        cx: &mut Context<Self>,
    ) {
        self.notifications = notifications;
        cx.notify();
    }

    /// Replaces the telemetry snapshot from its controller.
    pub fn set_telemetry(&mut self, telemetry: TelemetryPreferences, cx: &mut Context<Self>) {
        self.telemetry = telemetry;
        cx.notify();
    }

    /// Replaces the retention snapshot from its controller.
    pub fn set_retention(
        &mut self,
        retention: ThreadRetentionSettingsState,
        cx: &mut Context<Self>,
    ) {
        self.retention = retention;
        cx.notify();
    }

    /// Replaces the thread-title snapshot from its controller.
    pub fn set_thread_title(
        &mut self,
        thread_title: ThreadTitleSettingsState,
        cx: &mut Context<Self>,
    ) {
        self.thread_title = thread_title;
        cx.notify();
    }

    /// Replaces the usage-recovery snapshot from its controller.
    pub fn set_usage_recovery(
        &mut self,
        usage_recovery: UsageRecoverySettingsState,
        cx: &mut Context<Self>,
    ) {
        self.usage_recovery = usage_recovery;
        cx.notify();
    }

    /// Sets the painted prose-width toggle value.
    pub fn set_prose_width(&mut self, prose_width: ProseWidth, cx: &mut Context<Self>) {
        self.prose_width = prose_width;
        cx.notify();
    }

    /// Sets the painted clock toggle value.
    pub fn set_time_format(&mut self, time_format: AppearanceTimeFormat, cx: &mut Context<Self>) {
        self.time_format = time_format;
        cx.notify();
    }

    /// Sets the painted separator toggle value.
    pub fn set_path_separator(
        &mut self,
        path_separator: AppearancePathSeparator,
        cx: &mut Context<Self>,
    ) {
        self.path_separator = path_separator;
        cx.notify();
    }

    /// Sets the painted shader switch value.
    pub fn set_shader_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.shader_enabled = enabled;
        cx.notify();
    }

    /// Sets the painted text font trigger value.
    pub fn set_text_font(&mut self, family: String, cx: &mut Context<Self>) {
        self.text_font = family;
        cx.notify();
    }

    /// Sets the painted code font trigger value.
    pub fn set_code_font(&mut self, family: String, cx: &mut Context<Self>) {
        self.code_font = family;
        cx.notify();
    }

    /// Sets the painted agent-name dataset select value.
    pub fn set_agent_dataset(&mut self, dataset: String, cx: &mut Context<Self>) {
        self.agent_dataset = dataset;
        cx.notify();
    }
}

// --- Shared section chrome --------------------------------------------------
//
// header.svelte: h1 at text-xl semibold with a mt-1.5 text-sm muted
// description. section.svelte: mt-10 block with a text-sm medium heading,
// optional baseline action, and optional intro. card.svelte: the moulded
// gradient well with hairline dividers; GPUI paints the flat compact_card
// recipe instead of the CSS gradient (see the surface-225/200 note on
// settings_card). row.svelte: the sm: side-by-side row; the native window is
// always wide, so only the side-by-side arrangement is painted.

/// Paints the page header (`header.svelte`).
fn settings_header(
    theme: artisan_ui::theme::ArtisanTheme,
    title: String,
    description: &str,
) -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(20.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.foreground.to_paint())
                .child(title),
        )
        .child(
            div()
                .mt(px(6.0))
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(description.to_owned()),
        )
}

/// Paints one section block (`section.svelte`).
fn settings_section_shell(
    theme: artisan_ui::theme::ArtisanTheme,
    anchor: &str,
    title: &str,
    intro: Option<&str>,
    action: Option<AnyElement>,
    body: Div,
) -> Div {
    let selector = format!("settings-section-{anchor}");
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(16.0));
    head = head.child(
        div()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.colors.foreground.to_paint())
            .child(title.to_owned()),
    );
    if let Some(action) = action {
        head = head.child(action);
    }
    let mut section = div()
        .flex()
        .flex_col()
        .mt(px(40.0))
        .debug_selector(move || selector.clone())
        .child(head);
    if let Some(intro) = intro {
        section = section.child(
            div()
                .mt(px(4.0))
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(intro.to_owned()),
        );
    }
    section.child(div().mt(px(12.0)).child(body))
}

/// Paints one card (`card.svelte`).
///
/// Legacy: `rounded-xl` gradient well (`from-surface-225 to-surface-200`,
/// `dark:from-surface-800 dark:to-surface-925`) with `divide-y
/// divide-border/40` hairlines. The `surface-*` steps exist in
/// `artisan-ui` (`SurfaceStep::S200`/`S225`/`S800`/`S925`) but GPUI paints no
/// CSS gradient, so the flat `compact_card` recipe stands in; dividers reuse
/// the theme border at full alpha.
fn settings_card(theme: artisan_ui::theme::ArtisanTheme, blocks: Vec<Div>) -> Div {
    let style = CardStyle::resolve(theme);
    let border = theme.colors.border.to_paint();
    let mut card = compact_card(style).w_full().gap(px(0.0)).py(px(0.0));
    for (index, block) in blocks.into_iter().enumerate() {
        let mut band = compact_card_content(style).w_full().child(block);
        if index > 0 {
            band = band.border_t_1().border_color(border);
        }
        card = card.child(band);
    }
    card
}

/// Paints one row (`row.svelte`): title plus description with an optional
/// trailing control.
fn settings_row(
    theme: artisan_ui::theme::ArtisanTheme,
    title: &str,
    description: &str,
    control: Option<AnyElement>,
) -> Div {
    let mut row = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(24.0))
        .py(px(14.0));
    row = row.child(
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.0))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.colors.foreground.to_paint())
                    .child(title.to_owned()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(description.to_owned()),
            ),
    );
    if let Some(control) = control {
        row = row.child(div().flex_shrink_0().child(control));
    }
    row
}

/// Paints the rail heading and group labels (`+layout.svelte`, `nav.svelte`).
fn nav_heading(theme: artisan_ui::theme::ArtisanTheme) -> Div {
    div()
        .px(px(8.0))
        .pb(px(12.0))
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.foreground.to_paint())
        .child("Settings")
}

/// Paints one uppercase group label (`ARTISAN`, `ENGINES`).
///
/// Legacy uppercases via CSS; GPUI has no text transform, so the labels are
/// stored pre-uppercased.
fn nav_group_label(theme: artisan_ui::theme::ArtisanTheme, label: &'static str) -> Div {
    div()
        .px(px(8.0))
        .pt(px(20.0))
        .pb(px(6.0))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.muted_foreground.to_paint())
        .child(label)
}

/// Paints one nav row (`nav.svelte` `nav_link`).
///
/// The active row wears the opaque well: legacy paints the same surface
/// gradient as cards, approximated here with the flat muted fill. Icons are
/// omitted: no tabler-icon component exists in the native port yet. The
/// caller attaches the debug selector and the navigation click, so the rail
/// stays live without styling knowledge leaking into event wiring.
fn nav_link(theme: artisan_ui::theme::ArtisanTheme, label: String, active: bool) -> Div {
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(28.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .gap(px(8.0))
        .text_sm();
    if active {
        row.bg(theme.colors.muted.to_paint())
            .text_color(theme.colors.foreground.to_paint())
            .font_weight(FontWeight::MEDIUM)
            .child(label)
    } else {
        row.text_color(theme.colors.muted_foreground.to_paint())
            .child(label)
    }
}

/// Paints the anchor sub-list under the active nav row.
///
/// The rail sits on the group border in legacy (`ml-[0.9375rem]`,
/// `border-l`, `pl-3`); the active-hash tick is omitted because the screen
/// carries no hash state (deep-link scroll is an orchestrator gap).
fn anchor_list(theme: artisan_ui::theme::ArtisanTheme, anchors: &[SettingsAnchor]) -> Div {
    let mut list = div()
        .flex()
        .flex_col()
        .ml(px(15.0))
        .pl(px(12.0))
        .my(px(4.0))
        .border_l_1()
        .border_color(theme.colors.border.to_paint());
    for anchor in anchors {
        let selector = format!("settings-anchor-{}", anchor.hash);
        list = list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .h(px(24.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .text_xs()
                .text_color(theme.colors.muted_foreground.to_paint())
                .debug_selector(move || selector.clone())
                .child(anchor.label.to_owned()),
        );
    }
    list
}

impl SettingsScreen {
    /// Builds one disabled static button (`button.svelte` wrappers).
    fn settings_button(
        &self,
        theme: artisan_ui::theme::ArtisanTheme,
        id: &'static str,
        label: String,
        variant: ButtonVariant,
        selector: &'static str,
    ) -> Button {
        Button::new(
            id,
            self.focus.control.clone(),
            theme,
            MotionPolicy::Reduced,
            variant,
            ButtonSize::Small,
            ButtonContent::text(label),
        )
        .expect("static settings button configuration is valid")
        .disabled(true)
        .debug_selector(selector)
    }

    /// Builds one disabled static switch (`switch.svelte` wrappers).
    fn settings_switch(
        &self,
        theme: artisan_ui::theme::ArtisanTheme,
        id: &'static str,
        checked: bool,
        selector: &'static str,
    ) -> Switch {
        Switch::new(
            id,
            self.focus.control.clone(),
            theme,
            artisan_ui::switch::SwitchSize::Default,
            checked,
        )
        .disabled(true)
        .debug_selector(selector)
    }

    /// Builds one disabled static segmented group (`toggle-group` wrappers).
    fn segmented(
        &self,
        theme: artisan_ui::theme::ArtisanTheme,
        id: &'static str,
        options: &[(&'static str, &'static str)],
        selected: &str,
        selector: &'static str,
    ) -> ToggleGroup<SharedString> {
        let mut group =
            ToggleGroup::single(id, theme, Some(SharedString::from(selected.to_owned())))
                .variant(ToggleGroupVariant::Outline)
                .disabled(true)
                .debug_selector(selector);
        for (value, label) in options {
            group = group.item(
                SharedString::from((*value).to_owned()),
                *label,
                self.focus.group.clone(),
            );
        }
        group
    }

    /// Renders the nav rail for the mounted section.
    ///
    /// Every row navigates through [`SettingsScreenEvent::Navigate`]: the
    /// rail is live chrome, not a static fixture. The engine row keeps the
    /// mounted engine id so engine pages switch sections without losing it.
    fn render_nav(&self, theme: artisan_ui::theme::ArtisanTheme, cx: &mut Context<Self>) -> Div {
        let active = settings_section_for_route(self.section);
        let mut rail = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .w(px(SETTINGS_NAV_RAIL_WIDTH_PX))
            .gap(px(4.0))
            .debug_selector(|| SETTINGS_NAV_SELECTOR.to_owned());
        rail = rail.child(nav_heading(theme));
        rail = rail.child(nav_group_label(theme, "ARTISAN"));
        for section in [
            SettingsSection::Models,
            SettingsSection::Threads,
            SettingsSection::Appearance,
            SettingsSection::Notifications,
            SettingsSection::Privacy,
        ] {
            let selector = format!("settings-nav-{}", section.label().to_lowercase());
            let target = section_route(section);
            rail = rail.child(
                nav_link(theme, section.label().to_owned(), active == section)
                    .id(selector.clone())
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SettingsScreenEvent::Navigate {
                            section: target,
                            engine: None,
                        });
                    })),
            );
            if active == section {
                rail = rail.child(anchor_list(
                    theme,
                    visible_anchors(section, self.engine_enabled),
                ));
            }
        }
        rail = rail.child(nav_group_label(theme, "ENGINES"));
        if self.engines.is_empty() {
            let engine_active = active == SettingsSection::Engines;
            let engine_selector = format!(
                "settings-nav-engines-{}",
                self.engine_id.as_deref().unwrap_or(FIXTURE_ENGINE_ID)
            );
            let engine_id = self.engine_id.clone();
            rail = rail.child(
                nav_link(theme, self.engine_label(), engine_active)
                    .id(engine_selector.clone())
                    .debug_selector(move || engine_selector.clone())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SettingsScreenEvent::Navigate {
                            section: SettingsRoute::Engines,
                            engine: engine_id.clone(),
                        });
                    })),
            );
            if engine_active {
                rail = rail.child(anchor_list(
                    theme,
                    visible_anchors(SettingsSection::Engines, self.engine_enabled),
                ));
            }
            return rail;
        }
        for entry in self.engines.clone() {
            let row_active =
                active == SettingsSection::Engines && self.engine_id.as_deref() == Some(entry.id.as_str());
            let selector = format!("settings-nav-engines-{}", entry.id);
            let target = entry.id.clone();
            rail = rail.child(
                nav_link(theme, entry.label.clone(), row_active)
                    .id(selector.clone())
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SettingsScreenEvent::Navigate {
                            section: SettingsRoute::Engines,
                            engine: Some(target.clone()),
                        });
                    })),
            );
            if row_active {
                rail = rail.child(anchor_list(theme, SettingsSection::Engines.anchors()));
            }
        }
        rail
    }

    /// Renders the models section (`models.svelte`, `compaction-model.svelte`).
    ///
    /// The compaction picker popover and the favorite toggles need the
    /// session-defaults controller; the trigger paints the fixture
    /// `Curated` value and favorites paint the legacy empty state.
    fn render_models(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        let compaction = settings_card(
            theme,
            vec![settings_row(
                theme,
                "Cross-transfer compaction model",
                "Choose who writes a hand-off summary when a thread moves to another engine or model.",
                Some(
                    self.settings_button(
                        theme,
                        "settings-compaction-trigger",
                        "Curated \u{25be}".to_owned(),
                        ButtonVariant::Outline,
                        "settings-compaction-trigger",
                    )
                    .into_any_element(),
                ),
            )],
        );
        let favorites = settings_card(
            theme,
            vec![
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .px(px(16.0))
                    .py(px(28.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(
                                "No favorites yet. Starred models float to the top of every model picker; star one from the composer's picker or an engine page.",
                            ),
                    ),
            ],
        );
        div()
            .flex()
            .flex_col()
            .child(settings_header(
                theme,
                SettingsSection::Models.title().to_owned(),
                SettingsSection::Models.description(),
            ))
            .child(settings_section_shell(
                theme,
                "compaction",
                "Compaction",
                Some("Compaction choices are not yet configurable in the native app."),
                None,
                compaction,
            ))
            .child(settings_section_shell(
                theme,
                "favorites",
                "Favorites",
                None,
                None,
                favorites,
            ))
    }

    /// Renders the appearance app-icon and typography sections.
    fn render_appearance_top(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        let app_icon = settings_card(
            theme,
            vec![
                Self::app_icon_option(theme, APPEARANCE_APP_ICON_DEFAULT_LABEL, "Default", true),
                Self::app_icon_option(
                    theme,
                    APPEARANCE_APP_ICON_ALTERNATE_LABEL,
                    "Alternate",
                    false,
                ),
            ],
        );
        let restore = self
            .settings_button(
                theme,
                "settings-typography-restore",
                "Restore defaults".to_owned(),
                ButtonVariant::Ghost,
                "settings-typography-restore",
            )
            .into_any_element();
        let typography = settings_card(
            theme,
            vec![
                Self::typography_preview(theme),
                settings_row(
                    theme,
                    "Text",
                    "Interface controls, page titles, and reading text.",
                    Some(self.font_trigger(theme, "text", &self.text_font)),
                ),
                settings_row(
                    theme,
                    "Code",
                    "Code, terminals, and the editor.",
                    Some(self.font_trigger(theme, "code", &self.code_font)),
                ),
            ],
        );
        div().flex().flex_col()
            .child(settings_section_shell(theme, "app-icon", "App icon", None, None, app_icon))
            .child(
                div()
                    .mt(px(10.0))
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child("Open Appearance in the Artisan desktop app to switch its runtime icon."),
            )
            .child(settings_section_shell(
                theme,
                "typography",
                "Typography",
                Some("Font choices are not yet configurable in the native app."),
                Some(restore),
                typography,
            ))
            .child(
                div()
                    .mt(px(10.0))
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(
                        "Local font names are requested only when you open a picker. The list stays on this device; Artisan saves only the two family names you choose.",
                    ),
            )
    }

    /// Paints one app-icon option as a disabled static row.
    fn app_icon_option(
        theme: artisan_ui::theme::ArtisanTheme,
        label: &str,
        caption: &str,
        selected: bool,
    ) -> Div {
        let mut row = div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .rounded(px(12.0))
            .px(px(12.0))
            .py(px(12.0))
            .border_1()
            .border_color(theme.colors.border.to_paint());
        if selected {
            row = row.bg(theme.colors.muted.to_paint());
        }
        row.child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(4.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.colors.foreground.to_paint())
                        .child(label.to_owned()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child(caption.to_owned()),
                ),
        )
    }

    /// Paints the text/code preview panes of the typography card.
    fn typography_preview(theme: artisan_ui::theme::ArtisanTheme) -> Div {
        div()
            .w_full()
            .flex()
            .flex_row()
            .gap(px(16.0))
            .py(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child("TEXT"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child("Quiet tools, clear decisions."),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child("CODE"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.foreground.to_paint())
                            .child("craft = deliberate"),
                    ),
            )
    }

    /// Paints one font-picker trigger (`font-picker.svelte`).
    ///
    /// The full command popover needs browser font discovery; the trigger
    /// paints the fixture family with the legacy selector glyph.
    fn font_trigger(
        &self,
        theme: artisan_ui::theme::ArtisanTheme,
        role: &str,
        family: &str,
    ) -> AnyElement {
        let id = if role == "code" {
            "settings-font-code"
        } else {
            "settings-font-text"
        };
        self.settings_button(
            theme,
            id,
            format!("{family} \u{25be}"),
            ButtonVariant::Outline,
            id,
        )
        .into_any_element()
    }

    /// Renders the appearance formatting, glass, and reading sections.
    fn render_appearance_bottom(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        let time_options: [(&'static str, &'static str); 2] =
            [("12-hour", "12-hour"), ("24-hour", "24-hour")];
        let separator_options: [(&'static str, &'static str); 2] =
            [("backslash", "\\"), ("forward-slash", "/")];
        let width_options: [(&'static str, &'static str); 3] = [
            ("tight", "Tight"),
            ("balanced", "Balanced"),
            ("loose", "Loose"),
        ];
        let formatting = settings_card(
            theme,
            vec![
                settings_row(
                    theme,
                    "Time format",
                    "How local times are written throughout Artisan.",
                    Some(
                        self.segmented(
                            theme,
                            "settings-time-format",
                            &time_options,
                            self.time_format.as_str(),
                            "settings-time-format",
                        )
                        .into_any_element(),
                    ),
                ),
                settings_row(
                    theme,
                    "Path separator",
                    "Which separator file and folder paths use when displayed.",
                    Some(
                        self.segmented(
                            theme,
                            "settings-path-separator",
                            &separator_options,
                            self.path_separator.as_str(),
                            "settings-path-separator",
                        )
                        .into_any_element(),
                    ),
                ),
            ],
        );
        let glass = settings_card(
            theme,
            vec![settings_row(
                theme,
                "Shader under glass",
                "Lights glass surfaces with the animated shader they were designed around. Turning it off leaves the glass itself intact \u{2014} the material, highlight, and depth stay \u{2014} and only the moving light stops.",
                Some(
                    self.settings_switch(
                        theme,
                        "settings-switch-shader",
                        self.shader_enabled,
                        "settings-switch-shader",
                    )
                    .into_any_element(),
                ),
            )],
        );
        let reading = settings_card(
            theme,
            vec![settings_row(
                theme,
                "Prose width",
                "How wide the transcript's reading column runs. Balanced is the width Artisan was designed at; Tight shortens the line for focus, Loose spends more of the window on text.",
                Some(
                    self.segmented(
                        theme,
                        "settings-prose-width",
                        &width_options,
                        self.prose_width.as_str(),
                        "settings-prose-width",
                    )
                    .into_any_element(),
                ),
            )],
        );
        div()
            .flex()
            .flex_col()
            .child(settings_section_shell(
                theme,
                "formatting",
                "Formatting",
                Some("Time and path display choices are not yet configurable in the native app."),
                None,
                formatting,
            ))
            .child(settings_section_shell(
                theme, "glass", "Glass", Some("The shader choice is not yet configurable in the native app."), None, glass,
            ))
            .child(settings_section_shell(
                theme, "reading", "Reading", Some("The prose-width choice is not yet configurable in the native app."), None, reading,
            ))
    }

    /// Renders the appearance section (`appearance.svelte`).
    fn render_appearance(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        div()
            .flex()
            .flex_col()
            .child(settings_header(
                theme,
                SettingsSection::Appearance.title().to_owned(),
                SettingsSection::Appearance.description(),
            ))
            .child(self.render_appearance_top(theme))
            .child(self.render_appearance_bottom(theme))
    }

    /// Renders the engines page (`engine.svelte`).
    ///
    /// With a live [`SettingsEngineSnapshot`] the page paints the actual
    /// catalog, account, registry, and thread configuration with working
    /// refresh and model-choice actions; without one it keeps the static
    /// fixture copy with every action disabled. The switched-off branch of
    /// the fixture hides account and models exactly like legacy.
    fn render_engine(
        &self,
        theme: artisan_ui::theme::ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Div {
        let engine_id = self.engine_id.as_deref().unwrap_or(FIXTURE_ENGINE_ID);
        if !self.engine_known {
            return div().flex().flex_col().child(settings_header(
                theme,
                SETTINGS_UNKNOWN_ENGINE_TITLE.to_owned(),
                &unknown_engine_description(engine_id),
            ));
        }
        if let Some(snapshot) = self.engine_snapshot() {
            return self.render_live_engine(theme, snapshot, cx);
        }
        let label = self.engine_label();
        let description = format!(
            "Choose where {label} appears, manage its installation, and inspect its account and models."
        );
        let availability = settings_card(
            theme,
            vec![settings_row(
                theme,
                &format!("Enable {label}"),
                "Whether this engine is represented as available at all. Off, its models leave the model picker and its account is never asked for usage.",
                Some(
                    self.settings_switch(
                        theme,
                        "settings-switch-engine",
                        self.engine_enabled,
                        "settings-switch-engine",
                    )
                    .into_any_element(),
                ),
            )],
        );
        let mut page = div()
            .flex()
            .flex_col()
            .child(settings_header(theme, label.clone(), &description))
            .child(settings_section_shell(
                theme,
                "availability",
                "Availability",
                None,
                None,
                availability,
            ))
            .child(self.render_engine_installation(theme));
        if self.engine_enabled {
            page = page
                .child(self.render_engine_account(theme))
                .child(Self::render_engine_models(theme, engine_id, &label));
        } else {
            page = page.child(
                div()
                    .mt(px(32.0))
                    .text_sm()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(format!(
                        "{label} is switched off. Its models are hidden everywhere until it is enabled again."
                    )),
            );
        }
        page
    }

    /// Renders one live engine page from its orchestrator snapshot.
    ///
    /// Availability reads out the probed account verdict with a state badge
    /// instead of the fixture switch: native engines follow their installed
    /// CLI and account state and expose no separate enable control.
    /// Installation, account, and models paint the live check state with
    /// working refresh and model-choice actions; model configuration stays
    /// explicitly thread-scoped.
    fn render_live_engine(
        &self,
        theme: artisan_ui::theme::ArtisanTheme,
        snapshot: &SettingsEngineSnapshot,
        cx: &mut Context<Self>,
    ) -> Div {
        let label = self.engine_label();
        let description = format!(
            "Choose where {label} appears, manage its installation, and inspect its account and models."
        );
        let availability = settings_card(
            theme,
            vec![settings_row(
                theme,
                "Availability",
                "Whether this engine can run models. Native engines follow their installed CLI and account state; there is no separate switch.",
                Some(
                    div()
                        .flex_shrink_0()
                        .child(outline_badge(
                            BadgeStyle::resolve(theme),
                            snapshot.availability_badge(),
                        ))
                        .into_any_element(),
                ),
            )],
        );
        div()
            .flex()
            .flex_col()
            .child(settings_header(theme, label, &description))
            .child(settings_section_shell(
                theme,
                "availability",
                "Availability",
                None,
                None,
                availability,
            ))
            .child(self.render_live_engine_installation(theme, snapshot, cx))
            .child(self.render_live_engine_account(theme, snapshot, cx))
            .child(self.render_live_engine_models(theme, snapshot, cx))
    }

    /// Builds one live ghost-style action trigger emitting a screen event.
    fn live_action(
        selector: String,
        label: &'static str,
        event: SettingsScreenEvent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(selector.clone())
            .debug_selector(move || selector.clone())
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(event.clone());
            }))
            .p(px(4.0))
            .text_sm()
            .child(label.to_owned())
            .into_any_element()
    }

    /// Builds the engine refresh trigger for one live section.
    fn refresh_action(
        snapshot: &SettingsEngineSnapshot,
        selector: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let engine_id = snapshot.engine_id.clone();
        Self::live_action(
            selector.to_owned(),
            if snapshot.refreshing {
                "Checking…"
            } else {
                "Refresh"
            },
            SettingsScreenEvent::RefreshEngine { engine_id },
            cx,
        )
    }

    /// Renders the live installation section from the probed verdict.
    fn render_live_engine_installation(
        &self,
        theme: artisan_ui::theme::ArtisanTheme,
        snapshot: &SettingsEngineSnapshot,
        cx: &mut Context<Self>,
    ) -> Div {
        let body = settings_card(
            theme,
            vec![
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .py(px(20.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child(snapshot.installation_state()),
                    ),
            ],
        );
        settings_section_shell(
            theme,
            "installation",
            "Installation",
            None,
            Some(Self::refresh_action(snapshot, "settings-installation-refresh", cx)),
            body,
        )
    }

    /// Renders the live account section from the probed verdict.
    fn render_live_engine_account(
        &self,
        theme: artisan_ui::theme::ArtisanTheme,
        snapshot: &SettingsEngineSnapshot,
        cx: &mut Context<Self>,
    ) -> Div {
        let body = settings_card(
            theme,
            vec![
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .py(px(20.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child(snapshot.account_state()),
                    ),
            ],
        );
        settings_section_shell(
            theme,
            "account",
            "Account",
            None,
            Some(Self::refresh_action(snapshot, "settings-usage-refresh", cx)),
            body,
        )
    }

    /// Renders the live model rows with working choice actions.
    ///
    /// Every supported row chooses its model through
    /// [`SettingsScreenEvent::SelectEngineModel`], which the orchestrator
    /// serves through the existing `SelectPolicy` plus shared typed-save
    /// flow — the same path as the composer picker, with the same
    /// compare-and-swap save and acknowledgment. Catalog-disabled rows stay
    /// unclickable with their honest reason. Model configuration is
    /// thread-scoped: without a selected thread the rows name that instead
    /// of pretending to save.
    fn render_live_engine_models(
        &self,
        theme: artisan_ui::theme::ArtisanTheme,
        snapshot: &SettingsEngineSnapshot,
        cx: &mut Context<Self>,
    ) -> Div {
        let label = self.engine_label();
        let mut blocks = Vec::new();
        if let Some(notice) = snapshot.choice_notice.as_deref() {
            blocks.push(
                div()
                    .w_full()
                    .py(px(12.0))
                    .text_sm()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(notice.to_owned()),
            );
        }
        if snapshot.save_failed {
            blocks.push(
                div()
                    .w_full()
                    .py(px(12.0))
                    .text_sm()
                    .text_color(theme.colors.destructive.to_paint())
                    .child(
                        "The model configuration was not saved. Retry the choice or the save action.",
                    ),
            );
        }
        if snapshot.models.is_empty() {
            blocks.push(
                div()
                    .w_full()
                    .py(px(20.0))
                    .text_sm()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(format!("No models are listed for {label} yet.")),
            );
        }
        for model in &snapshot.models {
            blocks.push(self.live_engine_model_row(theme, snapshot, model, cx));
        }
        if snapshot.selected_thread.is_none() {
            blocks.push(
                div()
                    .w_full()
                    .py(px(12.0))
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(format!(
                        "Engine models save to the selected thread. Select a thread to configure {label}."
                    )),
            );
        } else if let (Some(saved), Some(profile)) =
            (snapshot.saved_model.as_deref(), snapshot.saved_profile.as_deref())
        {
            blocks.push(
                div()
                    .w_full()
                    .py(px(12.0))
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(format!("Saved for this thread: {saved} on profile {profile}.")),
            );
        }
        let action = if snapshot.pending_save {
            Some(
                div()
                    .p(px(4.0))
                    .text_sm()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child("Saving…")
                    .into_any_element(),
            )
        } else if snapshot.can_save_displayed {
            let engine_id = snapshot.engine_id.clone();
            Some(Self::live_action(
                "settings-engine-save-model".to_owned(),
                "Save",
                SettingsScreenEvent::SaveDisplayedModel { engine_id },
                cx,
            ))
        } else {
            None
        };
        settings_section_shell(theme, "models", "Models", None, action, settings_card(theme, blocks))
    }

    /// Paints one live engine model row with its state badges.
    ///
    /// Selectable rows (supported model, thread selected) emit
    /// [`SettingsScreenEvent::SelectEngineModel`] on activation; disabled
    /// rows and thread-less pages paint without an action.
    fn live_engine_model_row(
        &self,
        theme: artisan_ui::theme::ArtisanTheme,
        snapshot: &SettingsEngineSnapshot,
        model: &SettingsEngineModel,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut row = div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .py(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child(model.id.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(self.engine_label()),
                    ),
            );
        if model.saved {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .child(outline_badge(BadgeStyle::resolve(theme), "Saved")),
            );
        } else if model.displayed {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .child(outline_badge(BadgeStyle::resolve(theme), "Selected")),
            );
        }
        if let Some(reason) = model.disabled_reason.as_deref() {
            row = row.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .items_end()
                    .child(outline_badge(BadgeStyle::resolve(theme), "Disabled"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(reason.to_owned()),
                    ),
            );
            return row;
        }
        if snapshot.selected_thread.is_some() {
            let engine_id = snapshot.engine_id.clone();
            let model_id = model.id.clone();
            let selector = format!("settings-engine-model-{model_id}");
            row = row
                .id(selector.clone())
                .debug_selector(move || selector.clone())
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(SettingsScreenEvent::SelectEngineModel {
                        engine_id: engine_id.clone(),
                        model_id: model_id.clone(),
                    });
                }));
        }
        row
    }

    /// Renders the engine installation section with fixture status copy.
    fn render_engine_installation(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        let action = self
            .settings_button(
                theme,
                "settings-installation-check",
                "Check for updates".to_owned(),
                ButtonVariant::Ghost,
                "settings-installation-check",
            )
            .into_any_element();
        let body = settings_card(
            theme,
            vec![
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .py(px(20.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child("Managed installation ready."),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child("Managed by Artisan with an isolated provider home."),
                    ),
            ],
        );
        settings_section_shell(
            theme,
            "installation",
            "Installation",
            None,
            Some(action),
            body,
        )
    }

    /// Renders the engine account section with the honest unknown state.
    fn render_engine_account(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        let action = self
            .settings_button(
                theme,
                "settings-usage-refresh",
                "Refresh".to_owned(),
                ButtonVariant::Ghost,
                "settings-usage-refresh",
            )
            .into_any_element();
        let body = settings_card(
            theme,
            vec![
                div().w_full().flex().flex_col().py(px(20.0)).child(
                    div()
                        .text_sm()
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child("Sign-in status unknown"),
                ),
            ],
        );
        settings_section_shell(theme, "account", "Account", None, Some(action), body)
    }

    /// Renders the engine model rows from the fixture catalog.
    fn render_engine_models(
        theme: artisan_ui::theme::ArtisanTheme,
        engine_id: &str,
        label: &str,
    ) -> Div {
        let models = models_for_fixture_engine(engine_id);
        let mut blocks = Vec::new();
        if models.is_empty() {
            blocks.push(
                div()
                    .w_full()
                    .py(px(20.0))
                    .text_sm()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(format!("No models are listed for {label} yet.")),
            );
        }
        for model in &models {
            blocks.push(Self::engine_model_row(theme, model, label));
        }
        settings_section_shell(
            theme,
            "models",
            "Models",
            None,
            None,
            settings_card(theme, blocks),
        )
    }

    /// Paints one engine model row with its badges (`engine.svelte`).
    ///
    /// Variant counts and the compaction-default mark need the full catalog
    /// snapshot; the `Disabled` badge renders from the fixture definition
    /// today.
    fn engine_model_row(
        theme: artisan_ui::theme::ArtisanTheme,
        model: &crate::model_selection_presentation::ModelChoice,
        label: &str,
    ) -> Div {
        let mut row = div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .py(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.colors.foreground.to_paint())
                            .child(model.id.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(label.to_owned()),
                    ),
            );
        if model.definition.disabled.is_some() {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .child(outline_badge(BadgeStyle::resolve(theme), "Disabled")),
            );
        }
        row
    }

    /// Renders the notifications section (`notifications.svelte`).
    ///
    /// The enable switch and re-check button need the system-notifications
    /// service; the gap notice renders from the injected snapshot so wired
    /// states already show the legacy blocked/unprompted copy.
    fn render_notifications(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        let gap = system_notification_gap_for(self.notifications);
        let switch_control = if gap == SystemNotificationGap::Unsupported {
            settings_row(
                theme,
                "Notify me",
                "This host exposes no notification API, so there is nothing for Artisan to post to.",
                Some(
                    self.settings_switch(
                        theme,
                        "settings-switch-notify",
                        false,
                        "settings-switch-notify",
                    )
                    .into_any_element(),
                ),
            )
        } else {
            settings_row(
                theme,
                "Notify me",
                "Posts a system notification when a thread finishes, fails, or needs an answer from you. Artisan appears in your operating system's notification settings, where you can silence or restyle it.",
                Some(
                    self.settings_switch(
                        theme,
                        "settings-switch-notify",
                        self.notifications.enabled,
                        "settings-switch-notify",
                    )
                    .into_any_element(),
                ),
            )
        };
        let mut blocks = vec![switch_control];
        if let Some((title, description)) = notification_gap_notice(gap) {
            let retry = self
                .settings_button(
                    theme,
                    "settings-notifications-retry",
                    "Check again".to_owned(),
                    ButtonVariant::Outline,
                    "settings-notifications-retry",
                )
                .into_any_element();
            blocks.push(settings_row(theme, title, description, Some(retry)));
        }
        blocks.push(settings_row(
            theme,
            "Clears itself",
            "A notification you don't answer disappears after a few seconds. Nothing is lost by letting it go \u{2014} an approval or a question keeps waiting in its thread \u{2014} so a notification can never pile up into something you have to go and dismiss.",
            None,
        ));
        div()
            .flex()
            .flex_col()
            .child(settings_header(
                theme,
                SettingsSection::Notifications.title().to_owned(),
                SettingsSection::Notifications.description(),
            ))
            .child(settings_section_shell(
                theme,
                "system",
                "System",
                Some("Notification controls are not yet connected in the native app."),
                None,
                settings_card(theme, blocks),
            ))
    }

    /// Renders the privacy section (`privacy.svelte`).
    fn render_privacy(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        let telemetry = settings_card(
            theme,
            vec![
                self.telemetry_row(
                    theme,
                    "settings-switch-usage-analytics",
                    "Usage analytics",
                    "Sends allowlisted product events to PostHog using a random installation ID. Artisan never sends prompts, responses, source code, diffs, terminal activity, names, or paths.",
                    self.telemetry.usage_analytics,
                ),
                self.telemetry_row(
                    theme,
                    "settings-switch-crash-reports",
                    "Crash reports",
                    "Sends sanitized exceptions, crash reasons, release information, and coarse performance diagnostics to Sentry. Attachments, replay, console logs, request data, environment variables, and process arguments stay off.",
                    self.telemetry.crash_reports,
                ),
            ],
        );
        let never = settings_card(
            theme,
            vec![settings_row(
                theme,
                "Your work stays local",
                "Prompts, model responses, source code, file contents, diffs, terminal commands and output, repository and project names, paths, credentials, headers, request bodies, process arguments, and environment variables are prohibited from both systems.",
                None,
            )],
        );
        div()
            .flex()
            .flex_col()
            .child(settings_header(
                theme,
                SettingsSection::Privacy.title().to_owned(),
                SettingsSection::Privacy.description(),
            ))
            .child(settings_section_shell(
                theme,
                "telemetry",
                "Observability",
                Some("Telemetry choices are not yet configurable in the native app."),
                None,
                telemetry,
            ))
            .child(settings_section_shell(
                theme,
                "never-collected",
                "Never collected",
                None,
                None,
                never,
            ))
    }

    /// Paints one telemetry row with its choice caption and switch.
    fn telemetry_row(
        &self,
        theme: artisan_ui::theme::ArtisanTheme,
        id: &'static str,
        title: &str,
        description: &str,
        choice: TelemetryPreference,
    ) -> Div {
        let control = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(telemetry_choice_caption(choice).to_owned()),
            )
            .child(
                self.settings_switch(theme, id, choice == TelemetryPreference::Enabled, id)
                    .into_any_element(),
            );
        settings_row(theme, title, description, Some(control.into_any_element()))
    }

    /// Renders the threads section (`threads.svelte` plus `thread-titles`,
    /// `usage-recovery`, and `agent-names`).
    ///
    /// Retention, titles, recovery, and the name set need the retention and
    /// session-defaults controllers. The threshold paints as static text
    /// because the numeric input has no wired commit path yet; the agent
    /// name set uses the real closed `Select`.
    fn render_threads(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        div()
            .flex()
            .flex_col()
            .child(settings_header(
                theme,
                SettingsSection::Threads.title().to_owned(),
                SettingsSection::Threads.description(),
            ))
            .child(self.render_thread_retention(theme))
            .child(self.render_thread_titles(theme))
            .child(self.render_usage_recovery(theme))
            .child(self.render_agent_names(theme))
    }

    /// Renders the retention section with its loading and unverified states.
    fn render_thread_retention(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        let mut column = div().flex().flex_col();
        if matches!(
            self.retention.policy_state(),
            ThreadRetentionPolicyState::Unverified
        ) {
            column = column.child(self.retention_unverified_banner(theme));
        }
        column.child(settings_section_shell(
            theme,
            "retention",
            "Retention",
            Some("Retention controls are not yet connected in the native app."),
            None,
            self.retention_body(theme),
        ))
    }

    /// Paints the unverified-policy warning banner above the retention card.
    fn retention_unverified_banner(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .mt(px(12.0))
            .px(px(12.0))
            .py(px(12.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(theme.colors.banner_warning.to_paint())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.colors.foreground.to_paint())
                            .child(self.retention.failure_title().to_owned()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(
                                "Forge could not confirm the durable policy. Controls are disabled.",
                            ),
                    ),
            )
            .child(
                self.settings_button(
                    theme,
                    "settings-retention-retry",
                    "Retry".to_owned(),
                    ButtonVariant::Outline,
                    "settings-retention-retry",
                )
                .into_any_element(),
            )
    }

    /// Paints the retention card: loading copy or the two policy rows.
    fn retention_body(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        match self.retention.policy() {
            None => settings_card(
                theme,
                vec![
                    div()
                        .w_full()
                        .py(px(16.0))
                        .text_sm()
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child("Loading retention policy\u{2026}"),
                ],
            ),
            Some(policy) => settings_card(
                theme,
                vec![
                    settings_row(
                        theme,
                        "Erase inactive threads",
                        "Permanently erases a thread \u{2014} conversation, checkpoints, and lineage \u{2014} once it has been untouched for the configured number of days. This is deletion, not archival.",
                        Some(
                            self.settings_switch(
                                theme,
                                "settings-switch-retention",
                                policy.enabled,
                                "settings-switch-retention",
                            )
                            .into_any_element(),
                        ),
                    ),
                    settings_row(
                        theme,
                        "Inactivity threshold",
                        "Days a thread must be untouched before it is erased. Between 1 and 3650.",
                        Some(
                            div()
                                .text_sm()
                                .text_color(theme.colors.muted_foreground.to_paint())
                                .child(format!("{} days", policy.inactivity_days))
                                .into_any_element(),
                        ),
                    ),
                ],
            ),
        }
    }

    /// Renders the summary-titles section (`thread-titles.svelte`).
    fn render_thread_titles(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        let mut blocks = vec![settings_row(
            theme,
            "Summary titles",
            "Name threads with the harness's own generated summary. When off, a thread is named by the latest message you sent. A rename of your own always wins, and threads without a summary keep the message title.",
            Some(
                self.settings_switch(
                    theme,
                    "settings-switch-titles",
                    self.thread_title.summarized(),
                    "settings-switch-titles",
                )
                .into_any_element(),
            ),
        )];
        if !self.thread_title.message.is_empty() {
            blocks.push(
                div()
                    .w_full()
                    .py(px(12.0))
                    .text_sm()
                    .text_color(theme.colors.destructive.to_paint())
                    .child(self.thread_title.message.clone()),
            );
        }
        settings_section_shell(
            theme,
            "thread-titles",
            "Titles",
            Some("Title choices are not yet configurable in the native app."),
            None,
            settings_card(theme, blocks),
        )
    }

    /// Renders the usage-recovery section (`usage-recovery.svelte`).
    fn render_usage_recovery(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        let mut blocks = vec![settings_row(
            theme,
            "Automatically continue after usage resets",
            "New turns interrupted by a provider limit continue once Forge verifies the usage window has reset. You can still change this on each interruption card.",
            Some(
                self.settings_switch(
                    theme,
                    "settings-switch-recovery",
                    self.usage_recovery.auto_continue_usage_limits,
                    "settings-switch-recovery",
                )
                .into_any_element(),
            ),
        )];
        if !self.usage_recovery.message.is_empty() {
            blocks.push(
                div()
                    .w_full()
                    .py(px(12.0))
                    .text_sm()
                    .text_color(theme.colors.destructive.to_paint())
                    .child(self.usage_recovery.message.clone()),
            );
        }
        settings_section_shell(
            theme,
            "usage-recovery",
            "Usage recovery",
            Some("Usage-recovery choices are not yet configurable in the native app."),
            None,
            settings_card(theme, blocks),
        )
    }

    /// Renders the agent name-set section (`agent-names.svelte`).
    ///
    /// The dataset select paints the fixture value through the mountable
    /// `NativeSelect`. Note: the richer `Select` listbox used by the
    /// font-picker popovers has no `IntoElement` impl in `artisan-ui`, so it
    /// cannot mount here; those triggers stay disabled buttons (see
    /// [`SettingsScreen::font_trigger`]).
    fn render_agent_names(&self, theme: artisan_ui::theme::ArtisanTheme) -> Div {
        let mut options = Vec::new();
        for (id, label, description) in AGENT_NAME_DATASETS {
            options.push(
                NativeSelectOption::new(id, format!("{label} \u{2014} {description}"))
                    .expect("static agent dataset options are valid"),
            );
        }
        let select = NativeSelect::new(
            "settings-select-agent-names",
            self.focus.control.clone(),
            theme,
            self.agent_dataset.clone(),
            options,
        )
        .expect("static agent dataset select is valid")
        .disabled(true)
        .semantic_label("Agent name set")
        .debug_selector("settings-select-agent-names");
        let body = settings_card(
            theme,
            vec![settings_row(
                theme,
                "Name set",
                "The catalog Artisan uses when it names a new delegated agent.",
                Some(select.into_any_element()),
            )],
        );
        settings_section_shell(
            theme,
            "agents",
            "Agents",
            Some("The name set is not yet configurable in the native app."),
            None,
            body,
        )
    }

    /// Renders the scrollable content column for the mounted section.
    fn render_main(&self, theme: artisan_ui::theme::ArtisanTheme, cx: &mut Context<Self>) -> Div {
        let section = match self.section {
            SettingsRoute::Models => self.render_models(theme),
            SettingsRoute::Appearance => self.render_appearance(theme),
            SettingsRoute::Engines => self.render_engine(theme, cx),
            SettingsRoute::Notifications => self.render_notifications(theme),
            SettingsRoute::Privacy => self.render_privacy(theme),
            SettingsRoute::Threads => self.render_threads(theme),
        };
        div().flex().flex_col().flex_1().min_w_0().child(section)
    }
}

impl Render for SettingsScreen {
    /// Renders the settings frame: centered row of nav rail plus section.
    ///
    /// Legacy (`+layout.svelte`): `max-w-4xl` row with `md:gap-14`, sticky
    /// `md:w-44` aside, growing `main`. The native window is always wide, so
    /// only the desktop arrangement is painted; the mobile top-bar variant
    /// and the hash-scroll effect are orchestrator gaps.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let selector = settings_screen_selector(self.section);
        let nav = self.render_nav(theme, cx);
        let main = self.render_main(theme, cx);
        div()
            .id("settings-screen")
            .track_focus(&self.focus.root)
            .debug_selector(move || selector.clone())
            .size_full()
            .overflow_y_scroll()
            .bg(theme.colors.background.to_paint())
            .child(
                div().w_full().flex().flex_row().justify_center().child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .max_w(px(SETTINGS_CONTENT_MAX_WIDTH_PX))
                        .px(px(24.0))
                        .py(px(48.0))
                        .gap(px(SETTINGS_NAV_CONTENT_GAP_PX))
                        .child(nav)
                        .child(main),
                ),
            )
    }
}

#[cfg(test)]
mod settings_screen_tests {
    use super::*;
    use crate::native_route::NativeRoute;

    #[test]
    fn route_mapping_covers_every_section() {
        assert_eq!(
            settings_section_for_route(SettingsRoute::Models),
            SettingsSection::Models
        );
        assert_eq!(
            settings_section_for_route(SettingsRoute::Appearance),
            SettingsSection::Appearance
        );
        assert_eq!(
            settings_section_for_route(SettingsRoute::Engines),
            SettingsSection::Engines
        );
        assert_eq!(
            settings_section_for_route(SettingsRoute::Notifications),
            SettingsSection::Notifications
        );
        assert_eq!(
            settings_section_for_route(SettingsRoute::Privacy),
            SettingsSection::Privacy
        );
        assert_eq!(
            settings_section_for_route(SettingsRoute::Threads),
            SettingsSection::Threads
        );
    }

    #[test]
    fn screen_selector_matches_route_selector_suffix() {
        for route in [
            SettingsRoute::Models,
            SettingsRoute::Appearance,
            SettingsRoute::Engines,
            SettingsRoute::Notifications,
            SettingsRoute::Privacy,
            SettingsRoute::Threads,
        ] {
            assert_eq!(
                settings_screen_selector(route),
                NativeRoute::Settings {
                    section: route,
                    engine: None,
                }
                .selector_suffix()
            );
        }
        assert_eq!(
            settings_screen_selector(SettingsRoute::Models),
            "route-settings-models"
        );
    }

    #[test]
    fn engine_label_prefers_explicit_then_id_then_fixture() {        assert_eq!(
            resolve_engine_label(Some("Custom"), Some("codex")),
            "Custom"
        );
        assert_eq!(resolve_engine_label(None, Some("codex")), "codex");
        assert_eq!(resolve_engine_label(None, None), FIXTURE_ENGINE_LABEL);
        assert_eq!(
            unknown_engine_description("nope"),
            "No engine with id \"nope\" exists in the catalog."
        );
    }

    #[test]
    fn disabled_engine_hides_account_and_model_anchors() {
        let full = visible_anchors(SettingsSection::Engines, true);
        assert_eq!(full.len(), 4);
        let reduced = visible_anchors(SettingsSection::Engines, false);
        assert_eq!(reduced.len(), 2);
        assert_eq!(reduced[0].hash, "availability");
        assert_eq!(reduced[1].hash, "installation");
        assert_eq!(
            visible_anchors(SettingsSection::Models, false).len(),
            SettingsSection::Models.anchors().len()
        );
    }

    #[test]
    fn gap_notices_match_legacy_desktop_copy() {        assert!(notification_gap_notice(SystemNotificationGap::None).is_none());
        assert!(notification_gap_notice(SystemNotificationGap::Unsupported).is_none());
        let (blocked_title, _) =
            notification_gap_notice(SystemNotificationGap::Blocked).expect("blocked notice");
        assert_eq!(blocked_title, "Blocked by your system");
        let (unprompted_title, _) =
            notification_gap_notice(SystemNotificationGap::Unprompted).expect("unprompted notice");
        assert_eq!(unprompted_title, "Not allowed yet");
    }

    #[test]
    fn appearance_literals_match_legacy_defaults() {
        assert_eq!(AppearanceTimeFormat::TwelveHour.as_str(), "12-hour");
        assert_eq!(AppearanceTimeFormat::TwentyFourHour.as_str(), "24-hour");
        assert_eq!(AppearancePathSeparator::Backslash.character(), "\\");
        assert_eq!(AppearancePathSeparator::ForwardSlash.character(), "/");
        assert_eq!(AppearancePathSeparator::Backslash.label(), "Backslash");
        assert_eq!(APPEARANCE_DEFAULT_TEXT_FONT, "Spline Sans");
        assert_eq!(APPEARANCE_DEFAULT_CODE_FONT, "Spline Sans Mono");
        assert_eq!(AGENT_NAME_DATASET_DEFAULT, "norwegian");
        assert_eq!(AGENT_NAME_DATASETS.len(), 2);
        assert_eq!(ProseWidth::Balanced.as_str(), "balanced");
    }

    #[test]
    fn telemetry_captions_cover_every_choice() {
        assert_eq!(
            telemetry_choice_caption(TelemetryPreference::Unset),
            "Not decided"
        );
        assert_eq!(telemetry_choice_caption(TelemetryPreference::Enabled), "On");
        assert_eq!(
            telemetry_choice_caption(TelemetryPreference::Disabled),
            "Off"
        );
    }

    fn live_snapshot(
        readiness: crate::native_profile_usage::EngineReadiness,
    ) -> SettingsEngineSnapshot {
        SettingsEngineSnapshot {
            engine_id: "codex".to_owned(),
            readiness,
            account_email: None,
            refresh_failure: None,
            refreshing: false,
            catalog: SettingsEngineCatalogState::Loading,
            catalog_error: None,
            registry: SettingsEngineRegistryState::Missing,
            selected_thread: None,
            saved_model: None,
            saved_profile: None,
            displayed_model: None,
            displayed_authoritative: false,
            can_save_displayed: false,
            pending_save: false,
            save_failed: false,
            choice_notice: None,
            models: Vec::new(),
        }
    }

    #[test]
    fn live_engine_states_name_the_probed_verdict() {
        use crate::native_profile_usage::EngineReadiness;

        let ready = SettingsEngineSnapshot {
            account_email: Some("owner@example.test".to_owned()),
            ..live_snapshot(EngineReadiness::Ready)
        };
        assert_eq!(ready.availability_badge(), "Available");
        assert!(ready.installation_state().contains("owner@example.test"));
        assert!(ready.account_state().contains("owner@example.test"));

        let signin = live_snapshot(EngineReadiness::NeedsSignIn);
        assert_eq!(signin.availability_badge(), "Sign-in required");
        // A responding executable proves installation even without sign-in.
        assert!(signin.installation_state().contains("Installed"));
        assert!(signin.account_state().contains("No account"));

        let checking = live_snapshot(EngineReadiness::Checking);
        assert_eq!(checking.availability_badge(), "Checking");

        // An unchecked engine never claims a missing installation.
        let unknown = live_snapshot(EngineReadiness::NotReady);
        assert_eq!(unknown.availability_badge(), "Unavailable");
        assert!(!unknown.installation_state().contains("install"));
        assert!(!unknown.installation_state().contains("repair"));

        let failed = SettingsEngineSnapshot {
            refresh_failure: Some("provider usage read timed out".to_owned()),
            ..live_snapshot(EngineReadiness::NotReady)
        };
        assert!(failed.installation_state().contains("provider usage read timed out"));
        assert!(failed.account_state().contains("provider usage read timed out"));
    }
}
