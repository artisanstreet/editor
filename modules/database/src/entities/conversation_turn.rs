//! Conversation turn projection.

use sea_orm::entity::prelude::*;

use super::execution_value::{EntityLifecycle, OrdinalKind};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_turns")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub turn_id: String,
    pub thread_id: String,
    pub ordinal: i64,
    pub kind: OrdinalKind,
    pub revision: i64,
    pub lifecycle: EntityLifecycle,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::conversation_ordinal::Entity",
        from = "(Column::ThreadId, Column::Ordinal, Column::TurnId, Column::Kind)",
        to = "(super::conversation_ordinal::Column::ThreadId, super::conversation_ordinal::Column::Ordinal, super::conversation_ordinal::Column::EntityId, super::conversation_ordinal::Column::Kind)",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Ordinal,
    #[sea_orm(
        has_one = "super::assistant_run::Entity",
        from = "(Column::TurnId, Column::ThreadId)",
        to = "(super::assistant_run::Column::OriginTurnId, super::assistant_run::Column::ThreadId)"
    )]
    AssistantRun,
    #[sea_orm(
        has_many = "super::conversation_item::Entity",
        from = "(Column::TurnId, Column::ThreadId)",
        to = "(super::conversation_item::Column::TurnId, super::conversation_item::Column::ThreadId)"
    )]
    Items,
}

impl Related<super::conversation_ordinal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Ordinal.def()
    }
}

impl Related<super::assistant_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::AssistantRun.def()
    }
}

impl Related<super::conversation_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Items.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
