//! What ACP engines ask the user: tool permissions (on the tool they gate),
//! Cursor's questions and its plan approval, and the answers.

use super::elicitation::ElicitationField;
use super::tools::{content_blocks_text, shell_command, str_field};
use super::{AcpDecoder, Profile};
use crate::EngineEvent;
use artisan_api::{
    Answer, Approval, ApprovalRequest, Decision, ErrorKind, ItemKind, Phase, Question,
    QuestionOption, Status,
};
use serde_json::{Value, json};
use std::collections::HashSet;

/// An engine request waiting on the user, by item key.
#[derive(Debug)]
pub(super) enum Pending {
    /// `elicitation/create`: one schema property per question.
    Elicitation {
        rpc: Value,
        fields: Vec<ElicitationField>,
        asked: Vec<Question>,
    },
    /// `cursor/ask_question`: question ids and their `(option id, label)`s.
    CursorQuestion {
        rpc: Value,
        questions: Vec<(String, Vec<(String, String)>)>,
        asked: Vec<Question>,
    },
    CursorPlan {
        rpc: Value,
    },
}

/// One `session/request_permission` call, queued on its tool.
#[derive(Debug)]
pub(super) struct Permission {
    rpc: Value,
    /// The engine's option ids for the decisions it offers.
    options: Vec<(String, Decision)>,
    approval: Approval,
}

#[derive(serde::Deserialize)]
struct CursorQuestion {
    id: String,
    prompt: String,
    #[serde(default)]
    options: Vec<CursorChoice>,
    #[serde(default, rename = "allowMultiple")]
    allow_multiple: bool,
}
#[derive(serde::Deserialize)]
struct CursorChoice {
    id: String,
    label: String,
}

fn permission_options(value: &Value) -> Result<Vec<(String, Decision)>, String> {
    #[derive(serde::Deserialize)]
    struct OptionEntry {
        #[serde(rename = "optionId")]
        id: String,
        kind: OptionKind,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum OptionKind {
        AllowOnce,
        AllowAlways,
        RejectOnce,
        RejectAlways,
    }
    let entries: Vec<OptionEntry> = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid permission options: {error}"))?;
    if entries.is_empty() {
        return Err("permission request requires offered options".into());
    }
    let mut ids = HashSet::new();
    let mut options = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.id.trim().is_empty() || !ids.insert(entry.id.clone()) {
            return Err("permission options require nonempty unique native IDs".into());
        }
        let decision = match entry.kind {
            OptionKind::AllowOnce => Decision::AllowOnce,
            OptionKind::AllowAlways => Decision::AllowAlways,
            OptionKind::RejectOnce => Decision::DenyOnce,
            OptionKind::RejectAlways => Decision::DenyAlways,
        };
        options.push((entry.id, decision));
    }
    Ok(options)
}

impl AcpDecoder {
    /// `session/request_permission`: the approval goes on the tool call it
    /// gates. A second request for the same call waits behind the first.
    pub(super) fn permission(&mut self, rpc: Value, params: &Value, events: &mut Vec<EngineEvent>) {
        let call = &params["toolCall"];
        let parsed = permission_options(&params["options"]);
        let id = call["toolCallId"]
            .as_str()
            .filter(|id| !id.trim().is_empty());
        let (id, options) = match (id, parsed) {
            (Some(id), Ok(options)) => (id, options),
            (_, parsed) => {
                let message = parsed
                    .err()
                    .unwrap_or_else(|| "permission request requires a nonempty toolCallId".into());
                tracing::warn!(engine = ?self.profile, session = ?self.session_id, request = %rpc, message, "declining a malformed permission request");
                self.rpc
                    .reply_error(rpc, crate::rpc::INVALID_PARAMS, &message);
                self.questions += 1;
                events.push(error_item(
                    format!("permission-error:{}", self.questions),
                    message,
                ));
                return;
            }
        };
        self.close_message(Phase::Commentary, events);
        self.merge_tool(id, call);
        let tool = self.tools.get_mut(id).expect("merged above");
        if tool.status.is_some_and(Status::is_terminal) {
            tracing::warn!(engine = ?self.profile, session = ?self.session_id, tool = id, request = %rpc, "asked to approve a tool call that already ended; cancelling the request");
            self.reply(rpc, json!({ "outcome": { "outcome": "cancelled" } }));
            return;
        }
        let reason = content_blocks_text(&call["content"]);
        let request = match tool.kind.as_deref() {
            Some("execute") => match shell_command(tool) {
                Ok(command) => ApprovalRequest::Command {
                    command,
                    cwd: str_field(&tool.raw_input, &["cwd"]),
                },
                Err(message) => {
                    self.reply(rpc, json!({ "outcome": { "outcome": "cancelled" } }));
                    events.push(error_item(format!("{id}:permission-error"), message));
                    return;
                }
            },
            Some("edit" | "delete" | "move") => ApprovalRequest::FileChange,
            _ => ApprovalRequest::Action {
                description: (tool
                    .title
                    .clone()
                    .unwrap_or_else(|| "Approve this tool call?".to_owned()))
                .into(),
            },
        };
        let approval = Approval {
            request,
            reason: ((!reason.is_empty()).then_some(reason)).map(Into::into),
            options: options.iter().map(|(_, decision)| *decision).collect(),
            decision: None,
        };
        let queue = self.permissions.entry(id.to_owned()).or_default();
        queue.push_back(Permission {
            rpc,
            options,
            approval: approval.clone(),
        });
        if queue.len() == 1 {
            tool.approval = Some(approval);
            tool.status = Some(Status::AwaitingUser);
        }
        self.emit_tool(id, events);
    }

    /// The user decided the permission shown on tool `key`.
    fn answer_permission(&mut self, key: &str, decision: Decision, events: &mut Vec<EngineEvent>) {
        let Some(queue) = self.permissions.get_mut(key) else {
            return;
        };
        let Some(Permission {
            rpc,
            options,
            approval,
        }) = queue.pop_front()
        else {
            return;
        };
        let Some((option, _)) = options.iter().find(|(_, offered)| *offered == decision) else {
            tracing::warn!(engine = ?self.profile, session = ?self.session_id, key, ?decision, "approval answer was not offered; request remains pending");
            queue.push_front(Permission {
                rpc,
                options,
                approval,
            });
            return;
        };
        let next = queue.front().map(|next| next.approval.clone());
        if next.is_none() {
            self.permissions.remove(key);
        }
        self.reply(
            rpc,
            json!({"outcome":{"outcome":"selected","optionId":option}}),
        );
        if let Some(tool) = self.tools.get_mut(key) {
            settle_approval(tool, approval, decision, next);
        }
        self.emit_tool(key, events);
    }

    /// Cancels every permission request: `session/cancel` obliges the client
    /// to settle them.
    pub(super) fn cancel_permissions(&mut self, events: &mut Vec<EngineEvent>) {
        let mut keys: Vec<String> = self.permissions.keys().cloned().collect();
        keys.sort();
        for key in keys {
            let mut last = None;
            for permission in self.permissions.remove(&key).into_iter().flatten() {
                self.reply(permission.rpc, json!({"outcome":{"outcome":"cancelled"}}));
                last.get_or_insert(permission.approval);
            }
            if let (Some(tool), Some(approval)) = (self.tools.get_mut(&key), last) {
                settle_approval(tool, approval, Decision::Cancelled, None);
            }
            self.emit_tool(&key, events);
        }
    }

    pub(super) fn cursor_question(
        &mut self,
        rpc: Value,
        params: &Value,
        events: &mut Vec<EngineEvent>,
    ) {
        let key = params["toolCallId"].as_str().map_or_else(
            || {
                self.questions += 1;
                format!("question:{}", self.questions)
            },
            str::to_owned,
        );
        let parsed = <Vec<CursorQuestion> as serde::Deserialize>::deserialize(&params["questions"]);
        let parsed = match parsed {
            Ok(questions)
                if !questions.is_empty()
                    && questions.iter().all(|question| {
                        !question.id.trim().is_empty()
                            && !question.prompt.trim().is_empty()
                            && question.options.iter().all(|option| {
                                !option.id.trim().is_empty() && !option.label.trim().is_empty()
                            })
                    }) =>
            {
                questions
            }
            _ => {
                let message = "Cursor supplied malformed questions: they need nonempty IDs and prompts, with valid option IDs and labels";
                self.reject_question(rpc, &key, message, events);
                return;
            }
        };
        let duplicates = {
            let mut ids = HashSet::new();
            parsed.iter().any(|question| {
                !ids.insert(question.id.as_str()) || {
                    let mut options = HashSet::new();
                    question
                        .options
                        .iter()
                        .any(|option| !options.insert(option.id.as_str()))
                }
            })
        };
        if duplicates {
            let message = "Cursor supplied questions or options that share an ID, so no answer could name one";
            self.reject_question(rpc, &key, message, events);
            return;
        }
        self.hidden.insert(key.clone());
        let header = params["title"]
            .as_str()
            .filter(|title| !title.is_empty())
            .map(str::to_owned);
        let mut ids = Vec::new();
        let mut questions = Vec::new();
        for question in parsed {
            let id = question.id;
            let options: Vec<(String, String)> = question
                .options
                .into_iter()
                .map(|option| (option.id, option.label))
                .collect();
            questions.push(Question {
                id: id.clone(),
                header: header.clone(),
                text: (question.prompt).into(),
                multi_select: question.allow_multiple,
                options: options
                    .iter()
                    .map(|(id, label)| QuestionOption {
                        id: id.clone(),
                        label: label.clone(),
                        description: None,
                    })
                    .collect::<Vec<_>>()
                    .into(),
            });
            ids.push((id, options));
        }
        self.pending.insert(
            key.clone(),
            Pending::CursorQuestion {
                rpc,
                questions: ids,
                asked: questions.clone(),
            },
        );
        self.close_message(Phase::Commentary, events);
        events.push(EngineEvent::Item {
            key,
            parent: None,
            status: Status::AwaitingUser,
            kind: ItemKind::Question {
                blocking: true,
                questions: questions.into(),
                answers: None,
                answer_text: None,
            },
        });
    }

    /// A question Artisan can't show: the engine hears why, and so does the
    /// user, as for the other engines.
    fn reject_question(
        &mut self,
        rpc: Value,
        key: &str,
        message: &str,
        events: &mut Vec<EngineEvent>,
    ) {
        tracing::warn!(engine = ?self.profile, session = ?self.session_id, key, request = %rpc, message, "declining a malformed question");
        self.rpc
            .reply_error(rpc, crate::rpc::INVALID_PARAMS, message);
        events.push(error_item(
            format!("invalid-question:{key}"),
            message.to_owned(),
        ));
    }

    pub(super) fn answer_raw(&mut self, key: &str, answer: &Answer) -> Vec<EngineEvent> {
        let mut events = Vec::new();
        if let Answer::Approval { decision } = answer
            && self.permissions.contains_key(key)
        {
            self.answer_permission(key, *decision, &mut events);
            return events;
        }
        let Some(pending) = self.pending.remove(key) else {
            tracing::debug!(engine = ?self.profile, key, "answer for nothing pending");
            return events;
        };
        match (pending, answer) {
            (Pending::Elicitation { rpc, fields, asked }, Answer::Question { answers }) => {
                self.answer_elicitation(key, rpc, fields, asked, answers, &mut events);
            }
            (
                Pending::CursorQuestion {
                    rpc,
                    questions,
                    asked,
                },
                Answer::Question { answers },
            ) => {
                let outcome = if answers.iter().all(Option::is_none) {
                    json!({ "outcome": "skipped", "reason": "User skipped questions" })
                } else {
                    let selected = questions
                        .iter()
                        .zip(answers)
                        .filter_map(|((id, options), answer)| {
                            let answer = answer.as_ref()?;
                            let artisan_api::QuestionAnswer::Choices { ids } = answer else {
                                return Some(Err("Cursor accepts only choice IDs".to_owned()));
                            };
                            let Some(ids) = ids.inline() else {
                                return Some(Err(
                                    "Cursor submitted choices must be inline".to_owned()
                                ));
                            };
                            if ids.iter().any(|selected| {
                                !options.iter().any(|(option, _)| option == selected)
                            }) {
                                return Some(
                                    Err("Cursor received an unknown choice ID".to_owned()),
                                );
                            }
                            Some(Ok(json!({ "questionId": id, "selectedOptionIds": ids })))
                        })
                        .collect::<Result<Vec<_>, String>>();
                    let answers = match selected {
                        Ok(answers) => answers,
                        Err(message) => {
                            events.push(crate::question::invalid_answer(key, message));
                            self.pending.insert(
                                key.to_owned(),
                                Pending::CursorQuestion {
                                    rpc,
                                    questions,
                                    asked,
                                },
                            );
                            return events;
                        }
                    };
                    json!({ "outcome": "answered", "answers": answers })
                };
                self.reply(rpc, json!({ "outcome": outcome }));
                events.push(self.answered_question(key, asked, answers));
            }
            (Pending::CursorPlan { rpc }, Answer::Approval { decision }) => {
                let outcome = match decision {
                    Decision::Cancelled => "cancelled",
                    decision if decision.allows() => "accepted",
                    _ => "rejected",
                };
                self.reply(rpc, json!({ "outcome": { "outcome": outcome } }));
                let plan = self.plan.get_or_insert_default();
                if let Some(approval) = &mut plan.approval {
                    approval.decision = Some(*decision);
                }
                plan.status = Some(if decision.allows() {
                    Status::Running
                } else {
                    Status::Declined
                });
                self.emit_plan(&mut events);
            }
            (pending, answer) => {
                tracing::warn!(engine = ?self.profile, key, ?answer, "answer does not fit the pending request");
                self.pending.insert(key.to_owned(), pending);
            }
        }
        events
    }

    pub(super) fn answered_question(
        &self,
        key: &str,
        questions: Vec<Question>,
        answers: &[Option<artisan_api::QuestionAnswer>],
    ) -> EngineEvent {
        // Skipping every question declines them, as with the other engines.
        let skipped = answers.iter().all(Option::is_none);
        EngineEvent::Item {
            key: key.to_owned(),
            parent: None,
            status: if skipped {
                Status::Declined
            } else {
                Status::Done
            },
            kind: ItemKind::Question {
                blocking: self.profile == Profile::Cursor,
                questions: questions.into(),
                answers: Some(answers.to_vec().into()),
                answer_text: None,
            },
        }
    }
}

/// Records `decision` on the approval the tool showed. The next queued
/// request takes its place; otherwise the call runs, or was declined or
/// cancelled with its request.
fn settle_approval(
    tool: &mut super::tools::Tool,
    mut approval: Approval,
    decision: Decision,
    next: Option<Approval>,
) {
    approval.decision = Some(decision);
    if let Some(next) = next {
        tool.approval = Some(next);
        tool.status = Some(Status::AwaitingUser);
        return;
    }
    tool.approval = Some(approval);
    tool.status = Some(match decision {
        Decision::AllowOnce | Decision::AllowAlways => Status::Running,
        Decision::DenyOnce | Decision::DenyAlways => Status::Declined,
        Decision::Cancelled => Status::Cancelled,
    });
}

fn error_item(key: String, message: String) -> EngineEvent {
    EngineEvent::Item {
        key,
        parent: None,
        status: Status::Done,
        kind: ItemKind::Error {
            kind: ErrorKind::Execution,
            message: message.into(),
            retryable: false,
        },
    }
}
