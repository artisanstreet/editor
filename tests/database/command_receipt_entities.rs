use artisan_database::entities::{self, CommandKind};
use artisan_database::{SqliteConfig, connect};
use artisan_migrations::migrate_to_current;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, ModelTrait};

#[tokio::test]
async fn command_receipt_entity_round_trips_exact_queue_payload_and_relation() {
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

    entities::attached_project::ActiveModel {
        project_id: Set("project-1".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(100),
    }
    .insert(&database)
    .await
    .expect("project should insert");
    entities::thread::ActiveModel {
        thread_id: Set("thread-1".to_owned()),
        project_id: Set("project-1".to_owned()),
        title: Set("First thread".to_owned()),
        created_at_ms: Set(200),
        updated_at_ms: Set(200),
    }
    .insert(&database)
    .await
    .expect("thread should insert");
    entities::message::ActiveModel {
        message_id: Set("message-1".to_owned()),
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(0),
        body: Set("Hello".to_owned()),
        accepted_at_ms: Set(300),
    }
    .insert(&database)
    .await
    .expect("message should insert");
    entities::command_receipt::ActiveModel {
        request_id: Set("request-1".to_owned()),
        command_kind: Set(CommandKind::QueueFirstMessage),
        directory_id: Set(None),
        project_id: Set(None),
        thread_id: Set(Some("thread-1".to_owned())),
        title: Set(None),
        message_id: Set(Some("message-1".to_owned())),
        body: Set(Some("Hello".to_owned())),
        accepted_at_ms: Set(300),
    }
    .insert(&database)
    .await
    .expect("receipt should insert");

    let receipt = entities::command_receipt::Entity::find_by_id("request-1")
        .one(&database)
        .await
        .expect("receipt query should work")
        .expect("receipt should exist");
    let message = receipt
        .find_related(entities::message::Entity)
        .one(&database)
        .await
        .expect("message relation should work")
        .expect("message should exist");

    assert_eq!(receipt.command_kind, CommandKind::QueueFirstMessage);
    assert_eq!(receipt.thread_id.as_deref(), Some("thread-1"));
    assert_eq!(receipt.body.as_deref(), Some("Hello"));
    assert_eq!(message.message_id, "message-1");
}
