//! Live engine-page snapshots and the settings screen's owned state.
//!
//! Extracted verbatim from `native_settings.rs` during the module split.

use super::*;

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
#[expect(
    clippy::struct_excessive_bools,
    reason = "five independent readiness bits are the projected engine state; packing them into enums would not make the page decisions clearer"
)]
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
    ///
    /// A dashboard-authenticated Cursor is "Signed in", never "Available":
    /// dashboard auth proves the account, not a runnable local CLI.
    #[must_use]
    pub fn availability_badge(&self) -> &'static str {
        if self.engine_id == "cursor"
            && self.readiness == crate::native_profile_usage::EngineReadiness::Ready
        {
            return "Signed in";
        }
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
    /// or login-gated answer from a CLI probe means the binary runs,
    /// anything else is a check status, never an install claim. The
    /// dashboard-read Cursor states its unverified CLI explicitly:
    /// account verdicts never promote it to Installed.
    #[must_use]
    pub fn installation_state(&self) -> String {
        if self.engine_id == "cursor" {
            match self.readiness {
                crate::native_profile_usage::EngineReadiness::Ready => match &self.account_email {
                    Some(email) => {
                        return format!(
                            "Signed in as {email}. Cursor CLI installation is unverified."
                        );
                    }
                    None => {
                        return "Signed in. Cursor CLI installation is unverified.".to_owned();
                    }
                },
                crate::native_profile_usage::EngineReadiness::NeedsSignIn => {
                    return "Cursor CLI installation is unverified. Account sign-in is required."
                        .to_owned();
                }
                _ => {}
            }
        }
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
            crate::native_profile_usage::EngineReadiness::NotReady => match &self.refresh_failure {
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
            crate::native_profile_usage::EngineReadiness::NotReady => match &self.refresh_failure {
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
        self.engine_snapshot
            .as_ref()
            .filter(|snapshot| self.engine_id.as_deref() == Some(snapshot.engine_id.as_str()))
    }

    /// Replaces the real catalog engines behind the nav rail.
    ///
    /// The orchestrator feeds the manifest harness identities, so the rail
    /// enumerates actual engines and never fixture identities. An empty list
    /// keeps the legacy single engine row.
    pub fn set_engines(&mut self, engines: Vec<SettingsEngineNavEntry>, cx: &mut Context<Self>) {
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
