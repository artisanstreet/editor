//! The Forge user's preferences and navigation record against migrated
//! SQLite: most-recently-used project order, remembered threads, the last
//! route, the default engine configuration, and the one-time legacy import.

use artisan_database::{Repository, RepositoryError, SqliteConfig, connect};
use artisan_domain::{
    ApprovalMode, ByteLimit, CountLimit, EngineAgentId, EngineModelId, EnginePermissionPolicy,
    EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, FilesystemAccess, FiniteMillis,
    LegacyImportOutcome, NavigationRoute, NetworkAccess, OpenCode2Selection, PermissionId,
    ProjectId, ThreadId, UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ConnectionTrait, DatabaseConnection};

async fn repository() -> (DatabaseConnection, Repository) {
    let database = connect(
        SqliteConfig::in_memory()
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("memory database should open");
    migrate_to_current(&database)
        .await
        .expect("memory database should migrate");
    for statement in [
        "INSERT INTO attached_projects (project_id, root_path, display_name, attached_at_ms) VALUES ('p1', 'C:/work/p1', 'One', 1)",
        "INSERT INTO attached_projects (project_id, root_path, display_name, attached_at_ms) VALUES ('p2', 'C:/work/p2', 'Two', 1)",
        "INSERT INTO attached_projects (project_id, root_path, display_name, attached_at_ms) VALUES ('p3', 'C:/work/p3', 'Three', 1)",
        "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms) VALUES ('t1', 'p1', 'Thread', 2, 2)",
        "INSERT INTO threads (thread_id, project_id, title, created_at_ms, updated_at_ms) VALUES ('t2', 'p2', 'Thread', 2, 2)",
    ] {
        database
            .execute_unprepared(statement)
            .await
            .expect("seed row");
    }
    (database.clone(), Repository::new(database))
}

fn project(value: &str) -> ProjectId {
    ProjectId::parse(value).unwrap()
}

fn thread(value: &str) -> ThreadId {
    ThreadId::parse(value).unwrap()
}

fn at(millis: i64) -> UnixMillis {
    UnixMillis::from_millis(millis)
}

fn config(model: &str) -> EngineRunConfig {
    let one = FiniteMillis::new(1).unwrap();
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(100).unwrap(),
        readiness_budget: one,
        health_budget: one,
        prompt_budget: one,
        stream_budget: one,
        close_budget: one,
        max_json_body_bytes: ByteLimit::new(8_192).unwrap(),
        max_sse_line_bytes: ByteLimit::new(4_096).unwrap(),
        max_sse_event_bytes: ByteLimit::new(8_192).unwrap(),
        max_readiness_line_bytes: ByteLimit::new(4_096).unwrap(),
        max_header_count: CountLimit::new(8).unwrap(),
        max_http_buffer_bytes: ByteLimit::new(8_192).unwrap(),
        max_stderr_bytes: ByteLimit::new(4_096).unwrap(),
        observation_capacity: CountLimit::new(16).unwrap(),
    })
    .unwrap();
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile").unwrap(),
            EngineModelId::parse(model).unwrap(),
            EngineRouteId::parse("route").unwrap(),
            None,
            EnginePermissionPolicy::new(
                PermissionId::parse("permission").unwrap(),
                EngineAgentId::parse("agent").unwrap(),
                ApprovalMode::OnRequest,
                FilesystemAccess::Workspace,
                NetworkAccess::Enabled,
                WebSearchAccess::Disabled,
            ),
        )),
        runtime,
    )
}

fn order(preferences: &artisan_database::StoredUserPreferences) -> Vec<&str> {
    preferences
        .navigation
        .projects()
        .iter()
        .map(|entry| entry.project_id.as_str())
        .collect()
}

#[tokio::test]
async fn navigation_orders_projects_by_use_and_remembers_threads_and_route() {
    let (_, repository) = repository().await;
    let fresh = repository.read_user_preferences().await.unwrap();
    assert_eq!(fresh.revision, 0);
    assert!(fresh.navigation.projects().is_empty());
    assert_eq!(fresh.navigation.route(), None);
    assert_eq!(fresh.default_engine_config, None);

    repository
        .record_navigation(&project("p1"), Some(&thread("t1")), at(10))
        .await
        .unwrap();
    let second = repository
        .record_navigation(&project("p2"), None, at(11))
        .await
        .unwrap();
    assert_eq!(order(&second), ["p2", "p1"]);
    assert_eq!(
        second.navigation.last_thread(&project("p1")),
        Some(&thread("t1"))
    );
    assert_eq!(
        second.navigation.route(),
        Some(&NavigationRoute {
            project_id: project("p2"),
            thread_id: None,
        })
    );

    // Returning to a project without naming a thread resumes its thread.
    let back = repository
        .record_navigation(&project("p1"), None, at(12))
        .await
        .unwrap();
    assert_eq!(order(&back), ["p1", "p2"]);
    assert_eq!(
        back.navigation.route(),
        Some(&NavigationRoute {
            project_id: project("p1"),
            thread_id: Some(thread("t1")),
        })
    );

    // A repeated report changes nothing, not even the revision.
    let repeated = repository
        .record_navigation(&project("p1"), Some(&thread("t1")), at(13))
        .await
        .unwrap();
    assert_eq!(repeated, back);
    assert_eq!(repository.read_user_preferences().await.unwrap(), back);
}

#[tokio::test]
async fn navigation_refuses_unknown_projects_and_threads_of_other_projects() {
    let (_, repository) = repository().await;
    assert!(matches!(
        repository
            .record_navigation(&project("missing"), None, at(10))
            .await,
        Err(RepositoryError::ProjectNotFound { .. })
    ));
    assert!(matches!(
        repository
            .record_navigation(&project("p1"), Some(&thread("t2")), at(10))
            .await,
        Err(RepositoryError::ThreadNotFound { .. })
    ));
    assert_eq!(
        repository.read_user_preferences().await.unwrap().revision,
        0
    );
}

#[tokio::test]
async fn a_deleted_thread_is_forgotten() {
    let (database, repository) = repository().await;
    repository
        .record_navigation(&project("p2"), Some(&thread("t2")), at(10))
        .await
        .unwrap();
    database
        .execute_unprepared("DELETE FROM threads WHERE thread_id = 't2'")
        .await
        .expect("thread deletes");
    let preferences = repository.read_user_preferences().await.unwrap();
    assert_eq!(preferences.navigation.last_thread(&project("p2")), None);
    assert_eq!(
        preferences.navigation.route(),
        Some(&NavigationRoute {
            project_id: project("p2"),
            thread_id: None,
        })
    );
}

#[tokio::test]
async fn the_default_engine_configuration_follows_the_latest_choice() {
    let (_, repository) = repository().await;
    let first = repository
        .remember_default_engine_config(&config("model-a"))
        .await
        .unwrap();
    assert_eq!(first.default_engine_config, Some(config("model-a")));
    let same = repository
        .remember_default_engine_config(&config("model-a"))
        .await
        .unwrap();
    assert_eq!(same.revision, first.revision);
    let second = repository
        .remember_default_engine_config(&config("model-b"))
        .await
        .unwrap();
    assert_eq!(second.default_engine_config, Some(config("model-b")));
    assert!(second.revision > first.revision);
}

#[tokio::test]
async fn legacy_preferences_fill_only_what_the_forge_lacks() {
    let (_, repository) = repository().await;
    let imported = repository
        .import_legacy_preferences(
            Some(Some(&config("legacy"))),
            &[project("p3"), project("gone"), project("p1")],
            at(10),
        )
        .await
        .unwrap();
    assert_eq!(imported.default_model, LegacyImportOutcome::Imported);
    assert_eq!(imported.project_order, LegacyImportOutcome::Imported);
    assert_eq!(order(&imported.preferences), ["p3", "p1"]);
    assert_eq!(
        imported.preferences.default_engine_config,
        Some(config("legacy"))
    );

    // A second import (another Editor's files) keeps the Forge's own.
    let again = repository
        .import_legacy_preferences(Some(Some(&config("other"))), &[project("p2")], at(11))
        .await
        .unwrap();
    assert_eq!(again.default_model, LegacyImportOutcome::Kept);
    assert_eq!(again.project_order, LegacyImportOutcome::Kept);
    assert_eq!(again.preferences, imported.preferences);
}

#[tokio::test]
async fn legacy_preferences_the_forge_cannot_use_are_refused() {
    let (_, repository) = repository().await;
    let refused = repository
        .import_legacy_preferences(Some(None), &[project("gone")], at(10))
        .await
        .unwrap();
    assert_eq!(refused.default_model, LegacyImportOutcome::Refused);
    assert_eq!(refused.project_order, LegacyImportOutcome::Refused);
    let absent = repository
        .import_legacy_preferences(None, &[], at(11))
        .await
        .unwrap();
    assert_eq!(absent.default_model, LegacyImportOutcome::Absent);
    assert_eq!(absent.project_order, LegacyImportOutcome::Absent);
    assert_eq!(absent.preferences.revision, 0);
}
