//! Durable thread-engine configuration behavior against migrated SQLite.

use artisan_database::entities::{self, CommandKind};
use artisan_database::{
    CreateThreadInput, Repository, RepositoryError, SetThreadEngineConfigInput, SqliteConfig,
    connect,
};
use artisan_domain::{
    ApprovalMode, ByteLimit, CountLimit, EngineAgentId, EngineConfigRevision,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineSelection, EngineVariantId,
    FilesystemAccess, FiniteMillis, NetworkAccess, OpenCode2Selection, PermissionId, ProjectId,
    ReceiptDisposition, RequestId, ThreadId, ThreadTitle, UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ConnectionTrait, DbBackend, EntityTrait, Statement, Value};

const _: fn() = || {
    struct Marker;
    trait Ambiguous<A> {
        fn marker() {}
    }
    impl<T: ?Sized> Ambiguous<()> for T {}
    impl<T: ?Sized + Default> Ambiguous<Marker> for T {}
    let _ = <EngineRunConfig as Ambiguous<_>>::marker;
    let _ = <EngineRuntimeControls as Ambiguous<_>>::marker;
    let _ = <EnginePermissionPolicy as Ambiguous<_>>::marker;
    let _ = <EngineConfigRevision as Ambiguous<_>>::marker;
};

fn config(label: &str, with_variant: bool) -> EngineRunConfig {
    let one = FiniteMillis::new(1).expect("one millisecond is valid");
    let runtime = EngineRuntimeControls::new(
        FiniteMillis::new(100).expect("attempt budget is valid"),
        one,
        one,
        one,
        one,
        one,
        ByteLimit::new(8_192).expect("json body limit is valid"),
        ByteLimit::new(4_096).expect("sse line limit is valid"),
        ByteLimit::new(8_192).expect("sse event limit is valid"),
        ByteLimit::new(4_096).expect("readiness line limit is valid"),
        CountLimit::new(8).expect("header count is valid"),
        ByteLimit::new(8_192).expect("http buffer limit is valid"),
        ByteLimit::new(4_096).expect("stderr limit is valid"),
        CountLimit::new(16).expect("observation capacity is valid"),
    )
    .expect("runtime relationships are valid");
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse(format!("permission-{label}")).expect("permission id is valid"),
        EngineAgentId::parse(format!("agent-{label}")).expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse(format!("profile-{label}")).expect("profile id is valid"),
            EngineModelId::parse(format!("model-{label}")).expect("model id is valid"),
            EngineRouteId::parse(format!("route-{label}")).expect("route id is valid"),
            with_variant.then(|| {
                EngineVariantId::parse(format!("variant-{label}")).expect("variant id is valid")
            }),
            permission,
        )),
        runtime,
    )
}

async fn seeded_repository() -> (sea_orm::DatabaseConnection, Repository, ThreadId) {
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
    database
        .execute_unprepared(
            "INSERT INTO attached_projects (project_id, root_path, display_name, attached_at_ms) VALUES ('project-config', 'C:/repos/config', 'Config', 1)",
        )
        .await
        .expect("fixture project should insert");
    let thread_id = ThreadId::parse("thread-engine-config").expect("thread id is valid");
    Repository::new(database.clone())
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse("request-create-config").expect("request id is valid"),
            thread_id: thread_id.clone(),
            project_id: ProjectId::parse("project-config").expect("project id is valid"),
            title: ThreadTitle::parse("Config thread").expect("title is valid"),
            created_at: UnixMillis::from_millis(10),
            updated_at: UnixMillis::from_millis(10),
        })
        .await
        .expect("fixture thread should create");
    (database.clone(), Repository::new(database), thread_id)
}

fn input(
    request_id: &str,
    thread_id: &ThreadId,
    precondition: EngineConfigUpdatePrecondition,
    config: EngineRunConfig,
    accepted_at: i64,
) -> SetThreadEngineConfigInput {
    SetThreadEngineConfigInput {
        request_id: RequestId::parse(request_id).expect("request id is valid"),
        thread_id: thread_id.clone(),
        precondition,
        config,
        accepted_at: UnixMillis::from_millis(accepted_at),
    }
}

async fn receipt_count(database: &sea_orm::DatabaseConnection) -> usize {
    entities::command_receipt::Entity::find()
        .all(database)
        .await
        .expect("receipt query should work")
        .into_iter()
        .filter(|row| row.command_kind == CommandKind::SetThreadEngineConfig)
        .count()
}

async fn replace_thread_blob(database: &sea_orm::DatabaseConnection, blob: Vec<u8>) {
    database
        .execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE threads SET engine_run_config = ? WHERE thread_id = ?",
            [
                Value::Bytes(Some(Box::new(blob))),
                Value::String(Some("thread-engine-config".to_owned())),
            ],
        ))
        .await
        .expect("fixture blob replacement should work");
}

#[tokio::test]
async fn first_write_update_replay_and_revision_conflict_are_durable() {
    let (database, repository, thread_id) = seeded_repository().await;
    let first_config = config("first", false);
    let accepted = repository
        .set_thread_engine_config(input(
            "request-engine-first",
            &thread_id,
            EngineConfigUpdatePrecondition::Unconfigured,
            first_config.clone(),
            100,
        ))
        .await
        .expect("first configuration write should succeed");
    assert_eq!(accepted.revision().get(), 1);
    assert_eq!(accepted.receipt().disposition, ReceiptDisposition::Accepted);
    let first_receipt = entities::command_receipt::Entity::find_by_id("request-engine-first")
        .one(&database)
        .await
        .expect("engine receipt query should work")
        .expect("engine receipt should exist");
    assert_eq!(
        first_receipt.command_kind,
        CommandKind::SetThreadEngineConfig
    );
    assert_eq!(first_receipt.engine_run_config_version, Some(1));
    assert!(
        first_receipt
            .engine_run_config
            .as_ref()
            .is_some_and(|blob| { !blob.as_slice().is_empty() && blob.as_slice().len() <= 65_536 })
    );
    assert!(first_receipt.engine_run_config_expected_revision.is_none());
    assert_eq!(first_receipt.engine_run_config_result_revision, Some(1));

    let replay = repository
        .set_thread_engine_config(input(
            "request-engine-first",
            &thread_id,
            EngineConfigUpdatePrecondition::Unconfigured,
            first_config.clone(),
            999,
        ))
        .await
        .expect("exact configuration replay should succeed");
    assert_eq!(replay.revision(), accepted.revision());
    assert_eq!(replay.receipt().disposition, ReceiptDisposition::Duplicate);
    let thread = entities::thread::Entity::find_by_id(thread_id.as_str())
        .one(&database)
        .await
        .expect("thread query should work")
        .expect("thread should exist");
    assert_eq!(thread.updated_at_ms, 100);

    let second_config = config("second", true);
    let updated = repository
        .set_thread_engine_config(input(
            "request-engine-second",
            &thread_id,
            EngineConfigUpdatePrecondition::Exact(accepted.revision()),
            second_config.clone(),
            200,
        ))
        .await
        .expect("revision-fenced update should succeed");
    assert_eq!(updated.revision().get(), 2);
    assert_eq!(updated.receipt().disposition, ReceiptDisposition::Accepted);
    let update_receipt = entities::command_receipt::Entity::find_by_id("request-engine-second")
        .one(&database)
        .await
        .expect("updated engine receipt query should work")
        .expect("updated engine receipt should exist");
    assert_eq!(update_receipt.engine_run_config_expected_revision, Some(1));
    assert_eq!(update_receipt.engine_run_config_result_revision, Some(2));
    let settings = repository
        .read_thread_engine_settings(&thread_id)
        .await
        .expect("settings read should work")
        .expect("thread should be configured");
    assert_eq!(settings.revision(), updated.revision());
    assert_eq!(settings.config(), &second_config);

    let stale = repository
        .set_thread_engine_config(input(
            "request-engine-stale",
            &thread_id,
            EngineConfigUpdatePrecondition::Exact(accepted.revision()),
            config("stale", false),
            300,
        ))
        .await
        .expect_err("stale revision must be rejected");
    assert!(matches!(
        stale,
        RepositoryError::EngineConfigRevisionConflict { .. }
    ));
    assert_eq!(receipt_count(&database).await, 2);
    assert_eq!(
        repository
            .read_thread_engine_settings(&thread_id)
            .await
            .expect("settings read should work")
            .expect("settings should remain configured")
            .config(),
        &second_config
    );

    let idempotency = repository
        .set_thread_engine_config(input(
            "request-engine-first",
            &thread_id,
            EngineConfigUpdatePrecondition::Exact(updated.revision()),
            second_config,
            400,
        ))
        .await
        .expect_err("same request id with a different command must conflict");
    assert!(matches!(
        idempotency,
        RepositoryError::IdempotencyConflict { .. }
    ));
}

#[tokio::test]
async fn configuration_update_keeps_newer_current_timestamp_after_stale_read() {
    let (database, repository, thread_id) = seeded_repository().await;
    let accepted = repository
        .set_thread_engine_config(input(
            "request-engine-timestamp-base",
            &thread_id,
            EngineConfigUpdatePrecondition::Unconfigured,
            config("timestamp", false),
            100,
        ))
        .await
        .expect("configuration write should succeed");
    let stale_row = entities::thread::Entity::find_by_id(thread_id.as_str())
        .one(&database)
        .await
        .expect("thread query should work")
        .expect("thread should exist");
    let previous_blob = stale_row
        .engine_run_config
        .expect("configured blob should exist")
        .into_vec();
    assert_eq!(stale_row.updated_at_ms, 100);

    database
        .execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE threads SET updated_at_ms = ? WHERE thread_id = ?",
            [
                Value::BigInt(Some(900)),
                Value::String(Some(thread_id.as_str().to_owned())),
            ],
        ))
        .await
        .expect("concurrent timestamp advance should persist");

    // Execute the same conditional update shape with the captured row values,
    // modelling a writer that read the row before the timestamp advanced.
    let update = Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE threads SET engine_run_config_version = 1, engine_run_config_revision = ?, engine_run_config = ?, updated_at_ms = MAX(updated_at_ms, ?) WHERE thread_id = ? AND engine_run_config_version IS ? AND engine_run_config_revision = ? AND engine_run_config IS ?",
        [
            Value::BigInt(Some(accepted.revision().as_i64() + 1)),
            Value::Bytes(Some(Box::new(previous_blob.clone()))),
            Value::BigInt(Some(100)),
            Value::String(Some(thread_id.as_str().to_owned())),
            Value::BigInt(Some(1)),
            Value::BigInt(Some(accepted.revision().as_i64())),
            Value::Bytes(Some(Box::new(previous_blob))),
        ],
    );
    assert_eq!(
        database
            .execute(update)
            .await
            .expect("conditional update should work")
            .rows_affected(),
        1
    );

    let current = entities::thread::Entity::find_by_id(thread_id.as_str())
        .one(&database)
        .await
        .expect("thread query should work")
        .expect("thread should exist");
    assert_eq!(current.updated_at_ms, 900);
    assert_eq!(current.engine_run_config_revision, 2);
}

#[tokio::test]
async fn strict_blob_shape_is_corruption_and_failed_update_leaves_no_receipt() {
    let (database, repository, thread_id) = seeded_repository().await;
    let accepted = repository
        .set_thread_engine_config(input(
            "request-engine-canonical",
            &thread_id,
            EngineConfigUpdatePrecondition::Unconfigured,
            config("canonical", false),
            100,
        ))
        .await
        .expect("configuration write should succeed");
    let original_blob = entities::thread::Entity::find_by_id(thread_id.as_str())
        .one(&database)
        .await
        .expect("thread query should work")
        .expect("thread should exist")
        .engine_run_config
        .expect("configured blob should exist")
        .into_vec();

    let mut trailing = original_blob.clone();
    trailing.push(b' ');
    replace_thread_blob(&database, trailing).await;
    let trailing_error = repository
        .read_thread_engine_settings(&thread_id)
        .await
        .expect_err("trailing whitespace must be rejected");
    assert!(matches!(
        trailing_error,
        RepositoryError::CorruptData { .. }
    ));

    for malformed in [
        br#"{"version":1,"version":1,"engine":"opencode2","profile_id":"profile-canonical","model_id":"model-canonical","route_id":"route-canonical","variant_id":null,"permission":{"permission_id":"permission-canonical","agent_id":"agent-canonical","approval":"on_request","filesystem":"workspace","network":"enabled","web_search":"disabled"},"runtime":{"attempt_budget_ms":100,"readiness_budget_ms":1,"health_budget_ms":1,"prompt_budget_ms":1,"stream_budget_ms":1,"close_budget_ms":1,"max_json_body_bytes":8192,"max_sse_line_bytes":4096,"max_sse_event_bytes":8192,"max_readiness_line_bytes":4096,"max_header_count":8,"max_http_buffer_bytes":8192,"max_stderr_bytes":4096,"observation_capacity":16}}"#.to_vec(),
        br#"{"version":1,"unknown":true,"engine":"opencode2","profile_id":"profile-canonical","model_id":"model-canonical","route_id":"route-canonical","variant_id":null,"permission":{"permission_id":"permission-canonical","agent_id":"agent-canonical","approval":"on_request","filesystem":"workspace","network":"enabled","web_search":"disabled"},"runtime":{"attempt_budget_ms":100,"readiness_budget_ms":1,"health_budget_ms":1,"prompt_budget_ms":1,"stream_budget_ms":1,"close_budget_ms":1,"max_json_body_bytes":8192,"max_sse_line_bytes":4096,"max_sse_event_bytes":8192,"max_readiness_line_bytes":4096,"max_header_count":8,"max_http_buffer_bytes":8192,"max_stderr_bytes":4096,"observation_capacity":16}}"#.to_vec(),
        br#"{"engine":"opencode2","version":1,"profile_id":"profile-canonical","model_id":"model-canonical","route_id":"route-canonical","variant_id":null,"permission":{"permission_id":"permission-canonical","agent_id":"agent-canonical","approval":"on_request","filesystem":"workspace","network":"enabled","web_search":"disabled"},"runtime":{"attempt_budget_ms":100,"readiness_budget_ms":1,"health_budget_ms":1,"prompt_budget_ms":1,"stream_budget_ms":1,"close_budget_ms":1,"max_json_body_bytes":8192,"max_sse_line_bytes":4096,"max_sse_event_bytes":8192,"max_readiness_line_bytes":4096,"max_header_count":8,"max_http_buffer_bytes":8192,"max_stderr_bytes":4096,"observation_capacity":16}}"#.to_vec(),
    ] {
        replace_thread_blob(&database, malformed).await;
        assert!(matches!(
            repository.read_thread_engine_settings(&thread_id).await,
            Err(RepositoryError::CorruptData { .. })
        ));
    }

    let missing_variant = String::from_utf8(original_blob.clone())
        .expect("canonical configuration should be UTF-8")
        .replace("\"variant_id\":null,", "")
        .into_bytes();
    replace_thread_blob(&database, missing_variant).await;
    assert!(matches!(
        repository.read_thread_engine_settings(&thread_id).await,
        Err(RepositoryError::CorruptData { .. })
    ));

    replace_thread_blob(&database, original_blob).await;
    database
        .execute_unprepared(
            "CREATE TRIGGER test_abort_engine_config_update BEFORE UPDATE OF engine_run_config_version, engine_run_config_revision, engine_run_config ON threads BEGIN SELECT RAISE(ABORT, 'test rollback'); END",
        )
        .await
        .expect("rollback trigger should create");
    let failed = repository
        .set_thread_engine_config(input(
            "request-engine-rollback",
            &thread_id,
            EngineConfigUpdatePrecondition::Exact(accepted.revision()),
            config("rollback", false),
            500,
        ))
        .await
        .expect_err("aborted conditional update must fail");
    assert!(matches!(failed, RepositoryError::Database { .. }));
    database
        .execute_unprepared("DROP TRIGGER test_abort_engine_config_update")
        .await
        .expect("rollback trigger should drop");
    database
        .execute_unprepared(
            "CREATE TRIGGER test_abort_engine_config_receipt BEFORE INSERT ON command_receipts WHEN NEW.command_kind = 'set_thread_engine_config' BEGIN SELECT RAISE(ABORT, 'test receipt rollback'); END",
        )
        .await
        .expect("receipt rollback trigger should create");
    let receipt_failed = repository
        .set_thread_engine_config(input(
            "request-engine-receipt-rollback",
            &thread_id,
            EngineConfigUpdatePrecondition::Exact(accepted.revision()),
            config("receipt-rollback", false),
            600,
        ))
        .await
        .expect_err("aborted receipt insert must roll back the thread update");
    assert!(matches!(receipt_failed, RepositoryError::Database { .. }));
    database
        .execute_unprepared("DROP TRIGGER test_abort_engine_config_receipt")
        .await
        .expect("receipt rollback trigger should drop");
    let settings = repository
        .read_thread_engine_settings(&thread_id)
        .await
        .expect("settings read should work")
        .expect("settings should still exist");
    assert_eq!(settings.revision(), accepted.revision());
    assert_eq!(receipt_count(&database).await, 1);
    assert_eq!(
        entities::command_receipt::Entity::find()
            .all(&database)
            .await
            .expect("receipt query should work")
            .into_iter()
            .filter(|row| row.command_kind == CommandKind::SetThreadEngineConfig)
            .count(),
        1
    );
}

#[tokio::test]
async fn unconfigured_read_and_missing_thread_are_explicit() {
    let (_database, repository, thread_id) = seeded_repository().await;
    assert!(
        repository
            .read_thread_engine_settings(&thread_id)
            .await
            .expect("unconfigured read should work")
            .is_none()
    );

    let missing = ThreadId::parse("thread-missing-engine-config").expect("thread id is valid");
    let error = repository
        .set_thread_engine_config(input(
            "request-engine-missing",
            &missing,
            EngineConfigUpdatePrecondition::Unconfigured,
            config("missing", false),
            100,
        ))
        .await
        .expect_err("missing thread must be rejected");
    assert!(matches!(error, RepositoryError::ThreadNotFound { .. }));
}
