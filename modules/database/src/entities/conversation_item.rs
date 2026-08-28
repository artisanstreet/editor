//! Renderable conversation item projection.

use sea_orm::entity::prelude::*;

use super::execution_value::{ConversationItemKind, EntityLifecycle, OrdinalKind, RenderPhase};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "conversation_items")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub item_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub ordinal: i64,
    pub kind: OrdinalKind,
    pub revision: i64,
    pub lifecycle: EntityLifecycle,
    pub item_kind: ConversationItemKind,
    pub source_message_id: Option<String>,
    pub run_id: Option<String>,
    pub native_item_key: Option<String>,
    pub phase: Option<RenderPhase>,
    pub body: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::conversation_ordinal::Entity",
        from = "(Column::ThreadId, Column::Ordinal, Column::ItemId, Column::Kind)",
        to = "(super::conversation_ordinal::Column::ThreadId, super::conversation_ordinal::Column::Ordinal, super::conversation_ordinal::Column::EntityId, super::conversation_ordinal::Column::Kind)",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Ordinal,
    #[sea_orm(
        belongs_to = "super::conversation_turn::Entity",
        from = "(Column::TurnId, Column::ThreadId)",
        to = "(super::conversation_turn::Column::TurnId, super::conversation_turn::Column::ThreadId)",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Turn,
    #[sea_orm(
        belongs_to = "super::message::Entity",
        from = "(Column::SourceMessageId, Column::ThreadId)",
        to = "(super::message::Column::MessageId, super::message::Column::ThreadId)",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    SourceMessage,
    #[sea_orm(
        belongs_to = "super::assistant_run::Entity",
        from = "(Column::RunId, Column::ThreadId)",
        to = "(super::assistant_run::Column::RunId, super::assistant_run::Column::ThreadId)",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Run,
}

impl Related<super::conversation_ordinal::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Ordinal.def()
    }
}

impl Related<super::conversation_turn::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Turn.def()
    }
}

impl Related<super::message::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::SourceMessage.def()
    }
}

impl Related<super::assistant_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Run.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
