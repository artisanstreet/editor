//! Pending approval/question persistence model.
//!
//! One row per provider request on its run: the requested state with the
//! full request snapshot plus, once answered, the explicit resolution. Rows
//! are per-run and deleted when the run settles.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "pending_run_interactions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = true)]
    pub interaction_pk: i64,
    pub run_id: String,
    pub interaction_id: String,
    pub thread_id: String,
    pub kind: InteractionKind,
    pub state: InteractionState,
    pub request_json: String,
    pub requested_sequence: i64,
    pub approved: Option<i32>,
    pub answers_json: Option<String>,
    pub requested_at_ms: i64,
    pub resolved_at_ms: Option<i64>,
    pub resolved_sequence: Option<i64>,
    pub binding_version: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum InteractionKind {
    #[sea_orm(string_value = "approval")]
    Approval,
    #[sea_orm(string_value = "question")]
    Question,
}

#[derive(Clone, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
pub enum InteractionState {
    #[sea_orm(string_value = "requested")]
    Requested,
    #[sea_orm(string_value = "resolved")]
    Resolved,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
