//! Durable provider-engine checkpoint for an assistant run.

use sea_orm::entity::prelude::*;

use super::execution_value::OpaqueBytes;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "run_checkpoints")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub run_id: String,
    pub generation: i64,
    pub last_batch_sequence: i64,
    pub engine_checkpoint_version: Option<i64>,
    pub engine_checkpoint_blob: Option<OpaqueBytes>,
    pub updated_at_ms: i64,
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
}

impl Related<super::assistant_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Run.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
