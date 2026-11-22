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

fn opened_on(case: &str) -> Opened {
    let transcript = crate::transcript_tests::read("antigravity", case);
    let frames: Vec<Value> = transcript
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("{case} transcript line {} is not JSON: {error}", index + 1)
            })
        })
        .collect();
    let request = frames
        .iter()
        .filter_map(|frame| frame.get("client"))
        .find(|client| client["method"] == "session/new")
        .map(|client| client["id"].clone())
        .expect("invariant: the transcript opens a session");
    let result = frames
        .iter()
        .find(|frame| frame.get("client").is_none() && frame["id"] == request)
        .map(|frame| frame["result"].clone())
        .expect("invariant: the transcript answers session/new");
    serde_json::from_value(result).expect("the session/new result parses")
}

fn listed(opened: &Opened) -> Result<crate::catalog::Listing, crate::catalog::ListModelsError> {
    crate::catalog::listed_antigravity(super::models(opened))
        .map(|listing| crate::catalog::finish(artisan_api::EngineId::Antigravity, listing))
}

fn current_model(opened: &mut Opened, id: &str) {
    opened
        .config_options
        .iter_mut()
        .find(|option| option.is(super::protocol::MODEL))
        .expect("invariant: the session offers models")
        .current_value = Some(id.to_owned());
}

#[test]
fn the_picker_hides_orchestrators_and_shows_fixed_efforts_as_the_only_effort() {
    let listing = listed(&opened_on("plain")).unwrap();
    let rows: Vec<(&str, &str, Vec<&str>, Option<&str>)> = listing
        .models
        .iter()
        .map(|model| {
            (
                model.id.as_str(),
                model.name.as_str(),
                model.efforts.iter().map(String::as_str).collect(),
                model.default_effort.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (
                "gemini-4-neon",
                "Gemini 4 Neon",
                vec!["low", "medium", "high", "xhigh"],
                Some("high")
            ),
            (
                "gemini-4-argon",
                "Gemini 4 Argon",
                vec!["low", "medium", "high"],
                None
            ),
            (
                "gemini-4-hydrogen",
                "Gemini 4 Hydrogen",
                vec!["low", "medium", "high", "xhigh"],
                None
            ),
            (
                "gemini-3.8-flash",
                "Gemini 3.8 Flash",
                vec!["low", "medium", "high"],
                None
            ),
            (
                "claude-opus-5-5",
                "Claude Opus 5.5",
                vec!["low", "medium", "high"],
                None
            ),
            (
                "claude-sonnet-5-5",
                "Claude Sonnet 5.5",
                vec!["low", "medium", "high"],
                None
            ),
            (
                "gpt-oss-120b-medium",
                "GPT-OSS 120B",
                vec!["medium"],
                Some("medium")
            ),
        ]
    );
    assert_eq!(listing.default.as_deref(), Some("gemini-4-neon"));
}

#[test]
fn a_hidden_orchestrator_as_the_current_model_is_never_the_default() {
    let mut opened = opened_on("plain");
    current_model(&mut opened, "gemini-pro-orchestrator-neon");
    let listing = listed(&opened).unwrap();
    assert_eq!(listing.default, None);
    assert!(
        listing
            .models
            .iter()
            .all(|model| !model.id.starts_with("gemini-pro-orchestrator-")),
        "{:?}",
        listing.models
    );
}

#[test]
fn a_session_offering_only_orchestrators_says_so_instead_of_listing_nothing() {
    let mut opened = opened_on("plain");
    let model = opened
        .config_options
        .iter_mut()
        .find(|option| option.is(super::protocol::MODEL))
        .expect("invariant: the session offers models");
    model.options.retain(|choice| {
        matches!(choice, super::protocol::ConfigChoice::Value { value, .. }
            if value.starts_with("gemini-pro-orchestrator-"))
    });
    assert!(matches!(
        listed(&opened),
        Err(crate::catalog::ListModelsError::AntigravityOnlyOrchestrators { hidden: 2 })
    ));
}

async fn replayed_turn(case: &str, model: &str, effort: &str) {
    use crate::{Harness as _, RunIo, Turn, UserInput};
    use artisan_api::{EngineConfig, EngineId, ThreadId};
    let dir = tempfile::tempdir().unwrap();
    let engine = dir
        .path()
        .join(format!("agy{}", std::env::consts::EXE_SUFFIX));
    test_support::executable::install(&crate::fake_engine::binary(), &engine);
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("transcripts/replay/antigravity")
            .join(format!("{case}.jsonl")),
        dir.path().join("transcript.jsonl"),
    )
    .unwrap();
    let turn = Turn {
        thread_id: ThreadId::new(),
        engine: EngineId::Antigravity,
        binary: engine,
        home: dir.path().join("home"),
        cwd: dir.path().to_owned(),
        config: EngineConfig {
            model: Some(model.to_owned()),
            effort: Some(effort.to_owned()),
            permissions: Permissions::Full,
            ..EngineConfig::new(EngineId::Antigravity)
        },
        session_id: None,
        input: UserInput {
            text: "Reply with the single word: pong".to_owned(),
            ..UserInput::default()
        },
        launch: crate::Launch::default(),
        trace_dir: None,
    };
    let (_inputs, inbox) = tokio::sync::mpsc::channel(8);
    let (events, mut received) = tokio::sync::mpsc::channel(256);
    let running = tokio::spawn(super::AntigravityHarness.run(
        turn,
        RunIo {
            inbox,
            events,
            cancel: tokio_util::sync::CancellationToken::new(),
        },
    ));
    let drain = tokio::spawn(async move { while received.recv().await.is_some() {} });
    let outcome = test_support::wait::within(
        "the replayed turn ends",
        test_support::wait::LIVENESS,
        running,
    )
    .await
    .unwrap();
    drain.await.unwrap();
    let log = std::fs::read_to_string(dir.path().join("replay.log"))
        .unwrap_or_else(|error| format!("no replay log: {error}"));
    assert_eq!(
        outcome.status(),
        artisan_api::RunStatus::Done,
        "{:?}\n{log}",
        outcome.error()
    );
}

#[tokio::test]
async fn a_turn_on_a_fixed_effort_model_never_sets_the_thought_level() {
    replayed_turn("fixed-effort", "gpt-oss-120b-medium", "low").await;
}

#[tokio::test]
async fn a_turn_on_an_adjustable_model_sets_the_model_then_the_thought_level() {
    replayed_turn("adjustable-effort", "gemini-3.8-flash", "low").await;
}
