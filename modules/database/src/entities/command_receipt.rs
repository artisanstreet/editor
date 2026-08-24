//! Globally unique command receipt persistence model.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "command_receipts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub request_id: String,
    pub command_kind: CommandKind,
    pub directory_id: Option<String>,
    pub project_id: Option<String>,
    pub thread_id: Option<String>,
    pub title: Option<String>,
    pub message_id: Option<String>,
    pub body: Option<String>,
    pub accepted_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum CommandKind {
    #[sea_orm(string_value = "attach_project")]
    AttachProject,
    #[sea_orm(string_value = "create_thread")]
    CreateThread,
    #[sea_orm(string_value = "queue_first_message")]
    QueueFirstMessage,
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
    #[sea_orm(
        belongs_to = "super::thread::Entity",
        from = "Column::ThreadId",
        to = "super::thread::Column::ThreadId",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Thread,
    #[sea_orm(
        belongs_to = "super::message::Entity",
        from = "Column::MessageId",
        to = "super::message::Column::MessageId",
        on_update = "Restrict",
        on_delete = "Restrict"
    )]
    Message,
}

impl Related<super::attached_project::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl Related<super::thread::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Thread.def()
    }
}

impl Related<super::message::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Message.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
