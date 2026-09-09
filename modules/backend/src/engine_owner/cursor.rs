//! Finite C1 Cursor runtime on the ACP core: definition plus dispatch arms,
//! not runnable yet.
//!
//! Spawns nothing here. This leaf owns the native Cursor definition row over
//! the shared A1 transport core plus the A2 bridges: typed [`CursorSettings`]
//! derived from the durable [`CursorSelection`](artisan_domain::CursorSelection),
//! the model resolver (effort appended unless suffixed, `-fast` handling,
//! bracket passthrough), the args builder (`--mode ask` on read-only,
//! `--force` mapping, then `acp`), the `AE-PROVIDER-206` startup classifier
//! with model capture, the `cursor/ask_question` and `cursor/create_plan`
//! plan-approval extension mapping, and image-block mode. It extends the ACP
//! core with the cursor definition row and never forks it: spawning,
//! framing, handshake, session, update-loop, and teardown behavior stay in
//! `super::acp`, and permission/elicitation bridging stays in
//! `super::acp_bridges`.
//!
//! One `EngineOwner` task and queue stays the authority for admission,
//! custody, and quarantine; the dispatch arm in `super::operation` and the
//! dispatcher branch in `crate::native_run_dispatch` add only new match arms
//! beside the X1 Codex arm. The actual `cursor-agent` CLI is the only
//! executable this packet names; no fixture binary and no raw JSON cross into
//! the domain.
//!
//! Explicit non-goals for C1: catalog flag flip, frontend selection, account
//! usage beyond the disclosed-absent record below, and any engine beyond the
//! cursor row.
//!
//! Later packet: cursor-account catalog merge design (recorded, not
//! implemented).
//!
//! The curated Cursor catalog stays unread in C1. A later packet merges the
//! authenticated dashboard account surface (`MakeCursorUsage` in
//! `modules/engines/src/cursor/usage.ts`) with the curated model list the way
//! `crate::native_model_catalog` merges the OpenCode2 runtime result: only
//! rows disclosed for this account become runnable, static rows for other
//! harnesses stay readable but unavailable to new policy admission, and no
//! thinking, speed, cost, or image-input value is inferred when the provider
//! did not report it. Usage stays non-billable and never starts a run.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]
// C1 has no live dispatch caller yet for the extension parsers, the steer and
// continuation checks, or the usage record: the finite dispatch arm fails
// closed before spawning, and the fixture tests below prove the wire shape.
// Every item is covered by the inline tests.
#![allow(dead_code)]

use std::ffi::OsString;

use artisan_domain::{
    ApprovalRequest, CursorPermissionMode, CursorSelection, CursorSpeed, FilesystemAccess,
    ObservationId, PlanEntry, PlanEntryStatus, QuestionInput, QuestionOption,
};
use serde_json::Value;
use thiserror::Error;

use super::acp::{AcpDefinition, CURSOR_ACP, ImageMode, LaunchArgs, cursor_build_args};

/// Engine id carried by the C1 cursor definition row.
pub(crate) const CURSOR_ENGINE_ID: &str = "cursor";

/// Sentinel version carried by [`CursorLaunch`] until the probe/authority
/// packet lands. Preflight, catalog, and turn paths reject the cursor launch
/// before any version gate reads it; the value never authorizes execution.
pub(crate) const CURSOR_C1_UNPROBED_VERSION: &str = "0.0.0-cursor-c1-unprobed";

/// Reason reported by [`check_cursor_native_continuation`], mirroring the
/// TypeScript `native_continuation` capability: ACP does not guarantee that a
/// loaded session may change model identity.
pub(crate) const CURSOR_NATIVE_CONTINUATION_REASON: &str =
    "ACP does not guarantee that a loaded session may change model identity.";

/// Honest usage record: account usage is disclosed only through the
/// authenticated dashboard surface, so this packet claims none. Usage stays
/// exactly where the provider discloses it and never starts a run.
pub(crate) const CURSOR_USAGE_ABSENT_REASON: &str = "Cursor account usage is disclosed only through the authenticated dashboard surface; this packet claims none.";

/// Payload-free failure of the cursor definition boundary.
///
/// Transport mistakes (spawn, handshake, prompt, stream, stall, cancel,
/// shutdown, deadline, interruption, exit) surface as the owner
/// [`EngineOperationError`](super::operation::EngineOperationError) at the
/// dispatch arm; only typed-boundary mistakes originate here.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub(crate) enum CursorTurnError {
    /// The durable selection or a provider frame violates the cursor adapter
    /// contract. The turn fails closed instead of seating a wrong session.
    #[error("cursor turn misconfigured")]
    Configuration,
    /// A stdio write over the ACP transport failed.
    #[error("cursor stdio write failed")]
    StreamFailed,
    /// A steer was issued while a prompt round is active. A waiting ACP
    /// session accepts a follow-up; an active prompt cannot be steered in
    /// place, mirroring `EngineUnsupportedCommandError` for `steer`.
    #[error("cursor steer unsupported while a prompt is active")]
    UnsupportedCommand,
}

/// Typed cursor settings derived from the durable selection.
///
/// Mirrors `CursorAcpArgs`/`ResolveCursorModel` in
/// `modules/engines/src/cursor/engine.ts` through the shared [`LaunchArgs`]
/// shape: the model stays optional (the adapter omits `--model` when no model
/// is selected), a read-only canonical policy maps to `--mode ask` at
/// argument-building time, and only the `force` permission mode maps to a CLI
/// flag. Effort/speed suffix resolution lives in the shared
/// [`cursor_build_args`] row builder; the durable selection keeps the raw
/// choices.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorSettings {
    profile_id: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    speed_fast: bool,
    permission_force: bool,
    write_access: bool,
}

impl CursorSettings {
    /// Derives typed ACP settings from the durable selection.
    ///
    /// The cursor adapter imposes no construction-time permission rejections
    /// beyond the typed options: read-only maps to ask mode, never to a
    /// refusal.
    ///
    /// # Errors
    ///
    /// Returns [`CursorTurnError::Configuration`] only when a selected value
    /// cannot be represented for the CLI. The typed selection already bounds
    /// every field, so well-formed selections always succeed.
    pub(crate) fn from_selection(selection: &CursorSelection) -> Result<Self, CursorTurnError> {
        Ok(Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model: selection.model_id().map(|model| model.as_str().to_owned()),
            reasoning_effort: selection
                .reasoning_effort()
                .map(|effort| effort.as_str().to_owned()),
            speed_fast: selection.speed() == Some(CursorSpeed::Fast),
            permission_force: selection.permission_mode() == Some(CursorPermissionMode::Force),
            write_access: selection.permission().filesystem() != FilesystemAccess::None,
        })
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Projects these settings onto the shared ACP row args.
    #[must_use]
    pub(crate) fn launch_args(&self) -> LaunchArgs {
        LaunchArgs {
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            speed_fast: self.speed_fast,
            permission: if self.permission_force {
                Some("force".to_owned())
            } else {
                None
            },
            write_access: self.write_access,
        }
    }

    /// Builds the exact cursor stdio argv tail for one turn: optional
    /// resolved `--model`, ask mode on read-only, `--force` mapping, then
    /// `acp`.
    #[must_use]
    pub(crate) fn build_args(&self) -> Vec<OsString> {
        cursor_build_args(&self.launch_args())
    }

    /// Returns the shared cursor ACP definition row: platform executable,
    /// native image blocks, `status` auth probe, `cursor_login` selection.
    #[must_use]
    pub(crate) fn definition() -> &'static AcpDefinition {
        &CURSOR_ACP
    }

    /// Returns how this row carries image payloads: native image blocks,
    /// never embedded resources and never dropped.
    #[must_use]
    pub(crate) const fn image_mode() -> ImageMode {
        ImageMode::Image
    }
}

/// Finite C1 cursor launch capability.
///
/// Carries the managed profile identity plus the unprobed version sentinel
/// until the probe/authority packet lands. The dispatcher never constructs
/// this from a live probe in C1, and the dispatch arm fails closed before
/// spawning; the value exists so admission, binding (`cursor`, format 1), and
/// quarantine plumbing prove their cursor arms without claiming a runnable
/// engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorLaunch {
    profile_id: String,
    version: String,
}

impl CursorLaunch {
    /// Creates an unprobed launch for exactly one managed profile.
    #[must_use]
    pub(crate) fn unprobed(profile_id: String) -> Self {
        Self {
            profile_id,
            version: CURSOR_C1_UNPROBED_VERSION.to_owned(),
        }
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Returns the version string (the C1 sentinel until probed).
    #[must_use]
    pub(crate) fn version(&self) -> &str {
        &self.version
    }
}

/// Rejects a steer issued while a prompt round is active with the typed
/// unsupported-command failure, mirroring the shared ACP `Send` gate: a
/// waiting session accepts a follow-up, an active prompt cannot be steered in
/// place.
///
/// # Errors
///
/// Returns [`CursorTurnError::UnsupportedCommand`] when `prompt_active` is
/// set.
pub(crate) const fn check_cursor_steer(prompt_active: bool) -> Result<(), CursorTurnError> {
    if prompt_active {
        return Err(CursorTurnError::UnsupportedCommand);
    }
    Ok(())
}

/// The C1 native-continuation decision: always unsupported, with the
/// capability reason. Matching engine identifiers alone are never sufficient,
/// and ACP gives no model-identity guarantee for a loaded session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CursorContinuationDecision {
    /// The continuation cannot proceed natively, with the stable reason.
    Unsupported {
        /// Why native continuation is unavailable for cursor sessions.
        reason: &'static str,
    },
}

/// Reports the C1 native-continuation decision: always unsupported.
#[must_use]
pub(crate) const fn check_cursor_native_continuation() -> CursorContinuationDecision {
    CursorContinuationDecision::Unsupported {
        reason: CURSOR_NATIVE_CONTINUATION_REASON,
    }
}

/// Reports whether this packet claims a cursor account-usage surface: never
/// in C1. Usage stays only where the provider discloses it (the authenticated
/// dashboard read); the runtime claims no windows and invents no percentages.
#[must_use]
pub(crate) const fn cursor_account_usage_supported() -> bool {
    false
}

/// Typed `AE-PROVIDER-206` startup failure carrying the rejected model name.
pub(crate) use artisan_native_engine::cursor::CursorStartupFailure;
/// Converts cursor's known pre-session model rejection to Artisan's stable
/// model code, mirroring `ClassifyCursorStartupFailure` in
/// `modules/engines/src/cursor/engine.ts`: `Cannot use this model: X` maps to
/// `AE-PROVIDER-206` carrying the captured model name `X`.
pub(crate) use artisan_native_engine::cursor::classify_cursor_startup_failure;

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
    tool_call_id: String,
    /// Short category label shown beside the questions, when disclosed.
    title: Option<String>,
    /// The questions in this request group.
    questions: Vec<CursorQuestion>,
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
                .map(|option| option.id.clone())
                .unwrap_or_else(|| answer.clone())
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
    tool_call_id: String,
    /// Plan name, when disclosed.
    name: Option<String>,
    /// Plan overview, when disclosed.
    overview: Option<String>,
    /// Full plan text under review.
    plan: String,
    /// Plan steps in provider order, including cancelled ones.
    todos: Vec<CursorPlanTodo>,
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

// ---------------------------------------------------------------------------
// Fixture tests: cursor-shaped ACP args over the shared transport core
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::time::Duration;

    use artisan_domain::{
        ApprovalKind, ApprovalMode, CursorPermissionMode, CursorReasoningEffort, CursorSelection,
        CursorSpeed, EngineAgentId, EngineModelId, EnginePermissionPolicy, EngineProfileId,
        FilesystemAccess, NetworkAccess, PermissionId, PlanEntryStatus, WebSearchAccess,
    };
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, BufReader, split};

    use super::{
        CURSOR_C1_UNPROBED_VERSION, CURSOR_ENGINE_ID, CURSOR_NATIVE_CONTINUATION_REASON,
        CURSOR_USAGE_ABSENT_REASON, CursorContinuationDecision, CursorLaunch, CursorSettings,
        CursorTurnError, answer_cursor_plan, check_cursor_native_continuation, check_cursor_steer,
        classify_cursor_startup_failure, cursor_account_usage_supported, cursor_plan_approval,
        cursor_plan_description, cursor_plan_entries, cursor_question_to_domain,
        cursor_selected_option_ids, parse_cursor_plan_request, parse_cursor_question_request,
    };
    use crate::engine_owner::acp::{
        AcpBounds, AcpTransport, ImageBlock, ImageMode, PromptPart, UpdateEvent,
        build_prompt_content,
    };
    use crate::engine_owner::acp_bridges::{
        PermissionOutcome, answer_permission, normalize_permission_request,
    };
    use crate::native_run_dispatch::{binding_bytes_vec, binding_matches_bytes};

    fn permission(filesystem: FilesystemAccess) -> EnginePermissionPolicy {
        EnginePermissionPolicy::new(
            PermissionId::parse("permission-cursor").expect("permission id"),
            EngineAgentId::parse("agent-cursor").expect("agent id"),
            ApprovalMode::OnRequest,
            filesystem,
            NetworkAccess::Enabled,
            WebSearchAccess::Disabled,
        )
    }

    fn cursor_selection(
        model: Option<&str>,
        effort: Option<&str>,
        speed: Option<CursorSpeed>,
        permission_mode: Option<CursorPermissionMode>,
        filesystem: FilesystemAccess,
    ) -> CursorSelection {
        CursorSelection::new(
            EngineProfileId::parse("cursor-fixture").expect("profile id"),
            model.map(|model| EngineModelId::parse(model).expect("model id")),
            permission(filesystem),
            effort.map(|effort| CursorReasoningEffort::parse(effort).expect("reasoning effort")),
            speed,
            permission_mode,
        )
    }

    fn settings(
        model: Option<&str>,
        effort: Option<&str>,
        speed: Option<CursorSpeed>,
        permission_mode: Option<CursorPermissionMode>,
        filesystem: FilesystemAccess,
    ) -> CursorSettings {
        CursorSettings::from_selection(&cursor_selection(
            model,
            effort,
            speed,
            permission_mode,
            filesystem,
        ))
        .expect("cursor settings stay representable")
    }

    fn strict_bounds() -> AcpBounds {
        AcpBounds::new(
            4096,
            16_384,
            256,
            Duration::from_secs(5),
            Duration::from_secs(5),
            Duration::from_secs(2),
        )
        .expect("test bounds hold")
    }

    async fn agent_read_value(
        reader: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    ) -> Option<Value> {
        let mut line = String::new();
        let count = reader.read_line(&mut line).await.expect("agent reads");
        if count == 0 {
            return None;
        }
        Some(serde_json::from_str(line.trim_end()).expect("driver frames stay valid json"))
    }

    async fn agent_write_line(
        writer: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>,
        line: &str,
    ) {
        use tokio::io::AsyncWriteExt as _;
        writer
            .write_all(line.as_bytes())
            .await
            .expect("agent writes");
        writer.write_all(b"\n").await.expect("agent writes");
        writer.flush().await.expect("agent flushes");
    }

    fn update_line(session: &str, index: u32) -> String {
        json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "sessionId": session, "update": { "kind": "delta", "index": index } },
        })
        .to_string()
    }

    fn permission_params(tool_call_id: &str, command: &str) -> Value {
        json!({
            "toolCall": {
                "toolCallId": tool_call_id,
                "kind": "execute",
                "title": "Run tests",
                "rawInput": { "command": command, "cwd": "C:\\work" },
            },
            "options": [
                { "kind": "allow_once", "optionId": "allow-1", "label": "Allow" },
                { "kind": "reject_once", "optionId": "reject-1", "label": "Deny" },
            ],
        })
    }

    // -----------------------------------------------------------------------
    // Definition row: model resolution, args, classifier, image-block mode
    // -----------------------------------------------------------------------

    #[test]
    fn model_resolution_matrix() {
        // No model stays absent; effort and speed never invent one.
        assert_eq!(
            settings(
                None,
                Some("high"),
                Some(CursorSpeed::Fast),
                None,
                FilesystemAccess::Workspace
            )
            .launch_args()
            .model,
            None
        );

        // Effort appends unless the base already carries a suffix.
        assert_eq!(
            settings(
                Some("composer-1"),
                Some("high"),
                None,
                None,
                FilesystemAccess::Workspace
            )
            .launch_args()
            .model
            .as_deref(),
            Some("composer-1-high")
        );
        assert_eq!(
            settings(
                Some("composer-1-high"),
                Some("low"),
                None,
                None,
                FilesystemAccess::Workspace
            )
            .launch_args()
            .model
            .as_deref(),
            Some("composer-1-high")
        );
        assert_eq!(
            settings(
                Some("composer-1-high-fast"),
                Some("low"),
                None,
                None,
                FilesystemAccess::Workspace
            )
            .launch_args()
            .model
            .as_deref(),
            Some("composer-1-high-fast")
        );

        // Fast appends unless already present.
        assert_eq!(
            settings(
                Some("composer-1"),
                Some("high"),
                Some(CursorSpeed::Fast),
                None,
                FilesystemAccess::Workspace
            )
            .launch_args()
            .model
            .as_deref(),
            Some("composer-1-high-fast")
        );
        assert_eq!(
            settings(
                Some("composer-1-fast"),
                None,
                Some(CursorSpeed::Fast),
                None,
                FilesystemAccess::Workspace
            )
            .launch_args()
            .model
            .as_deref(),
            Some("composer-1-fast")
        );

        // Bracket models pass through untouched.
        assert_eq!(
            settings(
                Some("cursor[fast]"),
                Some("high"),
                Some(CursorSpeed::Fast),
                None,
                FilesystemAccess::Workspace
            )
            .launch_args()
            .model
            .as_deref(),
            Some("cursor[fast]")
        );
    }

    #[test]
    fn args_matrix() {
        // Bare writable session is just the subcommand.
        assert_eq!(
            settings(None, None, None, None, FilesystemAccess::Workspace).build_args(),
            vec![OsString::from("acp")]
        );

        // Resolved model leads.
        assert_eq!(
            settings(
                Some("composer-1"),
                Some("high"),
                None,
                None,
                FilesystemAccess::Workspace
            )
            .build_args(),
            vec![
                OsString::from("--model"),
                OsString::from("composer-1-high"),
                OsString::from("acp"),
            ]
        );

        // Read-only maps to ask mode and wins over force.
        assert_eq!(
            settings(None, None, None, None, FilesystemAccess::None).build_args(),
            vec![
                OsString::from("--mode"),
                OsString::from("ask"),
                OsString::from("acp"),
            ]
        );
        assert_eq!(
            settings(
                Some("composer-1"),
                None,
                None,
                Some(CursorPermissionMode::Force),
                FilesystemAccess::None
            )
            .build_args(),
            vec![
                OsString::from("--model"),
                OsString::from("composer-1"),
                OsString::from("--mode"),
                OsString::from("ask"),
                OsString::from("acp"),
            ]
        );

        // Force maps only when writes are allowed.
        assert!(
            settings(
                None,
                None,
                None,
                Some(CursorPermissionMode::Force),
                FilesystemAccess::Workspace
            )
            .build_args()
            .contains(&OsString::from("--force"))
        );
    }

    #[test]
    fn startup_rejection_captures_model_with_stable_code() {
        let failure = classify_cursor_startup_failure(
            "Cannot use this model: composer-1. Valid models: composer-1, composer-2",
        )
        .expect("known rejection classifies");
        assert_eq!(failure.model(), "composer-1");
        assert_eq!(failure.artisan_code(), "AE-PROVIDER-206");
        assert_eq!(failure.engine_id(), CURSOR_ENGINE_ID);
        assert_eq!(
            failure.message(),
            "Cursor does not make model composer-1 available to this account."
        );

        // Case-insensitive prefix with a line-break terminator.
        let newline = classify_cursor_startup_failure("cANNOT USE THIS MODEL:  sonar\nretry later")
            .expect("newline terminator classifies");
        assert_eq!(newline.model(), "sonar");

        // End-of-input terminator.
        let trailing = classify_cursor_startup_failure("Cannot use this model: nightly-x")
            .expect("trailing model classifies");
        assert_eq!(trailing.model(), "nightly-x");

        for silent in [
            "",
            "agent 2026.9.6-stable.1",
            "Error: not authenticated",
            "Cannot use this model:",
        ] {
            assert!(
                classify_cursor_startup_failure(silent).is_none(),
                "no rejection without a captured model: {silent:?}"
            );
        }
    }

    #[test]
    fn definition_row_carries_cursor_shape() {
        let row = CursorSettings::definition();
        assert_eq!(row.engine_id, CURSOR_ENGINE_ID);
        assert!(row.executable.contains("agent"));
        assert_eq!(row.version_args, &["--version"][..]);
        assert_eq!(row.auth_probe_args, &["status"][..]);
        assert_eq!(row.image_mode, ImageMode::Image);
        assert_eq!(CursorSettings::image_mode(), ImageMode::Image);

        let available = ["cursor_login"];
        assert_eq!(
            (row.select_auth_method)(&available, false),
            Some("cursor_login")
        );
        assert_eq!((row.select_auth_method)(&[], false), None);
        assert!((row.is_authenticated_output)("signed in as s"));
        assert!(!(row.is_authenticated_output)("Error: not logged in"));

        // The settings project onto the same row builder the core spawns.
        let resolved = settings(
            Some("composer-1"),
            Some("high"),
            None,
            Some(CursorPermissionMode::Force),
            FilesystemAccess::Workspace,
        );
        assert_eq!(
            (row.build_args)(&resolved.launch_args()),
            resolved.build_args()
        );
    }

    // -----------------------------------------------------------------------
    // Steer, continuation, usage honesty
    // -----------------------------------------------------------------------

    #[test]
    fn active_steer_rejects_with_typed_unsupported_command() {
        assert_eq!(check_cursor_steer(false), Ok(()));
        assert_eq!(
            check_cursor_steer(true),
            Err(CursorTurnError::UnsupportedCommand)
        );
    }

    #[test]
    fn native_continuation_is_always_unsupported_with_reason() {
        assert_eq!(
            check_cursor_native_continuation(),
            CursorContinuationDecision::Unsupported {
                reason: CURSOR_NATIVE_CONTINUATION_REASON,
            }
        );
        assert_eq!(
            CURSOR_NATIVE_CONTINUATION_REASON,
            "ACP does not guarantee that a loaded session may change model identity."
        );
    }

    #[test]
    fn usage_surface_stays_absent_and_honest() {
        assert!(!cursor_account_usage_supported());
        assert!(!CURSOR_USAGE_ABSENT_REASON.is_empty());

        // The C1 launch carries no usage scope: profile plus sentinel only.
        let launch = CursorLaunch::unprobed("cursor-fixture".to_owned());
        assert_eq!(launch.profile_id(), "cursor-fixture");
        assert_eq!(launch.version(), CURSOR_C1_UNPROBED_VERSION);
    }

    // -----------------------------------------------------------------------
    // Plan-approval extensions
    // -----------------------------------------------------------------------

    fn question_fixture() -> Value {
        json!({
            "toolCallId": "cursor-q-1",
            "title": "Pick",
            "questions": [
                {
                    "id": "q1",
                    "prompt": "Which?",
                    "allowMultiple": false,
                    "options": [
                        { "id": "o1", "label": "First" },
                        { "id": "o2", "label": "Second" },
                    ],
                },
                {
                    "id": "q2",
                    "prompt": "Which tags?",
                    "allowMultiple": true,
                    "options": [{ "id": "t1", "label": "Tag" }],
                },
            ],
        })
    }

    #[test]
    fn cursor_questions_map_to_domain_with_answer_identity() {
        let request = parse_cursor_question_request(&question_fixture()).expect("questions parse");
        assert_eq!(request.tool_call_id, "cursor-q-1");
        assert_eq!(request.title.as_deref(), Some("Pick"));
        assert_eq!(request.questions.len(), 2);

        let first = cursor_question_to_domain(&request, &request.questions[0]).expect("domain");
        assert_eq!(first.text, "Which?");
        assert_eq!(first.header.as_deref(), Some("Pick"));
        assert!(!first.multi_select);
        assert_eq!(first.options.as_ref().expect("options").len(), 2);

        let second = cursor_question_to_domain(&request, &request.questions[1]).expect("domain");
        assert!(second.multi_select);

        // Answers resolve by option id or label, verbatim otherwise.
        assert_eq!(
            cursor_selected_option_ids(
                &request.questions[0],
                &["o2".to_owned(), "First".to_owned(), "custom".to_owned()]
            ),
            vec!["o2".to_owned(), "o1".to_owned(), "custom".to_owned()]
        );

        for bad in [
            json!(null),
            json!({}),
            json!({ "toolCallId": "", "questions": [] }),
            json!({ "toolCallId": "x" }),
            json!({ "toolCallId": "x", "questions": [] }),
            json!({
                "toolCallId": "x",
                "questions": [{ "id": "q", "prompt": "p", "options": [] }],
            }),
            json!({
                "toolCallId": "x",
                "questions": [{ "id": "", "prompt": "p", "options": [{ "id": "o", "label": "l" }] }],
            }),
        ] {
            assert_eq!(
                parse_cursor_question_request(&bad),
                Err(CursorTurnError::Configuration),
                "question shape must fail closed: {bad}"
            );
        }
    }

    fn plan_fixture() -> Value {
        json!({
            "toolCallId": "cursor-plan-1",
            "name": "Plan",
            "overview": "Do things",
            "plan": "Steps to finish.",
            "todos": [
                { "id": "t1", "content": "Step one", "status": "pending" },
                { "id": "t2", "content": "Step two", "status": "in_progress" },
                { "id": "t3", "content": "Step three", "status": "completed" },
                { "id": "t4", "content": "Dropped", "status": "cancelled" },
                { "id": "", "content": "Junk", "status": "pending" },
                { "id": "t5", "content": "Weird", "status": "unknown" },
            ],
        })
    }

    #[test]
    fn cursor_plan_maps_entries_and_action_approval() {
        let request = parse_cursor_plan_request(&plan_fixture()).expect("plan parses");
        assert_eq!(request.tool_call_id, "cursor-plan-1");
        // Malformed todos drop the TypeScript flatMap way; cancelled stays
        // parsed here and filters at emission.
        assert_eq!(request.todos.len(), 4);

        let entries = cursor_plan_entries(&request).expect("entries project");
        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .iter()
                .all(|entry| entry.status() != PlanEntryStatus::Pending
                    || entry.text() == "Step one")
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.status())
                .collect::<Vec<_>>(),
            vec![
                PlanEntryStatus::Pending,
                PlanEntryStatus::InProgress,
                PlanEntryStatus::Completed,
            ]
        );

        assert_eq!(cursor_plan_description(&request), "Do things");
        let (description, approval) = cursor_plan_approval(&request).expect("approval maps");
        assert_eq!(description, "Do things");
        assert_eq!(approval.reason(), Some("Steps to finish."));

        // Overview falls back to name, then to the shared prompt.
        let mut nameless = request.clone();
        nameless.overview = None;
        assert_eq!(cursor_plan_description(&nameless), "Plan");
        nameless.name = None;
        assert_eq!(cursor_plan_description(&nameless), "Approve this plan?");

        // The explicit wire outcome accepts or rejects; the turn continues.
        assert_eq!(
            answer_cursor_plan(true),
            json!({ "outcome": { "outcome": "accepted" } })
        );
        assert_eq!(
            answer_cursor_plan(false),
            json!({ "outcome": { "outcome": "rejected" } })
        );

        for bad in [
            json!(null),
            json!({}),
            json!({ "toolCallId": "x", "plan": "p" }),
            json!({ "toolCallId": "", "plan": "p", "todos": [] }),
            json!({ "toolCallId": "x", "plan": 42, "todos": [] }),
        ] {
            assert_eq!(
                parse_cursor_plan_request(&bad),
                Err(CursorTurnError::Configuration),
                "plan shape must fail closed: {bad}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Binding tag/format round trip
    // -----------------------------------------------------------------------

    #[test]
    fn cursor_binding_round_trip_and_mismatch() {
        let raw =
            binding_bytes_vec("cursor", "cursor-fixture", "sess-cursor-1").expect("binding builds");
        assert!(binding_matches_bytes(
            &raw,
            "cursor",
            "cursor-fixture",
            "sess-cursor-1"
        ));
        for (engine, profile, session) in [
            ("opencode2", "cursor-fixture", "sess-cursor-1"),
            ("codex", "cursor-fixture", "sess-cursor-1"),
            ("cursor", "other-profile", "sess-cursor-1"),
            ("cursor", "cursor-fixture", "other-session"),
        ] {
            assert!(
                !binding_matches_bytes(&raw, engine, profile, session),
                "mismatch must requeue"
            );
        }
        assert!(binding_bytes_vec("", "cursor-fixture", "sess-cursor-1").is_none());
        assert!(binding_bytes_vec("cursor", "", "sess-cursor-1").is_none());
        assert!(binding_bytes_vec("cursor", "cursor-fixture", "").is_none());
    }

    // -----------------------------------------------------------------------
    // Fixture ACP turns: cursor-shaped args over the shared transport core
    // -----------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn fixture_cursor_start_deltas_approval_close() {
        let bounds = strict_bounds();
        let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, agent_write_half) = split(agent_io);
        let mut agent_read = BufReader::new(agent_read_half);
        let mut agent_write = agent_write_half;

        let agent = tokio::spawn(async move {
            let init = agent_read_value(&mut agent_read)
                .await
                .expect("initialize request");
            assert_eq!(
                init.get("method").and_then(Value::as_str),
                Some("initialize")
            );
            let init_id = init.get("id").cloned().expect("initialize id");
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": init_id,
                    "result": { "protocolVersion": 1, "authMethods": [{ "id": "cursor_login" }] },
                })
                .to_string(),
            )
            .await;

            let auth = agent_read_value(&mut agent_read)
                .await
                .expect("authenticate request");
            assert_eq!(
                auth.get("method").and_then(Value::as_str),
                Some("authenticate")
            );
            let auth_id = auth.get("id").cloned().expect("authenticate id");
            agent_write_line(
                &mut agent_write,
                &json!({ "jsonrpc": "2.0", "id": auth_id, "result": {} }).to_string(),
            )
            .await;

            let new = agent_read_value(&mut agent_read)
                .await
                .expect("session/new request");
            assert_eq!(
                new.get("method").and_then(Value::as_str),
                Some("session/new")
            );
            let new_id = new.get("id").cloned().expect("session/new id");
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": new_id,
                    "result": { "sessionId": "sess-cursor-1" },
                })
                .to_string(),
            )
            .await;

            let prompt = agent_read_value(&mut agent_read)
                .await
                .expect("session/prompt request");
            assert_eq!(
                prompt.get("method").and_then(Value::as_str),
                Some("session/prompt")
            );
            // Cursor carries images as native image blocks, never resources.
            let content = prompt
                .get("params")
                .and_then(|params| params.get("prompt"))
                .and_then(Value::as_array)
                .expect("prompt content");
            assert!(
                content
                    .iter()
                    .any(
                        |part| part.get("type").and_then(Value::as_str) == Some("image")
                            && part.get("mimeType").and_then(Value::as_str) == Some("image/png")
                    ),
                "image-block mode must cross the wire"
            );
            let prompt_id = prompt.get("id").cloned().expect("prompt id");

            agent_write_line(&mut agent_write, &update_line("sess-cursor-1", 1)).await;
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 50,
                    "method": "session/requestPermission",
                    "params": permission_params("cursor-tool-1", "cargo test"),
                })
                .to_string(),
            )
            .await;
            agent_write_line(&mut agent_write, &update_line("sess-cursor-1", 2)).await;
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 51,
                    "method": "session/requestPermission",
                    "params": permission_params("cursor-tool-2", "cargo test"),
                })
                .to_string(),
            )
            .await;
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": prompt_id,
                    "result": {
                        "stopReason": "completed",
                        "usage": { "inputTokens": 7, "outputTokens": 9 },
                    },
                })
                .to_string(),
            )
            .await;

            let mut saw_cancel = false;
            while let Some(frame) = agent_read_value(&mut agent_read).await {
                if frame.get("method").and_then(Value::as_str) == Some("session/cancel") {
                    saw_cancel = true;
                }
            }
            saw_cancel
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        let init = driver.initialize().await.expect("handshake");
        assert_eq!(init.protocol_version, 1);
        let available: Vec<&str> = init.auth_methods.iter().map(String::as_str).collect();
        assert_eq!(
            (CursorSettings::definition().select_auth_method)(&available, false),
            Some("cursor_login")
        );
        driver
            .authenticate("cursor_login")
            .await
            .expect("authenticate");
        let session = driver.new_session("C:\\work").await.expect("session/new");
        assert_eq!(session.as_str(), "sess-cursor-1");

        // Cursor-shaped args: resolved model plus force, then `acp`.
        let shaped = settings(
            Some("composer-1"),
            Some("high"),
            None,
            Some(CursorPermissionMode::Force),
            FilesystemAccess::Workspace,
        );
        assert_eq!(
            shaped.build_args(),
            vec![
                OsString::from("--model"),
                OsString::from("composer-1-high"),
                OsString::from("--force"),
                OsString::from("acp"),
            ]
        );

        let image = PromptPart::Image(ImageBlock {
            id: "attach-1".to_owned(),
            name: "shot.png".to_owned(),
            media_type: "image/png".to_owned(),
            bytes: vec![1, 2, 3],
        });
        let content =
            build_prompt_content(ImageMode::Image, "hello", &[image], None).expect("content");
        let prompt_id = driver.prompt(&session, content).await.expect("prompt");

        let mut deltas = 0_u32;
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("first delta")
        {
            UpdateEvent::SessionUpdate(_) => deltas += 1,
            other => panic!("expected session update, got {other:?}"),
        }

        // Deny lands with no side effect while the turn continues.
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("first approval")
        {
            UpdateEvent::AgentRequest { method, params, .. } => {
                assert_eq!(method, "session/requestPermission");
                let pending = normalize_permission_request(&params).expect("normalize");
                assert_eq!(pending.provider_id(), "cursor-tool-1");
                assert_eq!(
                    answer_permission(&pending, false),
                    PermissionOutcome::Selected {
                        option_id: "reject-1".to_owned(),
                    }
                );
            }
            other => panic!("expected agent request, got {other:?}"),
        }
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("second delta")
        {
            UpdateEvent::SessionUpdate(_) => deltas += 1,
            other => panic!("expected session update, got {other:?}"),
        }

        // Allow answers through the same durable path.
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("second approval")
        {
            UpdateEvent::AgentRequest { params, .. } => {
                let pending = normalize_permission_request(&params).expect("normalize");
                assert_eq!(pending.provider_id(), "cursor-tool-2");
                assert_eq!(
                    answer_permission(&pending, true),
                    PermissionOutcome::Selected {
                        option_id: "allow-1".to_owned(),
                    }
                );
            }
            other => panic!("expected agent request, got {other:?}"),
        }
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("prompt result")
        {
            UpdateEvent::PromptResult(outcome) => {
                assert!(!outcome.cancelled);
                let usage = outcome.usage.expect("usage reported");
                assert_eq!(usage.input, 7);
                assert_eq!(usage.output, 9);
            }
            other => panic!("expected prompt result, got {other:?}"),
        }
        assert_eq!(deltas, 2);

        driver.cancel(&session).await.expect("cancel notify");
        driver.shutdown_writer().await.expect("lifeline close");
        drop(driver);
        assert!(agent.await.expect("agent joins"), "cancel must be observed");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fixture_cursor_plan_and_question_extensions_surface() {
        let bounds = strict_bounds();
        let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, mut agent_write) = split(agent_io);
        drop(agent_read_half);

        let agent = tokio::spawn(async move {
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 60,
                    "method": "cursor/ask_question",
                    "params": question_fixture(),
                })
                .to_string(),
            )
            .await;
            agent_write_line(
                &mut agent_write,
                &json!({
                    "jsonrpc": "2.0",
                    "id": 61,
                    "method": "cursor/create_plan",
                    "params": plan_fixture(),
                })
                .to_string(),
            )
            .await;
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        let session =
            crate::engine_owner::acp::SessionId::parse("sess-cursor-1", 256).expect("session");
        let prompt_id = crate::engine_owner::acp::AcpId::Number(9);

        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("question request")
        {
            UpdateEvent::AgentRequest { method, params, .. } => {
                assert_eq!(method, "cursor/ask_question");
                let request = parse_cursor_question_request(&params).expect("questions parse");
                let domain =
                    cursor_question_to_domain(&request, &request.questions[0]).expect("domain");
                assert_eq!(domain.text, "Which?");
                assert_eq!(
                    cursor_selected_option_ids(&request.questions[0], &["Second".to_owned()]),
                    vec!["o2".to_owned()]
                );
            }
            other => panic!("expected agent request, got {other:?}"),
        }
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("plan request")
        {
            UpdateEvent::AgentRequest { method, params, .. } => {
                assert_eq!(method, "cursor/create_plan");
                let request = parse_cursor_plan_request(&params).expect("plan parses");
                assert_eq!(cursor_plan_entries(&request).expect("entries").len(), 3);
                let (description, approval) =
                    cursor_plan_approval(&request).expect("approval maps");
                assert_eq!(description, "Do things");
                assert_eq!(approval.kind(), ApprovalKind::Action);
                assert_eq!(
                    answer_cursor_plan(false),
                    json!({ "outcome": { "outcome": "rejected" } })
                );
            }
            other => panic!("expected agent request, got {other:?}"),
        }
        agent.await.expect("agent joins");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fixture_cursor_resume_round_trip() {
        let bounds = strict_bounds();
        let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, mut agent_write) = split(agent_io);
        let mut agent_read = BufReader::new(agent_read_half);

        let agent = tokio::spawn(async move {
            let first = agent_read_value(&mut agent_read)
                .await
                .expect("session/load request");
            assert_eq!(
                first.get("method").and_then(Value::as_str),
                Some("session/load")
            );
            assert_eq!(
                first
                    .get("params")
                    .and_then(|params| params.get("sessionId"))
                    .and_then(Value::as_str),
                Some("sess-cursor-9")
            );
            let first_id = first.get("id").cloned().expect("load id");
            agent_write_line(
                &mut agent_write,
                &json!({ "jsonrpc": "2.0", "id": first_id, "result": {} }).to_string(),
            )
            .await;

            let second = agent_read_value(&mut agent_read)
                .await
                .expect("second session/load request");
            let second_id = second.get("id").cloned().expect("load id");
            agent_write_line(
                &mut agent_write,
                &json!({ "jsonrpc": "2.0", "id": second_id, "error": { "code": -32_000 } })
                    .to_string(),
            )
            .await;
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        let session =
            crate::engine_owner::acp::SessionId::parse("sess-cursor-9", 256).expect("session");
        driver
            .load_session(&session, "C:\\work")
            .await
            .expect("resume");
        let error = driver
            .load_session(&session, "C:\\work")
            .await
            .expect_err("rejected resume");
        assert_eq!(error, crate::engine_owner::acp::AcpError::ChildFailed);
        agent.await.expect("agent joins");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fixture_cursor_malformed_frames_reject_without_shell() {
        use crate::engine_owner::acp::{AcpError, parse_envelope};

        assert_eq!(
            parse_envelope("not json", 4096).expect_err("malformed"),
            AcpError::MalformedEnvelope
        );
        assert_eq!(
            parse_envelope("{\"jsonrpc\":\"2.0\",\"id\":1}", 4096).expect_err("malformed"),
            AcpError::MalformedEnvelope
        );

        // A foreign session frame is skipped; the owned update still lands.
        let bounds = strict_bounds();
        let (driver_io, agent_io) = tokio::io::duplex(64 * 1024);
        let (driver_read, driver_write) = split(driver_io);
        let (agent_read_half, mut agent_write) = split(agent_io);
        drop(agent_read_half);
        let agent = tokio::spawn(async move {
            agent_write_line(&mut agent_write, &update_line("other-sess", 9)).await;
            agent_write_line(&mut agent_write, &update_line("sess-cursor-1", 1)).await;
        });

        let mut driver = AcpTransport::new(BufReader::new(driver_read), driver_write, bounds);
        let session =
            crate::engine_owner::acp::SessionId::parse("sess-cursor-1", 256).expect("session");
        let prompt_id = crate::engine_owner::acp::AcpId::Number(4);
        match driver
            .next_update(&session, &prompt_id)
            .await
            .expect("own update")
        {
            UpdateEvent::SessionUpdate(update) => {
                assert_eq!(update.session.as_str(), "sess-cursor-1")
            }
            other => panic!("expected session update, got {other:?}"),
        }
        agent.await.expect("agent joins");
    }

    #[test]
    fn unavailable_model_startup_rejection_marks_unrunnable_claim() {
        // The dispatcher requeues the claim; the stable code travels with the
        // transcript diagnostic instead of a raw provider string.
        let failure = classify_cursor_startup_failure(
            "Cannot use this model: composer-1. Valid models: composer-1",
        )
        .expect("rejection classifies");
        assert_eq!(failure.model(), "composer-1");
        assert_eq!(failure.artisan_code(), "AE-PROVIDER-206");
    }
}
