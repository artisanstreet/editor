//! Managed extensions and the global prompt against migrated SQLite: one
//! managed skill per name, establishing the prompt once, stale saves refused,
//! versions pruned, and the per-engine record of what Artisan wrote.

use artisan_database::{
    GLOBAL_PROMPT_VERSIONS_KEPT, GlobalPromptSave, Repository, SqliteConfig, StoredExtension,
    StoredExtensionOrigin, StoredPromptFile, connect,
};
use artisan_domain::UnixMillis;
use artisan_migrations::migrate_to_current;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

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
    (database.clone(), Repository::new(database))
}

fn at(millis: i64) -> UnixMillis {
    UnixMillis::from_millis(millis)
}

fn extension(id: &str, name: &str, enabled: bool) -> StoredExtension {
    StoredExtension {
        extension_id: id.to_owned(),
        origin: StoredExtensionOrigin::Adopted,
        name: name.to_owned(),
        digest: "b".repeat(64),
        enabled,
    }
}

#[tokio::test]
async fn extensions_are_saved_updated_and_unique_by_name() {
    let (_, repository) = repository().await;
    assert!(repository.read_extensions().await.unwrap().is_empty());
    repository
        .save_extension(&extension("local.zeta", "zeta", true), at(1))
        .await
        .unwrap();
    repository
        .save_extension(&extension("local.alpha", "alpha", true), at(1))
        .await
        .unwrap();
    repository
        .save_extension(&extension("local.zeta", "zeta", false), at(2))
        .await
        .unwrap();
    let stored = repository.read_extensions().await.unwrap();
    assert_eq!(
        stored
            .iter()
            .map(|entry| (entry.name.as_str(), entry.enabled))
            .collect::<Vec<_>>(),
        vec![("alpha", true), ("zeta", false)]
    );
    assert!(
        repository
            .save_extension(&extension("artisan.zeta", "zeta", true), at(3))
            .await
            .is_err(),
        "a second managed skill cannot take an existing name"
    );
}

#[tokio::test]
async fn the_prompt_is_established_once_and_saves_need_the_current_revision() {
    let (_, repository) = repository().await;
    let initial = repository.read_global_prompt().await.unwrap();
    assert_eq!((initial.revision, initial.established), (0, false));

    let established = repository
        .establish_global_prompt("Imported prompt.", at(1))
        .await
        .unwrap();
    assert_eq!(
        (
            established.revision,
            established.established,
            established.text.as_str()
        ),
        (1, true, "Imported prompt.")
    );
    let again = repository
        .establish_global_prompt("Other text.", at(2))
        .await
        .unwrap();
    assert_eq!(
        again, established,
        "an established prompt is never replaced by setup"
    );

    let GlobalPromptSave::Saved(saved) = repository
        .save_global_prompt(1, "Edited prompt.", at(3))
        .await
        .unwrap()
    else {
        panic!("a save on the current revision saves");
    };
    assert_eq!((saved.revision, saved.text.as_str()), (2, "Edited prompt."));
    assert_eq!(
        repository
            .save_global_prompt(1, "Concurrent edit.", at(4))
            .await
            .unwrap(),
        GlobalPromptSave::Stale(saved.clone())
    );
    assert_eq!(
        repository
            .save_global_prompt(1, "Edited prompt.", at(5))
            .await
            .unwrap(),
        GlobalPromptSave::Unchanged(saved),
        "saving the canonical text again is a no-op whatever the base"
    );
}

#[tokio::test]
async fn an_empty_setup_establishes_without_a_version() {
    let (database, repository) = repository().await;
    let established = repository.establish_global_prompt("", at(1)).await.unwrap();
    assert_eq!((established.revision, established.established), (0, true));
    assert_eq!(version_count(&database).await, 0);
}

#[tokio::test]
async fn saved_versions_are_pruned_to_the_retention_bound() {
    let (database, repository) = repository().await;
    let kept = i64::try_from(GLOBAL_PROMPT_VERSIONS_KEPT).unwrap();
    for revision in 0..=GLOBAL_PROMPT_VERSIONS_KEPT + 5 {
        let outcome = repository
            .save_global_prompt(revision, &format!("version {revision}"), at(1))
            .await
            .unwrap();
        assert!(matches!(outcome, GlobalPromptSave::Saved(_)));
    }
    assert_eq!(version_count(&database).await, kept);
}

#[tokio::test]
async fn prompt_file_records_follow_the_last_write_per_home() {
    let (_, repository) = repository().await;
    let file = |home: &str, digest: char| StoredPromptFile {
        engine_id: "opencode2".to_owned(),
        home_key: home.to_owned(),
        written_digest: digest.to_string().repeat(64),
    };
    repository
        .record_prompt_file(&file("home", 'a'), at(1))
        .await
        .unwrap();
    repository
        .record_prompt_file(&file("homes/work", 'b'), at(1))
        .await
        .unwrap();
    repository
        .record_prompt_file(&file("home", 'c'), at(2))
        .await
        .unwrap();
    assert_eq!(
        repository.read_prompt_files().await.unwrap(),
        vec![file("home", 'c'), file("homes/work", 'b')]
    );
}

async fn version_count(database: &DatabaseConnection) -> i64 {
    database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT count(*) FROM global_prompt_versions",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index::<i64>(0)
        .unwrap()
}
