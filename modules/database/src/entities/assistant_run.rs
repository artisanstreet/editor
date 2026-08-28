//! Durable assistant-run launch and provider state.

use sea_orm::entity::prelude::*;

use super::execution_value::{AssistantRunLifecycle, OpaqueBytes};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "assistant_runs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub run_id: String,
    pub thread_id: String,
    pub run_start_key: OpaqueBytes,
    pub origin_message_id: String,
    pub origin_turn_id: String,
    pub lifecycle: AssistantRunLifecycle,
    pub generation: i64,
    pub owner: Option<OpaqueBytes>,
    pub lease: Option<OpaqueBytes>,
    pub claim_token: Option<OpaqueBytes>,
    pub provider_binding_version: Option<i64>,
    pub provider_binding: Option<OpaqueBytes>,
    pub provider_bound_at_ms: Option<i64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub terminal_at_ms: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::message::Entity",
        from = "(Column::OriginMessageId, Column::ThreadId)",
        to = "(super::message::Column::MessageId, super::message::Column::ThreadId)",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    OriginMessage,
    #[sea_orm(
        belongs_to = "super::conversation_turn::Entity",
        from = "(Column::OriginTurnId, Column::ThreadId)",
        to = "(super::conversation_turn::Column::TurnId, super::conversation_turn::Column::ThreadId)",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    OriginTurn,
    #[sea_orm(
        has_many = "super::conversation_item::Entity",
        from = "(Column::RunId, Column::ThreadId)",
        to = "(super::conversation_item::Column::RunId, super::conversation_item::Column::ThreadId)"
    )]
    Items,
}

impl Related<super::message::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OriginMessage.def()
    }
}

impl Related<super::conversation_turn::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OriginTurn.def()
    }
}

impl Related<super::conversation_item::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Items.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
