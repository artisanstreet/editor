//! Forge-managed engine installs in the Editor: pushed statuses reach the
//! Settings projection, and the version controls send Forge requests.

use super::*;
use crate::native_settings::{SettingsEngineVersions, SettingsScreenEvent};
use crate::native_transport_service::{EngineInstallsCommand, EngineInstallsEvent, HostStateEvent};
use artisan_domain::{
    EngineInstallPhase, EngineInstallSnapshot, EngineInstallStatus, EngineVersionChange,
    EngineVersionEntry, EngineVersionList, EngineVersionSelection,
};

fn pushed(phase: EngineInstallPhase, active: Option<&str>) -> NativeTransportEvent {
    NativeTransportEvent::HostState(HostStateEvent::EngineInstalls(
        EngineInstallSnapshot::new(vec![EngineInstallStatus {
            engine_id: "claude".to_owned(),
            phase,
            active_version: active.map(ToOwned::to_owned),
            held_version: None,
            latest_version: Some("2.1.283".to_owned()),
            pending_version: None,
            rollback_version: Some("2.1.281".to_owned()),
            progress_percent: None,
            reason: None,
            overridden: false,
        }])
        .expect("snapshot"),
    ))
}

#[gpui::test]
fn pushed_statuses_and_version_controls_round_trip_through_the_forge(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            assert!(application.settings_engine_install("claude").is_none());
            application.handle_service_event(pushed(EngineInstallPhase::Installing, None), cx);
            let install = application
                .settings_engine_install("claude")
                .expect("install");
            assert_eq!(install.label, "Claude Code");
            assert!(
                install
                    .status_copy()
                    .starts_with("Installing Claude Code 2.1.283")
            );
            application
                .handle_service_event(pushed(EngineInstallPhase::Ready, Some("2.1.283")), cx);
            assert!(
                application
                    .settings_engine_install("claude")
                    .expect("install")
                    .status_copy()
                    .contains("2.1.283 is installed")
            );

            application.handle_engine_install_event(
                &SettingsScreenEvent::LoadEngineVersions {
                    engine_id: "claude".to_owned(),
                },
                cx,
            );
            assert_eq!(
                application
                    .settings_engine_install("claude")
                    .expect("install")
                    .versions,
                SettingsEngineVersions::Loading
            );
            application.handle_service_event(
                NativeTransportEvent::EngineInstalls(EngineInstallsEvent::Versions {
                    engine_id: "claude".to_owned(),
                    result: Ok(EngineVersionList::new(
                        "claude".to_owned(),
                        vec![EngineVersionEntry {
                            version: "2.1.282".to_owned(),
                            installed: true,
                            active: false,
                            below_floor: false,
                        }],
                    )
                    .expect("list")),
                }),
                cx,
            );
            assert!(matches!(
                application.settings_engine_install("claude").expect("install").versions,
                SettingsEngineVersions::Loaded(ref versions) if versions.len() == 1
            ));

            application.handle_engine_install_event(
                &SettingsScreenEvent::SelectEngineVersion {
                    engine_id: "claude".to_owned(),
                    version: Some("2.1.282".to_owned()),
                },
                cx,
            );
            application.handle_engine_install_event(
                &SettingsScreenEvent::RollbackEngine {
                    engine_id: "claude".to_owned(),
                },
                cx,
            );
            let sent: Vec<_> = commands
                .borrow()
                .iter()
                .filter_map(|command| match command {
                    NativeTransportCommand::EngineInstalls(command) => Some(command.clone()),
                    _ => None,
                })
                .collect();
            assert!(sent.contains(&EngineInstallsCommand::ListVersions("claude".to_owned())));
            assert!(sent.iter().any(|command| matches!(
                command,
                EngineInstallsCommand::Change(change)
                    if change.change
                        == EngineVersionChange::Select(EngineVersionSelection::Version(
                            "2.1.282".to_owned()
                        ))
            )));
            assert!(sent.iter().any(|command| matches!(
                command,
                EngineInstallsCommand::Change(change)
                    if change.change == EngineVersionChange::Rollback
            )));
        });
    });
}
