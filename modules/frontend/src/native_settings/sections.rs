//! Settings navigation model, static section copy, and fixture constructors.
//!
//! Extracted verbatim from `native_settings.rs` during the module split.

use super::*;

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
    /// `/settings/about`.
    About,
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

const ABOUT_ANCHORS: [SettingsAnchor; 1] = [SettingsAnchor::new("build", "Build")];

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

const ABOUT_PRIMITIVES: [SettingsPrimitive; 1] = [SettingsPrimitive::Card];

impl SettingsSection {
    /// Every section in legacy sticky-nav order (`nav.svelte`).
    pub const ALL: [Self; 7] = [
        Self::Models,
        Self::Threads,
        Self::Appearance,
        Self::Notifications,
        Self::Privacy,
        Self::About,
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
            Self::About => "/settings/about",
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
            Self::About => "About",
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
            Self::About => "About",
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
            Self::About => "Which build of Artisan is running and where it is installed.",
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
            Self::About => &ABOUT_ANCHORS,
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
            Self::About => &ABOUT_PRIMITIVES,
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
        "/settings/about" => Some(SettingsSection::About),
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
                | (SettingsSection::About, SettingsSection::About)
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
        SettingsRoute::About => SettingsSection::About,
    }
}

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
        SettingsSection::About => SettingsRoute::About,
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
