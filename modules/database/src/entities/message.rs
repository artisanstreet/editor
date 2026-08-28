//! Immutable message persistence model.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "messages")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub message_id: String,
    pub thread_id: String,
    pub ordinal: i64,
    pub body: String,
    pub accepted_at_ms: i64,
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
    #[sea_orm(has_one = "super::message_dispatch::Entity")]
    Dispatch,
    #[sea_orm(has_many = "super::command_receipt::Entity")]
    CommandReceipts,
    #[sea_orm(
        has_one = "super::assistant_run::Entity",
        from = "(Column::MessageId, Column::ThreadId)",
        to = "(super::assistant_run::Column::OriginMessageId, super::assistant_run::Column::ThreadId)"
    )]
    AssistantRun,
    #[sea_orm(
        has_one = "super::conversation_item::Entity",
        from = "(Column::MessageId, Column::ThreadId)",
        to = "(super::conversation_item::Column::SourceMessageId, super::conversation_item::Column::ThreadId)"
    )]
    ConversationItem,
}

impl Related<super::thread::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Thread.def()
    }
}

impl Related<super::message_dispatch::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Dispatch.def()
    }
}

impl Related<super::command_receipt::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::CommandReceipts.def()
    }
}

impl Related<super::assistant_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AssistantRun.def()
    }
}

impl Related<super::conversation_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ConversationItem.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
