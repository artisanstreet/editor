//! Durable append-only engine-observation ledger rows.
//!
//! One row is appended per committed observation inside the same
//! [`Repository::commit_run_batch`](crate::repository::Repository::commit_run_batch)
//! transaction that persists the batch checkpoint and receipt. Rows are
//! immutable once committed: the `(thread_id, delivery_sequence)` pair is the
//! subscription cursor key, and the `(run_id, observation_sequence)` pair
//! forbids sequence reuse within a run. The observation payload is the
//! canonical single-observation envelope produced by the existing batch codec,
//! so no JSON vocabulary is duplicated here.

use sea_orm::entity::prelude::*;

use super::execution_value::OpaqueBytes;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "observation_ledger")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub thread_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub delivery_sequence: i64,
    pub run_id: String,
    pub observation_sequence: i64,
    pub turn_id: String,
    pub committed_at_ms: i64,
    pub engine: String,
    pub binding_version: i64,
    pub observation_version: i64,
    pub observation_bytes: OpaqueBytes,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::assistant_run::Entity",
        from = "Column::RunId",
        to = "super::assistant_run::Column::RunId",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Run,
    #[sea_orm(
        belongs_to = "super::thread::Entity",
        from = "Column::ThreadId",
        to = "super::thread::Column::ThreadId",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Thread,
    #[sea_orm(
        belongs_to = "super::conversation_turn::Entity",
        from = "Column::TurnId",
        to = "super::conversation_turn::Column::TurnId",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Turn,
}

impl Related<super::assistant_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Run.def()
    }
}

impl Related<super::thread::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Thread.def()
    }
}

impl Related<super::conversation_turn::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Turn.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
