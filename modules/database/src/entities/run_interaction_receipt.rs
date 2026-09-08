//! Idempotent approval/question response receipt persistence model.
//!
//! One row per client-minted response request id: the exact intent
//! fingerprint plus the settled outcome, so replays answer `duplicate` with
//! no second effect while a reused id with a different intent is a conflict.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "run_interaction_receipts")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub request_id: String,
    pub command_kind: InteractionCommandKind,
    pub thread_id: String,
    pub run_id: String,
    pub interaction_id: String,
    pub outcome: InteractionOutcomeValue,
    pub disposition: InteractionDisposition,
    pub intent_key: String,
    pub approved: Option<i32>,
    pub answers_json: Option<String>,
    pub binding_version: i64,
    pub responded_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum InteractionCommandKind {
    #[sea_orm(string_value = "respond_approval")]
    RespondApproval,
    #[sea_orm(string_value = "respond_question")]
    RespondQuestion,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum InteractionOutcomeValue {
    #[sea_orm(string_value = "applied")]
    Applied,
    #[sea_orm(string_value = "unknown_target")]
    UnknownTarget,
    #[sea_orm(string_value = "already_resolved")]
    AlreadyResolved,
    #[sea_orm(string_value = "wrong_run")]
    WrongRun,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum InteractionDisposition {
    #[sea_orm(string_value = "accepted")]
    Accepted,
    #[sea_orm(string_value = "duplicate")]
    Duplicate,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
