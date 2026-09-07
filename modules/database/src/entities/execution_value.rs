//! Persisted values shared by the conversation execution entities.

use std::fmt;

use sea_orm::entity::prelude::*;

/// Database bytes whose contents must not appear in formatted models.
#[derive(Clone, PartialEq, Eq, DeriveValueType)]
pub struct OpaqueBytes(Vec<u8>);

impl OpaqueBytes {
    #[must_use]
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

impl From<Vec<u8>> for OpaqueBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl AsRef<[u8]> for OpaqueBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl fmt::Debug for OpaqueBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpaqueBytes")
            .field("len", &self.0.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum OrdinalKind {
    #[sea_orm(string_value = "turn")]
    Turn,
    #[sea_orm(string_value = "item")]
    Item,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum EntityLifecycle {
    #[sea_orm(string_value = "pending")]
    Pending,
    #[sea_orm(string_value = "streaming")]
    Streaming,
    #[sea_orm(string_value = "active")]
    Active,
    #[sea_orm(string_value = "waiting")]
    Waiting,
    #[sea_orm(string_value = "completed")]
    Completed,
    #[sea_orm(string_value = "failed")]
    Failed,
    #[sea_orm(string_value = "interrupted")]
    Interrupted,
    #[sea_orm(string_value = "cancelled")]
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum AssistantRunLifecycle {
    #[sea_orm(string_value = "queued")]
    Queued,
    #[sea_orm(string_value = "launching")]
    Launching,
    #[sea_orm(string_value = "running")]
    Running,
    #[sea_orm(string_value = "waiting")]
    Waiting,
    #[sea_orm(string_value = "cancel_requested")]
    CancelRequested,
    #[sea_orm(string_value = "interrupted")]
    Interrupted,
    #[sea_orm(string_value = "completed")]
    Completed,
    #[sea_orm(string_value = "failed")]
    Failed,
    #[sea_orm(string_value = "cancelled")]
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum ConversationItemKind {
    #[sea_orm(string_value = "user_message")]
    UserMessage,
    #[sea_orm(string_value = "assistant_message")]
    AssistantMessage,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum RenderPhase {
    #[sea_orm(string_value = "commentary")]
    Commentary,
    #[sea_orm(string_value = "final")]
    Final,
    #[sea_orm(string_value = "unspecified")]
    Unspecified,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum ConversationPatchKind {
    #[sea_orm(string_value = "turn_upsert")]
    TurnUpsert,
    #[sea_orm(string_value = "item_upsert")]
    ItemUpsert,
    #[sea_orm(string_value = "item_append")]
    ItemAppend,
    #[sea_orm(string_value = "item_lifecycle")]
    ItemLifecycle,
    #[sea_orm(string_value = "turn_lifecycle")]
    TurnLifecycle,
}
