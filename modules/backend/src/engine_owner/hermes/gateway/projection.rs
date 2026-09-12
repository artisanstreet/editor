//! Gateway event projection: approval/question decoding, pending interaction
//! tracking, usage sampling, and the streaming observation normalizer.

use std::collections::HashMap;

use artisan_domain::{
    ApprovalRequest, EngineModelId, EngineRouteId, ObservationId, ObservationSequence,
    QuestionInput, QuestionOption, RunId, RunUsageBasis, RunUsageReport, RunUsageReportInput,
    SubagentInput, SubagentObservation, SubagentState, ThreadId, UnixMillis,
};
use serde_json::Value;
use tokio::sync::mpsc;

use super::super::adapter::{HermesSettings, HermesTurnError};
use super::wire::HermesEvent;
use super::{
    HERMES_MAX_ID_BYTES, HERMES_MAX_OPTIONS_PER_QUESTION, HERMES_MAX_QUESTIONS_PER_FRAME,
    HERMES_MAX_TEXT_FIELD_BYTES,
};
use crate::engine_owner::observation::{
    EngineObservation, TerminalState, UsageObservation, chunk_text,
};

#[cfg(test)]
use super::HERMES_MAX_ANSWERS;
#[cfg(test)]
use super::session::GatewayClient;
#[cfg(test)]
use super::wire::RequestScope;

/// One typed Hermes approval request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HermesApprovalRequest {
    approval_id: String,
    description: String,
    command: Option<String>,
}

impl HermesApprovalRequest {
    /// Maps this request onto the domain approval vocabulary.
    ///
    /// The owner validates every request through this constructor before
    /// tracking it, so an out-of-bound provider frame never reaches the
    /// durable A-approve rows.
    ///
    /// # Errors
    ///
    /// Returns [`HermesTurnError::Configuration`] when the bounded fields
    /// violate domain ceilings.
    pub(crate) fn to_domain_request(&self) -> Result<ApprovalRequest, HermesTurnError> {
        if let Some(command) = self.command.clone() {
            ApprovalRequest::command(command, None, Some(self.description.clone()))
                .map_err(|_| HermesTurnError::Configuration)
        } else {
            ApprovalRequest::action(Some(self.description.clone()))
                .map_err(|_| HermesTurnError::Configuration)
        }
    }
}

/// One typed Hermes question.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HermesQuestion {
    question_id: String,
    question_key: Option<String>,
    text: String,
    multi_select: bool,
    options: Vec<String>,
}

impl HermesQuestion {
    /// Returns the provider question identity.
    pub(crate) fn question_id(&self) -> &str {
        &self.question_id
    }

    /// Maps this question onto the domain question vocabulary.
    ///
    /// # Errors
    ///
    /// Returns [`HermesTurnError::Configuration`] when identities, text, or
    /// options violate domain ceilings.
    pub(crate) fn to_domain_input(&self) -> Result<QuestionInput, HermesTurnError> {
        let question_id = ObservationId::parse(self.question_id.clone())
            .map_err(|_| HermesTurnError::Configuration)?;
        if self.text.trim().is_empty() {
            return Err(HermesTurnError::Configuration);
        }
        let mut options = Vec::new();
        for label in &self.options {
            options.push(
                QuestionOption::new(label.clone(), None)
                    .map_err(|_| HermesTurnError::Configuration)?,
            );
        }
        Ok(QuestionInput {
            question_id,
            text: self.text.clone(),
            header: Some("Hermes question".to_owned()),
            multi_select: self.multi_select,
            options: if options.is_empty() {
                None
            } else {
                Some(options)
            },
        })
    }
}

/// One typed Hermes question request frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HermesQuestionRequest {
    request_id: String,
    questions: Vec<HermesQuestion>,
}

impl HermesQuestionRequest {
    /// Returns the questions in this request group.
    pub(crate) fn questions(&self) -> &[HermesQuestion] {
        &self.questions
    }
}

fn bounded_text(value: &Value, field: &str) -> Option<String> {
    let text = value.get(field)?.as_str()?;
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > HERMES_MAX_TEXT_FIELD_BYTES {
        return None;
    }
    Some(trimmed.to_owned())
}

/// Decodes one `approval.request` event into a typed request.
pub(crate) fn decode_approval(event: &HermesEvent) -> Option<HermesApprovalRequest> {
    if event.event_type() != "approval.request" {
        return None;
    }
    let payload = event.payload().as_object()?;
    let id = payload
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)?;
    let command = payload
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let description = payload
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("Hermes requests approval to continue.")
        .to_owned();
    Some(HermesApprovalRequest {
        approval_id: id.to_owned(),
        description,
        command,
    })
}

/// Decodes one `clarify.request` event into typed question requests.
pub(crate) fn decode_questions(event: &HermesEvent) -> Vec<HermesQuestionRequest> {
    if event.event_type() != "clarify.request" {
        return Vec::new();
    }
    let Some(payload) = event.payload().as_object() else {
        return Vec::new();
    };
    let Some(request_id) = payload
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)
    else {
        return Vec::new();
    };
    if let Some(items) = payload.get("questions").and_then(Value::as_array) {
        let mut questions = Vec::new();
        for item in items.iter().take(HERMES_MAX_QUESTIONS_PER_FRAME) {
            if let Some(question) = decode_listed_question(request_id, item) {
                questions.push(question);
            }
        }
        if questions.is_empty() {
            return Vec::new();
        }
        return vec![HermesQuestionRequest {
            request_id: request_id.to_owned(),
            questions,
        }];
    }
    let Some(question) = decode_single_question(request_id, payload) else {
        return Vec::new();
    };
    vec![HermesQuestionRequest {
        request_id: request_id.to_owned(),
        questions: vec![question],
    }]
}

fn decode_listed_question(request_id: &str, item: &Value) -> Option<HermesQuestion> {
    let object = item.as_object()?;
    let key = object
        .get("qid")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty() && key.len() <= HERMES_MAX_ID_BYTES)?;
    let text = bounded_text(item, "question")?;
    Some(HermesQuestion {
        question_id: format!("{request_id}:{key}"),
        question_key: Some(key.to_owned()),
        text,
        multi_select: object
            .get("multi_select")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        options: decode_options(object.get("choices")),
    })
}

fn decode_single_question(
    request_id: &str,
    payload: &serde_json::Map<String, Value>,
) -> Option<HermesQuestion> {
    let value = Value::Object(payload.clone());
    let text = bounded_text(&value, "question")?;
    Some(HermesQuestion {
        question_id: request_id.to_owned(),
        question_key: None,
        text,
        multi_select: payload
            .get("multi_select")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        options: decode_options(payload.get("choices")),
    })
}

fn decode_options(choices: Option<&Value>) -> Vec<String> {
    choices
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(HERMES_MAX_OPTIONS_PER_QUESTION)
                .filter_map(|choice| {
                    choice
                        .as_str()
                        .filter(|label| {
                            !label.trim().is_empty() && label.len() <= HERMES_MAX_TEXT_FIELD_BYTES
                        })
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// In-memory pending interaction tracker for one live Hermes turn.
///
/// Approval requests land as pending approvals and `clarify.request` frames
/// land as pending questions. Resolutions apply through the durable resolve
/// path; a deny records the decision with no turn side effect while the run
/// continues. Subagent discoveries are retained for transcript projection but
/// never adopt the root turn.
#[derive(Debug, Default)]
pub(crate) struct HermesPendingTracker {
    approvals: HashMap<String, HermesApprovalRequest>,
    questions: HashMap<String, HermesQuestion>,
    subagents: Vec<(String, String)>,
}

impl HermesPendingTracker {
    /// Creates an empty tracker for one turn.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Notes one approval request; re-noting the same id is a no-op.
    ///
    /// The request is validated through the domain constructor first, so an
    /// out-of-bound provider frame never reaches the durable A-approve rows.
    pub(crate) fn note_approval(&mut self, request: HermesApprovalRequest) -> bool {
        if request.to_domain_request().is_err() {
            return false;
        }
        if self.approvals.contains_key(&request.approval_id) {
            return false;
        }
        self.approvals.insert(request.approval_id.clone(), request);
        true
    }

    /// Notes one question request group; re-noting the same id is a no-op.
    ///
    /// Each question is validated through the domain constructor first, so
    /// an out-of-bound provider frame never reaches the durable rows.
    pub(crate) fn note_questions(&mut self, request: &HermesQuestionRequest) -> usize {
        let mut added = 0;
        for question in request.questions() {
            if question.to_domain_input().is_err() {
                continue;
            }
            if !self.questions.contains_key(question.question_id()) {
                self.questions
                    .insert(question.question_id().to_owned(), question.clone());
                added += 1;
            }
        }
        added
    }

    /// Notes one subagent discovery without adopting the root turn.
    pub(crate) fn note_subagent(&mut self, agent_thread_id: &str, parent_thread_id: &str) {
        self.subagents
            .push((agent_thread_id.to_owned(), parent_thread_id.to_owned()));
    }

    /// Resolves one approval; returns whether it was pending.
    ///
    /// Test-only until dispatcher delivery wiring lands.
    #[cfg(test)]
    pub(crate) fn resolve_approval(&mut self, approval_id: &str) -> bool {
        self.approvals.remove(approval_id).is_some()
    }

    /// Resolves one question; returns whether it was pending.
    ///
    /// Test-only until dispatcher delivery wiring lands.
    #[cfg(test)]
    pub(crate) fn resolve_question(&mut self, question_id: &str) -> bool {
        self.questions.remove(question_id).is_some()
    }

    /// Returns the number of pending approvals.
    #[cfg(test)]
    pub(crate) fn pending_approvals(&self) -> usize {
        self.approvals.len()
    }

    /// Returns the number of pending questions.
    #[cfg(test)]
    pub(crate) fn pending_questions(&self) -> usize {
        self.questions.len()
    }
}

/// Answers one pending approval through the durable decision.
///
/// Test-only until dispatcher delivery wiring lands: deny carries no turn
/// side effect and the run continues either way.
///
/// # Errors
///
/// Returns [`HermesTurnError::Configuration`] for an unknown or resolved
/// target and [`HermesTurnError::StreamFailed`] when the gateway fails.
#[cfg(test)]
pub(crate) async fn answer_approval(
    client: &mut GatewayClient,
    tracker: &mut HermesPendingTracker,
    runtime_session_id: &str,
    approval_id: &str,
    approved: bool,
    scope: &RequestScope<'_>,
) -> Result<(), HermesTurnError> {
    if !tracker.resolve_approval(approval_id) {
        return Err(HermesTurnError::Configuration);
    }
    let params = serde_json::json!({
        "choice": if approved { "once" } else { "deny" },
        "request_id": approval_id,
        "session_id": runtime_session_id,
    });
    client
        .request("approval.respond", params, scope)
        .await
        .map_err(|_| HermesTurnError::StreamFailed)?;
    Ok(())
}

/// Answers one question request group through the durable answers.
///
/// Test-only until dispatcher delivery wiring lands.
///
/// # Errors
///
/// Returns [`HermesTurnError::Configuration`] for an unknown or resolved
/// target and [`HermesTurnError::StreamFailed`] when the gateway fails.
#[cfg(test)]
pub(crate) async fn answer_questions(
    client: &mut GatewayClient,
    tracker: &mut HermesPendingTracker,
    runtime_session_id: &str,
    request: &HermesQuestionRequest,
    answers: &[(String, Vec<String>)],
    scope: &RequestScope<'_>,
) -> Result<(), HermesTurnError> {
    for (question_id, _) in answers {
        if !tracker.resolve_question(question_id) {
            return Err(HermesTurnError::Configuration);
        }
    }
    for (question_id, options) in answers.iter().take(HERMES_MAX_ANSWERS) {
        let Some(question) = request
            .questions()
            .iter()
            .find(|known| known.question_id() == question_id)
        else {
            return Err(HermesTurnError::Configuration);
        };
        let mut params = serde_json::Map::new();
        params.insert("answer".to_owned(), Value::String(options.join(", ")));
        if let Some(key) = question.question_key.clone() {
            params.insert("question_id".to_owned(), Value::String(key));
        }
        params.insert(
            "request_id".to_owned(),
            Value::String(request.request_id.clone()),
        );
        params.insert(
            "session_id".to_owned(),
            Value::String(runtime_session_id.to_owned()),
        );
        client
            .request("clarify.respond", Value::Object(params), scope)
            .await
            .map_err(|_| HermesTurnError::StreamFailed)?;
    }
    Ok(())
}

/// Cumulative usage sample from one gateway usage payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UsageSample {
    input: Option<u64>,
    output: Option<u64>,
    context: Option<u64>,
    context_max: Option<u64>,
}

/// Extracts the cumulative usage sample from a gateway payload.
///
/// Reads `usage` when present, else the payload itself; accepts both the
/// short (`input`/`output`) and long (`input_tokens`/`output_tokens`)
/// spellings. Returns `None` when no token field carries a value: Hermes
/// reports cumulative totals, and an empty measurement is not a report.
///
/// Hermes `cost_usd` has no S1a usage row and is dropped at the boundary.
pub(crate) fn usage_sample(payload: &Value) -> Option<UsageSample> {
    let source = payload
        .get("usage")
        .filter(|usage| usage.is_object())
        .unwrap_or(payload);
    let number = |fields: &[&str]| {
        fields
            .iter()
            .find_map(|field| source.get(*field).and_then(Value::as_u64))
    };
    let sample = UsageSample {
        input: number(&["input", "input_tokens"]),
        output: number(&["output", "output_tokens"]),
        context: number(&["context_used"]),
        context_max: number(&["context_max"]),
    };
    if sample.input.is_none()
        && sample.output.is_none()
        && sample.context.is_none()
        && sample.context_max.is_none()
    {
        return None;
    }
    Some(sample)
}

fn current_unix_millis() -> Option<UnixMillis> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    let millis = i64::try_from(duration.as_millis()).ok()?;
    Some(UnixMillis::from_millis(millis))
}

/// Builds the cumulative usage report for one sample.
///
/// Fails closed (`None`) when the thread scope is missing, the selection
/// identities do not parse, or the report violates domain bounds: usage is
/// never synthesized from partial identities.
#[expect(
    clippy::too_many_arguments,
    reason = "arguments mirror the usage-report domain fields one-for-one; a wrapper struct would only rename them"
)]
pub(crate) fn usage_report(
    run_id: &RunId,
    thread_id: Option<&ThreadId>,
    provider_session: &str,
    provider_turn: Option<String>,
    source_sequence: u64,
    settings: &HermesSettings,
    sample: &UsageSample,
    observed_at: UnixMillis,
) -> Option<RunUsageReport> {
    let thread_id = thread_id.cloned()?;
    let model_id = EngineModelId::parse(settings.model_id().to_owned()).ok()?;
    let provider_route_id = EngineRouteId::parse(settings.route_id().to_owned()).ok()?;
    RunUsageReport::new(RunUsageReportInput {
        run_id: run_id.clone(),
        thread_id,
        provider_session_id: provider_session.to_owned(),
        source_sequence,
        model_id,
        provider_route_id,
        variant_id: None,
        basis: RunUsageBasis::Cumulative,
        provider_turn_id: provider_turn,
        input_tokens: sample.input,
        cached_input_tokens: None,
        output_tokens: sample.output,
        context_tokens: sample.context,
        context_window_tokens: sample.context_max,
        observed_at,
    })
    .ok()
}

/// One normalized Hermes observation for the owner pump.
pub(crate) enum HermesObservation {
    Text(String),
    Usage(UsageSample),
    Terminal(TerminalState),
    Subagent(SubagentObservation),
    Approval(HermesApprovalRequest),
    Question(HermesQuestionRequest),
}

/// Stateful projection from Hermes gateway events into owner observations.
///
/// Mirrors the TypeScript normalizer: message deltas and interim/final text
/// project to text, usage payloads project to cumulative samples, terminal
/// failures project distinctly, subagent lifecycles project without adopting
/// the root turn, and approvals/questions decode for the pending tracker.
/// Reasoning deltas, tool frames, and compaction markers carry no S1a row.
pub(crate) struct HermesNormalizer {
    turn_index: u64,
    frame_sequence: u64,
    active_compaction: Option<String>,
    compaction_index: u64,
}

impl HermesNormalizer {
    /// Creates a normalizer for one turn.
    pub(crate) fn new() -> Self {
        Self {
            turn_index: 0,
            frame_sequence: 0,
            active_compaction: None,
            compaction_index: 0,
        }
    }

    /// Returns the current provider turn identity for usage attribution.
    pub(crate) fn turn_label(&self, run_id: &RunId) -> String {
        format!("{}:turn:{}", run_id.as_str(), self.turn_index)
    }

    /// Normalizes one gateway event into owner observations.
    pub(crate) fn normalize(
        &mut self,
        run_id: &RunId,
        event: &HermesEvent,
    ) -> Vec<HermesObservation> {
        self.frame_sequence = self.frame_sequence.wrapping_add(1);
        let sequence = self.frame_sequence;
        let payload = event.payload().as_object();
        if self.active_compaction.is_some() && is_compaction_resume(event) {
            self.active_compaction = None;
        }
        match event.event_type() {
            "message.start" => {
                self.turn_index = self.turn_index.wrapping_add(1);
                Vec::new()
            }
            "message.delta" => match payload
                .and_then(|object| {
                    object
                        .get("text")
                        .or_else(|| object.get("rendered"))
                        .and_then(Value::as_str)
                })
                .filter(|text| !text.is_empty())
            {
                Some(delta) => vec![HermesObservation::Text(delta.to_owned())],
                None => Vec::new(),
            },
            "message.interim" => match payload
                .and_then(|object| object.get("text"))
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
            {
                Some(text) => vec![HermesObservation::Text(text.to_owned())],
                None => Vec::new(),
            },
            "message.complete" => {
                let mut out = Vec::new();
                if let Some(text) = payload
                    .and_then(|object| object.get("text"))
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    out.push(HermesObservation::Text(text.to_owned()));
                }
                if let Some(sample) = usage_sample(event.payload()) {
                    out.push(HermesObservation::Usage(sample));
                }
                let failed = payload
                    .and_then(|object| object.get("failure_reason"))
                    .and_then(Value::as_str)
                    .is_some();
                self.active_compaction = None;
                out.push(HermesObservation::Terminal(if failed {
                    TerminalState::Failed
                } else {
                    TerminalState::Completed
                }));
                out
            }
            "approval.request" => match decode_approval(event) {
                Some(request) => vec![HermesObservation::Approval(request)],
                None => Vec::new(),
            },
            "clarify.request" => decode_questions(event)
                .into_iter()
                .map(HermesObservation::Question)
                .collect(),
            "status.update" => {
                let compacting = payload
                    .and_then(|object| object.get("kind"))
                    .and_then(Value::as_str)
                    == Some("compacting");
                if compacting && self.active_compaction.is_none() {
                    self.compaction_index = self.compaction_index.wrapping_add(1);
                    self.active_compaction = Some(format!(
                        "{}:turn:{}:compaction:{}",
                        run_id.as_str(),
                        self.turn_index,
                        self.compaction_index
                    ));
                }
                Vec::new()
            }
            "session.usage" => match usage_sample(event.payload()) {
                Some(sample) => vec![HermesObservation::Usage(sample)],
                None => Vec::new(),
            },
            "subagent.spawn_requested"
            | "subagent.start"
            | "subagent.progress"
            | "subagent.complete" => match subagent_row(run_id, event, sequence) {
                Some(row) => vec![HermesObservation::Subagent(row)],
                None => Vec::new(),
            },
            "error" => vec![HermesObservation::Terminal(TerminalState::Failed)],
            _ => Vec::new(),
        }
    }
}

/// Events the gateway itself treats as proof that a compacted turn resumed.
fn is_compaction_resume(event: &HermesEvent) -> bool {
    if event.event_type() == "status.update" {
        return event.payload().get("kind").and_then(Value::as_str) == Some("compacted");
    }
    matches!(
        event.event_type(),
        "message.start"
            | "message.delta"
            | "message.interim"
            | "thinking.delta"
            | "reasoning.delta"
            | "reasoning.available"
            | "moa.reference"
            | "moa.aggregating"
            | "moa.progress"
            | "moa.phase"
            | "tool.start"
            | "tool.progress"
            | "tool.generating"
            | "tool.complete"
    )
}

fn subagent_row(run_id: &RunId, event: &HermesEvent, sequence: u64) -> Option<SubagentObservation> {
    let payload = event.payload().as_object()?;
    let agent_id = payload
        .get("subagent_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)?;
    let parent = payload
        .get("parent_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)
        .or_else(|| event.session_id())
        .filter(|id| !id.is_empty() && id.len() <= HERMES_MAX_ID_BYTES)?;
    let status = payload.get("status").and_then(Value::as_str);
    let state = match event.event_type() {
        "subagent.complete" if status == Some("failed") => SubagentState::Failed,
        "subagent.complete" => SubagentState::Completed,
        "subagent.spawn_requested" => SubagentState::Discovered,
        _ => SubagentState::Running,
    };
    let activity = payload
        .get("goal")
        .or_else(|| payload.get("summary"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let id = ObservationId::parse(format!(
        "{}:hermes:subagent:{agent_id}:{sequence}",
        run_id.as_str()
    ))
    .ok()?;
    let observation_sequence = ObservationSequence::new(sequence).ok()?;
    SubagentObservation::new(
        id,
        observation_sequence,
        SubagentInput {
            agent_native_thread_id: ObservationId::parse(agent_id).ok()?,
            parent_native_thread_id: ObservationId::parse(parent).ok()?,
            state,
            activity,
            agent_path: None,
            turn_id: None,
        },
    )
    .ok()
}

/// Borrowed context for applying one normalized observation batch.
pub(crate) struct ApplyContext<'a> {
    pub(crate) run_id: &'a RunId,
    pub(crate) thread_id: Option<&'a ThreadId>,
    pub(crate) settings: &'a HermesSettings,
    pub(crate) runtime_session_id: &'a str,
    pub(crate) tracker: &'a mut HermesPendingTracker,
    pub(crate) active_turn: &'a mut Option<String>,
    pub(crate) observations: &'a mpsc::Sender<EngineObservation>,
    pub(crate) frame_sequence: u64,
}

/// Applies one normalized batch; returns the terminal state when the turn ends.
///
/// Text projects onto the shared chunked vocabulary; usage reports fail
/// closed to nothing when their identities do not validate; approvals and
/// questions populate the pending tracker with no control-flow side effect;
/// subagent rows travel the owner channel without ever adopting the root
/// turn.
pub(crate) async fn apply_observations(
    normalizer: &mut HermesNormalizer,
    event: &HermesEvent,
    context: ApplyContext<'_>,
) -> Option<TerminalState> {
    let ApplyContext {
        run_id,
        thread_id,
        settings,
        runtime_session_id,
        tracker,
        active_turn,
        observations,
        frame_sequence,
    } = context;
    if event.session_id() != Some(runtime_session_id) {
        return None;
    }
    for observation in normalizer.normalize(run_id, event) {
        match observation {
            HermesObservation::Text(delta) => {
                if active_turn.is_none() {
                    *active_turn = Some(runtime_session_id.to_owned());
                }
                let native_id = format!("hermes:{frame_sequence}");
                for chunk in chunk_text(run_id, frame_sequence, &native_id, &delta) {
                    if observations
                        .send(EngineObservation::TextDelta(chunk))
                        .await
                        .is_err()
                    {
                        return Some(TerminalState::Interrupted);
                    }
                }
            }
            HermesObservation::Usage(sample) => {
                let Some(observed_at) = current_unix_millis() else {
                    continue;
                };
                let report = usage_report(
                    run_id,
                    thread_id,
                    runtime_session_id,
                    Some(normalizer.turn_label(run_id)),
                    frame_sequence,
                    settings,
                    &sample,
                    observed_at,
                );
                if let Some(report) = report
                    && observations
                        .send(EngineObservation::Usage(UsageObservation::new(report)))
                        .await
                        .is_err()
                {
                    return Some(TerminalState::Interrupted);
                }
            }
            HermesObservation::Terminal(state) => return Some(state),
            HermesObservation::Subagent(row) => {
                tracker.note_subagent(
                    row.agent_native_thread_id().as_str(),
                    row.parent_native_thread_id().as_str(),
                );
                if observations
                    .send(EngineObservation::Subagent(
                        crate::engine_owner::observation::SubagentLifecycleRow::new(row),
                    ))
                    .await
                    .is_err()
                {
                    return Some(TerminalState::Interrupted);
                }
            }
            HermesObservation::Approval(request) => {
                tracker.note_approval(request);
            }
            HermesObservation::Question(request) => {
                tracker.note_questions(&request);
            }
        }
    }
    None
}
