//! What Codex asks the client: approvals, which ride on the item they gate,
//! user-input questions, questions inside agent messages, and the replies.

use super::{Decoder, Scope, display_command};
use crate::EngineEvent;
use crate::codex::protocol::{AsyncQuestion, Request, UserQuestion};
use artisan_api::{
    Answer, Approval, ApprovalRequest, Decision, ErrorKind, ItemKind, NoticeKind, Question,
    QuestionAnswer, QuestionOption, Status, TextContent, ToolKind,
};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet, VecDeque};

/// Approval requests by the item they gate, oldest first: the item shows
/// the first until it is answered or Codex settles it.
pub(super) type Approvals = HashMap<String, VecDeque<Asked>>;

/// Questions waiting for the user, by item key.
pub(super) type Questions = HashMap<String, Asking>;

pub(super) struct Asked {
    rpc: Value,
    reply: Reply,
    approval: Approval,
    /// The item exists only for this request (permissions asked outside
    /// any item): the decision settles it.
    standalone: bool,
}

/// How a decision goes back to Codex.
enum Reply {
    /// Commands and file changes take a decision word.
    Decision,
    /// Permissions are granted back as asked, minus nulls. Asked outside
    /// any item, they get an item of their own.
    Permissions { requested: Value, standalone: bool },
}

pub(super) enum Asking {
    /// `item/tool/requestUserInput`, answered by question id.
    Request { rpc: Value, ids: Vec<String> },
    /// Questions inside an agent message; the answer goes back as a message.
    Message { texts: Vec<String> },
}

impl Decoder {
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per request Codex makes, with what each needs checked"
    )]
    pub(super) fn server_request(
        &mut self,
        rpc: Value,
        method: &str,
        params: &Value,
        out: &mut Vec<EngineEvent>,
    ) {
        let request = match Request::parse(method, params) {
            Ok(request) => request,
            Err(error) => {
                let thread = params["threadId"].as_str();
                let message = format!("Artisan could not read Codex's {method}: {error}");
                tracing::warn!(method, thread, request = %rpc, %error, "declining a Codex request Artisan can't read");
                self.reply_error(rpc, crate::rpc::INVALID_PARAMS, &message);
                if let Some(scope) = thread.and_then(|thread| self.scope(thread)) {
                    self.error_item(scope, ErrorKind::Execution, &message, out);
                }
                return;
            }
        };
        let thread = request.thread().map(str::to_owned);
        let thread = thread.as_deref();
        let scope = thread.and_then(|thread| self.scope(thread));
        let needs_item = matches!(
            request,
            Request::CommandApproval(_) | Request::FileChangeApproval(_) | Request::UserInput(_)
        );
        let item = match &request {
            Request::CommandApproval(request) => request.item_id.as_deref(),
            Request::FileChangeApproval(request) => request.item_id.as_deref(),
            Request::PermissionsApproval(request) => request.item_id.as_deref(),
            Request::UserInput(request) => request.item_id.as_deref(),
            Request::Elicitation(_) | Request::Unknown => None,
        }
        .filter(|item| !item.trim().is_empty())
        .map(str::to_owned);
        if needs_item && item.is_none() {
            let message = "Codex request omitted its required item ID";
            tracing::warn!(method, thread, request = %rpc, message);
            self.reply_error(rpc, crate::rpc::INVALID_PARAMS, message);
            if let Some(scope) = scope {
                self.error_item(scope, ErrorKind::Execution, message, out);
            }
            return;
        }
        match (request, scope) {
            (Request::CommandApproval(request), Some(scope)) => {
                let key = item.expect("checked above");
                let existing = match self.items.get(&key).map(|tracked| &tracked.kind) {
                    Some(ItemKind::Tool {
                        tool: ToolKind::Shell { command, cwd, .. },
                        ..
                    }) => Some((command.clone(), cwd.clone())),
                    _ => None,
                };
                let command = request
                    .command
                    .filter(|command| !command.trim().is_empty())
                    .map(|command| TextContent::from(display_command(&command)))
                    .or_else(|| {
                        existing
                            .as_ref()
                            .map(|(command, _)| command.clone())
                            .filter(|command| command.byte_len() > 0)
                    });
                let cwd = request.cwd.or_else(|| existing.and_then(|(_, cwd)| cwd));
                let stdin = request.kind.as_deref() == Some("writeStdin");
                let approval = match &command {
                    Some(command) if stdin => ApprovalRequest::Action {
                        description: command
                            .inline()
                            .map_or_else(
                                || "Send input to the running command".to_owned(),
                                |command| format!("Send input to `{command}`"),
                            )
                            .into(),
                    },
                    Some(command) => ApprovalRequest::Command {
                        command: command.clone(),
                        cwd: cwd.clone(),
                    },
                    None => ApprovalRequest::Action {
                        description: request
                            .reason
                            .clone()
                            .filter(|reason| !reason.trim().is_empty())
                            .unwrap_or_else(|| {
                                if stdin {
                                    "Send input to a running command"
                                } else {
                                    "Approve command execution"
                                }
                                .to_owned()
                            })
                            .into(),
                    },
                };
                let host = match command {
                    Some(command) => ToolKind::Shell {
                        command,
                        cwd,
                        background: false,
                    },
                    None => ToolKind::Other {
                        name: "command".to_owned(),
                        input: None,
                    },
                };
                self.ask(
                    &key,
                    scope,
                    approval,
                    request.reason,
                    rpc,
                    Reply::Decision,
                    host,
                    out,
                );
            }
            (Request::FileChangeApproval(request), Some(scope)) => {
                let key = item.expect("checked above");
                let host = ToolKind::Edit {
                    changes: Vec::new().into(),
                };
                let approval = ApprovalRequest::FileChange;
                self.ask(
                    &key,
                    scope,
                    approval,
                    request.reason,
                    rpc,
                    Reply::Decision,
                    host,
                    out,
                );
            }
            (Request::PermissionsApproval(request), Some(scope)) => {
                let standalone = item.is_none();
                let key = item.unwrap_or_else(|| format!("permissions:{rpc}"));
                let description = request
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("Grant more permissions: {}", request.permissions));
                let host = ToolKind::Other {
                    name: "permissions".to_owned(),
                    input: Some(request.permissions.to_string().into()),
                };
                let reply = Reply::Permissions {
                    requested: strip_nulls(&request.permissions),
                    standalone,
                };
                let approval = ApprovalRequest::Action {
                    description: description.into(),
                };
                self.ask(&key, scope, approval, request.reason, rpc, reply, host, out);
            }
            (Request::UserInput(request), Some(scope)) => {
                let key = item.expect("checked above");
                let Some(questions) = user_questions(request.questions) else {
                    let message = "Codex supplied malformed user-input prompts or options";
                    tracing::warn!(thread, item = key, request = %rpc, message);
                    self.reply_error(rpc, crate::rpc::INVALID_PARAMS, message);
                    self.error_item(scope, ErrorKind::Execution, message, out);
                    return;
                };
                let mut seen = HashSet::new();
                if questions
                    .iter()
                    .any(|question| !seen.insert(question.id.as_str()))
                {
                    let message = "Codex supplied duplicate user-input question IDs";
                    tracing::warn!(thread, item = key, request = %rpc, message);
                    self.reply_error(rpc, crate::rpc::INVALID_PARAMS, message);
                    self.error_item(scope, ErrorKind::Execution, message, out);
                    return;
                }
                let ids = questions
                    .iter()
                    .map(|question| question.id.clone())
                    .collect();
                self.items.remove(&key);
                let kind = ItemKind::Question {
                    blocking: request.is_blocking,
                    questions: questions.into(),
                    answers: None,
                    answer_text: None,
                };
                self.track(key.clone(), scope, Status::AwaitingUser, kind);
                out.push(self.event(&key));
                self.questions.insert(key, Asking::Request { rpc, ids });
            }
            (Request::Elicitation(request), scope) => {
                // Artisan has no form for MCP elicitation yet; say so and
                // let the server go on without the input.
                let text = format!(
                    "{} asked for input Artisan can't show yet: {}",
                    request.server_name, request.message
                );
                self.notices += 1;
                if let Some(scope) = scope {
                    let key = format!("elicitation-{}", self.notices);
                    self.notice(&key, scope, NoticeKind::Info, text, out);
                }
                self.reply(
                    rpc,
                    json!({ "action": "decline", "content": null, "_meta": null }),
                );
            }
            (_, _) => {
                tracing::debug!(
                    method,
                    thread,
                    "declining a Codex request Artisan doesn't serve"
                );
                let message = format!("Artisan does not handle {method}");
                self.reply_error(rpc, crate::rpc::METHOD_NOT_FOUND, &message);
            }
        }
    }

    /// Puts an approval on the item it gates, creating the item from the
    /// request when Codex asks before announcing it. A second request for
    /// the same item waits behind the first.
    #[allow(clippy::too_many_arguments)]
    fn ask(
        &mut self,
        key: &str,
        scope: Scope,
        request: ApprovalRequest,
        reason: Option<String>,
        rpc: Value,
        reply: Reply,
        host: ToolKind,
        out: &mut Vec<EngineEvent>,
    ) {
        if let Some(tracked) = self.items.get(key)
            && (tracked.status.is_terminal() || !matches!(tracked.kind, ItemKind::Tool { .. }))
        {
            tracing::warn!(item = key, request = %rpc, "Codex asked to approve an item that can't wait for it; declining");
            let decline = match reply {
                Reply::Decision => json!({ "decision": "decline" }),
                Reply::Permissions { .. } => json!({ "permissions": {}, "scope": "turn" }),
            };
            self.reply(rpc, decline);
            return;
        }
        let approval = Approval {
            request,
            reason: reason.map(Into::into),
            // Codex lists fewer in `availableDecisions`, but takes all four
            // (verified on 0.162.1: `decline` declines the call and
            // `acceptForSession` skips the next identical prompt).
            options: vec![
                Decision::AllowOnce,
                Decision::AllowAlways,
                Decision::DenyOnce,
                Decision::Cancelled,
            ],
            decision: None,
        };
        let standalone = matches!(
            reply,
            Reply::Permissions {
                standalone: true,
                ..
            }
        );
        let queue = self.approvals.entry(key.to_owned()).or_default();
        queue.push_back(Asked {
            rpc,
            reply,
            approval: approval.clone(),
            standalone,
        });
        if queue.len() > 1 {
            return;
        }
        if let Some(tracked) = self.items.get_mut(key) {
            if let ItemKind::Tool { approval: slot, .. } = &mut tracked.kind {
                *slot = Some(approval);
            }
            tracked.status = tracked.status.advance(Status::AwaitingUser);
        } else {
            let kind = ItemKind::Tool {
                tool: host,
                title: None,
                output: None,
                exit_code: None,
                approval: Some(approval),
            };
            self.track(key.to_owned(), scope, Status::AwaitingUser, kind);
        }
        out.push(self.event(key));
    }

    pub(super) fn answer_raw(&mut self, key: &str, answer: &Answer) -> Vec<EngineEvent> {
        match answer {
            Answer::Approval { decision } if self.approvals.contains_key(key) => {
                self.decide(key, *decision)
            }
            Answer::Question { answers } if self.questions.contains_key(key) => {
                self.answer_questions(key, answers)
            }
            _ => {
                tracing::debug!(key, "an answer for a Codex item that isn't waiting on one");
                Vec::new()
            }
        }
    }

    fn decide(&mut self, key: &str, decision: Decision) -> Vec<EngineEvent> {
        let queue = self.approvals.get_mut(key).expect("checked by the caller");
        let Some(asked) = queue.pop_front() else {
            return Vec::new();
        };
        let next = queue.front().map(|next| next.approval.clone());
        if next.is_none() {
            self.approvals.remove(key);
        }
        let result = match asked.reply {
            Reply::Decision => json!({ "decision": match decision {
                Decision::AllowOnce => "accept",
                Decision::AllowAlways => "acceptForSession",
                Decision::DenyOnce | Decision::DenyAlways => "decline",
                Decision::Cancelled => "cancel",
            }}),
            Reply::Permissions { requested, .. } => match decision {
                Decision::AllowOnce => json!({ "permissions": requested, "scope": "turn" }),
                Decision::AllowAlways => json!({ "permissions": requested, "scope": "session" }),
                _ => json!({ "permissions": {}, "scope": "turn" }),
            },
        };
        self.reply(asked.rpc, result);
        let Some(tracked) = self.items.get_mut(key) else {
            return Vec::new();
        };
        let mut decided = asked.approval;
        decided.decision = Some(decision);
        let (approval, status) = match next {
            Some(next) => (next, Status::AwaitingUser),
            None => (
                decided,
                match decision {
                    Decision::AllowOnce | Decision::AllowAlways if asked.standalone => Status::Done,
                    Decision::AllowOnce | Decision::AllowAlways => Status::Running,
                    Decision::DenyOnce | Decision::DenyAlways => Status::Declined,
                    Decision::Cancelled => Status::Cancelled,
                },
            ),
        };
        if let ItemKind::Tool { approval: slot, .. } = &mut tracked.kind {
            *slot = Some(approval);
        }
        tracked.status = tracked.status.advance(status);
        vec![self.event(key)]
    }

    fn answer_questions(
        &mut self,
        key: &str,
        given: &[Option<QuestionAnswer>],
    ) -> Vec<EngineEvent> {
        let native = match self.items.get(key).map(|tracked| &tracked.kind) {
            Some(ItemKind::Question { questions, .. }) => {
                match crate::question::native_collection_answers(questions, given) {
                    Ok(native) => native,
                    Err(message) => return vec![crate::question::invalid_answer(key, message)],
                }
            }
            _ => return Vec::new(),
        };
        match self.questions.remove(key).expect("checked by the caller") {
            Asking::Request { rpc, ids } => {
                let mut map = Map::new();
                for (id, answer) in ids.iter().zip(&native) {
                    if let Some(answer) = answer.as_ref().filter(|answer| !answer.is_empty()) {
                        map.insert(id.clone(), json!({ "answers": answer }));
                    }
                }
                self.reply(rpc, json!({ "answers": map }));
            }
            Asking::Message { texts } => {
                let parts: Vec<String> = texts
                    .iter()
                    .zip(&native)
                    .filter_map(|(text, answer)| {
                        let answer = answer.as_ref().filter(|answer| !answer.is_empty())?;
                        let quoted: Vec<String> =
                            text.lines().map(|line| format!("> {line}")).collect();
                        Some(format!("{}\n\n{}", quoted.join("\n"), answer.join(", ")))
                    })
                    .collect();
                if !parts.is_empty() {
                    self.messages.push(parts.join("\n\n"));
                }
            }
        }
        let Some(tracked) = self.items.get_mut(key) else {
            return Vec::new();
        };
        if let ItemKind::Question { answers, .. } = &mut tracked.kind {
            *answers = Some(given.to_vec().into());
        }
        // Skipping every question declines them, as with the other engines.
        let skipped = given.iter().all(Option::is_none);
        tracked.status = tracked.status.advance(if skipped {
            Status::Declined
        } else {
            Status::Done
        });
        vec![self.event(key)]
    }

    /// Codex settled a request without us (the turn ended, or it was
    /// auto-reviewed): the item stops waiting on it.
    pub(super) fn resolved(&mut self, rpc: &Value, out: &mut Vec<EngineEvent>) {
        let approval = self.approvals.iter().find_map(|(key, queue)| {
            let at = queue.iter().position(|asked| asked.rpc == *rpc)?;
            Some((key.clone(), at))
        });
        if let Some((key, at)) = approval {
            let queue = self.approvals.get_mut(&key).expect("found above");
            let standalone = queue.remove(at).is_some_and(|asked| asked.standalone);
            if at > 0 {
                return;
            }
            let next = queue.front().map(|next| next.approval.clone());
            if next.is_none() {
                self.approvals.remove(&key);
            }
            if let Some(tracked) = self.items.get_mut(&key)
                && tracked.status == Status::AwaitingUser
            {
                match next {
                    Some(next) => {
                        if let ItemKind::Tool { approval, .. } = &mut tracked.kind {
                            *approval = Some(next);
                        }
                    }
                    None if standalone => tracked.status = Status::Done,
                    None => tracked.status = Status::Running,
                }
                out.push(self.event(&key));
            }
            return;
        }
        let question = self
            .questions
            .iter()
            .find_map(|(key, asking)| match asking {
                Asking::Request { rpc: id, .. } if id == rpc => Some(key.clone()),
                _ => None,
            });
        if let Some(key) = question {
            self.questions.remove(&key);
            if let Some(tracked) = self.items.get_mut(&key)
                && tracked.status == Status::AwaitingUser
            {
                tracked.status = Status::Running;
                out.push(self.event(&key));
            }
        }
    }

    /// An agent message may carry questions it doesn't wait for; answers
    /// go back as the next user message.
    pub(super) fn async_questions(
        &mut self,
        key: &str,
        scope: Scope,
        questions: Option<Value>,
        out: &mut Vec<EngineEvent>,
    ) {
        // `null` when the message asks nothing.
        let Some(questions) = questions.filter(|value| !value.is_null()) else {
            return;
        };
        let native: Vec<AsyncQuestion> = match serde_json::from_value(questions) {
            Ok(native) => native,
            Err(error) => {
                let message = format!("invalid Codex asynchronous questions: {error}");
                self.error_item(scope, ErrorKind::Execution, &message, out);
                return;
            }
        };
        if native.iter().any(|question| {
            question.title.trim().is_empty()
                || question.options.iter().any(|label| label.trim().is_empty())
        }) {
            let message = "Codex asynchronous questions require nonempty prompts and option labels";
            self.error_item(scope, ErrorKind::Execution, message, out);
            return;
        }
        if native.is_empty() {
            return;
        }
        let texts = native
            .iter()
            .map(|question| question.title.clone())
            .collect();
        let questions: Vec<Question> = native
            .into_iter()
            .enumerate()
            .map(|(index, question)| Question {
                id: index.to_string(),
                header: None,
                text: question.title.into(),
                multi_select: false,
                options: question
                    .options
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| QuestionOption {
                        id: format!("option-{index}"),
                        label,
                        description: None,
                    })
                    .collect::<Vec<_>>()
                    .into(),
            })
            .collect();
        let key = format!("{key}:questions");
        self.questions
            .insert(key.clone(), Asking::Message { texts });
        let kind = ItemKind::Question {
            blocking: false,
            questions: questions.into(),
            answers: None,
            answer_text: None,
        };
        self.track(key.clone(), scope, Status::AwaitingUser, kind);
        out.push(self.event(&key));
    }

    pub(super) fn reply(&mut self, rpc: Value, result: Value) {
        self.rpc.reply(rpc, result);
    }

    fn reply_error(&mut self, rpc: Value, code: i64, message: &str) {
        self.rpc.reply_error(rpc, code, message);
    }
}

/// `requestUserInput` prompts as questions, or `None` when one lacks its id,
/// its text or an option label: never a question nobody can answer.
fn user_questions(questions: Vec<UserQuestion>) -> Option<Vec<Question>> {
    let questions = questions
        .into_iter()
        .map(|question| {
            let options = question
                .options
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .map(|(index, option)| {
                    Some(QuestionOption {
                        id: format!("option-{index}"),
                        label: option.label.filter(|label| !label.trim().is_empty())?,
                        description: option
                            .description
                            .filter(|text| !text.is_empty())
                            .map(Into::into),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Question {
                id: question.id.filter(|id| !id.trim().is_empty())?,
                text: question
                    .question
                    .filter(|text| !text.trim().is_empty())?
                    .into(),
                header: question.header.filter(|header| !header.is_empty()),
                multi_select: false,
                options: options.into(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    (!questions.is_empty()).then_some(questions)
}

fn strip_nulls(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key.clone(), strip_nulls(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}
