//! Thread persistence model.

use sea_orm::entity::prelude::*;

use super::execution_value::OpaqueBytes;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "threads")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub thread_id: String,
    pub project_id: String,
    pub title: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub engine_run_config_version: Option<i64>,
    pub engine_run_config_revision: i64,
    pub engine_run_config: Option<OpaqueBytes>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::attached_project::Entity",
        from = "Column::ProjectId",
        to = "super::attached_project::Column::ProjectId",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Project,
    #[sea_orm(has_many = "super::message::Entity")]
    Messages,
    #[sea_orm(has_many = "super::command_receipt::Entity")]
    CommandReceipts,
    #[sea_orm(has_one = "super::conversation_state::Entity")]
    ConversationState,
    #[sea_orm(has_many = "super::conversation_ordinal::Entity")]
    ConversationOrdinals,
}

impl Related<super::attached_project::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl Related<super::message::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Messages.def()
    }
}

impl Related<super::command_receipt::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::CommandReceipts.def()
    }
}

impl Related<super::conversation_state::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ConversationState.def()
    }
}

impl Related<super::conversation_ordinal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ConversationOrdinals.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
