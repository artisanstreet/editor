use artisan_database::entities::{
    self, AssistantRunLifecycle, ConversationItemKind, ConversationPatchKind, EntityLifecycle,
    OpaqueBytes, OrdinalKind, RenderPhase,
};
use artisan_database::{SqliteConfig, connect};
use artisan_migrations::migrate_to_current;
use sea_orm::{
    ActiveEnum, ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait, Iterable,
    ModelTrait, RelationTrait,
};

fn assert_string_enum<E>(expected: &[&str])
where
    E: ActiveEnum<Value = String>,
{
    assert_eq!(
        E::values(),
        expected.iter().map(ToString::to_string).collect::<Vec<_>>()
    );
    assert!(E::try_from_value(&"not-a-schema-value".to_owned()).is_err());
}

#[test]
fn execution_values_match_schema_and_redact_opaque_bytes() {
    assert_string_enum::<OrdinalKind>(&["turn", "item"]);
    assert_string_enum::<EntityLifecycle>(&[
        "pending",
        "streaming",
        "active",
        "waiting",
        "completed",
        "failed",
        "interrupted",
        "cancelled",
    ]);
    assert_string_enum::<AssistantRunLifecycle>(&[
        "queued",
        "launching",
        "running",
        "waiting",
        "cancel_requested",
        "interrupted",
        "completed",
        "failed",
        "cancelled",
    ]);
    assert_string_enum::<ConversationItemKind>(&["user_message", "assistant_message"]);
    assert_string_enum::<RenderPhase>(&["commentary", "final", "unspecified"]);
    assert_string_enum::<ConversationPatchKind>(&[
        "turn_upsert",
        "item_upsert",
        "item_append",
        "item_lifecycle",
        "turn_lifecycle",
    ]);

    let sentinel = b"e1-opaque-sentinel-material".to_vec();
    let opaque = OpaqueBytes::new(sentinel.clone());
    let formatted = format!("{opaque:?}");
    assert!(formatted.contains("OpaqueBytes"));
    assert!(formatted.contains(&sentinel.len().to_string()));
    assert!(!formatted.contains("e1-opaque-sentinel-material"));
    assert!(!formatted.contains(&format!("{sentinel:?}")));
    assert_eq!(opaque.as_slice(), sentinel);
    assert_eq!(opaque.into_vec(), sentinel);
}

struct CoreGraph {
    thread: entities::thread::Model,
    state: entities::conversation_state::Model,
    ordinal: entities::conversation_ordinal::Model,
    turn: entities::conversation_turn::Model,
}

async fn seed_core_graph(database: &DatabaseConnection) -> CoreGraph {
    entities::attached_project::ActiveModel {
        project_id: Set("project-1".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(10),
    }
    .insert(database)
    .await
    .expect("project should insert");
    let thread = entities::thread::ActiveModel {
        thread_id: Set("thread-1".to_owned()),
        project_id: Set("project-1".to_owned()),
        title: Set("Entity test".to_owned()),
        created_at_ms: Set(20),
        updated_at_ms: Set(20),
    }
    .insert(database)
    .await
    .expect("thread should insert");
    let state = entities::conversation_state::ActiveModel {
        thread_id: Set("thread-1".to_owned()),
        next_renderer_ordinal: Set(8),
        last_patch_sequence: Set(3),
        updated_at_ms: Set(30),
    }
    .insert(database)
    .await
    .expect("conversation state should insert");
    let ordinal = entities::conversation_ordinal::ActiveModel {
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(7),
        kind: Set(OrdinalKind::Turn),
        entity_id: Set("turn-1".to_owned()),
    }
    .insert(database)
    .await
    .expect("ordinal should insert");
    let turn = entities::conversation_turn::ActiveModel {
        turn_id: Set("turn-1".to_owned()),
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(7),
        kind: Set(OrdinalKind::Turn),
        revision: Set(2),
        lifecycle: Set(EntityLifecycle::Active),
        created_at_ms: Set(40),
        updated_at_ms: Set(41),
    }
    .insert(database)
    .await
    .expect("turn should insert");

    CoreGraph {
        thread,
        state,
        ordinal,
        turn,
    }
}

#[tokio::test]
async fn core_execution_entities_round_trip_with_exact_relations() {
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
    let CoreGraph {
        thread,
        state,
        ordinal,
        turn,
    } = seed_core_graph(&database).await;

    let loaded_state = entities::conversation_state::Entity::find_by_id("thread-1")
        .one(&database)
        .await
        .expect("state query should work")
        .expect("state should exist");
    assert_eq!(loaded_state, state);

    let mut ordinal_primary_key = entities::conversation_ordinal::PrimaryKey::iter();
    assert!(matches!(
        ordinal_primary_key.next(),
        Some(entities::conversation_ordinal::PrimaryKey::ThreadId)
    ));
    assert!(matches!(
        ordinal_primary_key.next(),
        Some(entities::conversation_ordinal::PrimaryKey::Ordinal)
    ));
    assert!(ordinal_primary_key.next().is_none());

    let loaded_ordinal =
        entities::conversation_ordinal::Entity::find_by_id(("thread-1".to_owned(), 7))
            .one(&database)
            .await
            .expect("ordinal query should work")
            .expect("ordinal should exist");
    assert_eq!(loaded_ordinal, ordinal);

    let loaded_turn = entities::conversation_turn::Entity::find_by_id("turn-1")
        .one(&database)
        .await
        .expect("turn query should work")
        .expect("turn should exist");
    assert_eq!(loaded_turn, turn);

    let relation = entities::conversation_turn::Relation::Ordinal.def();
    assert_eq!(relation.from_col.arity(), 4);
    assert_eq!(relation.to_col.arity(), 4);
    assert_eq!(
        relation
            .from_col
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["thread_id", "ordinal", "turn_id", "kind"]
    );
    assert_eq!(
        relation
            .to_col
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["thread_id", "ordinal", "entity_id", "kind"]
    );

    let related_ordinal = loaded_turn
        .find_related(entities::conversation_ordinal::Entity)
        .one(&database)
        .await
        .expect("turn-to-ordinal relation should query")
        .expect("turn should resolve its ordinal");
    assert_eq!(related_ordinal, ordinal);
    let related_turn = loaded_ordinal
        .find_related(entities::conversation_turn::Entity)
        .one(&database)
        .await
        .expect("ordinal-to-turn relation should query")
        .expect("ordinal should resolve its turn");
    assert_eq!(related_turn, turn);

    let related_state = thread
        .find_related(entities::conversation_state::Entity)
        .one(&database)
        .await
        .expect("thread-to-state relation should query")
        .expect("thread should resolve its state");
    assert_eq!(related_state, state);
    let related_ordinals = thread
        .find_related(entities::conversation_ordinal::Entity)
        .all(&database)
        .await
        .expect("thread-to-ordinal relation should query");
    assert_eq!(related_ordinals, [ordinal]);
}
