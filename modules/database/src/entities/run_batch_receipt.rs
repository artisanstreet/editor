//! Idempotency receipt for a committed assistant-run batch.

use sea_orm::entity::prelude::*;

use super::execution_value::OpaqueBytes;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "run_batch_receipts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub run_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub batch_sequence: i64,
    pub generation: i64,
    pub digest: OpaqueBytes,
    pub committed: bool,
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
