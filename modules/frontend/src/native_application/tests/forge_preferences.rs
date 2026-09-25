//! The Forge user's preferences as the Editor uses them: a new connection
//! resumes the Forge's project order and route, navigation is reported to
//! the Forge, threads without their own configuration show the Forge's
//! default model, the account comes from the Forge, and preferences an
//! older Editor kept in files are handed over once and then removed.

use super::forge_codex_config;
use super::*;
use crate::editor_settings::LegacyForgePreferences;
use crate::native_transport_service::{PreferencesCommand, PreferencesEvent};
use artisan_domain::{
    AccountProfile, CatalogSelection, LegacyImportOutcome, LegacyPreferencesImported,
    ModelFavoriteId, NavigationProject, NavigationRecord, NavigationRoute, UserPreferences,
};

fn preferences(
    projects: &[(&str, Option<&str>)],
    route: Option<(&str, Option<&str>)>,
    default_engine_config: Option<artisan_domain::EngineRunConfig>,
) -> UserPreferences {
    let project = |id: &str| ProjectId::parse(id).expect("project");
    let thread = |id: &str| ThreadId::parse(id).expect("thread");
    UserPreferences {
        revision: 7,
        default_engine_config,
        navigation: NavigationRecord::new(
            projects
                .iter()
                .map(|(project_id, last_thread)| NavigationProject {
                    project_id: project(project_id),
                    last_thread_id: last_thread.map(thread),
                })
                .collect(),
            route.map(|(project_id, thread_id)| NavigationRoute {
                project_id: project(project_id),
                thread_id: thread_id.map(thread),
            }),
        )
        .expect("record"),
        account: AccountProfile {
            display_name: DisplayName::parse("theo").expect("name"),
            host_name: DisplayName::parse("ubuntu").expect("host"),
        },
    }
}

fn loaded(preferences: UserPreferences) -> NativeTransportEvent {
    NativeTransportEvent::Preferences(PreferencesEvent::Loaded(Ok(Box::new(preferences))))
}

fn project_ids(application: &NativeApplication) -> Vec<&str> {
    application
        .project_options
        .iter()
        .map(|option| option.id.as_str())
        .collect()
}

fn recorded_navigation(commands: &[NativeTransportCommand]) -> Vec<(String, Option<String>)> {
    commands
        .iter()
        .filter_map(|command| match command {
            NativeTransportCommand::Preferences(PreferencesCommand::RecordNavigation(record)) => {
                Some((
                    record.project_id.as_str().to_owned(),
                    record
                        .thread_id
                        .as_ref()
                        .map(|thread| thread.as_str().to_owned()),
                ))
            }
            _ => None,
        })
        .collect()
}

#[gpui::test]
fn a_new_connection_resumes_the_forge_order_route_and_threads(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            // The transport reads the preferences before the projects.
            application.handle_service_event(
                loaded(preferences(
                    &[("beta", Some("beta-2")), ("alpha", Some("alpha-1"))],
                    Some(("beta", Some("beta-2"))),
                    None,
                )),
                cx,
            );
            application.handle_service_event(
                NativeTransportEvent::Projects(
                    ProjectListing::new(vec![
                        project("alpha", "Alpha"),
                        project("beta", "Beta"),
                        project("gamma", "Gamma"),
                    ])
                    .expect("projects"),
                ),
                cx,
            );
            assert_eq!(project_ids(application), ["beta", "alpha", "gamma"]);
            assert_eq!(
                application.selected_project.as_ref().map(ProjectId::as_str),
                Some("beta")
            );
            // The transport read the catalog's first project; the resumed
            // one needs its own read.
            assert!(commands.borrow().iter().any(|command| matches!(
                command,
                NativeTransportCommand::SelectProject(project) if project.as_str() == "beta"
            )));
            application.handle_service_event(
                NativeTransportEvent::Threads {
                    project_id: ProjectId::parse("beta").unwrap(),
                    listing: ThreadListing::new(vec![
                        thread("beta-1", "beta", "Newer"),
                        thread("beta-2", "beta", "Where I was"),
                    ])
                    .expect("threads"),
                },
                cx,
            );
            assert_eq!(
                application
                    .selected_thread
                    .as_ref()
                    .or(application.pending_thread.as_ref())
                    .map(ThreadId::as_str),
                Some("beta-2")
            );
            // Opening it is reported back, once.
            assert_eq!(
                recorded_navigation(&commands.borrow()),
                [("beta".to_owned(), Some("beta-2".to_owned()))]
            );
        });
    });
}

#[gpui::test]
fn navigation_is_reported_to_the_forge_once_per_change(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.project_options = project_options_from_listing(
                &ProjectListing::new(vec![project("alpha", "Alpha"), project("beta", "Beta")])
                    .expect("projects"),
            );
            application.selected_project = Some(ProjectId::parse("alpha").unwrap());
            let beta = ProjectId::parse("beta").unwrap();
            application.select_project_from_sidebar(beta.clone(), cx);
            application.report_navigation(beta.clone(), None);
            application.report_navigation(beta, Some(ThreadId::parse("beta-1").unwrap()));
            assert_eq!(
                recorded_navigation(&commands.borrow()),
                [
                    ("beta".to_owned(), None),
                    ("beta".to_owned(), Some("beta-1".to_owned()))
                ]
            );
            // Every report holds the connection until the Forge answers.
            assert!(commands.borrow().iter().any(|command| {
                command.hold_kind() == Some(crate::native_transport_service::HoldKind::Preferences)
            }));
        });
    });
}

#[gpui::test]
fn threads_without_their_own_configuration_show_the_forge_default(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, _) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(
                application,
                cx,
                ThreadId::parse("new-task").unwrap(),
                "",
                sink,
            );
            assert!(application.engine_settings.authoritative_config().is_none());
            let default = forge_codex_config(Some(1_050_000));
            let expected = crate::picker_selection::saved_config_policy(
                &application.served_catalog(cx),
                &default,
            )
            .expect("the fixture displays the default");
            let displayed = |application: &NativeApplication,
                             cx: &mut Context<NativeApplication>| {
                application
                    .model_selector
                    .read(cx)
                    .state()
                    .policy()
                    .cloned()
            };
            assert_ne!(displayed(application, cx).as_ref(), Some(&expected));
            // The Forge pushes the default a saved choice made.
            application.handle_service_event(
                NativeTransportEvent::HostState(
                    crate::native_transport_service::HostStateEvent::Preferences(preferences(
                        &[],
                        None,
                        Some(default),
                    )),
                ),
                cx,
            );
            assert_eq!(displayed(application, cx), Some(expected));
        });
    });
}

#[gpui::test]
fn the_account_comes_from_the_forge(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            assert_eq!(application.profile_name, None);
            application.handle_service_event(loaded(preferences(&[], None, None)), cx);
            assert_eq!(application.profile_name.as_deref(), Some("theo"));
            assert_eq!(application.profile_hostname.as_deref(), Some("ubuntu"));
            assert_eq!(application.profile_display_name(cx), "theo");
            cx.remove_global::<crate::native_account_identity::ArtisanAccountIdentity>();
        });
    });
}

fn legacy_files(label: &str) -> Vec<std::path::PathBuf> {
    let directory = std::env::temp_dir().join(format!(
        "artisan-legacy-forge-{label}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("legacy directory");
    let files = vec![directory.join("last-used-model"), directory.join("local")];
    for file in &files {
        std::fs::write(file, b"{}").expect("legacy file");
    }
    files
}

fn legacy_selection() -> CatalogSelection {
    CatalogSelection {
        model_id: ModelFavoriteId::parse("codex-sol").expect("model"),
        profile_id: None,
        reasoning_effort: None,
        speed: None,
        context_window: None,
        permission: None,
    }
}

#[gpui::test]
fn legacy_file_preferences_are_imported_once_then_removed(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    let files = legacy_files("import");
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.test_legacy_preferences = Some(LegacyForgePreferences::for_test(
                Some(legacy_selection()),
                vec![ProjectId::parse("beta").unwrap()],
                files.clone(),
            ));
            application.handle_service_event(loaded(preferences(&[], None, None)), cx);
            let import = commands
                .borrow()
                .iter()
                .find_map(|command| match command {
                    NativeTransportCommand::Preferences(PreferencesCommand::ImportLegacy(
                        import,
                    )) => Some(import.clone()),
                    _ => None,
                })
                .expect("the legacy preferences are handed to the Forge");
            assert_eq!(import.default_selection, Some(legacy_selection()));
            assert_eq!(import.project_order, [ProjectId::parse("beta").unwrap()]);
            assert!(
                files.iter().all(|file| file.exists()),
                "kept until answered"
            );

            // A later answer to a recorded navigation imports nothing again.
            application.handle_service_event(loaded(preferences(&[], None, None)), cx);
            application.handle_service_event(
                NativeTransportEvent::Preferences(PreferencesEvent::LegacyImported(Ok(Box::new(
                    LegacyPreferencesImported {
                        default_model: LegacyImportOutcome::Imported,
                        project_order: LegacyImportOutcome::Imported,
                        preferences: preferences(&[("beta", None)], None, None),
                    },
                )))),
                cx,
            );
            assert!(
                files.iter().all(|file| !file.exists()),
                "removed once answered"
            );
            let imports = commands
                .borrow()
                .iter()
                .filter(|command| {
                    matches!(
                        command,
                        NativeTransportCommand::Preferences(PreferencesCommand::ImportLegacy(_))
                    )
                })
                .count();
            assert_eq!(imports, 1);
            cx.remove_global::<crate::native_account_identity::ArtisanAccountIdentity>();
        });
    });
}

#[gpui::test]
fn legacy_files_the_forge_does_not_need_are_removed_without_an_import(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    let files = legacy_files("unneeded");
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.test_legacy_preferences = Some(LegacyForgePreferences::for_test(
                Some(legacy_selection()),
                vec![ProjectId::parse("beta").unwrap()],
                files.clone(),
            ));
            application.handle_service_event(
                loaded(preferences(
                    &[("alpha", None)],
                    None,
                    Some(forge_codex_config(None)),
                )),
                cx,
            );
            assert!(commands.borrow().iter().all(|command| !matches!(
                command,
                NativeTransportCommand::Preferences(PreferencesCommand::ImportLegacy(_))
            )));
            assert!(files.iter().all(|file| !file.exists()));
            cx.remove_global::<crate::native_account_identity::ArtisanAccountIdentity>();
        });
    });
}
