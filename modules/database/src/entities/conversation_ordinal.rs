//! Shared renderer ordinal ledger entry.

use sea_orm::entity::prelude::*;

use super::execution_value::OrdinalKind;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_ordinals")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub thread_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub ordinal: i64,
    pub kind: OrdinalKind,
    pub entity_id: String,
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
    #[sea_orm(
        has_one = "super::conversation_turn::Entity",
        from = "(Column::ThreadId, Column::Ordinal, Column::EntityId, Column::Kind)",
        to = "(super::conversation_turn::Column::ThreadId, super::conversation_turn::Column::Ordinal, super::conversation_turn::Column::TurnId, super::conversation_turn::Column::Kind)"
    )]
    Turn,
    #[sea_orm(
        has_one = "super::conversation_item::Entity",
        from = "(Column::ThreadId, Column::Ordinal, Column::EntityId, Column::Kind)",
        to = "(super::conversation_item::Column::ThreadId, super::conversation_item::Column::Ordinal, super::conversation_item::Column::ItemId, super::conversation_item::Column::Kind)"
    )]
    Item,
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

impl Related<super::conversation_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Item.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
