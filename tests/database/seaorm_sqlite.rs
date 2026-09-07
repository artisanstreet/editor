//! Phase 1 feasibility proof for `SeaORM` 2 over a real in-memory `SQLite`
//! database.
//!
//! Every test drives the public `artisan_database` connection boundary and a
//! `SeaORM`-derived entity that exists only inside this file. The
//! `proof_task` table is deliberately not product schema: it proves the
//! toolchain (derive macros, bundled `SQLite`, pooled memory databases,
//! transactions) without choosing anything the later schema packets must
//! honor.

use artisan_database::{ConnectError, SqliteConfig, connect};
use sea_orm::sea_query::{ColumnDef, Table};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait, Set,
    TransactionTrait,
};

mod proof_task {
    //! Feasibility entity; owned entirely by this test.

    use sea_orm::entity::prelude::*;

    /// A row proving derived-model round trips.
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "proof_task")]
    pub struct Model {
        /// Synthetic primary key assigned by `SQLite`.
        #[sea_orm(primary_key)]
        pub id: i32,
        /// Free-form label round-tripped through the driver.
        pub label: String,
        /// Boolean column proving typed value conversion.
        pub done: bool,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

use proof_task::{Column, Entity};

/// Opens a fresh shared-cache memory database with statement logging off.
async fn open_proof_database() -> Result<DatabaseConnection, ConnectError> {
    connect(SqliteConfig::in_memory().sqlx_logging(false)).await
}

/// Creates the feasibility table exactly once per database.
async fn create_proof_task_table(db: &DatabaseConnection) -> Result<(), DbErr> {
    let statement = Table::create()
        .table(Entity)
        .col(
            ColumnDef::new(Column::Id)
                .integer()
                .not_null()
                .auto_increment()
                .primary_key(),
        )
        .col(ColumnDef::new(Column::Label).string().not_null())
        .col(ColumnDef::new(Column::Done).boolean().not_null())
        .to_owned();

    db.execute(&statement).await?;
    Ok(())
}

#[tokio::test]
async fn derived_entity_round_trips_insert_update_and_query() {
    let db = open_proof_database().await.expect("memory database opens");
    create_proof_task_table(&db)
        .await
        .expect("feasibility table creates");

    let inserted = proof_task::ActiveModel {
        label: Set("first".to_owned()),
        done: Set(false),
        ..ActiveModelTrait::default()
    }
    .insert(&db)
    .await
    .expect("derived active model inserts");

    assert!(inserted.id > 0, "sqlite assigns the synthetic key");
    assert_eq!(inserted.label, "first");
    assert!(!inserted.done);

    let loaded = Entity::find_by_id(inserted.id)
        .one(&db)
        .await
        .expect("query succeeds")
        .expect("row is present");

    assert_eq!(loaded, inserted);

    let updated = proof_task::ActiveModel {
        id: Set(loaded.id),
        done: Set(true),
        ..ActiveModelTrait::default()
    }
    .update(&db)
    .await
    .expect("update succeeds");

    assert!(updated.done);

    let total = Entity::find().count(&db).await.expect("count succeeds");
    assert_eq!(total, 1, "exactly one committed row exists");

    db.close().await.expect("connection closes cleanly");
}

#[tokio::test]
async fn rolled_back_transaction_leaves_no_committed_row() {
    let db = open_proof_database().await.expect("memory database opens");
    create_proof_task_table(&db)
        .await
        .expect("feasibility table creates");

    let kept = proof_task::ActiveModel {
        label: Set("kept".to_owned()),
        done: Set(false),
        ..ActiveModelTrait::default()
    }
    .insert(&db)
    .await
    .expect("committed baseline row inserts");

    let txn = db.begin().await.expect("transaction begins");

    let ghost = proof_task::ActiveModel {
        label: Set("ghost".to_owned()),
        done: Set(true),
        ..ActiveModelTrait::default()
    }
    .insert(&txn)
    .await
    .expect("row inserts inside the transaction");

    let visible_inside = Entity::find_by_id(ghost.id)
        .one(&txn)
        .await
        .expect("transaction reads its own writes")
        .expect("uncommitted row is visible to its own transaction");
    assert_eq!(visible_inside.label, "ghost");

    txn.rollback().await.expect("rollback succeeds");

    let rolled_back = Entity::find_by_id(ghost.id).one(&db).await;
    match rolled_back {
        Err(error) => panic!("post-rollback query must not fail: {error}"),
        Ok(Some(row)) => panic!("rolled-back row survived commit boundary: {row:?}"),
        Ok(None) => {}
    }

    let total = Entity::find().count(&db).await.expect("count succeeds");
    assert_eq!(total, 1, "only the baseline row was committed");
    assert_ne!(ghost.id, kept.id);

    db.close().await.expect("connection closes cleanly");
}

#[tokio::test]
async fn pooled_concurrent_inserts_land_in_one_shared_database() {
    let db = open_proof_database().await.expect("memory database opens");
    create_proof_task_table(&db)
        .await
        .expect("feasibility table creates");

    let insert_a = proof_task::ActiveModel {
        label: Set("a".to_owned()),
        done: Set(false),
        ..ActiveModelTrait::default()
    }
    .insert(&db);

    let insert_b = proof_task::ActiveModel {
        label: Set("b".to_owned()),
        done: Set(true),
        ..ActiveModelTrait::default()
    }
    .insert(&db);

    let (first, second) = tokio::join!(insert_a, insert_b);
    let first = first.expect("first concurrent insert lands");
    let second = second.expect("second concurrent insert lands");

    let total = Entity::find().count(&db).await.expect("count succeeds");
    assert_eq!(
        total, 2,
        "pool connections share one memory database, not two"
    );
    assert_ne!(first.id, second.id);

    db.close().await.expect("connection closes cleanly");
}

#[tokio::test]
async fn each_connect_call_gets_a_fresh_isolated_memory_database() {
    let first = open_proof_database()
        .await
        .expect("first memory database opens");
    create_proof_task_table(&first)
        .await
        .expect("feasibility table creates in the first database");

    let second = open_proof_database()
        .await
        .expect("second memory database opens");

    let absent_table = Entity::find().one(&second).await;
    assert!(
        absent_table.is_err(),
        "a fresh connect call must not observe the previous database"
    );

    second.close().await.expect("second handle closes cleanly");
    first.close().await.expect("first handle closes cleanly");
}

#[tokio::test]
async fn rejects_pool_configuration_that_cannot_be_satisfied() {
    let zero_min = SqliteConfig::in_memory().min_connections(0);
    let rejected_zero_min = connect(zero_min).await;
    assert!(
        matches!(rejected_zero_min, Err(ConnectError::InvalidConfig { .. })),
        "an in-memory pool must retain at least one resident connection"
    );

    let unbounded_min = SqliteConfig::in_memory().min_connections(8);
    let rejected_min = connect(unbounded_min).await;
    assert!(
        matches!(rejected_min, Err(ConnectError::InvalidConfig { .. })),
        "min_connections above max_connections is rejected before opening"
    );

    let zero_max = SqliteConfig::in_memory().max_connections(0);
    let rejected_zero = connect(zero_max).await;
    assert!(
        matches!(rejected_zero, Err(ConnectError::InvalidConfig { .. })),
        "max_connections of zero is rejected before opening"
    );
}
