//! Durable model-favorites behavior against migrated SQLite.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::{
    ModelFavoritesRepositoryError, Repository, SetModelFavoriteInput, SqliteConfig, connect,
};
use artisan_domain::{
    MODEL_FAVORITES_MAX_MODELS, ModelFavoriteId, ReceiptDisposition, RequestId, UnixMillis,
};
use artisan_migrations::migrate_to_current;
use sea_orm::ConnectionTrait;

async fn memory_repository() -> (sea_orm::DatabaseConnection, Repository) {
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
    let repository = Repository::new(database.clone());
    (database, repository)
}

fn input(
    request_id: &str,
    model_id: &str,
    favorite: bool,
    accepted_at: i64,
) -> SetModelFavoriteInput {
    SetModelFavoriteInput {
        request_id: RequestId::parse(request_id).expect("request id should be valid"),
        model_id: ModelFavoriteId::parse(model_id).expect("model id should be valid"),
        favorite,
        accepted_at: UnixMillis::from_millis(accepted_at),
    }
}

fn ids(snapshot: &artisan_domain::ModelFavoritesSnapshot) -> Vec<&str> {
    snapshot
        .model_ids()
        .iter()
        .map(ModelFavoriteId::as_str)
        .collect()
}

struct TempDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TempDatabase {
    fn new(label: &str) -> Result<Self, Box<dyn Error>> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "artisan-editor-model-favorites-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory)?;
        let database = directory.join("forge.sqlite3");
        Ok(Self {
            directory,
            database,
        })
    }

    fn database(&self) -> &Path {
        &self.database
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test]
async fn set_unset_and_order_are_durable_across_reopen() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("reopen")?;
    let database = connect(
        SqliteConfig::file(temp.database())
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await?;
    migrate_to_current(&database).await?;
    let repository = Repository::new(database.clone());

    let first = repository
        .set_model_favorite(input("favorite-z", "model-z", true, 20))
        .await?;
    assert_eq!(first.disposition(), ReceiptDisposition::Accepted);
    assert_eq!(first.revision().get(), 1);

    let second = repository
        .set_model_favorite(input("favorite-a", "model-a", true, 20))
        .await?;
    assert_eq!(second.revision().get(), 2);
    assert_eq!(ids(second.snapshot()), vec!["model-a", "model-z"]);

    // An idempotent desired-state write does not refresh the original
    // timestamp, reorder the list, or consume a new revision.
    let no_change = repository
        .set_model_favorite(input("favorite-a-no-change", "model-a", true, 1))
        .await?;
    assert_eq!(no_change.revision().get(), 2);
    assert_eq!(ids(no_change.snapshot()), vec!["model-a", "model-z"]);

    let unset = repository
        .set_model_favorite(input("unfavorite-a", "model-a", false, 30))
        .await?;
    assert_eq!(unset.revision().get(), 3);
    assert_eq!(ids(unset.snapshot()), vec!["model-z"]);

    let readd = repository
        .set_model_favorite(input("refavorite-a", "model-a", true, 30))
        .await?;
    assert_eq!(readd.revision().get(), 4);
    assert_eq!(ids(readd.snapshot()), vec!["model-z", "model-a"]);

    drop(repository);
    database.close().await?;

    let reopened = connect(
        SqliteConfig::file(temp.database())
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await?;
    migrate_to_current(&reopened).await?;
    let reopened_snapshot = Repository::new(reopened.clone())
        .read_model_favorites()
        .await?;
    assert_eq!(reopened_snapshot.revision().get(), 4);
    assert_eq!(ids(&reopened_snapshot), vec!["model-z", "model-a"]);
    reopened.close().await?;
    Ok(())
}

#[tokio::test]
async fn duplicate_after_an_intervening_mutation_returns_its_original_snapshot()
-> Result<(), Box<dyn Error>> {
    let (_database, repository) = memory_repository().await;

    let original = repository
        .set_model_favorite(input("request-old", "model-a", true, 10))
        .await?;
    let intervening = repository
        .set_model_favorite(input("request-new", "model-a", false, 20))
        .await?;
    assert_eq!(intervening.revision().get(), 2);
    assert!(intervening.snapshot().model_ids().is_empty());

    let replay = repository
        .set_model_favorite(input("request-old", "model-a", true, 999))
        .await?;
    assert_eq!(replay.disposition(), ReceiptDisposition::Duplicate);
    assert_eq!(replay.revision(), original.revision());
    assert_eq!(ids(replay.snapshot()), vec!["model-a"]);

    let current = repository.read_model_favorites().await?;
    assert_eq!(current.revision().get(), 2);
    assert!(current.model_ids().is_empty());
    Ok(())
}

#[tokio::test]
async fn reusing_a_request_id_with_a_different_payload_is_rejected() -> Result<(), Box<dyn Error>> {
    let (_database, repository) = memory_repository().await;
    repository
        .set_model_favorite(input("request-once", "model-a", true, 10))
        .await?;

    let different_model = repository
        .set_model_favorite(input("request-once", "model-b", true, 11))
        .await;
    assert!(matches!(
        different_model,
        Err(ModelFavoritesRepositoryError::IdempotencyConflict { .. })
    ));

    let different_desired_state = repository
        .set_model_favorite(input("request-once", "model-a", false, 12))
        .await;
    assert!(matches!(
        different_desired_state,
        Err(ModelFavoritesRepositoryError::IdempotencyConflict { .. })
    ));

    let current = repository.read_model_favorites().await?;
    assert_eq!(current.revision().get(), 1);
    assert_eq!(ids(&current), vec!["model-a"]);
    Ok(())
}

#[tokio::test]
async fn failed_receipt_insert_rolls_back_state_and_cardinality_is_bounded()
-> Result<(), Box<dyn Error>> {
    let (database, repository) = memory_repository().await;
    database
        .execute_unprepared(
            "CREATE TRIGGER test_model_favorites_receipt_failure BEFORE INSERT ON model_favorite_receipts BEGIN SELECT RAISE(ABORT, 'test receipt failure'); END",
        )
        .await?;

    let failed = repository
        .set_model_favorite(input("request-rollback", "model-rollback", true, 1))
        .await;
    assert!(matches!(
        failed,
        Err(ModelFavoritesRepositoryError::Database { .. })
    ));
    database
        .execute_unprepared("DROP TRIGGER test_model_favorites_receipt_failure")
        .await?;
    let after_rollback = repository.read_model_favorites().await?;
    assert_eq!(after_rollback.revision().get(), 0);
    assert!(after_rollback.model_ids().is_empty());

    for index in 0..MODEL_FAVORITES_MAX_MODELS {
        repository
            .set_model_favorite(input(
                &format!("request-boundary-{index}"),
                &format!("model-boundary-{index}"),
                true,
                i64::try_from(index).expect("model index fits i64"),
            ))
            .await?;
    }
    let full = repository.read_model_favorites().await?;
    assert_eq!(full.revision().get(), MODEL_FAVORITES_MAX_MODELS as u64);
    assert_eq!(full.model_ids().len(), MODEL_FAVORITES_MAX_MODELS);

    let over_limit = repository
        .set_model_favorite(input(
            "request-boundary-over",
            "model-boundary-over",
            true,
            2_000,
        ))
        .await;
    match over_limit {
        Err(ModelFavoritesRepositoryError::CardinalityExceeded { count, maximum }) => {
            assert_eq!(count, MODEL_FAVORITES_MAX_MODELS + 1);
            assert_eq!(maximum, MODEL_FAVORITES_MAX_MODELS);
        }
        other => panic!("expected cardinality error, got {other:?}"),
    }
    let still_full = repository.read_model_favorites().await?;
    assert_eq!(still_full.revision(), full.revision());
    assert_eq!(still_full.model_ids().len(), MODEL_FAVORITES_MAX_MODELS);
    Ok(())
}

#[tokio::test]
async fn immediate_transactions_serialize_concurrent_revisions() -> Result<(), Box<dyn Error>> {
    let temp = TempDatabase::new("concurrent")?;
    let database = connect(
        SqliteConfig::file(temp.database())
            .min_connections(1)
            .max_connections(2)
            .sqlx_logging(false),
    )
    .await?;
    migrate_to_current(&database).await?;
    let left = Repository::new(database.clone());
    let right = Repository::new(database.clone());

    let (left_result, right_result) = tokio::join!(
        left.set_model_favorite(input("request-concurrent-a", "model-concurrent-a", true, 1)),
        right.set_model_favorite(input("request-concurrent-b", "model-concurrent-b", true, 2)),
    );
    let left_result = left_result?;
    let right_result = right_result?;
    assert_eq!(left_result.disposition(), ReceiptDisposition::Accepted);
    assert_eq!(right_result.disposition(), ReceiptDisposition::Accepted);
    let revisions = [left_result.revision().get(), right_result.revision().get()];
    assert!(revisions.contains(&1));
    assert!(revisions.contains(&2));

    let final_snapshot = Repository::new(database.clone())
        .read_model_favorites()
        .await?;
    assert_eq!(final_snapshot.revision().get(), 2);
    assert_eq!(final_snapshot.model_ids().len(), 2);
    drop(left);
    drop(right);
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn native_catalog_identity_uses_its_own_full_byte_bound() {
    let (_database, repository) = memory_repository().await;
    let model_id = "m".repeat(artisan_domain::MODEL_FAVORITE_ID_MAX_BYTES);
    let saved = repository
        .set_model_favorite(input("long-catalog-id", &model_id, true, 1))
        .await
        .expect("a valid catalog ID must fit the migration");
    assert_eq!(ids(saved.snapshot()), vec![model_id.as_str()]);
    let read = repository
        .read_model_favorites()
        .await
        .expect("stored favorites");
    assert_eq!(ids(&read), vec![model_id.as_str()]);
}
