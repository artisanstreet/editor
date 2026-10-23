//! Transcript snapshots of the OpenCode decoder.
//!
//! Each transcript line is one event from OpenCode's global stream
//! (`GET /api/event`), recorded from OpenCode 2.1.0. The replay plays the
//! driver's part: it registers each prompt OpenCode admits (the first as the
//! run's message, later ones as steers) and answers permissions and forms
//! the way the recording did. Consecutive live appends to the same target
//! are merged to keep snapshots readable.

use super::decoder::{Decoder, Reply};
use crate::EngineEvent;
use artisan_api::{Answer, Decision, ItemId, QuestionAnswer};
use serde_json::Value;

#[derive(Debug)]
#[expect(dead_code, reason = "read through the Debug snapshot")]
enum Step {
    Event(Box<EngineEvent>),
    /// The request the driver sends OpenCode.
    Reply(Reply),
    /// Whether the run would end here.
    Idle(bool),
    /// The error the run ends with.
    RunError(String),
    /// A line OpenCode's protocol can't carry: the run fails.
    ProtocolError(String),
}

/// What the recording answered: the later `permission.replied` or
/// `form.replied`/`form.cancelled` naming `request`.
fn recorded_answer(events: &[Value], request: &str, form: &Value) -> Option<Answer> {
    events.iter().find_map(|event| {
        let data = &event["data"];
        match event["type"].as_str()? {
            "permission.replied" if data["requestID"] == request => {
                let decision = match data["reply"].as_str()? {
                    "once" => Decision::AllowOnce,
                    "always" => Decision::AllowAlways,
                    _ => Decision::DenyOnce,
                };
                Some(Answer::Approval { decision })
            }
            "form.cancelled" if data["id"] == request => {
                let fields = form["fields"].as_array()?.len();
                Some(Answer::Question {
                    answers: vec![None; fields],
                })
            }
            "form.replied" if data["id"] == request => {
                let answers = form["fields"]
                    .as_array()?
                    .iter()
                    .map(|field| {
                        let given = &data["answer"][field["key"].as_str()?];
                        let given: Vec<&str> = given.as_array().map_or_else(
                            || given.as_str().into_iter().collect(),
                            |items| items.iter().filter_map(Value::as_str).collect(),
                        );
                        let options = field["options"].as_array().cloned().unwrap_or_default();
                        let ids: Option<Vec<String>> = given
                            .iter()
                            .map(|value| {
                                options
                                    .iter()
                                    .position(|option| option["value"] == *value)
                                    .map(|index| format!("option-{index}"))
                            })
                            .collect();
                        Some(match ids {
                            Some(ids) if !ids.is_empty() => {
                                QuestionAnswer::Choices { ids: ids.into() }
                            }
                            _ => QuestionAnswer::Text {
                                text: given.join(", "),
                            },
                        })
                    })
                    .collect();
                Some(Answer::Question { answers })
            }
            _ => None,
        }
    })
}

fn replay(case: &str) -> Vec<Step> {
    let transcript = crate::transcript_tests::read("opencode", case);
    let lines: Vec<&str> = transcript
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    // A line that isn't JSON still goes to the decoder, which fails it.
    let parsed: Vec<Value> = lines
        .iter()
        .map(|line| serde_json::from_str(line).unwrap_or(Value::Null))
        .collect();
    let mut decoder: Option<Decoder> = None;
    let mut root: Option<String> = None;
    let mut prompts = 0;
    let mut steps = Vec::new();
    for (index, (line, event)) in lines.iter().zip(&parsed).enumerate() {
        let data = &event["data"];
        // The driver learns the session id when it creates the session.
        let decoder = decoder.get_or_insert_with(|| Decoder::new(None));
        if !decoder.has_root()
            && let Some(session) = data["sessionID"].as_str()
        {
            decoder.follow_root(session.to_owned());
            root = Some(session.to_owned());
        }
        let mut events = match decoder.feed(line) {
            Ok(events) => events,
            Err(error) => {
                steps.push(Step::ProtocolError(format!("{error:#}")));
                continue;
            }
        };
        let later = &parsed[index + 1..];
        let asked = match event["type"].as_str() {
            // The driver sends only the root's prompts; subagents get theirs
            // from OpenCode.
            Some("session.inbox.enqueued")
                if data["item"]["type"] == "user"
                    && data["sessionID"].as_str() == root.as_deref() =>
            {
                let message = data["inboxID"].as_str().expect("inbox id");
                prompts += 1;
                let steer = (prompts > 1).then(|| ItemId::from(format!("steer-{prompts}")));
                events.extend(decoder.prompt_sent(message, steer));
                None
            }
            Some("permission.asked") => {
                let key = data["source"]["id"].as_str().expect("call id");
                let request = data["id"].as_str().expect("request id");
                recorded_answer(later, request, &Value::Null).map(|answer| (key, answer))
            }
            Some("form.created") => {
                let form = &data["form"];
                let key = form["metadata"]["tool"]["id"]
                    .as_str()
                    .or_else(|| form["id"].as_str())
                    .expect("form id");
                let request = form["id"].as_str().expect("form id");
                recorded_answer(later, request, form).map(|answer| (key, answer))
            }
            _ => None,
        };
        if let Some((key, answer)) = asked
            && let Some((reply, answered)) = decoder.answer(key, &answer)
        {
            events.extend(answered);
            steps.extend(
                std::mem::take(&mut events)
                    .into_iter()
                    .map(|event| Step::Event(Box::new(event))),
            );
            steps.push(Step::Reply(reply));
        }
        steps.extend(events.into_iter().map(|event| Step::Event(Box::new(event))));
    }
    steps.push(Step::Idle(decoder.as_ref().is_some_and(Decoder::idle)));
    if let Some(error) = decoder.and_then(|decoder| decoder.error) {
        steps.push(Step::RunError(error));
    }
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
fn shell_permission_is_asked_on_the_tool_and_interrupt_cancels_the_next() {
    insta::assert_debug_snapshot!(replay("permission"));
}

#[test]
fn edit_read_and_subagent() {
    insta::assert_debug_snapshot!(replay("tools"));
}

#[test]
fn steer_lands_at_the_next_step() {
    insta::assert_debug_snapshot!(replay("steer"));
}

#[test]
fn question_is_asked_as_a_form_and_answered_through_the_api() {
    insta::assert_debug_snapshot!(replay("question"));
}

/// Skipping every question dismisses the form, which ends OpenCode's turn.
#[test]
fn a_dismissed_question_cancels_the_form() {
    insta::assert_debug_snapshot!(replay("question-dismissed"));
}

/// A model OpenCode can't route fails the execution, which ends the run.
#[test]
fn a_failed_execution_ends_the_run_with_its_error() {
    insta::assert_debug_snapshot!(replay("execution-failed"));
}

/// Lines that aren't OpenCode events fail the run.
#[test]
fn malformed_frames_are_protocol_errors() {
    insta::assert_debug_snapshot!(replay("malformed"));
}

/// A text that ends without its body is an error, never an empty reply.
#[test]
fn a_text_without_its_body_fails_the_run() {
    insta::assert_debug_snapshot!(replay("text-without-body"));
}

/// Forms Artisan can't ask and tool calls missing their fields become
/// explicit errors, never empty cards.
#[test]
fn malformed_native_payloads_become_errors() {
    insta::assert_debug_snapshot!("malformed_question", replay("malformed-question"));
    insta::assert_debug_snapshot!("tool_fields", replay("tool-fields"));
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
    assert_eq!(config["agent"].as_object().unwrap().len(), 3);
    assert_eq!(config["agent"]["artisan-summary"]["permission"]["*"], "deny");
}

/// A background shell's call returns at once (`status: "running"` and its
/// shell id) and the shell ends with `shell.exited`: the command stays
/// running, in the background, until then. Frames follow the recorded
/// `shell` call and `shell.exited`; the background fields follow the schema
/// OpenCode 2.1's binary carries, not a recording.
#[test]
fn a_background_shell_runs_until_it_exits() {
    use artisan_api::Status::{Done, Running};
    let session = "ses_ed6217996ffeh7hIqceloWYI1S";
    let call = "call_00_p9ymnkbi7jpt3qyw3zpfd46z";
    let frames = [
        serde_json::json!({"id":"evt_0","type":"session.tool.input.started","data":{"sessionID":session,
            "id":call,"name":"shell"}}),
        serde_json::json!({"id":"evt_1","type":"session.tool.called","data":{"sessionID":session,
            "id":call,"input":{"command":"sleep 60","background":true}}}),
        serde_json::json!({"id":"evt_2","type":"session.tool.success","data":{"sessionID":session,
            "id":call,"metadata":{"status":"running","shellID":"sh_1","truncated":false},
            "content":[{"type":"text","text":"Command moved to the background (shell ID: sh_1)"}]}}),
        serde_json::json!({"id":"evt_3","type":"shell.exited","data":{"id":"sh_1","exit":0,"status":"exited"}}),
    ];
    let mut decoder = Decoder::new(Some(session.to_owned()));
    let mut states = Vec::new();
    for frame in frames {
        for event in decoder.feed(&frame.to_string()).unwrap() {
            if let EngineEvent::Item {
                status,
                kind:
                    artisan_api::ItemKind::Tool {
                        tool: artisan_api::ToolKind::Shell { background, .. },
                        exit_code,
                        ..
                    },
                ..
            } = event
            {
                states.push((status, background, exit_code));
            }
        }
    }
    assert_eq!(
        states,
        [
            (Running, true, None),
            (Running, true, None),
            (Done, true, Some(0)),
        ],
    );
}
