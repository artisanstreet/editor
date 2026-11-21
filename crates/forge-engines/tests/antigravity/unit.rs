use super::decoder::{Answered, Decoder, Formed, Permitted};
use super::protocol::{Frame, Opened, Prompted};
use crate::EngineEvent;
use artisan_api::{Answer, Decision, Permissions, QuestionAnswer};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug)]
#[expect(dead_code, reason = "read through the Debug snapshot")]
enum Step {
    Event(Box<EngineEvent>),
    Reply(Value),
    AutoReply(Value),
    ProtocolError(String),
}

fn recorded_answer(later: &[Value], rpc: &Value, request: &Value) -> Option<Answer> {
    let reply = later
        .iter()
        .find_map(|line| line.get("client").filter(|client| client["id"] == *rpc))?;
    let outcome = &reply["result"]["outcome"];
    let question = request["toolCall"]["toolCallId"]
        .as_str()
        .is_some_and(|id| id.starts_with("interaction_"));
    let chosen = (outcome["outcome"] == "selected")
        .then(|| outcome["optionId"].as_str())
        .flatten();
    if question {
        return Some(Answer::Question {
            answers: vec![chosen.map(|id| QuestionAnswer::Choices {
                ids: vec![id.to_owned()].into(),
            })],
        });
    }
    let Some(chosen) = chosen else {
        return Some(Answer::Approval {
            decision: Decision::Cancelled,
        });
    };
    let kind = request["options"]
        .as_array()?
        .iter()
        .find(|option| option["optionId"] == chosen)?["kind"]
        .as_str()?;
    let decision = match kind {
        "allow_once" => Decision::AllowOnce,
        "allow_always" => Decision::AllowAlways,
        "reject_always" => Decision::DenyAlways,
        _ => Decision::DenyOnce,
    };
    Some(Answer::Approval { decision })
}

fn recorded_form(later: &[Value], rpc: &Value, request: &Value) -> Option<Answer> {
    let reply = later
        .iter()
        .find_map(|line| line.get("client").filter(|client| client["id"] == *rpc))?;
    let result = &reply["result"];
    let properties = request["requestedSchema"]["properties"].as_object()?;
    let asked = properties.keys().filter(|key| {
        key.strip_suffix("_other")
            .is_none_or(|base| !properties.contains_key(base))
    });
    let answers = asked
        .map(|key| {
            if result["action"] != "accept" {
                return None;
            }
            match &result["content"][key] {
                Value::String(id) => Some(QuestionAnswer::Choices {
                    ids: vec![id.clone()].into(),
                }),
                Value::Array(ids) => Some(QuestionAnswer::Choices {
                    ids: ids
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                        .into(),
                }),
                Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => None,
            }
        })
        .collect();
    Some(Answer::Question { answers })
}

#[derive(Default)]
struct Sent {
    prompts: HashSet<Value>,
    open_prompts: HashSet<Value>,
    opens: HashSet<Value>,
}

impl Sent {
    fn track(&mut self, client: &Value) {
        match client["method"].as_str() {
            Some("session/prompt") => {
                self.prompts.insert(client["id"].clone());
                self.open_prompts.insert(client["id"].clone());
            }
            Some("session/new" | "session/resume" | "session/set_config_option") => {
                self.opens.insert(client["id"].clone());
            }
            _ => {}
        }
    }
}

fn answered(
    decoder: &mut Decoder,
    key: &str,
    asked: Vec<EngineEvent>,
    answer: Option<Answer>,
    steps: &mut Vec<Step>,
) {
    steps.extend(asked.into_iter().map(|event| Step::Event(Box::new(event))));
    if let Some(answer) = answer
        && let Answered::Reply { result, events, .. } = decoder.answer(key, &answer)
    {
        steps.push(Step::Reply(result));
        steps.extend(events.into_iter().map(|event| Step::Event(Box::new(event))));
    }
}

fn replay(case: &str, permissions: Permissions) -> Vec<Step> {
    let transcript = crate::transcript_tests::read("antigravity", case);
    let lines: Vec<&str> = transcript
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let parsed: Vec<Value> = lines
        .iter()
        .map(|line| serde_json::from_str(line).unwrap_or(Value::Null))
        .collect();
    let mut decoder = Decoder::default();
    let mut sent = Sent::default();
    let mut steps = Vec::new();
    for (index, (line, value)) in lines.iter().zip(&parsed).enumerate() {
        if let Some(client) = value.get("client") {
            sent.track(client);
            continue;
        }
        let frame = match Frame::parse(line) {
            Ok(frame) => frame,
            Err(error) => {
                steps.push(Step::ProtocolError(diagnostics::chain(&error)));
                continue;
            }
        };
        let mut events = Vec::new();
        match frame {
            Frame::Update { update } => events.extend(decoder.update(*update)),
            Frame::Response {
                id,
                result: Ok(result),
            } if sent.prompts.contains(&Value::from(id)) => {
                sent.open_prompts.remove(&Value::from(id));
                match serde_json::from_value::<Prompted>(result) {
                    Ok(prompted) => {
                        events.extend(decoder.ended(&prompted, !sent.open_prompts.is_empty()));
                    }
                    Err(error) => steps.push(Step::ProtocolError(error.to_string())),
                }
            }
            Frame::Response {
                id,
                result: Ok(result),
            } if sent.opens.contains(&Value::from(id)) => {
                if let Ok(opened) = serde_json::from_value::<Opened>(result) {
                    decoder.configured(&opened.config_options, &mut events);
                }
            }
            Frame::Permission { id, request } => {
                let key = request.tool_call.tool_call_id.clone();
                let raw = value["params"].clone();
                match decoder.permission(id.clone(), *request, permissions) {
                    Permitted::Ask(asked) => answered(
                        &mut decoder,
                        &key,
                        asked,
                        recorded_answer(&parsed[index + 1..], &id, &raw),
                        &mut steps,
                    ),
                    Permitted::Reply {
                        result,
                        events: announced,
                        ..
                    } => {
                        events.extend(announced);
                        steps.push(Step::AutoReply(result));
                    }
                }
            }
            Frame::Form { id, request } => {
                let key = request.tool_call_id.clone().unwrap_or_default();
                let raw = value["params"].clone();
                match decoder.form(id.clone(), *request) {
                    Formed::Asked(asked) => answered(
                        &mut decoder,
                        &key,
                        asked,
                        recorded_form(&parsed[index + 1..], &id, &raw),
                        &mut steps,
                    ),
                    Formed::Refused {
                        result,
                        events: refused,
                        ..
                    } => {
                        events.extend(refused);
                        steps.push(Step::AutoReply(result));
                    }
                }
            }
            Frame::Response { .. } | Frame::Notice { .. } | Frame::Unsupported { .. } => {}
        }
        steps.extend(events.into_iter().map(|event| Step::Event(Box::new(event))));
    }
    steps.extend(
        decoder
            .finish()
            .into_iter()
            .map(|event| Step::Event(Box::new(event))),
    );
    merge_live(steps)
}

fn merge_live(steps: Vec<Step>) -> Vec<Step> {
    let mut merged: Vec<Step> = Vec::new();
    for step in steps {
        if let Step::Event(event) = &step
            && let Some(Step::Event(last)) = merged.last_mut()
            && crate::transcript_tests::merge_live(last.as_mut(), event.as_ref())
        {
            continue;
        }
        merged.push(step);
    }
    merged
}

#[test]
fn a_plain_reply_streams_into_one_final_message_with_its_usage() {
    insta::assert_debug_snapshot!(replay("plain", Permissions::Auto));
}

#[test]
fn reads_commands_and_new_files_become_typed_tool_rows() {
    insta::assert_debug_snapshot!(replay("tools", Permissions::Auto));
}

#[test]
fn a_command_waits_on_the_user_and_the_answer_goes_back_as_the_chosen_option() {
    insta::assert_debug_snapshot!(replay("approval", Permissions::Auto));
}

#[test]
fn full_access_approves_for_the_user_without_asking() {
    let steps = replay("approval", Permissions::Full);
    assert!(
        steps.iter().any(|step| matches!(step, Step::AutoReply(_))),
        "Full access left Antigravity's approval for the user: {steps:#?}"
    );
    assert!(
        !steps.iter().any(|step| matches!(
            step,
            Step::Event(event) if matches!(**event, EngineEvent::Item { status: artisan_api::Status::AwaitingUser, .. })
        )),
        "Full access showed an approval card: {steps:#?}"
    );
}

#[test]
fn a_form_question_is_asked_and_answered_with_the_chosen_choice() {
    insta::assert_debug_snapshot!(replay("question", Permissions::Full));
}

#[test]
fn a_steer_keeps_streaming_into_one_reply_instead_of_ending_the_turn() {
    insta::assert_debug_snapshot!(replay("steer", Permissions::Auto));
}

#[test]
fn a_cancelled_prompt_settles_the_partial_reply() {
    insta::assert_debug_snapshot!(replay("cancel", Permissions::Auto));
}

#[test]
fn a_resumed_session_answers_from_its_history() {
    insta::assert_debug_snapshot!(replay("resume", Permissions::Auto));
}
