use artisan_database::entities::{
    self, AssistantRunLifecycle, ConversationItemKind, EntityLifecycle, OpaqueBytes, OrdinalKind,
    RenderPhase,
};
use artisan_database::{SqliteConfig, connect};
use artisan_migrations::migrate_to_current;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait, ModelTrait, RelationDef,
    RelationTrait,
};

fn assert_relation_columns(relation: &RelationDef, from: &[&str], to: &[&str]) {
    let actual_from = relation
        .from_col
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let actual_to = relation
        .to_col
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    assert_eq!(
        actual_from,
        from.iter().map(ToString::to_string).collect::<Vec<_>>()
    );
    assert_eq!(
        actual_to,
        to.iter().map(ToString::to_string).collect::<Vec<_>>()
    );
}

struct RunItemGraph {
    message: entities::message::Model,
    turn: entities::conversation_turn::Model,
    run: entities::assistant_run::Model,
    user_item: entities::conversation_item::Model,
    assistant_item: entities::conversation_item::Model,
    assistant_ordinal: entities::conversation_ordinal::Model,
}

async fn seed_parents(
    database: &DatabaseConnection,
) -> (entities::message::Model, entities::conversation_turn::Model) {
    entities::attached_project::ActiveModel {
        project_id: Set("project-1".to_owned()),
        root_path: Set("C:/repos/artisan".to_owned()),
        display_name: Set("Artisan".to_owned()),
        attached_at_ms: Set(10),
    }
    .insert(database)
    .await
    .expect("project should insert");
    entities::thread::ActiveModel {
        thread_id: Set("thread-1".to_owned()),
        project_id: Set("project-1".to_owned()),
        title: Set("Run and item entities".to_owned()),
        created_at_ms: Set(20),
        updated_at_ms: Set(20),
        engine_run_config_version: Set(None),
        engine_run_config_revision: Set(0),
        engine_run_config: Set(None),
    }
    .insert(database)
    .await
    .expect("thread should insert");
    let message = entities::message::ActiveModel {
        message_id: Set("message-1".to_owned()),
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(0),
        body: Set("Build the feature".to_owned()),
        accepted_at_ms: Set(30),
    }
    .insert(database)
    .await
    .expect("message should insert");
    entities::conversation_ordinal::ActiveModel {
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(1),
        kind: Set(OrdinalKind::Turn),
        entity_id: Set("turn-1".to_owned()),
    }
    .insert(database)
    .await
    .expect("turn ordinal should insert");
    let turn = entities::conversation_turn::ActiveModel {
        turn_id: Set("turn-1".to_owned()),
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(1),
        kind: Set(OrdinalKind::Turn),
        revision: Set(0),
        lifecycle: Set(EntityLifecycle::Active),
        created_at_ms: Set(40),
        updated_at_ms: Set(40),
    }
    .insert(database)
    .await
    .expect("turn should insert");
    (message, turn)
}

async fn seed_run(database: &DatabaseConnection) -> entities::assistant_run::Model {
    entities::assistant_run::ActiveModel {
        run_id: Set("run-1".to_owned()),
        thread_id: Set("thread-1".to_owned()),
        run_start_key: Set(OpaqueBytes::new(vec![0xa5; 32])),
        origin_message_id: Set("message-1".to_owned()),
        origin_turn_id: Set("turn-1".to_owned()),
        lifecycle: Set(AssistantRunLifecycle::Queued),
        generation: Set(0),
        owner: Set(None),
        lease: Set(None),
        claim_token: Set(None),
        provider_binding_version: Set(None),
        provider_binding: Set(None),
        provider_bound_at_ms: Set(None),
        error_code: Set(None),
        error_message: Set(None),
        created_at_ms: Set(50),
        updated_at_ms: Set(50),
        terminal_at_ms: Set(None),
        engine_run_config_version: Set(Some(1)),
        engine_run_config_revision: Set(Some(1)),
        engine_run_config: Set(Some(OpaqueBytes::new(
            br#"{"version":1,"engine":"opencode2","profile_id":"profile-fixture","model_id":"model-fixture","route_id":"route-fixture","variant_id":null,"permission":{"permission_id":"permission-fixture","agent_id":"agent-fixture","approval":"on_request","filesystem":"workspace","network":"enabled","web_search":"disabled"},"runtime":{"attempt_budget_ms":100,"readiness_budget_ms":1,"health_budget_ms":1,"prompt_budget_ms":1,"stream_budget_ms":1,"close_budget_ms":1,"max_json_body_bytes":8192,"max_sse_line_bytes":4096,"max_sse_event_bytes":8192,"max_readiness_line_bytes":4096,"max_header_count":8,"max_http_buffer_bytes":8192,"max_stderr_bytes":4096,"observation_capacity":16}}"#.to_vec(),
        ))),
    }
    .insert(database)
    .await
    .expect("assistant run should insert")
}

async fn seed_item(
    database: &DatabaseConnection,
    item_id: &str,
    ordinal: i64,
    item_kind: ConversationItemKind,
    source_message_id: Option<&str>,
    run_id: Option<&str>,
    phase: Option<RenderPhase>,
) -> (
    entities::conversation_ordinal::Model,
    entities::conversation_item::Model,
) {
    let ordinal_model = entities::conversation_ordinal::ActiveModel {
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(ordinal),
        kind: Set(OrdinalKind::Item),
        entity_id: Set(item_id.to_owned()),
    }
    .insert(database)
    .await
    .expect("item ordinal should insert");
    let item = entities::conversation_item::ActiveModel {
        item_id: Set(item_id.to_owned()),
        thread_id: Set("thread-1".to_owned()),
        turn_id: Set("turn-1".to_owned()),
        ordinal: Set(ordinal),
        kind: Set(OrdinalKind::Item),
        revision: Set(0),
        lifecycle: Set(EntityLifecycle::Active),
        item_kind: Set(item_kind),
        source_message_id: Set(source_message_id.map(str::to_owned)),
        run_id: Set(run_id.map(str::to_owned)),
        native_item_key: Set(run_id.map(|_| format!("native-{item_id}"))),
        phase: Set(phase),
        body: Set(format!("body for {item_id}")),
        created_at_ms: Set(60 + ordinal),
        updated_at_ms: Set(60 + ordinal),
    }
    .insert(database)
    .await
    .expect("conversation item should insert");
    (ordinal_model, item)
}

async fn seed_graph(database: &DatabaseConnection) -> RunItemGraph {
    let (message, turn) = seed_parents(database).await;
    let run = seed_run(database).await;
    let (_, user_item) = seed_item(
        database,
        "user-item-1",
        2,
        ConversationItemKind::UserMessage,
        Some("message-1"),
        None,
        None,
    )
    .await;
    let (assistant_ordinal, assistant_item) = seed_item(
        database,
        "assistant-item-1",
        3,
        ConversationItemKind::AssistantMessage,
        None,
        Some("run-1"),
        Some(RenderPhase::Commentary),
    )
    .await;
    RunItemGraph {
        message,
        turn,
        run,
        user_item,
        assistant_item,
        assistant_ordinal,
    }
}

fn assert_exact_relation_definitions() {
    let relations = [
        (
            entities::assistant_run::Relation::OriginMessage.def(),
            &["origin_message_id", "thread_id"][..],
            &["message_id", "thread_id"][..],
        ),
        (
            entities::message::Relation::AssistantRun.def(),
            &["message_id", "thread_id"],
            &["origin_message_id", "thread_id"],
        ),
        (
            entities::assistant_run::Relation::OriginTurn.def(),
            &["origin_turn_id", "thread_id"],
            &["turn_id", "thread_id"],
        ),
        (
            entities::conversation_turn::Relation::AssistantRun.def(),
            &["turn_id", "thread_id"],
            &["origin_turn_id", "thread_id"],
        ),
        (
            entities::conversation_item::Relation::Ordinal.def(),
            &["thread_id", "ordinal", "item_id", "kind"],
            &["thread_id", "ordinal", "entity_id", "kind"],
        ),
        (
            entities::conversation_ordinal::Relation::Item.def(),
            &["thread_id", "ordinal", "entity_id", "kind"],
            &["thread_id", "ordinal", "item_id", "kind"],
        ),
        (
            entities::conversation_item::Relation::Turn.def(),
            &["turn_id", "thread_id"],
            &["turn_id", "thread_id"],
        ),
        (
            entities::conversation_turn::Relation::Items.def(),
            &["turn_id", "thread_id"],
            &["turn_id", "thread_id"],
        ),
        (
            entities::conversation_item::Relation::SourceMessage.def(),
            &["source_message_id", "thread_id"],
            &["message_id", "thread_id"],
        ),
        (
            entities::message::Relation::ConversationItem.def(),
            &["message_id", "thread_id"],
            &["source_message_id", "thread_id"],
        ),
        (
            entities::conversation_item::Relation::Run.def(),
            &["run_id", "thread_id"],
            &["run_id", "thread_id"],
        ),
        (
            entities::assistant_run::Relation::Items.def(),
            &["run_id", "thread_id"],
            &["run_id", "thread_id"],
        ),
    ];
    for (relation, from, to) in relations {
        assert_relation_columns(&relation, from, to);
    }
}

async fn assert_run_parent_relations(
    database: &DatabaseConnection,
    graph: &RunItemGraph,
    loaded_run: &entities::assistant_run::Model,
) {
    assert_eq!(
        loaded_run
            .find_related(entities::message::Entity)
            .one(database)
            .await
            .expect("run origin message relation should query"),
        Some(graph.message.clone())
    );
    assert_eq!(
        graph
            .message
            .find_related(entities::assistant_run::Entity)
            .one(database)
            .await
            .expect("message run relation should query"),
        Some(graph.run.clone())
    );
    assert_eq!(
        loaded_run
            .find_related(entities::conversation_turn::Entity)
            .one(database)
            .await
            .expect("run origin turn relation should query"),
        Some(graph.turn.clone())
    );
    assert_eq!(
        graph
            .turn
            .find_related(entities::assistant_run::Entity)
            .one(database)
            .await
            .expect("turn run relation should query"),
        Some(graph.run.clone())
    );
}

async fn assert_optional_item_relations(
    database: &DatabaseConnection,
    graph: &RunItemGraph,
    user_item: &entities::conversation_item::Model,
    assistant_item: &entities::conversation_item::Model,
) {
    assert_eq!(
        user_item
            .find_related(entities::message::Entity)
            .one(database)
            .await
            .expect("user item source relation should query"),
        Some(graph.message.clone())
    );
    assert!(
        user_item
            .find_related(entities::assistant_run::Entity)
            .one(database)
            .await
            .expect("null user item run relation should query")
            .is_none()
    );
    assert!(
        assistant_item
            .find_related(entities::message::Entity)
            .one(database)
            .await
            .expect("null assistant item source relation should query")
            .is_none()
    );
    assert_eq!(
        assistant_item
            .find_related(entities::assistant_run::Entity)
            .one(database)
            .await
            .expect("assistant item run relation should query"),
        Some(graph.run.clone())
    );
}

async fn assert_item_inverse_relations(database: &DatabaseConnection, graph: &RunItemGraph) {
    assert_eq!(
        graph
            .assistant_ordinal
            .find_related(entities::conversation_item::Entity)
            .one(database)
            .await
            .expect("ordinal item relation should query"),
        Some(graph.assistant_item.clone())
    );

    let mut turn_item_ids = graph
        .turn
        .find_related(entities::conversation_item::Entity)
        .all(database)
        .await
        .expect("turn items relation should query")
        .into_iter()
        .map(|item| item.item_id)
        .collect::<Vec<_>>();
    turn_item_ids.sort();
    assert_eq!(turn_item_ids, ["assistant-item-1", "user-item-1"]);
    let run_items = graph
        .run
        .find_related(entities::conversation_item::Entity)
        .all(database)
        .await
        .expect("run items relation should query");
    assert_eq!(
        run_items.as_slice(),
        std::slice::from_ref(&graph.assistant_item)
    );
}

#[tokio::test]
async fn run_and_item_entities_round_trip_with_exact_bidirectional_relations() {
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
    let graph = seed_graph(&database).await;

    let loaded_run = entities::assistant_run::Entity::find_by_id("run-1")
        .one(&database)
        .await
        .expect("run query should work")
        .expect("run should exist");
    let loaded_user_item = entities::conversation_item::Entity::find_by_id("user-item-1")
        .one(&database)
        .await
        .expect("user item query should work")
        .expect("user item should exist");
    let loaded_assistant_item = entities::conversation_item::Entity::find_by_id("assistant-item-1")
        .one(&database)
        .await
        .expect("assistant item query should work")
        .expect("assistant item should exist");
    assert_eq!(loaded_run, graph.run);
    assert_eq!(loaded_user_item, graph.user_item);
    assert_eq!(loaded_assistant_item, graph.assistant_item);

    let raw_key = vec![0xa5; 32];
    let formatted_run = format!("{loaded_run:?}");
    assert!(formatted_run.contains("OpaqueBytes"));
    assert!(!formatted_run.contains(&format!("{raw_key:?}")));
    assert_exact_relation_definitions();

    assert_run_parent_relations(&database, &graph, &loaded_run).await;
    assert_optional_item_relations(&database, &graph, &loaded_user_item, &loaded_assistant_item)
        .await;
    assert_item_inverse_relations(&database, &graph).await;
}
