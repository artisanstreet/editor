//! Per-thread conversation execution counters.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_state")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub thread_id: String,
    pub next_renderer_ordinal: i64,
    pub last_patch_sequence: i64,
    pub updated_at_ms: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::thread::Entity",
        from = "Column::ThreadId",
        to = "super::thread::Column::ThreadId",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Thread,
}

impl Related<super::thread::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Thread.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
