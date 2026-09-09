//! Finite ACP A2 bridges: permission/elicitation to approval/question rails.
//!
//! Pure normalization over typed ACP frames, bridging onto the A1 transport
//! core's update loop. Permission requests (`session/requestPermission`)
//! map to the domain [`ApprovalRequest`] (command with cwd/reason,
//! file-change, action, mirroring the TypeScript `approval_request`
//! evidence); elicitation requests (`elicitation/create`) map to domain
//! [`QuestionInput`] rows (single/multi select plus free-form, mirroring the
//! TypeScript elicitation-to-question evidence); and prompt-end accumulation
//! projects completed messages/thoughts, mirroring `CompleteAcpMessages`.
//!
//! Everything here is pure: no I/O, no tasks, no retained runtime. The
//! pending-request table is owned by the caller (one per dispatch arm) and
//! tracks provider request ids with idempotent answer application: the same
//! request id plus the same answers succeeds, changed answers fail, unknown
//! ids fail. Provider answer payloads are validated before application. The
//! bridges never auto-answer and never default: a missing allow/reject
//! option surfaces the explicit `Cancelled` outcome shape, and non-form
//! elicitations surface `UnsupportedMode` so the caller can decline
//! explicitly.
//!
//! Bridge outputs are exactly the S1a/A-approve domain types
//! ([`ApprovalRequest`], [`QuestionInput`]); no new domain shapes are
//! introduced. Wire answer payloads (`PermissionOutcome`, elicitation
//! content objects) stay boundary-typed until the dispatch arm speaks them.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]
// A2 has no in-crate callers yet: per-engine dispatch arms land in a later
// packet. Every item below is covered by the inline fixture tests.
#![allow(dead_code)]

use std::collections::BTreeMap;

use artisan_domain::{ApprovalRequest, ObservationId, QuestionInput, QuestionOption};
use serde_json::Value;
use thiserror::Error;

/// Structural ceiling for one provider request identity held as a table key.
const MAX_PROVIDER_ID_BYTES: usize = 256;

/// Default description when a permission request discloses no title,
/// mirroring the TypeScript evidence.
const DEFAULT_APPROVAL_DESCRIPTION: &str = "Approve this tool call?";

/// Typed, payload-free ACP bridge failure.
///
/// `Debug` and `Display` are constant strings; no frame, prompt, answer, or
/// credential bytes are embedded.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum BridgeError {
    /// A request identity failed wire validation (missing, empty, or
    /// over the structural ceiling).
    #[error("acp bridge request malformed")]
    MalformedRequest,
    /// A request value failed domain validation (oversized command, reason,
    /// or working directory; bad question key; empty option label).
    #[error("acp bridge value rejected")]
    InvalidRequest,
    /// An elicitation arrived in a non-form mode the rails cannot ask.
    #[error("acp elicitation mode unsupported")]
    UnsupportedMode,
    /// No pending request carries this provider identity.
    #[error("acp bridge request unknown")]
    UnknownRequest,
    /// The provider identity is pending as the other interaction kind.
    #[error("acp bridge request kind mismatch")]
    WrongKind,
    /// The same request id was answered again with different answers.
    #[error("acp bridge answer conflicts")]
    AnswerConflict,
    /// The pending table reached the caller-supplied entry bound.
    #[error("acp bridge table full")]
    TableFull,
    /// Accumulated completion text crossed the caller-supplied byte bound.
    #[error("acp completion state exceeded its bound")]
    CompletionTooLarge,
    /// An answer payload failed validation before application.
    #[error("acp bridge answer invalid")]
    InvalidAnswer,
}

// ---------------------------------------------------------------------------
// Permission requests to EngineApprovalRequest
// ---------------------------------------------------------------------------

/// One normalized permission request awaiting an explicit decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingApproval {
    provider_id: String,
    description: String,
    request: ApprovalRequest,
    allow_option: Option<String>,
    reject_option: Option<String>,
}

impl PendingApproval {
    /// Returns the provider request identity keying the pending table.
    #[must_use]
    pub(crate) fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// Returns the human-readable approval description.
    #[must_use]
    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    /// Returns the domain approval request for the rails.
    #[must_use]
    pub(crate) fn request(&self) -> &ApprovalRequest {
        &self.request
    }
}

fn provider_id(value: Option<&Value>) -> Result<String, BridgeError> {
    let id = value
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= MAX_PROVIDER_ID_BYTES)
        .ok_or(BridgeError::MalformedRequest)?;
    Ok(id.to_owned())
}

fn permission_option(options: Option<&Vec<Value>>, kinds: [&str; 2]) -> Option<String> {
    let options = options?;
    for option in options {
        let obj = option.as_object()?;
        let kind = obj.get("kind")?.as_str()?;
        if !kinds.contains(&kind) {
            continue;
        }
        let id = obj.get("optionId")?.as_str()?;
        if !id.is_empty() {
            return Some(id.to_owned());
        }
    }
    None
}

/// Normalizes one `session/requestPermission` params object, mirroring the
/// TypeScript `approval_request` evidence: `execute` tools map the disclosed
/// command (plus `cwd` when the provider discloses it, per this contract)
/// with the title as reason; `edit`/`delete`/`move` map to file-change;
/// every other kind maps to a generic action.
///
/// An `execute` tool with no disclosed command still gates as an action
/// rather than vanishing: the approval is preserved with less detail. A
/// missing kind is treated as an action, mirroring the TypeScript fallthrough.
///
/// # Errors
///
/// Returns [`BridgeError::MalformedRequest`] when params or `toolCall` are
/// not objects, or [`BridgeError::InvalidRequest`] when the request identity
/// or a domain value is rejected.
pub(crate) fn normalize_permission_request(params: &Value) -> Result<PendingApproval, BridgeError> {
    let obj = params.as_object().ok_or(BridgeError::MalformedRequest)?;
    let tool = obj
        .get("toolCall")
        .and_then(Value::as_object)
        .ok_or(BridgeError::MalformedRequest)?;
    let provider_id = provider_id(tool.get("toolCallId"))?;
    let title = tool.get("title").and_then(Value::as_str);
    let reason = title.filter(|reason| !reason.is_empty()).map(str::to_owned);
    let description = title
        .filter(|title| !title.is_empty())
        .map_or_else(|| DEFAULT_APPROVAL_DESCRIPTION.to_owned(), str::to_owned);
    let kind = tool.get("kind").and_then(Value::as_str);
    let request = match kind {
        Some("execute") => {
            let raw = tool.get("rawInput").and_then(Value::as_object);
            let command = raw
                .and_then(|raw| raw.get("command"))
                .and_then(Value::as_str)
                .filter(|command| !command.is_empty());
            match command {
                Some(command) => {
                    let cwd = raw
                        .and_then(|raw| raw.get("cwd"))
                        .and_then(Value::as_str)
                        .filter(|cwd| !cwd.is_empty())
                        .map(str::to_owned);
                    ApprovalRequest::command(command.to_owned(), cwd, reason)
                        .map_err(|_| BridgeError::InvalidRequest)?
                }
                None => ApprovalRequest::action(reason).map_err(|_| BridgeError::InvalidRequest)?,
            }
        }
        Some("edit" | "delete" | "move") => {
            ApprovalRequest::file_change(reason).map_err(|_| BridgeError::InvalidRequest)?
        }
        _ => ApprovalRequest::action(reason).map_err(|_| BridgeError::InvalidRequest)?,
    };
    let options = obj.get("options").and_then(Value::as_array);
    Ok(PendingApproval {
        provider_id,
        description,
        request,
        allow_option: permission_option(options, ["allow_once", "allow_always"]),
        reject_option: permission_option(options, ["reject_once", "reject_always"]),
    })
}

/// The explicit wire outcome of one answered permission request, mirroring
/// `resolve_permission`: the selected option id, or the cancelled shape when
/// no matching option was offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PermissionOutcome {
    Selected { option_id: String },
    Cancelled,
}

/// Answers one normalized permission request with an explicit decision.
/// Never defaults: approval without an allow option (or denial without a
/// reject option) yields the explicit `Cancelled` outcome.
#[must_use = "answers must reach the pending table and the wire"]
pub(crate) fn answer_permission(pending: &PendingApproval, approved: bool) -> PermissionOutcome {
    let selected = if approved {
        pending.allow_option.as_ref()
    } else {
        pending.reject_option.as_ref()
    };
    match selected {
        Some(option_id) => PermissionOutcome::Selected {
            option_id: option_id.clone(),
        },
        None => PermissionOutcome::Cancelled,
    }
}

// ---------------------------------------------------------------------------
// Elicitations to EngineQuestionObservation inputs
// ---------------------------------------------------------------------------

/// The schema type behind one elicited question, driving answer encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ElicitedType {
    Text,
    MultiText,
    Flag,
    Integer,
    Number,
}

/// One normalized elicited question: the domain input for the rails plus the
/// schema type needed to encode answers back to the provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ElicitedQuestion {
    input: QuestionInput,
    value_type: ElicitedType,
}

impl ElicitedQuestion {
    /// Returns the domain question input for the rails.
    #[must_use]
    pub(crate) fn input(&self) -> &QuestionInput {
        &self.input
    }

    /// Returns the schema type driving answer encoding.
    #[must_use]
    pub(crate) fn value_type(&self) -> ElicitedType {
        self.value_type
    }
}

/// One normalized elicitation awaiting explicit answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingElicitation {
    provider_id: String,
    questions: Vec<ElicitedQuestion>,
}

impl PendingElicitation {
    /// Returns the provider request identity keying the pending table.
    #[must_use]
    pub(crate) fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// Returns the normalized questions in schema order.
    #[must_use]
    pub(crate) fn questions(&self) -> &[ElicitedQuestion] {
        &self.questions
    }
}

fn elicitation_choices(
    schema: &serde_json::Map<String, Value>,
) -> Result<Option<Vec<QuestionOption>>, BridgeError> {
    let one_of = schema.get("oneOf").and_then(Value::as_array);
    let enumeration = schema.get("enum").and_then(Value::as_array);
    let choices = if one_of.is_some() || enumeration.is_some() {
        one_of.or(enumeration)
    } else if schema.get("type").and_then(Value::as_str) == Some("array") {
        let items = schema.get("items")?.as_object()?;
        items
            .get("anyOf")
            .and_then(Value::as_array)
            .or_else(|| items.get("enum").and_then(Value::as_array))
    } else {
        None
    };
    let Some(choices) = choices else {
        return Ok(None);
    };
    let mut options = Vec::new();
    for choice in choices {
        match choice {
            Value::String(label) => {
                options.push(
                    QuestionOption::new(label.clone(), None)
                        .map_err(|_| BridgeError::InvalidRequest)?,
                );
            }
            Value::Object(choice) => {
                let label = choice
                    .get("title")
                    .and_then(Value::as_str)
                    .or_else(|| choice.get("const").and_then(Value::as_str));
                let Some(label) = label else { continue };
                let description = choice
                    .get("description")
                    .and_then(Value::as_str)
                    .filter(|description| !description.is_empty())
                    .map(str::to_owned);
                options.push(
                    QuestionOption::new(label.to_owned(), description)
                        .map_err(|_| BridgeError::InvalidRequest)?,
                );
            }
            _ => {}
        }
    }
    Ok(Some(options))
}

/// Normalizes one `elicitation/create` params object for `provider_id`,
/// mirroring the TypeScript elicitation-to-question evidence: one
/// [`QuestionInput`] per schema property in schema order, single select
/// from `oneOf`/`enum` choices, multi select for array-typed properties,
/// free-form (`options: None`) when no choices are disclosed, and text from
/// description, then title, then the form message.
///
/// # Errors
///
/// Returns [`BridgeError::MalformedRequest`] for non-object params, a
/// missing `requestedSchema`, non-object properties, or a bad provider
/// identity; returns [`BridgeError::UnsupportedMode`] for non-form modes
/// so the caller declines explicitly; returns
/// [`BridgeError::InvalidRequest`] when a question identity or option
/// violates domain bounds.
pub(crate) fn normalize_elicitation_request(
    provider_id: &str,
    params: &Value,
) -> Result<PendingElicitation, BridgeError> {
    if provider_id.is_empty() || provider_id.len() > MAX_PROVIDER_ID_BYTES {
        return Err(BridgeError::MalformedRequest);
    }
    let obj = params.as_object().ok_or(BridgeError::MalformedRequest)?;
    if obj.get("mode").and_then(Value::as_str) != Some("form") {
        return Err(BridgeError::UnsupportedMode);
    }
    let schema = obj
        .get("requestedSchema")
        .and_then(Value::as_object)
        .ok_or(BridgeError::MalformedRequest)?;
    let message = obj.get("message").and_then(Value::as_str).unwrap_or("");
    let mut questions = Vec::new();
    match schema.get("properties") {
        None => {}
        Some(Value::Object(properties)) => {
            for (key, property) in properties {
                let property = property.as_object().ok_or(BridgeError::MalformedRequest)?;
                let question_id =
                    ObservationId::parse(key.clone()).map_err(|_| BridgeError::InvalidRequest)?;
                let title = property
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|title| !title.is_empty());
                let description = property
                    .get("description")
                    .and_then(Value::as_str)
                    .filter(|description| !description.is_empty());
                let text = description.or(title).unwrap_or(message).to_owned();
                let property_type = property.get("type").and_then(Value::as_str);
                let multi_select = property_type == Some("array");
                let value_type = match property_type {
                    Some("array") => ElicitedType::MultiText,
                    Some("boolean") => ElicitedType::Flag,
                    Some("integer") => ElicitedType::Integer,
                    Some("number") => ElicitedType::Number,
                    _ => ElicitedType::Text,
                };
                let options = elicitation_choices(property)?;
                questions.push(ElicitedQuestion {
                    input: QuestionInput {
                        question_id,
                        text,
                        header: title.map(str::to_owned),
                        multi_select,
                        options,
                    },
                    value_type,
                });
            }
        }
        Some(_) => return Err(BridgeError::MalformedRequest),
    }
    Ok(PendingElicitation {
        provider_id: provider_id.to_owned(),
        questions,
    })
}

fn encode_number(text: &str) -> Result<Value, BridgeError> {
    let trimmed = text.trim();
    let number = if trimmed.is_empty() {
        0.0_f64
    } else {
        trimmed
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or(BridgeError::InvalidAnswer)?
    };
    serde_json::Number::from_f64(number)
        .map(Value::Number)
        .ok_or(BridgeError::InvalidAnswer)
}

/// Encodes explicit answers for one normalized elicitation, mirroring the
/// TypeScript `elicitation_content` evidence with validation before
/// application: arrays pass through, flags match case-insensitive `"true"`,
/// integers/numbers parse as finite JSON numbers (empty counts as zero,
/// mirroring `Number("")`), and text takes the first answer or empty.
/// Unknown answer keys are ignored; missing keys take the type defaults.
///
/// # Errors
///
/// Returns [`BridgeError::InvalidAnswer`] for a non-numeric or non-finite
/// integer/number answer.
pub(crate) fn answer_elicitation(
    pending: &PendingElicitation,
    answers: &BTreeMap<String, Vec<String>>,
) -> Result<Value, BridgeError> {
    let mut content = serde_json::Map::new();
    for question in &pending.questions {
        let key = question.input.question_id.as_str();
        let empty: Vec<String> = Vec::new();
        let values = answers.get(key).unwrap_or(&empty);
        let encoded = match question.value_type {
            ElicitedType::MultiText => Value::Array(
                values
                    .iter()
                    .map(|answer| Value::String(answer.clone()))
                    .collect(),
            ),
            ElicitedType::Flag => Value::Bool(
                values
                    .first()
                    .is_some_and(|answer| answer.eq_ignore_ascii_case("true")),
            ),
            ElicitedType::Integer | ElicitedType::Number => {
                encode_number(values.first().map_or("", String::as_str))?
            }
            ElicitedType::Text => Value::String(values.first().cloned().unwrap_or_default()),
        };
        content.insert(key.to_owned(), encoded);
    }
    Ok(Value::Object(content))
}

// ---------------------------------------------------------------------------
// Pending-request table with idempotent answer application
// ---------------------------------------------------------------------------

/// One pending bridge request slot with its recorded answer, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
enum BridgeSlot {
    Approval {
        pending: PendingApproval,
        answered: Option<bool>,
    },
    Elicitation {
        pending: PendingElicitation,
        answered: Option<BTreeMap<String, Vec<String>>>,
    },
}

/// The caller-owned pending-request table keyed by provider request id.
///
/// Insert on the agent's request, answer on the explicit user decision
/// (idempotent for repeats, conflicting on changed answers), and remove
/// when the observation round resolves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PendingBridgeTable {
    entries: BTreeMap<String, BridgeSlot>,
}

impl PendingBridgeTable {
    /// Creates an empty table.
    #[must_use = "tables must be held by the caller"]
    pub(crate) fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Returns the number of tracked requests.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no request is tracked.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Tracks one normalized permission request, replacing any same-id slot
    /// the way the TypeScript `Map.set` evidence does.
    ///
    /// # Errors
    ///
    /// Returns [`BridgeError::TableFull`] when a new identity would cross
    /// `max_entries`.
    pub(crate) fn insert_approval(
        &mut self,
        max_entries: usize,
        pending: PendingApproval,
    ) -> Result<(), BridgeError> {
        if !self.entries.contains_key(pending.provider_id.as_str())
            && self.entries.len() >= max_entries
        {
            return Err(BridgeError::TableFull);
        }
        self.entries.insert(
            pending.provider_id.clone(),
            BridgeSlot::Approval {
                pending,
                answered: None,
            },
        );
        Ok(())
    }

    /// Tracks one normalized elicitation, replacing any same-id slot.
    ///
    /// # Errors
    ///
    /// Returns [`BridgeError::TableFull`] when a new identity would cross
    /// `max_entries`.
    pub(crate) fn insert_elicitation(
        &mut self,
        max_entries: usize,
        pending: PendingElicitation,
    ) -> Result<(), BridgeError> {
        if !self.entries.contains_key(pending.provider_id.as_str())
            && self.entries.len() >= max_entries
        {
            return Err(BridgeError::TableFull);
        }
        self.entries.insert(
            pending.provider_id.clone(),
            BridgeSlot::Elicitation {
                pending,
                answered: None,
            },
        );
        Ok(())
    }

    /// Applies an explicit approval decision idempotently: repeats with the
    /// same decision succeed, changed decisions fail, unknown ids fail.
    ///
    /// # Errors
    ///
    /// Returns [`BridgeError::UnknownRequest`], [`BridgeError::WrongKind`],
    /// or [`BridgeError::AnswerConflict`].
    pub(crate) fn answer_approval(
        &mut self,
        provider_id: &str,
        approved: bool,
    ) -> Result<PermissionOutcome, BridgeError> {
        match self.entries.get_mut(provider_id) {
            None => Err(BridgeError::UnknownRequest),
            Some(BridgeSlot::Elicitation { .. }) => Err(BridgeError::WrongKind),
            Some(BridgeSlot::Approval { pending, answered }) => {
                if let Some(prior) = *answered {
                    if prior == approved {
                        return Ok(answer_permission(pending, approved));
                    }
                    return Err(BridgeError::AnswerConflict);
                }
                *answered = Some(approved);
                Ok(answer_permission(pending, approved))
            }
        }
    }

    /// Applies explicit elicitation answers idempotently: repeats with the
    /// same answers succeed, changed answers fail, unknown ids fail. Answer
    /// payloads are validated before application.
    ///
    /// # Errors
    ///
    /// Returns [`BridgeError::UnknownRequest`], [`BridgeError::WrongKind`],
    /// [`BridgeError::AnswerConflict`], or [`BridgeError::InvalidAnswer`].
    pub(crate) fn answer_elicitation(
        &mut self,
        provider_id: &str,
        answers: &BTreeMap<String, Vec<String>>,
    ) -> Result<Value, BridgeError> {
        match self.entries.get_mut(provider_id) {
            None => Err(BridgeError::UnknownRequest),
            Some(BridgeSlot::Approval { .. }) => Err(BridgeError::WrongKind),
            Some(BridgeSlot::Elicitation { pending, answered }) => {
                if let Some(prior) = answered {
                    if *prior == *answers {
                        return answer_elicitation(pending, answers);
                    }
                    return Err(BridgeError::AnswerConflict);
                }
                let content = answer_elicitation(pending, answers)?;
                *answered = Some(answers.clone());
                Ok(content)
            }
        }
    }

    /// Drops one tracked request. Returns whether an entry was present.
    pub(crate) fn remove(&mut self, provider_id: &str) -> bool {
        self.entries.remove(provider_id).is_some()
    }
}

// ---------------------------------------------------------------------------
// CompleteAcpMessages completion projection
// ---------------------------------------------------------------------------

/// Accumulated prompt-end text per item, projecting `CompleteAcpMessages`:
/// per-message and per-thought text keyed by item id. The dispatch arm maps
/// the completed pairs onto its durable snapshots; ordering here is by
/// item id for determinism.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AcpCompletionState {
    messages: BTreeMap<String, String>,
    thoughts: BTreeMap<String, String>,
    bytes: usize,
}

impl AcpCompletionState {
    /// Creates an empty completion state.
    #[must_use = "states must be held by the caller"]
    pub(crate) fn new() -> Self {
        Self {
            messages: BTreeMap::new(),
            thoughts: BTreeMap::new(),
            bytes: 0,
        }
    }

    fn push(
        slot: &mut BTreeMap<String, String>,
        bytes: &mut usize,
        max_bytes: usize,
        item_id: &str,
        delta: &str,
    ) -> Result<(), BridgeError> {
        if item_id.is_empty() || item_id.len() > MAX_PROVIDER_ID_BYTES {
            return Err(BridgeError::MalformedRequest);
        }
        let added = item_id
            .len()
            .checked_add(delta.len())
            .ok_or(BridgeError::CompletionTooLarge)?;
        let total = bytes
            .checked_add(added)
            .ok_or(BridgeError::CompletionTooLarge)?;
        if total > max_bytes {
            return Err(BridgeError::CompletionTooLarge);
        }
        *bytes = total;
        slot.entry(item_id.to_owned())
            .and_modify(|text| text.push_str(delta))
            .or_insert_with(|| delta.to_owned());
        Ok(())
    }

    /// Accumulates one agent-message delta. Empty deltas are ignored,
    /// mirroring the normalizer evidence.
    ///
    /// # Errors
    ///
    /// Returns [`BridgeError::MalformedRequest`] for a bad item id, or
    /// [`BridgeError::CompletionTooLarge`] past `max_bytes`.
    pub(crate) fn push_message_text(
        &mut self,
        max_bytes: usize,
        item_id: &str,
        delta: &str,
    ) -> Result<(), BridgeError> {
        if delta.is_empty() {
            return Ok(());
        }
        Self::push(
            &mut self.messages,
            &mut self.bytes,
            max_bytes,
            item_id,
            delta,
        )
    }

    /// Accumulates one thought delta. Entries are touched even for empty
    /// deltas, mirroring the normalizer evidence.
    ///
    /// # Errors
    ///
    /// Returns [`BridgeError::MalformedRequest`] for a bad item id, or
    /// [`BridgeError::CompletionTooLarge`] past `max_bytes`.
    pub(crate) fn push_thought_text(
        &mut self,
        max_bytes: usize,
        item_id: &str,
        delta: &str,
    ) -> Result<(), BridgeError> {
        Self::push(
            &mut self.thoughts,
            &mut self.bytes,
            max_bytes,
            item_id,
            delta,
        )
    }

    /// Returns the completed `(item id, message)` pairs.
    #[must_use = "completed pairs must reach the durable snapshots"]
    pub(crate) fn completed_messages(&self) -> Vec<(String, String)> {
        self.messages
            .iter()
            .map(|(item_id, message)| (item_id.clone(), message.clone()))
            .collect()
    }

    /// Returns the completed `(item id, thought)` pairs.
    #[must_use = "completed pairs must reach the durable snapshots"]
    pub(crate) fn completed_thoughts(&self) -> Vec<(String, String)> {
        self.thoughts
            .iter()
            .map(|(item_id, thought)| (item_id.clone(), thought.clone()))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Fixture tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_domain::ApprovalKind;

    fn permission_params(tool_call: Value, options: Value) -> Value {
        serde_json::json!({
            "toolCall": tool_call,
            "options": options,
        })
    }

    fn standard_options() -> Value {
        serde_json::json!([
            { "kind": "allow_once", "optionId": "allow-1", "label": "Allow" },
            { "kind": "reject_once", "optionId": "reject-1", "label": "Deny" },
        ])
    }

    #[test]
    fn command_approval_maps_command_cwd_reason_options() {
        let pending = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "tool-1",
                "kind": "execute",
                "title": "Run tests",
                "rawInput": { "command": "cargo test", "cwd": "C:\\work" },
            }),
            standard_options(),
        ))
        .expect("command approval");
        assert_eq!(pending.provider_id(), "tool-1");
        assert_eq!(pending.description(), "Run tests");
        assert_eq!(pending.request().kind(), ApprovalKind::Command);
        assert_eq!(pending.request().command_text(), Some("cargo test"));
        assert_eq!(pending.request().cwd(), Some("C:\\work"));
        assert_eq!(pending.request().reason(), Some("Run tests"));
        assert_eq!(
            answer_permission(&pending, true),
            PermissionOutcome::Selected {
                option_id: "allow-1".to_owned(),
            }
        );
        assert_eq!(
            answer_permission(&pending, false),
            PermissionOutcome::Selected {
                option_id: "reject-1".to_owned(),
            }
        );
    }

    #[test]
    fn file_change_and_action_kinds_map() {
        for kind in ["edit", "delete", "move"] {
            let pending = normalize_permission_request(&permission_params(
                serde_json::json!({
                    "toolCallId": format!("tool-{kind}"),
                    "kind": kind,
                    "title": "Touch file",
                }),
                standard_options(),
            ))
            .expect("file approval");
            assert_eq!(pending.request().kind(), ApprovalKind::FileChange);
            assert_eq!(pending.request().command_text(), None);
            assert_eq!(pending.request().reason(), Some("Touch file"));
        }
        let other = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "tool-web",
                "kind": "fetch",
                "title": "Fetch page",
            }),
            standard_options(),
        ))
        .expect("action approval");
        assert_eq!(other.request().kind(), ApprovalKind::Action);
        assert_eq!(other.request().reason(), Some("Fetch page"));
    }

    #[test]
    fn execute_without_command_gates_as_action() {
        let pending = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "tool-nocmd",
                "kind": "execute",
                "title": "Mystery run",
                "rawInput": {},
            }),
            standard_options(),
        ))
        .expect("action fallback");
        assert_eq!(pending.request().kind(), ApprovalKind::Action);
        assert_eq!(pending.request().reason(), Some("Mystery run"));
    }

    #[test]
    fn missing_options_answer_cancelled_never_defaults() {
        let pending = normalize_permission_request(&serde_json::json!({
            "toolCall": {
                "toolCallId": "tool-bare",
                "kind": "execute",
                "rawInput": { "command": "ls" },
            },
        }))
        .expect("options stay optional");
        assert_eq!(pending.description(), DEFAULT_APPROVAL_DESCRIPTION);
        assert_eq!(pending.request().reason(), None);
        assert_eq!(
            answer_permission(&pending, true),
            PermissionOutcome::Cancelled
        );
        assert_eq!(
            answer_permission(&pending, false),
            PermissionOutcome::Cancelled
        );
    }

    #[test]
    fn malformed_permission_payloads_rejected() {
        for params in [
            serde_json::json!(null),
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::json!({ "toolCall": null }),
            serde_json::json!({ "toolCall": [] }),
            serde_json::json!({ "toolCall": { "kind": "execute" } }),
            serde_json::json!({ "toolCall": { "toolCallId": "", "kind": "execute" } }),
        ] {
            assert_eq!(
                normalize_permission_request(&params).expect_err("malformed"),
                BridgeError::MalformedRequest,
                "params: {params}"
            );
        }
        // Well-shaped but domain-rejected: empty command text is InvalidRequest
        // only when it would build a command; here the fallback applies, so an
        // over-long reason exercises the invalid path instead.
        let long_reason = "r".repeat(4_096);
        assert_eq!(
            normalize_permission_request(&permission_params(
                serde_json::json!({
                    "toolCallId": "tool-long",
                    "kind": "fetch",
                    "title": long_reason,
                }),
                standard_options(),
            ))
            .expect_err("oversized reason"),
            BridgeError::InvalidRequest
        );
    }

    fn elicitation_params(properties: Value, message: &str) -> Value {
        serde_json::json!({
            "mode": "form",
            "message": message,
            "requestedSchema": { "properties": properties },
        })
    }

    #[test]
    fn single_select_question_maps_options_and_text() {
        let pending = normalize_elicitation_request(
            "elicit-1",
            &elicitation_params(
                serde_json::json!({
                    "color": {
                        "type": "string",
                        "title": "Pick a color",
                        "description": "Choose wisely",
                        "oneOf": ["red", "green"],
                    },
                }),
                "fallback message",
            ),
        )
        .expect("single select");
        assert_eq!(pending.provider_id(), "elicit-1");
        assert_eq!(pending.questions().len(), 1);
        let question = &pending.questions()[0];
        assert_eq!(question.value_type(), ElicitedType::Text);
        assert_eq!(question.input.text, "Choose wisely");
        assert_eq!(question.input.header.as_deref(), Some("Pick a color"));
        assert!(!question.input.multi_select);
        let options = question.input.options.as_ref().expect("options");
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].label(), "red");
        let answers = BTreeMap::from([("color".to_owned(), vec!["green".to_owned()])]);
        assert_eq!(
            answer_elicitation(&pending, &answers).expect("encode"),
            serde_json::json!({ "color": "green" })
        );
    }

    #[test]
    fn multi_select_question_encodes_arrays() {
        let pending = normalize_elicitation_request(
            "elicit-2",
            &elicitation_params(
                serde_json::json!({
                    "tags": {
                        "type": "array",
                        "title": "Tags",
                        "items": { "enum": ["a", "b"] },
                    },
                }),
                "pick tags",
            ),
        )
        .expect("multi select");
        let question = &pending.questions()[0];
        assert_eq!(question.value_type(), ElicitedType::MultiText);
        assert!(question.input.multi_select);
        assert_eq!(question.input.text, "Tags");
        let answers = BTreeMap::from([("tags".to_owned(), vec!["a".to_owned(), "b".to_owned()])]);
        assert_eq!(
            answer_elicitation(&pending, &answers).expect("encode"),
            serde_json::json!({ "tags": ["a", "b"] })
        );
    }

    #[test]
    fn free_form_question_has_no_options() {
        let pending = normalize_elicitation_request(
            "elicit-3",
            &elicitation_params(
                serde_json::json!({
                    "notes": { "type": "string", "title": "Notes" },
                }),
                "tell me",
            ),
        )
        .expect("free form");
        let question = &pending.questions()[0];
        assert_eq!(question.value_type(), ElicitedType::Text);
        assert_eq!(question.input.options, None);
        assert!(!question.input.multi_select);
        let answers = BTreeMap::from([("notes".to_owned(), vec!["hi".to_owned()])]);
        assert_eq!(
            answer_elicitation(&pending, &answers).expect("encode"),
            serde_json::json!({ "notes": "hi" })
        );
        let missing = BTreeMap::new();
        assert_eq!(
            answer_elicitation(&pending, &missing).expect("defaults"),
            serde_json::json!({ "notes": "" })
        );
    }

    #[test]
    fn scalar_types_encode_with_validation() {
        let pending = normalize_elicitation_request(
            "elicit-4",
            &elicitation_params(
                serde_json::json!({
                    "enabled": { "type": "boolean", "title": "On?" },
                    "count": { "type": "integer", "title": "N" },
                    "ratio": { "type": "number", "title": "R" },
                }),
                "scalars",
            ),
        )
        .expect("scalars");
        let answers = BTreeMap::from([
            ("enabled".to_owned(), vec!["TRUE".to_owned()]),
            ("count".to_owned(), vec!["42".to_owned()]),
            ("ratio".to_owned(), vec!["".to_owned()]),
        ]);
        let content = answer_elicitation(&pending, &answers).expect("encode");
        assert_eq!(content.get("enabled"), Some(&Value::Bool(true)));
        assert_eq!(content.get("count"), Some(&serde_json::json!(42.0)));
        assert_eq!(content.get("ratio"), Some(&serde_json::json!(0.0)));
        let bad = BTreeMap::from([("count".to_owned(), vec!["many".to_owned()])]);
        assert_eq!(
            answer_elicitation(&pending, &bad).expect_err("invalid number"),
            BridgeError::InvalidAnswer
        );
    }

    #[test]
    fn object_options_keep_title_const_labels_and_skip_junk() {
        let pending = normalize_elicitation_request(
            "elicit-5",
            &elicitation_params(
                serde_json::json!({
                    "choice": {
                        "type": "string",
                        "title": "Pick",
                        "oneOf": [
                            { "title": "First", "description": "The first" },
                            { "const": "second" },
                            42,
                            { "description": "no label" },
                        ],
                    },
                }),
                "msg",
            ),
        )
        .expect("object options");
        let options = pending.questions()[0]
            .input
            .options
            .as_ref()
            .expect("options");
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].label(), "First");
        assert_eq!(options[0].description(), Some("The first"));
        assert_eq!(options[1].label(), "second");
        assert_eq!(options[1].description(), None);
    }

    #[test]
    fn malformed_elicitation_payloads_rejected() {
        assert_eq!(
            normalize_elicitation_request(
                "e",
                &serde_json::json!({ "mode": "dialog", "requestedSchema": {} }),
            )
            .expect_err("non-form"),
            BridgeError::UnsupportedMode
        );
        for params in [
            serde_json::json!(null),
            serde_json::json!({ "mode": "form" }),
            serde_json::json!({ "mode": "form", "requestedSchema": [] }),
            serde_json::json!({
                "mode": "form",
                "requestedSchema": { "properties": [] },
            }),
            serde_json::json!({
                "mode": "form",
                "requestedSchema": { "properties": { "q": [] } },
            }),
        ] {
            assert_eq!(
                normalize_elicitation_request("e", &params).expect_err("malformed"),
                BridgeError::MalformedRequest,
                "params: {params}"
            );
        }
        assert_eq!(
            normalize_elicitation_request("", &elicitation_params(serde_json::json!({}), "m"),)
                .expect_err("empty provider id"),
            BridgeError::MalformedRequest
        );
        assert_eq!(
            normalize_elicitation_request(
                "e",
                &elicitation_params(serde_json::json!({ "bad id": { "type": "string" } }), "m",),
            )
            .expect_err("bad question id"),
            BridgeError::InvalidRequest
        );
        assert_eq!(
            normalize_elicitation_request(
                "e",
                &elicitation_params(
                    serde_json::json!({
                        "q": {
                            "type": "string",
                            "title": "T",
                            "oneOf": [{ "title": "" }],
                        },
                    }),
                    "m",
                ),
            )
            .expect_err("empty option label"),
            BridgeError::InvalidRequest
        );
    }

    #[test]
    fn approval_answers_are_idempotent_then_conflicting() {
        let mut table = PendingBridgeTable::new();
        assert!(table.is_empty());
        let pending = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "tool-7",
                "kind": "execute",
                "title": "Run",
                "rawInput": { "command": "make" },
            }),
            standard_options(),
        ))
        .expect("normalize");
        table.insert_approval(8, pending).expect("insert");
        assert_eq!(table.len(), 1);
        let first = table.answer_approval("tool-7", true).expect("answer");
        let repeat = table.answer_approval("tool-7", true).expect("idempotent");
        assert_eq!(first, repeat);
        assert_eq!(
            table
                .answer_approval("tool-7", false)
                .expect_err("conflict"),
            BridgeError::AnswerConflict
        );
        assert_eq!(
            table.answer_approval("nope", true).expect_err("unknown"),
            BridgeError::UnknownRequest
        );
        assert!(table.remove("tool-7"));
        assert!(!table.remove("tool-7"));
        assert!(table.is_empty());
    }

    #[test]
    fn elicitation_answers_are_idempotent_then_conflicting() {
        let mut table = PendingBridgeTable::new();
        let pending = normalize_elicitation_request(
            "elicit-9",
            &elicitation_params(
                serde_json::json!({ "color": { "type": "string", "title": "C" } }),
                "msg",
            ),
        )
        .expect("normalize");
        table.insert_elicitation(8, pending).expect("insert");
        let answers = BTreeMap::from([("color".to_owned(), vec!["red".to_owned()])]);
        let first = table
            .answer_elicitation("elicit-9", &answers)
            .expect("answer");
        assert_eq!(first, serde_json::json!({ "color": "red" }));
        let repeat = table
            .answer_elicitation("elicit-9", &answers)
            .expect("idempotent");
        assert_eq!(first, repeat);
        let changed = BTreeMap::from([("color".to_owned(), vec!["blue".to_owned()])]);
        assert_eq!(
            table
                .answer_elicitation("elicit-9", &changed)
                .expect_err("conflict"),
            BridgeError::AnswerConflict
        );
        assert_eq!(
            table
                .answer_approval("elicit-9", true)
                .expect_err("wrong kind"),
            BridgeError::WrongKind
        );
    }

    #[test]
    fn table_bound_rejects_new_identities() {
        let mut table = PendingBridgeTable::new();
        let first = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "a",
                "kind": "action",
                "title": "first",
            }),
            standard_options(),
        ))
        .expect("normalize");
        table.insert_approval(1, first).expect("insert");
        // Replacing the same identity must not count against the bound.
        let again = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "a",
                "kind": "action",
                "title": "again",
            }),
            standard_options(),
        ))
        .expect("normalize");
        table
            .insert_approval(1, again)
            .expect("replace under bound");
        assert_eq!(table.len(), 1);
        assert_eq!(
            table.answer_approval("a", true).expect("answer replaced"),
            PermissionOutcome::Selected {
                option_id: "allow-1".to_owned(),
            }
        );
        let extra = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "overflow",
                "kind": "action",
            }),
            standard_options(),
        ))
        .expect("normalize");
        assert_eq!(
            table.insert_approval(1, extra).expect_err("table full"),
            BridgeError::TableFull
        );
    }

    #[test]
    fn completion_projection_accumulates_and_completes() {
        let mut state = AcpCompletionState::new();
        state
            .push_message_text(1_024, "m2", "hello ")
            .expect("message");
        state
            .push_message_text(1_024, "m1", "first")
            .expect("message");
        state
            .push_message_text(1_024, "m2", "world")
            .expect("append");
        state
            .push_message_text(1_024, "m1", "")
            .expect("empty delta ignored");
        state.push_thought_text(1_024, "t1", "").expect("touch");
        state
            .push_thought_text(1_024, "t1", "hmm")
            .expect("thought");
        assert_eq!(
            state.completed_messages(),
            vec![
                ("m1".to_owned(), "first".to_owned()),
                ("m2".to_owned(), "hello world".to_owned()),
            ]
        );
        assert_eq!(
            state.completed_thoughts(),
            vec![("t1".to_owned(), "hmm".to_owned())]
        );
        assert_eq!(
            state
                .push_message_text(10, "m3", "way too long for ten bytes")
                .expect_err("cap"),
            BridgeError::CompletionTooLarge
        );
        assert_eq!(
            state
                .push_message_text(1_024, "", "x")
                .expect_err("empty id"),
            BridgeError::MalformedRequest
        );
    }
}
