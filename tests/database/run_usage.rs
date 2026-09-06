//! Focused migrated-SQLite coverage for provider usage scope and fencing.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities::{self, AssistantRunLifecycle, EntityLifecycle, OrdinalKind};
use artisan_database::{
    CreateThreadInput, QueueFirstMessageInput, RecordRunUsage, RecordRunUsageOutcome, Repository,
    RunUsageRepositoryError, SetThreadEngineConfigInput, SqliteConfig, connect,
};
use artisan_domain::{
    ApprovalMode, ByteLimit, CodexSelection, CountLimit, EngineAgentId, EngineConfigRevision,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, MessageBody, MessageId, NetworkAccess,
    OpenCode2Selection, PermissionId, ProjectId, RequestId, RunId, RunUsageBasis, RunUsageReport,
    RunUsageReportInput, ThreadId, ThreadTitle, UnixMillis, WebSearchAccess,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseConnection, EntityTrait,
};

const PROJECT_ID: &str = "project-usage";
const THREAD_ID: &str = "thread-usage";
const RUN_ID: &str = "run-usage";
const MESSAGE_ID: &str = "message-usage";
const TURN_ID: &str = "turn-usage";

struct TemporaryDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl TemporaryDatabase {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time is after the epoch")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("artisan-run-usage-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&directory).expect("temporary database directory should create");
        let path = directory.join("forge.sqlite3");
        Self { directory, path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

async fn open_database(path: Option<&Path>) -> (DatabaseConnection, Repository) {
    let config = match path {
        Some(path) => SqliteConfig::file(path),
        None => SqliteConfig::in_memory(),
    };
    let database = connect(
        config
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("database should open");
    migrate_to_current(&database)
        .await
        .expect("database should migrate through run usage");
    (database.clone(), Repository::new(database))
}

fn runtime() -> EngineRuntimeControls {
    let one = FiniteMillis::new(1).expect("one millisecond is valid");
    EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(100).expect("attempt budget is valid"),
        readiness_budget: one,
        health_budget: one,
        prompt_budget: one,
        stream_budget: one,
        close_budget: one,
        max_json_body_bytes: ByteLimit::new(8_192).expect("json body limit is valid"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("sse line limit is valid"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("sse event limit is valid"),
        max_readiness_line_bytes: ByteLimit::new(4_096).expect("readiness line limit is valid"),
        max_header_count: CountLimit::new(8).expect("header count is valid"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("http buffer limit is valid"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr limit is valid"),
        observation_capacity: CountLimit::new(16).expect("observation capacity is valid"),
    })
    .expect("runtime relationships are valid")
}

fn config() -> EngineRunConfig {
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-usage").expect("permission id is valid"),
        EngineAgentId::parse("agent-usage").expect("agent id is valid"),
        ApprovalMode::Never,
        FilesystemAccess::Workspace,
        NetworkAccess::Disabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-usage").expect("profile id is valid"),
            EngineModelId::parse("model-usage").expect("model id is valid"),
            EngineRouteId::parse("route-usage").expect("route id is valid"),
            None,
            permission,
        )),
        runtime(),
    )
}

fn codex_config() -> EngineRunConfig {
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-usage").expect("permission id is valid"),
        EngineAgentId::parse("agent-usage").expect("agent id is valid"),
        ApprovalMode::Never,
        FilesystemAccess::Workspace,
        NetworkAccess::Disabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::Codex(
            CodexSelection::new(
                EngineProfileId::parse("profile-usage").expect("profile id is valid"),
                Some(EngineModelId::parse("model-usage").expect("model id is valid")),
                permission,
                None,
                None,
                None,
            )
            .expect("codex selection is valid"),
        ),
        runtime(),
    )
}

async fn seed_project_and_thread(database: &DatabaseConnection, repository: &Repository) {
    entities::attached_project::ActiveModel {
        project_id: Set(PROJECT_ID.to_owned()),
        root_path: Set("C:/repos/usage".to_owned()),
        display_name: Set("Usage".to_owned()),
        attached_at_ms: Set(1),
    }
    .insert(database)
    .await
    .expect("project should insert");
    repository
        .create_thread(CreateThreadInput {
            request_id: RequestId::parse("request-create-usage").expect("request id"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
            project_id: ProjectId::parse(PROJECT_ID).expect("project id"),
            title: ThreadTitle::parse("Usage").expect("title"),
            created_at: UnixMillis::from_millis(1),
            updated_at: UnixMillis::from_millis(1),
        })
        .await
        .expect("thread should create");
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse("request-config-usage").expect("request id"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
            precondition: EngineConfigUpdatePrecondition::Unconfigured,
            config: config(),
            accepted_at: UnixMillis::from_millis(1),
        })
        .await
        .expect("thread configuration should persist");
}

async fn seed_run(database: &DatabaseConnection, repository: &Repository) {
    repository
        .queue_first_message(QueueFirstMessageInput {
            request_id: RequestId::parse("request-first-usage").expect("request id"),
            message_id: MessageId::parse(MESSAGE_ID).expect("message id"),
            thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
            body: MessageBody::parse("usage fixture").expect("body"),
            accepted_at: UnixMillis::from_millis(2),
        })
        .await
        .expect("message should queue");

    entities::conversation_ordinal::ActiveModel {
        thread_id: Set(THREAD_ID.to_owned()),
        ordinal: Set(0),
        kind: Set(OrdinalKind::Turn),
        entity_id: Set(TURN_ID.to_owned()),
    }
    .insert(database)
    .await
    .expect("turn ordinal should insert");
    entities::conversation_turn::ActiveModel {
        turn_id: Set(TURN_ID.to_owned()),
        thread_id: Set(THREAD_ID.to_owned()),
        ordinal: Set(0),
        kind: Set(OrdinalKind::Turn),
        revision: Set(0),
        lifecycle: Set(EntityLifecycle::Pending),
        created_at_ms: Set(2),
        updated_at_ms: Set(2),
    }
    .insert(database)
    .await
    .expect("turn should insert");

    let thread = entities::thread::Entity::find_by_id(THREAD_ID)
        .one(database)
        .await
        .expect("thread should read")
        .expect("thread should exist");
    let config_blob = thread
        .engine_run_config
        .expect("configured thread has a snapshot")
        .into_vec();
    entities::assistant_run::ActiveModel {
        run_id: Set(RUN_ID.to_owned()),
        thread_id: Set(THREAD_ID.to_owned()),
        run_start_key: Set(entities::OpaqueBytes::new(vec![0; 32])),
        origin_message_id: Set(MESSAGE_ID.to_owned()),
        origin_turn_id: Set(TURN_ID.to_owned()),
        lifecycle: Set(AssistantRunLifecycle::Completed),
        generation: Set(1),
        owner: Set(None),
        lease: Set(None),
        claim_token: Set(None),
        provider_binding_version: Set(None),
        provider_binding: Set(None),
        provider_bound_at_ms: Set(None),
        error_code: Set(None),
        error_message: Set(None),
        created_at_ms: Set(3),
        updated_at_ms: Set(3),
        terminal_at_ms: Set(Some(3)),
        engine_run_config_version: Set(Some(1)),
        engine_run_config_revision: Set(Some(thread.engine_run_config_revision)),
        engine_run_config: Set(Some(entities::OpaqueBytes::new(config_blob))),
    }
    .insert(database)
    .await
    .expect("assistant run should insert");
}

async fn seeded(path: Option<&Path>) -> (DatabaseConnection, Repository, RunId, ThreadId) {
    let (database, repository) = open_database(path).await;
    seed_project_and_thread(&database, &repository).await;
    seed_run(&database, &repository).await;
    (
        database,
        repository,
        RunId::parse(RUN_ID).expect("run id"),
        ThreadId::parse(THREAD_ID).expect("thread id"),
    )
}

fn report(sequence: u64, input_tokens: Option<u64>, observed_at: i64) -> RunUsageReport {
    RunUsageReport::new(RunUsageReportInput {
        run_id: RunId::parse(RUN_ID).expect("run id"),
        thread_id: ThreadId::parse(THREAD_ID).expect("thread id"),
        provider_session_id: "provider-session-usage".to_owned(),
        source_sequence: sequence,
        model_id: EngineModelId::parse("model-usage").expect("model id"),
        provider_route_id: EngineRouteId::parse("route-usage").expect("route id"),
        variant_id: None,
        basis: RunUsageBasis::Delta,
        provider_turn_id: Some("assistant-usage".to_owned()),
        input_tokens,
        cached_input_tokens: Some(2),
        output_tokens: Some(3),
        context_tokens: None,
        context_window_tokens: None,
        observed_at: UnixMillis::from_millis(observed_at),
    })
    .expect("usage report should validate")
}

fn record_command<'a>(report: &'a RunUsageReport) -> RecordRunUsage<'a> {
    RecordRunUsage {
        run_id: report.run_id(),
        thread_id: report.thread_id(),
        report,
    }
}

#[tokio::test]
async fn usage_survives_restart_and_returns_the_highest_sequence() {
    let temporary = TemporaryDatabase::new();
    let (database, repository, run_id, thread_id) = seeded(Some(temporary.path())).await;
    let first = report(4, Some(10), 10);
    let second = report(5, Some(11), 11);
    assert!(matches!(
        repository.record_run_usage(record_command(&first)).await,
        Ok(RecordRunUsageOutcome::Recorded(_))
    ));
    let second_result = repository.record_run_usage(record_command(&second)).await;
    assert!(matches!(second_result, Ok(RecordRunUsageOutcome::Replaced(_))), "{second_result:?}");
    drop(repository);
    database.close().await.expect("close SQLite before restart");

    let (reopened_database, reopened) = open_database(Some(temporary.path())).await;
    let latest = reopened
        .read_latest_run_usage(&run_id, &thread_id)
        .await
        .expect("latest usage should read after restart")
        .expect("usage should exist after restart");
    assert_eq!(latest.source_sequence(), 5);
    assert_eq!(latest.input_tokens(), Some(11));
    drop(reopened);
    reopened_database.close().await.expect("close reopened SQLite");
}

#[tokio::test]
async fn duplicate_is_idempotent_stale_is_rejected_and_equal_sequence_conflicts() {
    let (database, repository, run_id, thread_id) = seeded(None).await;
    let first = report(4, Some(10), 10);
    repository
        .record_run_usage(record_command(&first))
        .await
        .expect("first usage should record");

    let duplicate = report(4, Some(10), 99);
    assert!(matches!(
        repository
            .record_run_usage(record_command(&duplicate))
            .await,
        Ok(RecordRunUsageOutcome::Duplicate(_))
    ));
    let stale = report(3, Some(9), 98);
    assert!(matches!(
        repository.record_run_usage(record_command(&stale)).await,
        Err(RunUsageRepositoryError::StaleSequence { .. })
    ));
    let conflict = report(4, Some(12), 97);
    assert!(matches!(
        repository.record_run_usage(record_command(&conflict)).await,
        Err(RunUsageRepositoryError::SequenceConflict { .. })
    ));
    let latest = repository
        .read_latest_run_usage(&run_id, &thread_id)
        .await
        .expect("usage should read")
        .expect("usage should remain");
    assert_eq!(latest.input_tokens(), Some(10));
    drop(database);
}

#[tokio::test]
async fn cross_scope_and_provider_session_writes_are_rejected() {
    let (database, repository, run_id, thread_id) = seeded(None).await;
    let first = report(4, Some(10), 10);
    repository
        .record_run_usage(record_command(&first))
        .await
        .expect("first usage should record");

    let other_thread = ThreadId::parse("thread-other").expect("thread id");
    assert!(matches!(
        repository
            .read_latest_run_usage(&run_id, &other_thread)
            .await,
        Err(RunUsageRepositoryError::RunThreadMismatch { .. })
    ));

    let other_run = RunId::parse("run-other").expect("run id");
    let cross_run = RecordRunUsage {
        run_id: &other_run,
        thread_id: &thread_id,
        report: &first,
    };
    assert!(matches!(
        repository.record_run_usage(cross_run).await,
        Err(RunUsageRepositoryError::ReportRunMismatch)
    ));

    let wrong_origin = RunUsageReport::new(RunUsageReportInput {
        run_id: run_id.clone(),
        thread_id: thread_id.clone(),
        provider_session_id: "provider-session-usage".to_owned(),
        source_sequence: 5,
        model_id: EngineModelId::parse("model-from-ui").expect("model id"),
        provider_route_id: EngineRouteId::parse("route-usage").expect("route id"),
        variant_id: None,
        basis: RunUsageBasis::Delta,
        provider_turn_id: Some("assistant-usage".to_owned()),
        input_tokens: Some(11),
        cached_input_tokens: Some(2),
        output_tokens: Some(3),
        context_tokens: None,
        context_window_tokens: None,
        observed_at: UnixMillis::from_millis(11),
    })
    .expect("wrong-origin report should validate independently");
    assert!(matches!(
        repository
            .record_run_usage(record_command(&wrong_origin))
            .await,
        Err(RunUsageRepositoryError::ModelOriginMismatch { .. })
    ));

    let different_session = RunUsageReport::new(RunUsageReportInput {
        run_id,
        thread_id,
        provider_session_id: "provider-session-other".to_owned(),
        source_sequence: 5,
        model_id: EngineModelId::parse("model-usage").expect("model id"),
        provider_route_id: EngineRouteId::parse("route-usage").expect("route id"),
        variant_id: None,
        basis: RunUsageBasis::Delta,
        provider_turn_id: Some("assistant-usage".to_owned()),
        input_tokens: Some(11),
        cached_input_tokens: Some(2),
        output_tokens: Some(3),
        context_tokens: None,
        context_window_tokens: None,
        observed_at: UnixMillis::from_millis(12),
    })
    .expect("alternate session report should validate");
    assert!(matches!(
        repository
            .record_run_usage(record_command(&different_session))
            .await,
        Err(RunUsageRepositoryError::ProviderSessionConflict { .. })
    ));
    drop(database);
}

#[tokio::test]
async fn unknown_run_is_rejected_without_creating_a_usage_row() {
    let (database, repository, _run_id, thread_id) = seeded(None).await;
    let unknown_run = RunId::parse("run-missing").expect("run id");
    let unknown_report = RunUsageReport::new(RunUsageReportInput {
        run_id: unknown_run.clone(),
        thread_id: thread_id.clone(),
        provider_session_id: "provider-session-usage".to_owned(),
        source_sequence: 1,
        model_id: EngineModelId::parse("model-usage").expect("model id"),
        provider_route_id: EngineRouteId::parse("route-usage").expect("route id"),
        variant_id: None,
        basis: RunUsageBasis::Delta,
        provider_turn_id: None,
        input_tokens: Some(1),
        cached_input_tokens: None,
        output_tokens: Some(1),
        context_tokens: None,
        context_window_tokens: None,
        observed_at: UnixMillis::EPOCH,
    })
    .expect("unknown-run report should validate");
    assert!(matches!(
        repository
            .record_run_usage(RecordRunUsage {
                run_id: &unknown_run,
                thread_id: &thread_id,
                report: &unknown_report,
            })
            .await,
        Err(RunUsageRepositoryError::RunNotFound { .. })
    ));
    drop(database);
}

#[tokio::test]
async fn non_opencode2_snapshot_cannot_authorize_usage_as_opencode2() {
    let (database, repository, run_id, thread_id) = seeded(None).await;
    // Promote the thread to a Codex configuration, then snapshot that exact
    // configuration onto the run row the way dispatch would.
    repository
        .set_thread_engine_config(SetThreadEngineConfigInput {
            request_id: RequestId::parse("request-config-codex").expect("request id"),
            thread_id: thread_id.clone(),
            precondition: EngineConfigUpdatePrecondition::Exact(
                EngineConfigRevision::new(1).expect("revision is valid"),
            ),
            config: codex_config(),
            accepted_at: UnixMillis::from_millis(4),
        })
        .await
        .expect("codex configuration should persist");
    database
        .execute_unprepared(
            "UPDATE assistant_runs SET engine_run_config_version = 2, engine_run_config_revision = 2, engine_run_config = (SELECT engine_run_config FROM threads WHERE thread_id = 'thread-usage') WHERE run_id = 'run-usage'",
        )
        .await
        .expect("run snapshot should follow the thread configuration");
    let first = report(4, Some(10), 10);
    assert!(matches!(
        repository.record_run_usage(record_command(&first)).await,
        Err(RunUsageRepositoryError::InvalidRunSnapshot { .. })
    ));
    drop(database);
    drop(run_id);
}
