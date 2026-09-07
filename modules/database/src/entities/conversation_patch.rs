//! Durable renderer patch log entry.

use sea_orm::entity::prelude::*;

use super::execution_value::{
    ConversationItemKind, ConversationPatchKind, EntityLifecycle, RenderPhase,
};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_patches")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub patch_id: String,
    pub thread_id: String,
    pub sequence: i64,
    pub kind: ConversationPatchKind,
    pub revision: i64,
    pub recorded_at_ms: i64,
    pub turn_id: Option<String>,
    pub item_id: Option<String>,
    pub ordinal: Option<i64>,
    pub lifecycle: Option<EntityLifecycle>,
    pub item_kind: Option<ConversationItemKind>,
    pub run_id: Option<String>,
    pub phase: Option<RenderPhase>,
    pub body: Option<String>,
    pub fragment: Option<String>,
    pub entity_created_at_ms: Option<i64>,
    pub entity_updated_at_ms: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::conversation_turn::Entity",
        from = "(Column::TurnId, Column::ThreadId)",
        to = "(super::conversation_turn::Column::TurnId, super::conversation_turn::Column::ThreadId)",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Turn,
    #[sea_orm(
        belongs_to = "super::conversation_item::Entity",
        from = "(Column::ItemId, Column::ThreadId)",
        to = "(super::conversation_item::Column::ItemId, super::conversation_item::Column::ThreadId)",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Item,
    #[sea_orm(
        belongs_to = "super::assistant_run::Entity",
        from = "(Column::RunId, Column::ThreadId)",
        to = "(super::assistant_run::Column::RunId, super::assistant_run::Column::ThreadId)",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Run,
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

impl Related<super::assistant_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Run.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
