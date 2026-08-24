//! Attached project persistence model.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "attached_projects")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub project_id: String,
    pub root_path: String,
    pub display_name: String,
    pub attached_at_ms: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::thread::Entity")]
    Threads,
}

impl Related<super::thread::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Threads.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
