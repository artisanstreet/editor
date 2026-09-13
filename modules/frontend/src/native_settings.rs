//! Native settings shell composition over static fixture data (no Forge).
//!
//! # Live vs static inventory
//!
//! LIVE (pure, deterministic, covered by the inline tests below):
//! - The FPS select applies a local window limit and persists it through
//!   `native_frame_rate`; its dropdown and keyboard interaction are tested.
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
//! - The fixture switches, tabs, toggle groups, cards, collapsibles, and tooltips
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

// Phase-1 split submodules (see native_settings/).

#[path = "native_settings/sections.rs"]
mod sections;

#[path = "native_settings/screen.rs"]
mod screen;

#[path = "native_settings/render.rs"]
mod render;

#[cfg(test)]
#[path = "native_settings/tests.rs"]
mod tests;

// Re-exports keep the public surface identical to the pre-split module.
pub use render::{notification_gap_notice, telemetry_choice_caption};
pub use screen::{
    SettingsEngineCatalogState, SettingsEngineModel, SettingsEngineNavEntry,
    SettingsEngineRegistryState, SettingsEngineSnapshot, SettingsScreenEvent, resolve_engine_label,
    unknown_engine_description,
};
pub use sections::{
    AGENT_NAME_DATASET_DEFAULT, AGENT_NAME_DATASETS, APPEARANCE_APP_ICON_ALTERNATE,
    APPEARANCE_APP_ICON_ALTERNATE_LABEL, APPEARANCE_APP_ICON_DEFAULT,
    APPEARANCE_APP_ICON_DEFAULT_LABEL, APPEARANCE_DEFAULT_CODE_FONT, APPEARANCE_DEFAULT_TEXT_FONT,
    AppearancePathSeparator, AppearanceTimeFormat, FIXTURE_ENGINE_ID, FIXTURE_ENGINE_LABEL,
    FIXTURE_RETENTION_DAYS, SETTINGS_CONTENT_MAX_WIDTH_PX, SETTINGS_NAV_CONTENT_GAP_PX,
    SETTINGS_NAV_RAIL_WIDTH_PX, SETTINGS_NAV_SELECTOR, SETTINGS_SCREEN_SELECTOR_PREFIX,
    SETTINGS_UNKNOWN_ENGINE_TITLE, SettingsAnchor, SettingsPrimitive, SettingsSection,
    SettingsSectionSnapshot, SettingsShell, fixture_card_style, fixture_collapsible_state,
    fixture_engine_status, fixture_engine_template, fixture_models, fixture_notification_default,
    fixture_notifications, fixture_retention_is_valid, fixture_retention_policy,
    fixture_retention_state, fixture_session_defaults, fixture_switch_style, fixture_tabs_style,
    fixture_telemetry, fixture_theme, fixture_toggle_group_style, fixture_tooltip_style,
    fixture_usage_recovery, models_for_fixture_engine, nav_tab_specs, resolve_fixture_telemetry,
    section_for_href, section_route, section_snapshot, settings_screen_selector,
    settings_section_for_route, thinking_for_fixture_model, visible_anchors,
};

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
    frame_rate_control: FrameRateControl,
    text_font: String,
    code_font: String,
    agent_dataset: String,
    engine_snapshot: Option<SettingsEngineSnapshot>,
    engines: Vec<SettingsEngineNavEntry>,
}

#[derive(Default)]
struct FrameRateControl {
    open: bool,
    scroll: gpui::ScrollHandle,
    interaction: std::rc::Rc<std::cell::RefCell<artisan_ui::select::SelectState>>,
    error: Option<String>,
}
