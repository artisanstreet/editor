//! Transcript snapshots of the OpenCode decoder.
//!
//! Each transcript line is one event from OpenCode's global stream
//! (`GET /api/event`), recorded from OpenCode 1.18.32. The replay plays the
//! driver's part: it registers each prompt OpenCode admits (the first as the
//! run's message, later ones as steers) and answers permissions and
//! questions the way the recording did. Consecutive live appends to the same
//! target are merged to keep snapshots readable.

use super::decoder::{Decoder, Reply};
use crate::EngineEvent;
use artisan_api::{Answer, Decision, ItemId};
use serde_json::Value;

#[derive(Debug)]
#[expect(dead_code, reason = "read through the Debug snapshot")]
enum Step {
    Event(Box<EngineEvent>),
    /// The request the driver sends OpenCode.
    Reply(Reply),
    /// Whether the run would end here.
    Idle(bool),
}

fn replay(case: &str) -> Vec<Step> {
    let transcript = crate::transcript_tests::read("opencode", case);
    let mut decoder: Option<Decoder> = None;
    let mut prompts = 0;
    let mut steps = Vec::new();
    for line in transcript.lines().filter(|line| !line.trim().is_empty()) {
        let event: Value =
            serde_json::from_str(line).unwrap_or_else(|error| panic!("{case}: {error}: {line}"));
        let data = &event["data"];
        // The driver learns the session id when it creates the session.
        let decoder = decoder.get_or_insert_with(|| Decoder::new(None));
        if !decoder.has_root()
            && let Some(session) = data["sessionID"].as_str()
        {
            decoder.follow_root(session.to_owned());
        }
        let mut events = decoder.feed(line).unwrap();
        match event["type"].as_str() {
            Some("session.next.prompt.admitted") => {
                let message = data["messageID"].as_str().expect("message id");
                prompts += 1;
                let steer = (prompts > 1).then(|| ItemId::from(format!("steer-{prompts}")));
                events.extend(decoder.prompt_sent(message, steer));
            }
            Some("permission.v2.asked") => {
                let key = data["source"]["callID"].as_str().expect("call id");
                let answer = Answer::Approval {
                    decision: Decision::AllowOnce,
                };
                let (reply, answered) = decoder.answer(key, &answer).expect("pending permission");
                events.extend(answered);
                steps.extend(events.drain(..).map(|event| Step::Event(Box::new(event))));
                steps.push(Step::Reply(reply));
            }
            Some("question.v2.asked") => {
                let key = data["tool"]["callID"].as_str().expect("call id");
                let answer = Answer::Question {
                    answers: vec![Some(artisan_api::QuestionAnswer::Choices {
                        ids: (vec!["option-1".to_owned()]).into(),
                    })],
                };
                let (reply, answered) = decoder.answer(key, &answer).expect("pending question");
                events.extend(answered);
                steps.extend(events.drain(..).map(|event| Step::Event(Box::new(event))));
                steps.push(Step::Reply(reply));
            }
            _ => {}
        }
        steps.extend(events.into_iter().map(|event| Step::Event(Box::new(event))));
    }
    steps.push(Step::Idle(decoder.as_ref().is_some_and(Decoder::idle)));
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
fn plain_reply() {
    insta::assert_debug_snapshot!(replay("plain"));
}

#[test]
fn file_tools_and_todo_list() {
    insta::assert_debug_snapshot!(replay("tools"));
}

#[test]
fn steer_lands_at_the_next_step_and_queue_after_the_turn() {
    insta::assert_debug_snapshot!(replay("steer"));
}

#[test]
fn permission_is_asked_on_the_tool_and_answered_through_the_api() {
    insta::assert_debug_snapshot!(replay("permission"));
}

#[test]
fn question_is_asked_and_answered_through_the_api() {
    insta::assert_debug_snapshot!(replay("question"));
}

/// The chosen variant is the `model.variant` OpenCode runs; older threads
/// kept it in their effort.
#[test]
fn the_chosen_variant_is_the_model_variant() {
    let model = super::model_ref(Some("anthropic/claude-opus-5"), Some("max")).expect("model");
    assert_eq!(
        model,
        serde_json::json!({ "providerID": "anthropic", "id": "claude-opus-5", "variant": "max" })
    );
}

/// OpenCode runs a Forge agent per permission level: Auto asks before
/// shell commands and other folders, Full allows everything.
#[test]
fn each_permission_level_is_an_agent_opencode_knows() {
    let home = tempfile::tempdir().unwrap();
    let dir = super::write_agents(home.path()).unwrap();
    let config: Value =
        serde_json::from_slice(&std::fs::read(dir.join("opencode.json")).unwrap()).unwrap();
    let rules = |permissions| config["agent"][super::agent_name(permissions)]["permission"].clone();
    let auto = rules(artisan_api::Permissions::Auto);
    assert_eq!(
        (&auto["bash"], &auto["external_directory"]),
        (&Value::from("ask"), &Value::from("ask"))
    );
    let full = rules(artisan_api::Permissions::Full);
    assert_eq!(full["external_directory"], "allow");
    assert!(full.get("bash").is_none(), "nothing asks: {full}");
    assert_eq!(config["agent"].as_object().unwrap().len(), 2);
}
