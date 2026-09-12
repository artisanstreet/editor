#![allow(clippy::module_name_repetitions)]
#![allow(dead_code)]

use artisan_domain::{
    ApprovalRequest, ObservationId, PlanEntry, PlanEntryStatus, QuestionInput, QuestionOption,
};
use serde_json::Value;

use super::CursorTurnError;

// ---------------------------------------------------------------------------
// Cursor plan-approval extensions: `cursor/ask_question`, `cursor/create_plan`
// ---------------------------------------------------------------------------

/// One offered answer to a cursor question.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorQuestionOption {
    /// Provider option identity answered back on the wire.
    id: String,
    /// Human label shown beside the question.
    label: String,
}

/// One typed cursor question (`cursor/ask_question` entry).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorQuestion {
    /// Provider question identity.
    id: String,
    /// The question itself.
    prompt: String,
    /// Offered answers.
    options: Vec<CursorQuestionOption>,
    /// Whether more than one option may be chosen at once.
    allow_multiple: bool,
}

/// One typed cursor question request frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorQuestionRequest {
    /// Provider request identity answered by the response.
    pub(super) tool_call_id: String,
    /// Short category label shown beside the questions, when disclosed.
    pub(super) title: Option<String>,
    /// The questions in this request group.
    pub(super) questions: Vec<CursorQuestion>,
}

fn nonempty_string(value: &Value, field: &str) -> Result<String, CursorTurnError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or(CursorTurnError::Configuration)
}

/// Parses one `cursor/ask_question` params object, mirroring
/// `parse_cursor_question` in `modules/engines/src/acp/engine.ts`: a
/// non-empty `toolCallId`, a non-empty `questions` array whose entries carry
/// a non-empty `id`, a non-empty `prompt`, and a non-empty `options` array of
/// `{ id, label }` pairs, with `allowMultiple` defaulting to single select
/// and an optional `title` header.
///
/// # Errors
///
/// Returns [`CursorTurnError::Configuration`] when any identity, prompt, or
/// option violates the extension shape. The turn fails closed instead of
/// rendering a question with nothing to choose.
pub(crate) fn parse_cursor_question_request(
    value: &Value,
) -> Result<CursorQuestionRequest, CursorTurnError> {
    let obj = value.as_object().ok_or(CursorTurnError::Configuration)?;
    let tool_call_id = nonempty_string(value, "toolCallId")?;
    let title = obj
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .map(str::to_owned);
    let raw_questions = obj
        .get("questions")
        .and_then(Value::as_array)
        .ok_or(CursorTurnError::Configuration)?;
    if raw_questions.is_empty() {
        return Err(CursorTurnError::Configuration);
    }
    let mut questions = Vec::new();
    for raw in raw_questions {
        let question = raw.as_object().ok_or(CursorTurnError::Configuration)?;
        let id = nonempty_string(raw, "id")?;
        let prompt = nonempty_string(raw, "prompt")?;
        let allow_multiple = question
            .get("allowMultiple")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let raw_options = question
            .get("options")
            .and_then(Value::as_array)
            .ok_or(CursorTurnError::Configuration)?;
        if raw_options.is_empty() {
            return Err(CursorTurnError::Configuration);
        }
        let mut options = Vec::new();
        for raw_option in raw_options {
            options.push(CursorQuestionOption {
                id: nonempty_string(raw_option, "id")?,
                label: nonempty_string(raw_option, "label")?,
            });
        }
        questions.push(CursorQuestion {
            id,
            prompt,
            options,
            allow_multiple,
        });
    }
    Ok(CursorQuestionRequest {
        tool_call_id,
        title,
        questions,
    })
}

/// Maps one cursor question onto the domain question vocabulary: the prompt
/// becomes the text, the request title becomes the header, `allowMultiple`
/// becomes multi-select, and option labels become the offered choices.
///
/// # Errors
///
/// Returns [`CursorTurnError::Configuration`] when identities, text, or
/// options violate domain ceilings. An out-of-bound provider frame never
/// reaches the durable A-approve rows.
pub(crate) fn cursor_question_to_domain(
    request: &CursorQuestionRequest,
    question: &CursorQuestion,
) -> Result<QuestionInput, CursorTurnError> {
    let question_id =
        ObservationId::parse(question.id.clone()).map_err(|_| CursorTurnError::Configuration)?;
    if question.prompt.trim().is_empty() {
        return Err(CursorTurnError::Configuration);
    }
    let mut options = Vec::new();
    for option in &question.options {
        options.push(
            QuestionOption::new(option.label.clone(), None)
                .map_err(|_| CursorTurnError::Configuration)?,
        );
    }
    if options.is_empty() {
        return Err(CursorTurnError::Configuration);
    }
    Ok(QuestionInput {
        question_id,
        text: question.prompt.clone(),
        header: request.title.clone(),
        multi_select: question.allow_multiple,
        options: Some(options),
    })
}

/// Maps explicit answers for one cursor question back to provider option
/// identities, mirroring the TypeScript answer encoding: an answer matching
/// an option id or label answers that option id, anything else answers
/// verbatim.
#[must_use]
pub(crate) fn cursor_selected_option_ids(
    question: &CursorQuestion,
    answers: &[String],
) -> Vec<String> {
    answers
        .iter()
        .map(|answer| {
            question
                .options
                .iter()
                .find(|option| option.id == *answer || option.label == *answer)
                .map_or_else(|| answer.clone(), |option| option.id.clone())
        })
        .collect()
}

/// One typed cursor plan step (`cursor/create_plan` todo).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorPlanTodo {
    /// Provider step identity.
    id: String,
    /// Step text.
    content: String,
    /// Provider status spelling (`cancelled`/`completed`/`in_progress`/`pending`).
    status: String,
}

/// One typed cursor plan request frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorPlanRequest {
    /// Provider request identity answered by the approval response.
    pub(super) tool_call_id: String,
    /// Plan name, when disclosed.
    pub(super) name: Option<String>,
    /// Plan overview, when disclosed.
    pub(super) overview: Option<String>,
    /// Full plan text under review.
    plan: String,
    /// Plan steps in provider order, including cancelled ones.
    pub(super) todos: Vec<CursorPlanTodo>,
}

/// Parses one `cursor/create_plan` params object, mirroring
/// `parse_cursor_plan` in `modules/engines/src/acp/engine.ts`: a non-empty
/// `toolCallId`, a string `plan`, and a `todos` array whose malformed entries
/// are dropped the way the TypeScript `flatMap` evidence does. Only the four
/// provider statuses survive; anything else is not a step.
///
/// # Errors
///
/// Returns [`CursorTurnError::Configuration`] when the tool-call identity,
/// plan text, or todo list violates the extension shape.
pub(crate) fn parse_cursor_plan_request(
    value: &Value,
) -> Result<CursorPlanRequest, CursorTurnError> {
    let obj = value.as_object().ok_or(CursorTurnError::Configuration)?;
    let tool_call_id = nonempty_string(value, "toolCallId")?;
    let plan = obj
        .get("plan")
        .and_then(Value::as_str)
        .ok_or(CursorTurnError::Configuration)?
        .to_owned();
    let raw_todos = obj
        .get("todos")
        .and_then(Value::as_array)
        .ok_or(CursorTurnError::Configuration)?;
    let mut todos = Vec::new();
    for raw in raw_todos {
        let Some(todo) = raw.as_object() else {
            continue;
        };
        let (Some(id), Some(content), Some(status)) = (
            todo.get("id").and_then(Value::as_str),
            todo.get("content").and_then(Value::as_str),
            todo.get("status").and_then(Value::as_str),
        ) else {
            continue;
        };
        if id.is_empty()
            || !matches!(
                status,
                "cancelled" | "completed" | "in_progress" | "pending"
            )
        {
            continue;
        }
        todos.push(CursorPlanTodo {
            id: id.to_owned(),
            content: content.to_owned(),
            status: status.to_owned(),
        });
    }
    let optional_string = |field: &str| {
        obj.get(field)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    Ok(CursorPlanRequest {
        tool_call_id,
        name: optional_string("name"),
        overview: optional_string("overview"),
        plan,
        todos,
    })
}

/// Returns the human-readable plan approval description: the overview, then
/// the name, then the shared fallback, mirroring the TypeScript evidence.
#[must_use]
pub(crate) fn cursor_plan_description(request: &CursorPlanRequest) -> String {
    request
        .overview
        .clone()
        .or_else(|| request.name.clone())
        .unwrap_or_else(|| "Approve this plan?".to_owned())
}

/// Maps one cursor plan request onto the domain approval vocabulary: a
/// generic action carrying the full plan text as its reason. An empty plan
/// still gates as a reason-free action rather than vanishing.
///
/// # Errors
///
/// Returns [`CursorTurnError::Configuration`] when the plan text violates
/// domain ceilings.
pub(crate) fn cursor_plan_approval(
    request: &CursorPlanRequest,
) -> Result<(String, ApprovalRequest), CursorTurnError> {
    let description = cursor_plan_description(request);
    let reason = if request.plan.is_empty() {
        None
    } else {
        Some(request.plan.clone())
    };
    let approval = ApprovalRequest::action(reason).map_err(|_| CursorTurnError::Configuration)?;
    Ok((description, approval))
}

/// Projects one cursor plan request onto provider-neutral plan entries,
/// mirroring the TypeScript emission: cancelled steps are filtered, and the
/// surviving statuses map verbatim (`pending`, `in_progress`, `completed`).
/// Steps without renderable text are skipped so an out-of-bound provider
/// frame never reaches the durable rows.
///
/// # Errors
///
/// Returns [`CursorTurnError::Configuration`] when an identity violates
/// domain ceilings.
pub(crate) fn cursor_plan_entries(
    request: &CursorPlanRequest,
) -> Result<Vec<PlanEntry>, CursorTurnError> {
    let mut entries = Vec::new();
    for todo in &request.todos {
        if todo.status == "cancelled" || todo.content.is_empty() {
            continue;
        }
        let status = PlanEntryStatus::parse(todo.status.as_str())
            .map_err(|_| CursorTurnError::Configuration)?;
        let id =
            ObservationId::parse(todo.id.clone()).map_err(|_| CursorTurnError::Configuration)?;
        entries.push(
            PlanEntry::new(id, status, todo.content.clone())
                .map_err(|_| CursorTurnError::Configuration)?,
        );
    }
    Ok(entries)
}

/// Builds the explicit `cursor/create_plan` wire outcome: an approved plan is
/// `accepted`, a denied plan is `rejected`. The turn continues either way.
#[must_use]
pub(crate) fn answer_cursor_plan(approved: bool) -> Value {
    serde_json::json!({
        "outcome": { "outcome": if approved { "accepted" } else { "rejected" } },
    })
}
