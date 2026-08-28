use std::fmt::Debug;

use artisan_database::entities::{
    self, AssistantRunLifecycle, ConversationItemKind, ConversationPatchKind, EntityLifecycle,
    OpaqueBytes, OrdinalKind, RenderPhase,
};
use artisan_database::{SqliteConfig, connect};
use artisan_migrations::migrate_to_current;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait, Iterable, ModelTrait,
    RelationDef, RelationTrait,
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

fn assert_model_redacts_bytes(model: &impl Debug, bytes: &[u8]) {
    let formatted = format!("{model:?}");
    assert!(formatted.contains("OpaqueBytes"));
    assert!(!formatted.contains(&format!("{bytes:?}")));
}

struct ParentGraph {
    turn: entities::conversation_turn::Model,
    item: entities::conversation_item::Model,
    run: entities::assistant_run::Model,
}

struct ExecutionGraph {
    parents: ParentGraph,
    turn_patch: entities::conversation_patch::Model,
    item_patch: entities::conversation_patch::Model,
    checkpoint: entities::run_checkpoint::Model,
    receipt: entities::run_batch_receipt::Model,
}

async fn seed_project_thread_message(database: &DatabaseConnection) {
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
        title: Set("Patch and checkpoint entities".to_owned()),
        created_at_ms: Set(20),
        updated_at_ms: Set(20),
    }
    .insert(database)
    .await
    .expect("thread should insert");
    entities::message::ActiveModel {
        message_id: Set("message-1".to_owned()),
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(0),
        body: Set("Continue".to_owned()),
        accepted_at_ms: Set(30),
    }
    .insert(database)
    .await
    .expect("message should insert");
}

async fn seed_turn(database: &DatabaseConnection) -> entities::conversation_turn::Model {
    entities::conversation_ordinal::ActiveModel {
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(1),
        kind: Set(OrdinalKind::Turn),
        entity_id: Set("turn-1".to_owned()),
    }
    .insert(database)
    .await
    .expect("turn ordinal should insert");
    entities::conversation_turn::ActiveModel {
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
    .expect("turn should insert")
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
    }
    .insert(database)
    .await
    .expect("run should insert")
}

async fn seed_item(database: &DatabaseConnection) -> entities::conversation_item::Model {
    entities::conversation_ordinal::ActiveModel {
        thread_id: Set("thread-1".to_owned()),
        ordinal: Set(2),
        kind: Set(OrdinalKind::Item),
        entity_id: Set("item-1".to_owned()),
    }
    .insert(database)
    .await
    .expect("item ordinal should insert");
    entities::conversation_item::ActiveModel {
        item_id: Set("item-1".to_owned()),
        thread_id: Set("thread-1".to_owned()),
        turn_id: Set("turn-1".to_owned()),
        ordinal: Set(2),
        kind: Set(OrdinalKind::Item),
        revision: Set(0),
        lifecycle: Set(EntityLifecycle::Active),
        item_kind: Set(ConversationItemKind::AssistantMessage),
        source_message_id: Set(None),
        run_id: Set(Some("run-1".to_owned())),
        native_item_key: Set(Some("native-item-1".to_owned())),
        phase: Set(Some(RenderPhase::Final)),
        body: Set("Finished".to_owned()),
        created_at_ms: Set(60),
        updated_at_ms: Set(60),
    }
    .insert(database)
    .await
    .expect("item should insert")
}

async fn seed_parents(database: &DatabaseConnection) -> ParentGraph {
    seed_project_thread_message(database).await;
    let turn = seed_turn(database).await;
    let run = seed_run(database).await;
    let item = seed_item(database).await;
    ParentGraph { turn, item, run }
}

async fn seed_patches(
    database: &DatabaseConnection,
) -> (
    entities::conversation_patch::Model,
    entities::conversation_patch::Model,
) {
    let turn_patch = entities::conversation_patch::ActiveModel {
        patch_id: Set("patch-turn-1".to_owned()),
        thread_id: Set("thread-1".to_owned()),
        sequence: Set(1),
        kind: Set(ConversationPatchKind::TurnUpsert),
        revision: Set(0),
        recorded_at_ms: Set(70),
        turn_id: Set(Some("turn-1".to_owned())),
        item_id: Set(None),
        ordinal: Set(Some(1)),
        lifecycle: Set(Some(EntityLifecycle::Active)),
        item_kind: Set(None),
        run_id: Set(None),
        phase: Set(None),
        body: Set(None),
        fragment: Set(None),
        entity_created_at_ms: Set(Some(40)),
        entity_updated_at_ms: Set(Some(40)),
    }
    .insert(database)
    .await
    .expect("turn patch should insert");
    let item_patch = entities::conversation_patch::ActiveModel {
        patch_id: Set("patch-item-1".to_owned()),
        thread_id: Set("thread-1".to_owned()),
        sequence: Set(2),
        kind: Set(ConversationPatchKind::ItemUpsert),
        revision: Set(0),
        recorded_at_ms: Set(71),
        turn_id: Set(Some("turn-1".to_owned())),
        item_id: Set(Some("item-1".to_owned())),
        ordinal: Set(Some(2)),
        lifecycle: Set(Some(EntityLifecycle::Active)),
        item_kind: Set(Some(ConversationItemKind::AssistantMessage)),
        run_id: Set(Some("run-1".to_owned())),
        phase: Set(Some(RenderPhase::Final)),
        body: Set(Some("Finished".to_owned())),
        fragment: Set(None),
        entity_created_at_ms: Set(Some(60)),
        entity_updated_at_ms: Set(Some(60)),
    }
    .insert(database)
    .await
    .expect("item patch should insert");
    (turn_patch, item_patch)
}

async fn seed_checkpoint(database: &DatabaseConnection) -> entities::run_checkpoint::Model {
    entities::run_checkpoint::ActiveModel {
        run_id: Set("run-1".to_owned()),
        generation: Set(0),
        last_batch_sequence: Set(1),
        engine_checkpoint_version: Set(Some(1)),
        engine_checkpoint_blob: Set(Some(OpaqueBytes::new(vec![0xc3; 16]))),
        updated_at_ms: Set(80),
    }
    .insert(database)
    .await
    .expect("checkpoint should insert")
}

async fn seed_receipt(database: &DatabaseConnection) -> entities::run_batch_receipt::Model {
    entities::run_batch_receipt::ActiveModel {
        run_id: Set("run-1".to_owned()),
        batch_sequence: Set(1),
        generation: Set(0),
        digest: Set(OpaqueBytes::new(vec![0xd4; 32])),
        committed: Set(true),
    }
    .insert(database)
    .await
    .expect("batch receipt should insert")
}

async fn seed_graph(database: &DatabaseConnection) -> ExecutionGraph {
    let parents = seed_parents(database).await;
    let (turn_patch, item_patch) = seed_patches(database).await;
    let checkpoint = seed_checkpoint(database).await;
    let receipt = seed_receipt(database).await;
    ExecutionGraph {
        parents,
        turn_patch,
        item_patch,
        checkpoint,
        receipt,
    }
}

fn assert_exact_relation_definitions() {
    let relations = [
        (
            entities::conversation_patch::Relation::Turn.def(),
            &["turn_id", "thread_id"][..],
            &["turn_id", "thread_id"][..],
        ),
        (
            entities::conversation_turn::Relation::Patches.def(),
            &["turn_id", "thread_id"],
            &["turn_id", "thread_id"],
        ),
        (
            entities::conversation_patch::Relation::Item.def(),
            &["item_id", "thread_id"],
            &["item_id", "thread_id"],
        ),
        (
            entities::conversation_item::Relation::Patches.def(),
            &["item_id", "thread_id"],
            &["item_id", "thread_id"],
        ),
        (
            entities::conversation_patch::Relation::Run.def(),
            &["run_id", "thread_id"],
            &["run_id", "thread_id"],
        ),
        (
            entities::assistant_run::Relation::Patches.def(),
            &["run_id", "thread_id"],
            &["run_id", "thread_id"],
        ),
        (
            entities::run_checkpoint::Relation::Run.def(),
            &["run_id"],
            &["run_id"],
        ),
        (
            entities::assistant_run::Relation::Checkpoint.def(),
            &["run_id"],
            &["run_id"],
        ),
        (
            entities::run_batch_receipt::Relation::Run.def(),
            &["run_id"],
            &["run_id"],
        ),
        (
            entities::assistant_run::Relation::BatchReceipts.def(),
            &["run_id"],
            &["run_id"],
        ),
    ];
    for (relation, from, to) in relations {
        assert_relation_columns(&relation, from, to);
    }
}

async fn assert_patch_relations(database: &DatabaseConnection, graph: &ExecutionGraph) {
    assert_eq!(
        graph
            .item_patch
            .find_related(entities::conversation_turn::Entity)
            .one(database)
            .await
            .expect("patch turn relation should query"),
        Some(graph.parents.turn.clone())
    );
    assert_eq!(
        graph
            .item_patch
            .find_related(entities::conversation_item::Entity)
            .one(database)
            .await
            .expect("patch item relation should query"),
        Some(graph.parents.item.clone())
    );
    assert_eq!(
        graph
            .item_patch
            .find_related(entities::assistant_run::Entity)
            .one(database)
            .await
            .expect("patch run relation should query"),
        Some(graph.parents.run.clone())
    );
    assert!(
        graph
            .turn_patch
            .find_related(entities::conversation_item::Entity)
            .one(database)
            .await
            .expect("null patch item relation should query")
            .is_none()
    );
    assert!(
        graph
            .turn_patch
            .find_related(entities::assistant_run::Entity)
            .one(database)
            .await
            .expect("null patch run relation should query")
            .is_none()
    );
}

async fn assert_inverse_relations(database: &DatabaseConnection, graph: &ExecutionGraph) {
    let mut turn_patches = graph
        .parents
        .turn
        .find_related(entities::conversation_patch::Entity)
        .all(database)
        .await
        .expect("turn patches relation should query")
        .into_iter()
        .map(|patch| patch.patch_id)
        .collect::<Vec<_>>();
    turn_patches.sort();
    assert_eq!(turn_patches, ["patch-item-1", "patch-turn-1"]);
    let item_patches = graph
        .parents
        .item
        .find_related(entities::conversation_patch::Entity)
        .all(database)
        .await
        .expect("item patches relation should query");
    assert_eq!(
        item_patches.as_slice(),
        std::slice::from_ref(&graph.item_patch)
    );
    let run_patches = graph
        .parents
        .run
        .find_related(entities::conversation_patch::Entity)
        .all(database)
        .await
        .expect("run patches relation should query");
    assert_eq!(
        run_patches.as_slice(),
        std::slice::from_ref(&graph.item_patch)
    );
}

async fn assert_run_state_relations(database: &DatabaseConnection, graph: &ExecutionGraph) {
    assert_eq!(
        graph
            .checkpoint
            .find_related(entities::assistant_run::Entity)
            .one(database)
            .await
            .expect("checkpoint run relation should query"),
        Some(graph.parents.run.clone())
    );
    assert_eq!(
        graph
            .parents
            .run
            .find_related(entities::run_checkpoint::Entity)
            .one(database)
            .await
            .expect("run checkpoint relation should query"),
        Some(graph.checkpoint.clone())
    );
    assert_eq!(
        graph
            .receipt
            .find_related(entities::assistant_run::Entity)
            .one(database)
            .await
            .expect("receipt run relation should query"),
        Some(graph.parents.run.clone())
    );
    let receipts = graph
        .parents
        .run
        .find_related(entities::run_batch_receipt::Entity)
        .all(database)
        .await
        .expect("run receipts relation should query");
    assert_eq!(receipts.as_slice(), std::slice::from_ref(&graph.receipt));
}

#[tokio::test]
async fn patch_checkpoint_and_receipt_entities_match_migration_contract() {
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

    let loaded_turn_patch = entities::conversation_patch::Entity::find_by_id("patch-turn-1")
        .one(&database)
        .await
        .expect("turn patch query should work")
        .expect("turn patch should exist");
    let loaded_item_patch = entities::conversation_patch::Entity::find_by_id("patch-item-1")
        .one(&database)
        .await
        .expect("item patch query should work")
        .expect("item patch should exist");
    let loaded_checkpoint = entities::run_checkpoint::Entity::find_by_id("run-1")
        .one(&database)
        .await
        .expect("checkpoint query should work")
        .expect("checkpoint should exist");
    let loaded_receipt = entities::run_batch_receipt::Entity::find_by_id(("run-1".to_owned(), 1))
        .one(&database)
        .await
        .expect("receipt query should work")
        .expect("receipt should exist");
    assert_eq!(loaded_turn_patch, graph.turn_patch);
    assert_eq!(loaded_item_patch, graph.item_patch);
    assert_eq!(loaded_checkpoint, graph.checkpoint);
    assert_eq!(loaded_receipt, graph.receipt);

    let mut receipt_primary_key = entities::run_batch_receipt::PrimaryKey::iter();
    assert!(matches!(
        receipt_primary_key.next(),
        Some(entities::run_batch_receipt::PrimaryKey::RunId)
    ));
    assert!(matches!(
        receipt_primary_key.next(),
        Some(entities::run_batch_receipt::PrimaryKey::BatchSequence)
    ));
    assert!(receipt_primary_key.next().is_none());

    assert_model_redacts_bytes(&loaded_checkpoint, &[0xc3; 16]);
    assert_model_redacts_bytes(&loaded_receipt, &[0xd4; 32]);
    assert_exact_relation_definitions();
    assert_patch_relations(&database, &graph).await;
    assert_inverse_relations(&database, &graph).await;
    assert_run_state_relations(&database, &graph).await;
}
