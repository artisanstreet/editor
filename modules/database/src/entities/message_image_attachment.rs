//! Immutable ordered image bytes attached to one queued message.

use sea_orm::entity::prelude::*;

use super::execution_value::OpaqueBytes;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "message_image_attachments")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub message_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub position: i64,
    pub mime_type: String,
    pub name: String,
    pub size_bytes: i64,
    pub bytes: OpaqueBytes,
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
