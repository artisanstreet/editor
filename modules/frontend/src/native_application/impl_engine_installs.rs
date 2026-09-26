//! The Forge-managed engine installs as the Editor sees them: the snapshot
//! the Forge serves and pushes, the vendor version lists the Settings page
//! asked for, and version changes sent back to the Forge.
//!
//! The Editor decides nothing about installs; it renders Forge data and
//! forwards the user's selections.

use std::collections::BTreeMap;

use artisan_domain::{
    ChangeEngineVersion, EngineInstallSnapshot, EngineVersionChange, EngineVersionSelection,
};

use crate::native_settings::{SettingsEngineInstall, SettingsEngineVersions};
use crate::native_transport_service::{
    EngineInstallsCommand, EngineInstallsEvent, NativeTransportCommand,
};

use super::{Context, NativeApplication, SettingsRoute, SettingsScreenEvent};

/// Host-scoped engine install state; dropped with the view on a host switch.
#[derive(Debug, Default)]
pub(super) struct EngineInstallsState {
    /// The Forge's latest snapshot, once read or pushed.
    snapshot: Option<EngineInstallSnapshot>,
    /// Whether the first read was sent.
    requested: bool,
    /// Vendor version lists per engine.
    versions: BTreeMap<String, SettingsEngineVersions>,
    /// The last refused version request per engine.
    failures: BTreeMap<String, String>,
}

impl NativeApplication {
    /// Reads the engine installs once per connection; later changes are
    /// pushed by the Forge.
    pub(super) fn ensure_engine_installs_read(&mut self) {
        if self.engine_installs.requested {
            return;
        }
        if self
            .submit_command(NativeTransportCommand::EngineInstalls(
                EngineInstallsCommand::Read,
            ))
            .is_ok()
        {
            self.engine_installs.requested = true;
        }
    }

    /// Applies a pushed or served snapshot.
    pub(super) fn apply_engine_installs(
        &mut self,
        snapshot: EngineInstallSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.engine_installs.snapshot = Some(snapshot);
        self.refresh_settings_engine_snapshot(cx);
    }

    /// Receives one engine-install transport answer.
    pub(super) fn receive_engine_installs(
        &mut self,
        event: EngineInstallsEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            EngineInstallsEvent::Snapshot(Ok(snapshot)) => {
                self.engine_installs.failures.clear();
                self.apply_engine_installs(snapshot, cx);
            }
            EngineInstallsEvent::Snapshot(Err(_)) => {
                self.engine_installs.requested = self.engine_installs.snapshot.is_some();
                if let Some((SettingsRoute::Engines, Some(engine_id))) =
                    self.settings_screen_key.clone()
                {
                    self.engine_installs.failures.insert(
                        engine_id,
                        "The Forge could not apply the version change. Try again.".to_owned(),
                    );
                }
                self.refresh_settings_engine_snapshot(cx);
            }
            EngineInstallsEvent::Versions { engine_id, result } => {
                let versions = match result {
                    Ok(list) => SettingsEngineVersions::Loaded(list.versions().to_vec()),
                    Err(_) => SettingsEngineVersions::Failed(
                        "The Forge could not reach the vendor\u{2019}s release list. Try again."
                            .to_owned(),
                    ),
                };
                self.engine_installs.versions.insert(engine_id, versions);
                self.refresh_settings_engine_snapshot(cx);
            }
        }
    }

    /// Asks the Forge for one engine's published versions.
    pub(super) fn load_engine_versions(&mut self, engine_id: &str, cx: &mut Context<Self>) {
        let command = NativeTransportCommand::EngineInstalls(EngineInstallsCommand::ListVersions(
            engine_id.to_owned(),
        ));
        if self.submit_command(command).is_ok() {
            self.engine_installs
                .versions
                .insert(engine_id.to_owned(), SettingsEngineVersions::Loading);
        }
        self.refresh_settings_engine_snapshot(cx);
    }

    /// Sends one version change; the Forge answers with the queued snapshot
    /// and pushes progress.
    pub(super) fn change_engine_version(
        &mut self,
        engine_id: &str,
        change: EngineVersionChange,
        cx: &mut Context<Self>,
    ) {
        self.engine_installs.failures.remove(engine_id);
        let command = NativeTransportCommand::EngineInstalls(EngineInstallsCommand::Change(
            ChangeEngineVersion {
                engine_id: engine_id.to_owned(),
                change,
            },
        ));
        if self.submit_command(command).is_err() {
            self.engine_installs.failures.insert(
                engine_id.to_owned(),
                "The Editor is not connected to a Forge.".to_owned(),
            );
        }
        // A chosen version list is stale once the selection changes.
        self.engine_installs.versions.remove(engine_id);
        self.refresh_settings_engine_snapshot(cx);
    }

    /// Serves the Settings version events.
    pub(super) fn handle_engine_install_event(
        &mut self,
        event: &SettingsScreenEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            SettingsScreenEvent::LoadEngineVersions { engine_id } => {
                self.load_engine_versions(engine_id, cx);
            }
            SettingsScreenEvent::SelectEngineVersion { engine_id, version } => {
                let selection = version.clone().map_or(
                    EngineVersionSelection::Latest,
                    EngineVersionSelection::Version,
                );
                self.change_engine_version(engine_id, EngineVersionChange::Select(selection), cx);
            }
            SettingsScreenEvent::RollbackEngine { engine_id } => {
                self.change_engine_version(engine_id, EngineVersionChange::Rollback, cx);
            }
            _ => {}
        }
    }

    /// Projects one engine's managed install for the Settings page.
    pub(super) fn settings_engine_install(&self, engine_id: &str) -> Option<SettingsEngineInstall> {
        let status = self
            .engine_installs
            .snapshot
            .as_ref()?
            .engine(engine_id)?
            .clone();
        Some(SettingsEngineInstall {
            label: managed_engine_label(engine_id).to_owned(),
            status,
            versions: self
                .engine_installs
                .versions
                .get(engine_id)
                .cloned()
                .unwrap_or_default(),
            request_failure: self.engine_installs.failures.get(engine_id).cloned(),
        })
    }
}

/// The product name of a managed engine.
fn managed_engine_label(engine_id: &str) -> &str {
    match engine_id {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "cursor" => "Cursor Agent",
        "grok" => "Grok Build",
        "opencode2" => "OpenCode2",
        other => other,
    }
}
