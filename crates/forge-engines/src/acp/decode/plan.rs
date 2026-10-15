//! The run's plan: ACP `plan` updates, Cursor's todo calls and its plan
//! proposal, all one `Plan` item keyed [`PLAN_KEY`].

use super::{AcpDecoder, PLAN_KEY, Pending};
use crate::EngineEvent;
use artisan_api::{
    Approval, ApprovalRequest, Decision, ErrorKind, ItemKind, Phase, PlanStep, Status, StepStatus,
};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub(super) struct Plan {
    /// Cursor's todo ids → step index, for merges.
    todo_index: HashMap<String, usize>,
    steps: Vec<PlanStep>,
    proposal: Option<artisan_api::TextContent>,
    pub(super) approval: Option<Approval>,
    /// Set while a proposal waits or after it was answered; otherwise the
    /// steps decide.
    pub(super) status: Option<Status>,
}

impl AcpDecoder {
    /// ACP `plan` session update: the whole plan.
    pub(super) fn session_plan(&mut self, entries: &Value, events: &mut Vec<EngineEvent>) {
        let parsed = match plan_steps(entries, false) {
            Ok(parsed) => parsed,
            Err(message) => {
                self.plan_error(message, events);
                return;
            }
        };
        let plan = self.plan.get_or_insert_default();
        plan.steps = parsed.steps;
        plan.todo_index = parsed.index;
        if plan.status != Some(Status::AwaitingUser) {
            plan.status = None;
        }
        self.emit_plan(events);
    }

    /// `cursor/update_todos`: replaces the todos, or merges them by id.
    pub(super) fn update_todos(
        &mut self,
        rpc: Value,
        params: &Value,
        events: &mut Vec<EngineEvent>,
    ) {
        let parsed = match plan_steps(&params["todos"], true) {
            Ok(parsed) => parsed,
            Err(message) => {
                self.invalid_plan_request(rpc, message, events);
                return;
            }
        };
        let plan = self.plan.get_or_insert_default();
        if params["merge"].as_bool().unwrap_or(false) {
            let mut incoming = parsed.index.into_iter().collect::<Vec<_>>();
            incoming.sort_by_key(|(_, index)| *index);
            for (id, index) in incoming {
                let step = parsed.steps[index].clone();
                if let Some(index) = plan.todo_index.get(&id) {
                    plan.steps[*index] = step;
                } else {
                    plan.todo_index.insert(id, plan.steps.len());
                    plan.steps.push(step);
                }
            }
        } else {
            plan.steps = parsed.steps;
            plan.todo_index = parsed.index;
        }
        if plan.status != Some(Status::AwaitingUser) {
            plan.status = None;
        }
        self.emit_plan(events);
        self.reply(rpc, json!({}));
    }

    /// `cursor/create_plan`: a proposal the user approves on the plan.
    pub(super) fn cursor_plan(
        &mut self,
        rpc: Value,
        params: &Value,
        events: &mut Vec<EngineEvent>,
    ) {
        let parsed = match params
            .get("todos")
            .map(|todos| plan_steps(todos, true))
            .transpose()
        {
            Ok(parsed) => parsed,
            Err(message) => {
                self.invalid_plan_request(rpc, message, events);
                return;
            }
        };
        if let Some(id) = params["toolCallId"].as_str() {
            self.hidden.insert(id.to_owned());
        }
        let field = |name: &str| {
            params[name]
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
        };
        let description = field("overview")
            .or_else(|| field("name"))
            .unwrap_or("Approve this plan?")
            .to_owned();
        let plan = self.plan.get_or_insert_default();
        if let Some(parsed) = parsed {
            plan.steps = parsed.steps;
            plan.todo_index = parsed.index;
        }
        plan.proposal = field("plan").map(|text| text.to_owned().into());
        plan.approval = Some(Approval {
            request: ApprovalRequest::Action {
                description: description.into(),
            },
            reason: None,
            options: vec![Decision::AllowOnce, Decision::DenyOnce],
            decision: None,
        });
        plan.status = Some(Status::AwaitingUser);
        self.pending
            .insert(PLAN_KEY.to_owned(), Pending::CursorPlan { rpc });
        self.close_message(Phase::Commentary, events);
        self.emit_plan(events);
    }

    pub(super) fn plan_error(&mut self, message: String, events: &mut Vec<EngineEvent>) {
        self.questions += 1;
        events.push(EngineEvent::Item {
            key: format!("plan-error:{}", self.questions),
            parent: None,
            status: Status::Done,
            kind: ItemKind::Error {
                kind: ErrorKind::Execution,
                message: message.into(),
                retryable: false,
            },
        });
    }

    fn invalid_plan_request(&mut self, rpc: Value, message: String, events: &mut Vec<EngineEvent>) {
        self.rpc
            .reply_error(rpc, crate::rpc::INVALID_PARAMS, &message);
        self.plan_error(message, events);
    }

    pub(super) fn emit_plan(&mut self, events: &mut Vec<EngineEvent>) {
        let Some(plan) = &self.plan else { return };
        let status = plan.status.unwrap_or_else(|| {
            if plan
                .steps
                .iter()
                .all(|step| matches!(step.status, StepStatus::Done | StepStatus::Cancelled))
            {
                Status::Done
            } else {
                Status::Running
            }
        });
        let kind = ItemKind::Plan {
            steps: (plan.steps.clone()).into(),
            proposal: plan.proposal.clone(),
            approval: plan.approval.clone(),
        };
        self.emit_changed(PLAN_KEY, status, kind, events);
    }
}

#[derive(serde::Deserialize)]
struct NativePlanEntry {
    #[serde(default)]
    id: Option<String>,
    content: String,
    status: NativePlanStatus,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum NativePlanStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}
struct ParsedPlan {
    steps: Vec<PlanStep>,
    index: HashMap<String, usize>,
}

/// Every entry or none: a malformed batch never becomes a partial plan.
fn plan_steps(value: &Value, require_ids: bool) -> Result<ParsedPlan, String> {
    let entries = <Vec<NativePlanEntry> as serde::Deserialize>::deserialize(value)
        .map_err(|error| format!("malformed native plan entries: {error}"))?;
    let mut steps = Vec::with_capacity(entries.len());
    let mut index = HashMap::new();
    for entry in entries {
        if entry.content.trim().is_empty() {
            return Err("native plan step is empty".into());
        }
        if require_ids && entry.id.as_ref().is_none_or(|id| id.trim().is_empty()) {
            return Err("Cursor todo omitted its native ID".into());
        }
        if let Some(id) = entry.id
            && (id.trim().is_empty() || index.insert(id, steps.len()).is_some())
        {
            return Err("native todo IDs must be nonempty and unique".into());
        }
        steps.push(PlanStep {
            text: (entry.content).into(),
            status: match entry.status {
                NativePlanStatus::Pending => StepStatus::Pending,
                NativePlanStatus::InProgress => StepStatus::InProgress,
                NativePlanStatus::Completed => StepStatus::Done,
                NativePlanStatus::Cancelled => StepStatus::Cancelled,
            },
        });
    }
    Ok(ParsedPlan { steps, index })
}
