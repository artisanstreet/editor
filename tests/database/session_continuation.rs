use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities::{
    self, AssistantRunLifecycle, EntityLifecycle, OpaqueBytes, OrdinalKind,
};
use artisan_database::{
    Repository, SessionContinuationIncompatibility, SessionContinuationLookup,
    SessionContinuationQuery, SessionContinuationUnavailableReason, SqliteConfig, connect,
};
use artisan_domain::{EngineId, EngineProfileId, RunId, ThreadId};
use artisan_migrations::migrate_to_current;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection};

const THREAD_ID: &str = "thread-continuation";
const PROJECT_ID: &str = "project-continuation";

struct TemporaryDatabase {
    directory: PathBuf,
    file: PathBuf,
}

impl TemporaryDatabase {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after the Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "artisan-session-continuation-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("temporary database directory should be created");
        let file = directory.join("forge.sqlite3");
        Self { directory, file }
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

async fn migrated_memory_database() -> (DatabaseConnection, Repository) {
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
    seed_thread(&database).await;
    (database, repository)
}

fn config_blob(profile_id: &str) -> Vec<u8> {
    let template = r#"{"version":1,"engine":"opencode2","profile_id":"__PROFILE__","model_id":"model-fixture","route_id":"route-fixture","variant_id":null,"permission":{"permission_id":"permission-fixture","agent_id":"agent-fixture","approval":"on_request","filesystem":"workspace","network":"enabled","web_search":"disabled"},"runtime":{"attempt_budget_ms":100,"readiness_budget_ms":1,"health_budget_ms":1,"prompt_budget_ms":1,"stream_budget_ms":1,"close_budget_ms":1,"max_json_body_bytes":8192,"max_sse_line_bytes":4096,"max_sse_event_bytes":8192,"max_readiness_line_bytes":4096,"max_header_count":8,"max_http_buffer_bytes":8192,"max_stderr_bytes":4096,"observation_capacity":16}}"#;
    template.replace("__PROFILE__", profile_id).into_bytes()
}

async fn seed_thread(database: &DatabaseConnection) {
    entities::attached_project::ActiveModel {
        project_id: Set(PROJECT_ID.to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Continuation tests".to_owned()),
        attached_at_ms: Set(1),
    }
    .insert(database)
    .await
    .expect("project should insert");
    entities::thread::ActiveModel {
        thread_id: Set(THREAD_ID.to_owned()),
        project_id: Set(PROJECT_ID.to_owned()),
        title: Set("Continuation tests".to_owned()),
        created_at_ms: Set(10),
        updated_at_ms: Set(10),
        engine_run_config_version: Set(Some(1)),
        engine_run_config_revision: Set(1),
        engine_run_config: Set(Some(OpaqueBytes::new(config_blob("profile-fixture")))),
    }
    .insert(database)
    .await
    .expect("thread should insert");
    entities::conversation_state::ActiveModel {
        thread_id: Set(THREAD_ID.to_owned()),
        next_renderer_ordinal: Set(10),
        last_patch_sequence: Set(7),
        updated_at_ms: Set(500),
    }
    .insert(database)
    .await
    .expect("conversation state should insert");
}

async fn seed_run(
    database: &DatabaseConnection,
    run_id: &str,
    created_at_ms: i64,
    lifecycle: AssistantRunLifecycle,
    profile_id: &str,
    binding: Option<&str>,
    checkpoint_sequence: Option<i64>,
) {
    let message_id = format!("message-{run_id}");
    let turn_id = format!("turn-{run_id}");
    let ordinal = run_id
        .bytes()
        .fold(1_i64, |value, byte| value.wrapping_mul(37).wrapping_add(i64::from(byte)) & i64::MAX);
    entities::message::ActiveModel {
        message_id: Set(message_id.clone()),
        thread_id: Set(THREAD_ID.to_owned()),
        ordinal: Set(ordinal),
        body: Set(format!("message for {run_id}")),
        accepted_at_ms: Set(created_at_ms),
    }
    .insert(database)
    .await
    .expect("message should insert");
    entities::conversation_ordinal::ActiveModel {
        thread_id: Set(THREAD_ID.to_owned()),
        ordinal: Set(ordinal),
        kind: Set(OrdinalKind::Turn),
        entity_id: Set(turn_id.clone()),
    }
    .insert(database)
    .await
    .expect("turn ordinal should insert");
    entities::conversation_turn::ActiveModel {
        turn_id: Set(turn_id.clone()),
        thread_id: Set(THREAD_ID.to_owned()),
        ordinal: Set(ordinal),
        kind: Set(OrdinalKind::Turn),
        revision: Set(0),
        lifecycle: Set(if is_active(&lifecycle) {
            EntityLifecycle::Active
        } else {
            EntityLifecycle::Completed
        }),
        created_at_ms: Set(created_at_ms),
        updated_at_ms: Set(created_at_ms + 10),
    }
    .insert(database)
    .await
    .expect("turn should insert");

    let active = is_active(&lifecycle);
    let settled = is_settled(&lifecycle);
    let has_error = has_error(&lifecycle);
    let binding_bytes = binding.map(|value| {
        if value.starts_with('{') {
            value.as_bytes().to_vec()
        } else {
            serde_json::to_vec(&serde_json::json!({
                "engine": "opencode2",
                "profile_id": profile_id,
                "session_id": value,
            }))
            .expect("binding should serialize")
        }
    });
    let mut run_start_key = [created_at_ms as u8; 32];
    for (index, byte) in run_id.bytes().enumerate() {
        let slot = index % run_start_key.len();
        run_start_key[slot] = run_start_key[slot].wrapping_add(byte);
    }
    entities::assistant_run::ActiveModel {
        run_id: Set(run_id.to_owned()),
        thread_id: Set(THREAD_ID.to_owned()),
        run_start_key: Set(OpaqueBytes::new(run_start_key.to_vec())),
        origin_message_id: Set(message_id),
        origin_turn_id: Set(turn_id),
        lifecycle: Set(lifecycle),
        generation: Set(if active || settled { 1 } else { 0 }),
        owner: Set(active.then(|| OpaqueBytes::new(vec![0xa1; 32]))),
        lease: Set(active.then(|| OpaqueBytes::new(vec![0xb2; 32]))),
        claim_token: Set(None),
        provider_binding_version: Set(binding_bytes.as_ref().map(|_| 1)),
        provider_binding: Set(binding_bytes.map(OpaqueBytes::new)),
        provider_bound_at_ms: Set(binding.map(|_| created_at_ms + 2)),
        error_code: Set(has_error.then_some("provider_failed".to_owned())),
        error_message: Set(has_error.then_some("fixture failure".to_owned())),
        created_at_ms: Set(created_at_ms),
        updated_at_ms: Set(created_at_ms + 10),
        terminal_at_ms: Set(settled.then_some(created_at_ms + 10)),
        engine_run_config_version: Set(Some(1)),
        engine_run_config_revision: Set(Some(1)),
        engine_run_config: Set(Some(OpaqueBytes::new(config_blob(profile_id)))),
    }
    .insert(database)
    .await
    .expect("assistant run should insert");

    if let Some(sequence) = checkpoint_sequence {
        entities::run_checkpoint::ActiveModel {
            run_id: Set(run_id.to_owned()),
            generation: Set(1),
            last_batch_sequence: Set(sequence),
            engine_checkpoint_version: Set(Some(1)),
            engine_checkpoint_blob: Set(Some(OpaqueBytes::new(vec![0xcc; 4]))),
            updated_at_ms: Set(created_at_ms + 5),
        }
        .insert(database)
        .await
        .expect("checkpoint should insert");
        entities::run_batch_receipt::ActiveModel {
            run_id: Set(run_id.to_owned()),
            batch_sequence: Set(sequence),
            generation: Set(1),
            digest: Set(OpaqueBytes::new(vec![0xdd; 32])),
            committed: Set(true),
        }
        .insert(database)
        .await
        .expect("batch receipt should insert");
    }
}

fn is_active(lifecycle: &AssistantRunLifecycle) -> bool {
    matches!(
        lifecycle,
        AssistantRunLifecycle::Queued
            | AssistantRunLifecycle::Launching
            | AssistantRunLifecycle::Running
            | AssistantRunLifecycle::Waiting
            | AssistantRunLifecycle::CancelRequested
    )
}

fn is_settled(lifecycle: &AssistantRunLifecycle) -> bool {
    matches!(
        lifecycle,
        AssistantRunLifecycle::Completed
            | AssistantRunLifecycle::Failed
            | AssistantRunLifecycle::Cancelled
            | AssistantRunLifecycle::Interrupted
    )
}

fn has_error(lifecycle: &AssistantRunLifecycle) -> bool {
    matches!(
        lifecycle,
        AssistantRunLifecycle::Failed | AssistantRunLifecycle::Interrupted
    )
}

fn query(profile_id: &str, exclude_run_id: Option<&str>) -> SessionContinuationQuery {
    SessionContinuationQuery {
        thread_id: ThreadId::parse(THREAD_ID).expect("thread id should be valid"),
        engine_id: EngineId::OpenCode2,
        profile_id: EngineProfileId::parse(profile_id).expect("profile id should be valid"),
        exclude_run_id: exclude_run_id.map(|value| RunId::parse(value).expect("run id is valid")),
    }
}

#[tokio::test]
async fn empty_thread_reports_no_history() {
    let (_database, repository) = migrated_memory_database().await;
    assert_eq!(
        repository
            .read_session_continuation(query("profile-fixture", None))
            .await
            .expect("continuation read should succeed"),
        SessionContinuationLookup::NoHistory
    );
}

#[tokio::test]
async fn two_turn_restart_returns_latest_session_and_durable_facts() {
    let temporary = TemporaryDatabase::new();
    let first = connect(
        SqliteConfig::file(temporary.file.as_path())
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("file database should open");
    migrate_to_current(&first)
        .await
        .expect("database should migrate");
    seed_thread(&first).await;
    seed_run(
        &first,
        "run-first",
        100,
        AssistantRunLifecycle::Completed,
        "profile-fixture",
        Some("session-first"),
        Some(1),
    )
    .await;
    seed_run(
        &first,
        "run-second",
        200,
        AssistantRunLifecycle::Completed,
        "profile-fixture",
        Some("session-second"),
        Some(2),
    )
    .await;
    first.close().await.expect("first connection should close");

    let reopened = connect(
        SqliteConfig::file(temporary.file.as_path())
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("database should reopen");
    let lookup = Repository::new(reopened.clone())
        .read_session_continuation(query("profile-fixture", None))
        .await
        .expect("continuation read should succeed");
    let SessionContinuationLookup::Usable(continuation) = lookup else {
        panic!("latest settled binding should be usable");
    };
    assert_eq!(continuation.session_id.as_str(), "session-second");
    assert_eq!(continuation.prior_run.run_id.as_str(), "run-second");
    assert_eq!(continuation.sequence.last_batch_sequence, 2);
    assert_eq!(continuation.sequence.last_committed_batch_sequence, Some(2));
    assert_eq!(continuation.sequence.last_patch_sequence, Some(7));
    assert_eq!(
        continuation
            .checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.engine_checkpoint_version),
        Some(Some(1))
    );
    assert!(!format!("{continuation:?}").contains("session-second"));
    reopened
        .close()
        .await
        .expect("reopened connection should close");
}

#[tokio::test]
async fn newer_profile_mismatch_blocks_older_matching_history() {
    let (database, repository) = migrated_memory_database().await;
    seed_run(
        &database,
        "run-old-profile",
        100,
        AssistantRunLifecycle::Completed,
        "profile-fixture",
        Some("old-session"),
        None,
    )
    .await;
    seed_run(
        &database,
        "run-new-profile",
        200,
        AssistantRunLifecycle::Failed,
        "another-profile",
        Some("new-session"),
        None,
    )
    .await;

    let lookup = repository
        .read_session_continuation(query("profile-fixture", None))
        .await
        .expect("continuation read should succeed");
    assert_eq!(
        lookup,
        SessionContinuationLookup::Incompatible(
            artisan_database::SessionContinuationIncompatible {
                run_id: RunId::parse("run-new-profile").expect("run id is valid"),
                reason: SessionContinuationIncompatibility::Profile,
            },
        )
    );
}

#[tokio::test]
async fn active_run_is_not_resumed_over_an_older_session() {
    let (database, repository) = migrated_memory_database().await;
    seed_run(
        &database,
        "run-old",
        100,
        AssistantRunLifecycle::Completed,
        "profile-fixture",
        Some("old-session"),
        None,
    )
    .await;
    seed_run(
        &database,
        "run-active",
        200,
        AssistantRunLifecycle::Running,
        "profile-fixture",
        Some("active-session"),
        None,
    )
    .await;

    let lookup = repository
        .read_session_continuation(query("profile-fixture", None))
        .await
        .expect("continuation read should succeed");
    assert_eq!(
        lookup,
        SessionContinuationLookup::Unavailable(artisan_database::SessionContinuationUnavailable {
            run_id: RunId::parse("run-active").expect("run id is valid"),
            reason: SessionContinuationUnavailableReason::ActiveRun,
        },)
    );
}

#[tokio::test]
async fn invalid_binding_is_rejected_without_debugging_raw_bytes() {
    let (database, repository) = migrated_memory_database().await;
    seed_run(
        &database,
        "run-invalid-binding",
        100,
        AssistantRunLifecycle::Failed,
        "profile-fixture",
        Some("{not-json"),
        None,
    )
    .await;

    let error = repository
        .read_session_continuation(query("profile-fixture", None))
        .await
        .expect_err("corrupt provider binding must fail the read");
    let debug = format!("{error:?}");
    assert!(debug.contains("provider_binding"));
    assert!(!debug.contains("not-json"));
}

#[tokio::test]
async fn ordering_is_newest_first_and_current_run_is_the_only_exclusion() {
    let (database, repository) = migrated_memory_database().await;
    seed_run(
        &database,
        "run-a",
        100,
        AssistantRunLifecycle::Completed,
        "profile-fixture",
        Some("session-a"),
        None,
    )
    .await;
    seed_run(
        &database,
        "run-b",
        100,
        AssistantRunLifecycle::Completed,
        "profile-fixture",
        Some("session-b"),
        None,
    )
    .await;

    let newest = repository
        .read_session_continuation(query("profile-fixture", None))
        .await
        .expect("continuation read should succeed");
    let SessionContinuationLookup::Usable(newest) = newest else {
        panic!("newest settled binding should be usable");
    };
    assert_eq!(newest.prior_run.run_id.as_str(), "run-b");

    let excluded = repository
        .read_session_continuation(query("profile-fixture", Some("run-b")))
        .await
        .expect("continuation read should succeed");
    let SessionContinuationLookup::Usable(excluded) = excluded else {
        panic!("older settled binding should be usable after excluding current run");
    };
    assert_eq!(excluded.prior_run.run_id.as_str(), "run-a");
}

#[tokio::test]
async fn newer_unbound_failure_does_not_fall_back_to_an_old_session() {
    let (database, repository) = migrated_memory_database().await;
    seed_run(
        &database,
        "run-old",
        100,
        AssistantRunLifecycle::Completed,
        "profile-fixture",
        Some("old-session"),
        None,
    )
    .await;
    seed_run(
        &database,
        "run-unbound-failure",
        200,
        AssistantRunLifecycle::Failed,
        "profile-fixture",
        None,
        None,
    )
    .await;

    let lookup = repository
        .read_session_continuation(query("profile-fixture", None))
        .await
        .expect("continuation read should succeed");
    assert_eq!(
        lookup,
        SessionContinuationLookup::Unavailable(artisan_database::SessionContinuationUnavailable {
            run_id: RunId::parse("run-unbound-failure").expect("run id is valid"),
            reason: SessionContinuationUnavailableReason::UnboundSettledRun,
        },)
    );
}
