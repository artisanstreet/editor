use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::entities::{
    self, AssistantRunLifecycle, ConversationItemKind, EntityLifecycle, OpaqueBytes, OrdinalKind,
    RenderPhase,
};
use artisan_database::{Repository, RepositoryError, SqliteConfig, connect};
use artisan_domain::{
    AssistantMessagePhase, ConversationItem, ConversationLifecycle, ConversationQuery,
    ConversationQueryBounds, ItemOrdinal, QueryTurnCount, Revision, ThreadId, TurnOrdinal,
    UnixMillis,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseConnection, EntityTrait,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const THREAD_ID: &str = "thread-1";

async fn memory_database() -> (DatabaseConnection, Repository) {
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

fn thread_id() -> ThreadId {
    ThreadId::parse(THREAD_ID).expect("thread id should parse")
}

fn window(maximum_turn_count: u64) -> ConversationQuery {
    ConversationQuery {
        thread_id: thread_id(),
        bounds: ConversationQueryBounds::Window {
            maximum_turn_count: QueryTurnCount::new(maximum_turn_count)
                .expect("query count should be bounded"),
        },
    }
}

fn range(
    before_turn_ordinal: u64,
    minimum_turn_ordinal: Option<u64>,
    maximum_turn_count: u64,
) -> ConversationQuery {
    ConversationQuery {
        thread_id: thread_id(),
        bounds: ConversationQueryBounds::Range {
            before_turn_ordinal: TurnOrdinal::new(before_turn_ordinal),
            minimum_turn_ordinal: minimum_turn_ordinal.map(TurnOrdinal::new),
            maximum_turn_count: QueryTurnCount::new(maximum_turn_count)
                .expect("query count should be bounded"),
        },
    }
}

async fn seed_thread(database: &DatabaseConnection, updated_at_ms: i64) {
    entities::attached_project::ActiveModel {
        project_id: Set("project-1".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(1),
    }
    .insert(database)
    .await
    .expect("project should insert");
    entities::thread::ActiveModel {
        thread_id: Set(THREAD_ID.to_owned()),
        project_id: Set("project-1".to_owned()),
        title: Set("Conversation projection".to_owned()),
        created_at_ms: Set(10),
        updated_at_ms: Set(updated_at_ms),
        engine_run_config_version: Set(None),
        engine_run_config_revision: Set(0),
        engine_run_config: Set(None),
    }
    .insert(database)
    .await
    .expect("thread should insert");
}

async fn insert_turn(
    database: &DatabaseConnection,
    turn_id: &str,
    ordinal: i64,
    revision: i64,
    lifecycle: EntityLifecycle,
    created_at_ms: i64,
    updated_at_ms: i64,
) {
    entities::conversation_ordinal::ActiveModel {
        thread_id: Set(THREAD_ID.to_owned()),
        ordinal: Set(ordinal),
        kind: Set(OrdinalKind::Turn),
        entity_id: Set(turn_id.to_owned()),
    }
    .insert(database)
    .await
    .expect("turn ordinal should insert");
    entities::conversation_turn::ActiveModel {
        turn_id: Set(turn_id.to_owned()),
        thread_id: Set(THREAD_ID.to_owned()),
        ordinal: Set(ordinal),
        kind: Set(OrdinalKind::Turn),
        revision: Set(revision),
        lifecycle: Set(lifecycle),
        created_at_ms: Set(created_at_ms),
        updated_at_ms: Set(updated_at_ms),
    }
    .insert(database)
    .await
    .expect("turn should insert");
}

async fn insert_message(
    database: &DatabaseConnection,
    message_id: &str,
    ordinal: i64,
    body: &str,
    accepted_at_ms: i64,
) {
    entities::message::ActiveModel {
        message_id: Set(message_id.to_owned()),
        thread_id: Set(THREAD_ID.to_owned()),
        ordinal: Set(ordinal),
        body: Set(body.to_owned()),
        accepted_at_ms: Set(accepted_at_ms),
    }
    .insert(database)
    .await
    .expect("message should insert");
}

struct UserItemSeed<'a> {
    item_id: &'a str,
    turn_id: &'a str,
    ordinal: i64,
    revision: i64,
    lifecycle: EntityLifecycle,
    message_id: &'a str,
    message_ordinal: i64,
    body: &'a str,
    created_at_ms: i64,
    updated_at_ms: i64,
}

async fn insert_user_item(database: &DatabaseConnection, seed: UserItemSeed<'_>) {
    let UserItemSeed {
        item_id,
        turn_id,
        ordinal,
        revision,
        lifecycle,
        message_id,
        message_ordinal,
        body,
        created_at_ms,
        updated_at_ms,
    } = seed;
    insert_message(database, message_id, message_ordinal, body, created_at_ms).await;
    entities::conversation_ordinal::ActiveModel {
        thread_id: Set(THREAD_ID.to_owned()),
        ordinal: Set(ordinal),
        kind: Set(OrdinalKind::Item),
        entity_id: Set(item_id.to_owned()),
    }
    .insert(database)
    .await
    .expect("user item ordinal should insert");
    entities::conversation_item::ActiveModel {
        item_id: Set(item_id.to_owned()),
        thread_id: Set(THREAD_ID.to_owned()),
        turn_id: Set(turn_id.to_owned()),
        ordinal: Set(ordinal),
        kind: Set(OrdinalKind::Item),
        revision: Set(revision),
        lifecycle: Set(lifecycle),
        item_kind: Set(ConversationItemKind::UserMessage),
        source_message_id: Set(Some(message_id.to_owned())),
        run_id: Set(None),
        native_item_key: Set(None),
        phase: Set(None),
        body: Set(body.to_owned()),
        created_at_ms: Set(created_at_ms),
        updated_at_ms: Set(updated_at_ms),
    }
    .insert(database)
    .await
    .expect("user item should insert");
}

struct AssistantItemSeed<'a> {
    item_id: &'a str,
    turn_id: &'a str,
    ordinal: i64,
    revision: i64,
    lifecycle: EntityLifecycle,
    run_id: &'a str,
    phase: RenderPhase,
    origin_message_id: &'a str,
    origin_message_ordinal: i64,
    body: &'a str,
    created_at_ms: i64,
    updated_at_ms: i64,
    key_byte: u8,
}

async fn insert_assistant_item(database: &DatabaseConnection, seed: AssistantItemSeed<'_>) {
    let AssistantItemSeed {
        item_id,
        turn_id,
        ordinal,
        revision,
        lifecycle,
        run_id,
        phase,
        origin_message_id,
        origin_message_ordinal,
        body,
        created_at_ms,
        updated_at_ms,
        key_byte,
    } = seed;
    insert_message(
        database,
        origin_message_id,
        origin_message_ordinal,
        "assistant prompt",
        created_at_ms,
    )
    .await;
    entities::assistant_run::ActiveModel {
        run_id: Set(run_id.to_owned()),
        thread_id: Set(THREAD_ID.to_owned()),
        run_start_key: Set(OpaqueBytes::new(vec![key_byte; 32])),
        origin_message_id: Set(origin_message_id.to_owned()),
        origin_turn_id: Set(turn_id.to_owned()),
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
        created_at_ms: Set(created_at_ms),
        updated_at_ms: Set(updated_at_ms),
        terminal_at_ms: Set(Some(updated_at_ms)),
        engine_run_config_version: Set(Some(1)),
        engine_run_config_revision: Set(Some(1)),
        engine_run_config: Set(Some(OpaqueBytes::new(
            br#"{"version":1,"engine":"opencode2","profile_id":"profile-fixture","model_id":"model-fixture","route_id":"route-fixture","variant_id":null,"permission":{"permission_id":"permission-fixture","agent_id":"agent-fixture","approval":"on_request","filesystem":"workspace","network":"enabled","web_search":"disabled"},"runtime":{"attempt_budget_ms":100,"readiness_budget_ms":1,"health_budget_ms":1,"prompt_budget_ms":1,"stream_budget_ms":1,"close_budget_ms":1,"max_json_body_bytes":8192,"max_sse_line_bytes":4096,"max_sse_event_bytes":8192,"max_readiness_line_bytes":4096,"max_header_count":8,"max_http_buffer_bytes":8192,"max_stderr_bytes":4096,"observation_capacity":16}}"#.to_vec(),
        ))),
    }
    .insert(database)
    .await
    .expect("assistant run should insert");
    entities::conversation_ordinal::ActiveModel {
        thread_id: Set(THREAD_ID.to_owned()),
        ordinal: Set(ordinal),
        kind: Set(OrdinalKind::Item),
        entity_id: Set(item_id.to_owned()),
    }
    .insert(database)
    .await
    .expect("assistant item ordinal should insert");
    entities::conversation_item::ActiveModel {
        item_id: Set(item_id.to_owned()),
        thread_id: Set(THREAD_ID.to_owned()),
        turn_id: Set(turn_id.to_owned()),
        ordinal: Set(ordinal),
        kind: Set(OrdinalKind::Item),
        revision: Set(revision),
        lifecycle: Set(lifecycle),
        item_kind: Set(ConversationItemKind::AssistantMessage),
        source_message_id: Set(None),
        run_id: Set(Some(run_id.to_owned())),
        native_item_key: Set(Some(format!("native-{item_id}"))),
        phase: Set(Some(phase)),
        body: Set(body.to_owned()),
        created_at_ms: Set(created_at_ms),
        updated_at_ms: Set(updated_at_ms),
    }
    .insert(database)
    .await
    .expect("assistant item should insert");
}

async fn seed_projection(database: &DatabaseConnection) {
    seed_thread(database, 900).await;
    insert_turn(database, "turn-1", 0, 0, EntityLifecycle::Pending, 100, 110).await;
    insert_turn(database, "turn-2", 2, 1, EntityLifecycle::Active, 200, 250).await;
    insert_turn(database, "turn-3", 4, 2, EntityLifecycle::Waiting, 300, 350).await;
    insert_turn(
        database,
        "turn-4",
        6,
        3,
        EntityLifecycle::Completed,
        400,
        450,
    )
    .await;
    insert_user_item(
        database,
        UserItemSeed {
            item_id: "item-1",
            turn_id: "turn-1",
            ordinal: 1,
            revision: 0,
            lifecycle: EntityLifecycle::Completed,
            message_id: "message-1",
            message_ordinal: 0,
            body: "hello",
            created_at_ms: 100,
            updated_at_ms: 110,
        },
    )
    .await;
    insert_assistant_item(
        database,
        AssistantItemSeed {
            item_id: "item-2",
            turn_id: "turn-2",
            ordinal: 3,
            revision: 1,
            lifecycle: EntityLifecycle::Streaming,
            run_id: "run-2",
            phase: RenderPhase::Commentary,
            origin_message_id: "prompt-2",
            origin_message_ordinal: 1,
            body: "thinking",
            created_at_ms: 200,
            updated_at_ms: 260,
            key_byte: 2,
        },
    )
    .await;
    insert_user_item(
        database,
        UserItemSeed {
            item_id: "item-3",
            turn_id: "turn-3",
            ordinal: 5,
            revision: 2,
            lifecycle: EntityLifecycle::Completed,
            message_id: "message-3",
            message_ordinal: 2,
            body: "world",
            created_at_ms: 300,
            updated_at_ms: 360,
        },
    )
    .await;
    insert_assistant_item(
        database,
        AssistantItemSeed {
            item_id: "item-4",
            turn_id: "turn-4",
            ordinal: 7,
            revision: 4,
            lifecycle: EntityLifecycle::Completed,
            run_id: "run-4",
            phase: RenderPhase::Final,
            origin_message_id: "prompt-4",
            origin_message_ordinal: 3,
            body: "done",
            created_at_ms: 400,
            updated_at_ms: 500,
            key_byte: 4,
        },
    )
    .await;
    entities::conversation_state::ActiveModel {
        thread_id: Set(THREAD_ID.to_owned()),
        next_renderer_ordinal: Set(8),
        last_patch_sequence: Set(42),
        updated_at_ms: Set(900),
    }
    .insert(database)
    .await
    .expect("conversation state should insert");
}

fn assert_corrupt(error: &RepositoryError, table: &'static str, field: &'static str) {
    assert!(matches!(
        error,
        RepositoryError::CorruptData {
            table: actual_table,
            field: actual_field,
            ..
        } if *actual_table == table && *actual_field == field
    ));
}

#[tokio::test]
async fn missing_thread_is_typed_not_found() {
    let (_database, repository) = memory_database().await;
    let error = repository
        .read_conversation_snapshot(&ConversationQuery {
            thread_id: ThreadId::parse("missing-thread").expect("thread id should parse"),
            bounds: window(1).bounds,
        })
        .await
        .expect_err("missing thread should fail");
    assert!(matches!(
        error,
        RepositoryError::ThreadNotFound { thread_id } if thread_id.as_str() == "missing-thread"
    ));
}

#[tokio::test]
async fn never_launched_thread_is_empty_without_creating_state() {
    let (database, repository) = memory_database().await;
    seed_thread(&database, 777).await;
    let before = entities::conversation_state::Entity::find()
        .all(&database)
        .await
        .expect("state query should work");

    let snapshot = repository
        .read_conversation_snapshot(&window(4))
        .await
        .expect("never-launched thread should have an empty snapshot");
    assert_eq!(snapshot.thread_id().as_str(), THREAD_ID);
    assert_eq!(snapshot.cursor().get(), 0);
    assert!(snapshot.turns().is_empty());
    assert!(snapshot.items().is_empty());
    assert_eq!(snapshot.updated_at(), UnixMillis::from_millis(777));
    let after = entities::conversation_state::Entity::find()
        .all(&database)
        .await
        .expect("state query should work");
    assert_eq!(after, before);
}

#[tokio::test]
async fn window_is_sql_bounded_and_items_follow_only_selected_turns() {
    let (database, repository) = memory_database().await;
    seed_projection(&database).await;
    database
        .execute_unprepared("UPDATE conversation_items SET body = '' WHERE item_id = 'item-1'")
        .await
        .expect("old item corruption should be writable for the fixture");

    let snapshot = repository
        .read_conversation_snapshot(&window(2))
        .await
        .expect("newest bounded turns should be readable");
    assert_eq!(
        snapshot
            .turns()
            .iter()
            .map(|turn| turn.turn_id.as_str())
            .collect::<Vec<_>>(),
        vec!["turn-3", "turn-4"]
    );
    assert_eq!(
        snapshot
            .items()
            .iter()
            .map(ConversationItem::item_id)
            .map(artisan_domain::ItemId::as_str)
            .collect::<Vec<_>>(),
        vec!["item-3", "item-4"]
    );
    assert_eq!(snapshot.cursor().get(), 42);
    assert_eq!(snapshot.updated_at(), UnixMillis::from_millis(900));
}

#[tokio::test]
async fn range_is_strict_before_inclusive_at_floor_and_count_bounded() {
    let (database, repository) = memory_database().await;
    seed_projection(&database).await;

    let slice = repository
        .read_conversation_snapshot(&range(6, Some(2), 2))
        .await
        .expect("bounded range should be readable");
    assert_eq!(
        slice
            .turns()
            .iter()
            .map(|turn| turn.ordinal.get())
            .collect::<Vec<_>>(),
        vec![2, 4]
    );
    assert_eq!(
        slice
            .items()
            .iter()
            .map(|item| item.ordinal().get())
            .collect::<Vec<_>>(),
        vec![3, 5]
    );

    let newest_before_upper_bound = repository
        .read_conversation_snapshot(&range(8, Some(0), 1))
        .await
        .expect("maximum count should be honored");
    assert_eq!(
        newest_before_upper_bound
            .turns()
            .iter()
            .map(|turn| turn.turn_id.as_str())
            .collect::<Vec<_>>(),
        vec!["turn-4"]
    );

    let strict = repository
        .read_conversation_snapshot(&range(4, None, 4))
        .await
        .expect("strict upper range should be readable");
    assert_eq!(
        strict
            .turns()
            .iter()
            .map(|turn| turn.turn_id.as_str())
            .collect::<Vec<_>>(),
        vec!["turn-1", "turn-2"]
    );
}

#[tokio::test]
async fn representative_user_and_assistant_values_round_trip_from_entities() {
    let (database, repository) = memory_database().await;
    seed_projection(&database).await;
    let snapshot = repository
        .read_conversation_snapshot(&window(4))
        .await
        .expect("projection should be readable");

    let user = match &snapshot.items()[0] {
        ConversationItem::UserMessage(item) => item,
        ConversationItem::AssistantMessage(_) => panic!("first item should be a user message"),
    };
    assert_eq!(user.item_id.as_str(), "item-1");
    assert_eq!(user.turn_id.as_str(), "turn-1");
    assert_eq!(user.ordinal, ItemOrdinal::new(1));
    assert_eq!(user.revision, Revision::new(0));
    assert_eq!(user.lifecycle, ConversationLifecycle::Completed);
    assert_eq!(user.body.as_str(), "hello");
    assert_eq!(user.created_at, UnixMillis::from_millis(100));
    assert_eq!(user.updated_at, UnixMillis::from_millis(110));

    let assistant = match &snapshot.items()[1] {
        ConversationItem::AssistantMessage(item) => item,
        ConversationItem::UserMessage(_) => panic!("second item should be an assistant message"),
    };
    assert_eq!(assistant.item_id.as_str(), "item-2");
    assert_eq!(assistant.turn_id.as_str(), "turn-2");
    assert_eq!(assistant.run_id.as_str(), "run-2");
    assert_eq!(assistant.ordinal, ItemOrdinal::new(3));
    assert_eq!(assistant.revision, Revision::new(1));
    assert_eq!(assistant.lifecycle, ConversationLifecycle::Streaming);
    assert_eq!(assistant.body.as_str(), "thinking");
    assert_eq!(assistant.phase, AssistantMessagePhase::Commentary);
    assert_eq!(assistant.created_at, UnixMillis::from_millis(200));
    assert_eq!(assistant.updated_at, UnixMillis::from_millis(260));
}

#[tokio::test]
async fn malformed_persisted_body_is_rejected_as_corrupt_data() {
    let (database, repository) = memory_database().await;
    seed_projection(&database).await;
    database
        .execute_unprepared("UPDATE conversation_items SET body = '' WHERE item_id = 'item-3'")
        .await
        .expect("fixture body corruption should be writable");

    let error = repository
        .read_conversation_snapshot(&window(2))
        .await
        .expect_err("blank persisted user body should be rejected");
    assert_corrupt(&error, "conversation_items", "body");
}

#[tokio::test]
async fn malformed_persisted_identity_is_rejected_as_corrupt_data() {
    let (database, repository) = memory_database().await;
    seed_projection(&database).await;
    database
        .execute_unprepared("PRAGMA foreign_keys = OFF")
        .await
        .expect("fixture should temporarily disable foreign keys");
    database
        .execute_unprepared(
            "UPDATE conversation_ordinals SET entity_id = 'bad item' \
             WHERE thread_id = 'thread-1' AND ordinal = 5",
        )
        .await
        .expect("fixture ordinal identity should be writable");
    database
        .execute_unprepared(
            "UPDATE conversation_items SET item_id = 'bad item' WHERE item_id = 'item-3'",
        )
        .await
        .expect("fixture item identity should be writable");
    database
        .execute_unprepared("PRAGMA foreign_keys = ON")
        .await
        .expect("fixture should restore foreign keys");

    let error = repository
        .read_conversation_snapshot(&window(2))
        .await
        .expect_err("invalid persisted item identity should be rejected");
    assert_corrupt(&error, "conversation_items", "item_id");
}

#[tokio::test]
async fn malformed_persisted_ordinal_is_rejected_as_corrupt_data() {
    let (database, repository) = memory_database().await;
    seed_projection(&database).await;
    database
        .execute_unprepared(
            "UPDATE conversation_state SET next_renderer_ordinal = 8.5 \
             WHERE thread_id = 'thread-1'",
        )
        .await
        .expect("fractional counter corruption should be writable");

    let error = repository
        .read_conversation_snapshot(&window(2))
        .await
        .expect_err("fractional persisted counter should be rejected");
    assert_corrupt(&error, "conversation_state", "next_renderer_ordinal");
}

#[tokio::test]
async fn file_backed_snapshot_survives_reopen() {
    let temporary = TemporaryDatabase::new();
    let database = connect(
        SqliteConfig::file(temporary.path())
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("file database should open");
    migrate_to_current(&database)
        .await
        .expect("file database should migrate");
    seed_projection(&database).await;
    let expected = Repository::new(database.clone())
        .read_conversation_snapshot(&range(8, Some(2), 2))
        .await
        .expect("file snapshot should read");
    database.close().await.expect("database should close");

    let reopened = connect(
        SqliteConfig::file(temporary.path())
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("file database should reopen");
    migrate_to_current(&reopened)
        .await
        .expect("reopened database migration should be idempotent");
    let actual = Repository::new(reopened.clone())
        .read_conversation_snapshot(&range(8, Some(2), 2))
        .await
        .expect("reopened snapshot should read");
    assert_eq!(actual, expected);
    reopened
        .close()
        .await
        .expect("reopened database should close");
}

struct TemporaryDatabase {
    directory: PathBuf,
    path: PathBuf,
}

impl TemporaryDatabase {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should follow Unix epoch")
            .as_nanos();
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "artisan-conversation-projection-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("temporary directory should be created");
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
