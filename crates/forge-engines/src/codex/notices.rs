//! Consumed notice fields from Codex's generated rust-v0.162.1 v2 schema.
//! Unconsumed metadata stays outside these borrowed provider projections.
//! https://github.com/openai/codex/tree/rust-v0.162.1/codex-rs/app-server-protocol/schema/typescript/v2
use anyhow::{Context as _, Result, ensure};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Rerouted<'a> {
    thread_id: &'a str,
    pub turn_id: &'a str,
    pub from_model: &'a str,
    pub to_model: &'a str,
    pub reason: RerouteReason,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum RerouteReason {
    HighRiskCyberActivity,
}
impl RerouteReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HighRiskCyberActivity => "highRiskCyberActivity",
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SafetyBuffering<'a> {
    thread_id: &'a str,
    pub turn_id: &'a str,
    pub show_buffering_ui: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GoalUpdated<'a> {
    thread_id: &'a str,
    // turnId is nullable in this notification and is not consumed by Forge.
    #[serde(borrow)]
    pub goal: Goal<'a>,
}
#[derive(Deserialize)]
pub(super) struct Goal<'a> {
    pub objective: &'a str,
    pub status: GoalStatus,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum GoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}
impl GoalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::UsageLimited => "usageLimited",
            Self::BudgetLimited => "budgetLimited",
            Self::Complete => "complete",
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GoalCleared<'a> {
    thread_id: &'a str,
}

pub(super) enum NativeNotice<'a> {
    Rerouted(Rerouted<'a>),
    SafetyBuffering(SafetyBuffering<'a>),
    GoalUpdated(GoalUpdated<'a>),
    GoalCleared(GoalCleared<'a>),
}
impl<'a> NativeNotice<'a> {
    /// Validate before any decoder state, relation queue or canonical Item changes.
    pub fn parse(method: &str, params: &'a Value) -> Result<Option<Self>> {
        let notice = match method {
            "model/rerouted" => Self::Rerouted(
                Rerouted::deserialize(params).context("invalid Codex model/rerouted payload")?,
            ),
            "model/safetyBuffering/updated" => Self::SafetyBuffering(
                SafetyBuffering::deserialize(params)
                    .context("invalid Codex model/safetyBuffering/updated payload")?,
            ),
            "thread/goal/updated" => Self::GoalUpdated(
                GoalUpdated::deserialize(params)
                    .context("invalid Codex thread/goal/updated payload")?,
            ),
            "thread/goal/cleared" => Self::GoalCleared(
                GoalCleared::deserialize(params)
                    .context("invalid Codex thread/goal/cleared payload")?,
            ),
            _ => return Ok(None),
        };
        nonblank(notice.thread_id(), "threadId")?;
        match &notice {
            Self::Rerouted(notice) => {
                nonblank(notice.turn_id, "turnId")?;
                nonblank(notice.from_model, "fromModel")?;
                nonblank(notice.to_model, "toModel")?;
            }
            Self::SafetyBuffering(notice) => nonblank(notice.turn_id, "turnId")?,
            Self::GoalUpdated(notice) => nonblank(notice.goal.objective, "goal.objective")?,
            Self::GoalCleared(_) => {}
        }
        Ok(Some(notice))
    }
    fn thread_id(&self) -> &str {
        match self {
            Self::Rerouted(notice) => notice.thread_id,
            Self::SafetyBuffering(notice) => notice.thread_id,
            Self::GoalUpdated(notice) => notice.thread_id,
            Self::GoalCleared(notice) => notice.thread_id,
        }
    }
}
fn nonblank(value: &str, field: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty(),
        "Codex required {field} must not be blank"
    );
    Ok(())
}
