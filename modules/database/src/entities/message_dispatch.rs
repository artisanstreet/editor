//! Durable message dispatch persistence model.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "message_dispatches")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub message_id: String,
    pub correlation_id: String,
    pub state: DispatchState,
    pub attempt_count: i32,
    pub queued_at_ms: i64,
    pub available_at_ms: i64,
    pub lease_owner: Option<String>,
    pub lease_expires_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum DispatchState {
    #[sea_orm(string_value = "queued")]
    Queued,
    #[sea_orm(string_value = "leased")]
    Leased,
    #[sea_orm(string_value = "running")]
    Running,
    #[sea_orm(string_value = "completed")]
    Completed,
    #[sea_orm(string_value = "failed")]
    Failed,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::message::Entity",
        from = "Column::MessageId",
        to = "super::message::Column::MessageId",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Message,
}

impl Related<super::message::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Message.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
