//! Transcript snapshots of the Codex decoder.
//!
//! Each transcript is a recorded `codex app-server` session (Codex 0.162.1):
//! server lines as Codex wrote them, and `{"client": …}` lines for what the
//! recorder sent. The replay plays the driver's part: it registers each
//! request so response ids line up, and answers approvals the way the
//! recording did. Consecutive live appends to one target are merged.

use super::decoder::Decoder;
use crate::{EngineEvent, LiveKind, Outcome};
use artisan_api::{Answer, Decision};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug)]
#[expect(dead_code, reason = "read through the Debug snapshot")]
enum Step {
    Event(Box<EngineEvent>),
    /// What the driver writes back to Codex.
    Reply(Value),
    /// A response to one of the driver's requests.
    Response {
        method: String,
        result: Result<(), String>,
    },
    /// The root thread's turn ended this way.
    TurnEnd(Outcome),
}

fn replay(case: &str) -> Vec<Step> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("transcripts/codex/{case}.jsonl"));
    let transcript = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    let mut decoder = Decoder::default();
    // The recorder's `initialize` (id 1) isn't in the transcript.
    decoder.request("initialize", json!({}));
    decoder.take_outbox();
    let mut asked: HashMap<String, String> = HashMap::new();
    let mut steps = Vec::new();
    for line in transcript.lines().filter(|line| !line.trim().is_empty()) {
        let message: Value =
            serde_json::from_str(line).unwrap_or_else(|error| panic!("{case}: {error}: {line}"));
        if let Some(client) = message.get("client") {
            match (client["method"].as_str(), &client["id"]) {
                (Some(method), id) if !id.is_null() => {
                    let assigned = decoder.request(method, client["params"].clone());
                    assert_eq!(Some(assigned), id.as_u64(), "{case}: request ids drifted");
                    decoder.take_outbox();
                }
                (None, id) => {
                    let key = asked
                        .get(&id.to_string())
                        .expect("answer to a recorded request");
                    let decision = match client["result"]["decision"].as_str() {
                        Some("accept") => Decision::AllowOnce,
                        Some("acceptForSession") => Decision::AllowAlways,
                        Some("decline") => Decision::DenyOnce,
                        _ => Decision::Cancelled,
                    };
                    let events = decoder.answer(key, &Answer::Approval { decision });
                    steps.extend(events.into_iter().map(|event| Step::Event(Box::new(event))));
                    steps.extend(decoder.take_outbox().into_iter().map(Step::Reply));
                }
                _ => {}
            }
            continue;
        }
        if message.get("method").is_some()
            && let Some(id) = message.get("id")
            && let Some(item) = message["params"]["itemId"].as_str()
        {
            asked.insert(id.to_string(), item.to_owned());
        }
        steps.extend(
            decoder
                .feed(line)
                .into_iter()
                .map(|event| Step::Event(Box::new(event))),
        );
        steps.extend(decoder.take_outbox().into_iter().map(Step::Reply));
        for response in decoder.take_responses() {
            steps.push(Step::Response {
                method: response.method,
                result: response.result.map(|_| ()),
            });
        }
        if let Some(outcome) = decoder.take_turn_end() {
            steps.push(Step::TurnEnd(outcome));
        }
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
            && let EngineEvent::Live {
                target,
                text,
                append: true,
            } = event.as_ref()
            && let Some(Step::Event(last)) = merged.last_mut()
            && let EngineEvent::Live {
                target: last_target,
                text: last_text,
                append: true,
            } = last.as_mut()
            && last_target == target
            && *target != LiveKind::Status
        {
            last_text.push_str(text);
            continue;
        }
        merged.push(step);
    }
    merged
}

#[test]
fn command_runs_and_reply_is_final() {
    insta::assert_debug_snapshot!(replay("command"));
}

#[test]
fn approval_lands_on_its_command_and_allow_always_skips_the_next_prompt() {
    insta::assert_debug_snapshot!(replay("approval"));
}

#[test]
fn declined_approval_declines_the_command() {
    insta::assert_debug_snapshot!(replay("decline"));
}

#[test]
fn steer_lands_in_the_running_turn_and_a_late_one_is_rejected() {
    insta::assert_debug_snapshot!(replay("steer"));
}

#[test]
fn subagent_becomes_an_agent_item_owning_its_child_items() {
    insta::assert_debug_snapshot!(replay("subagent"));
}

/// The tier chosen in the picker is what Codex is asked for, on the thread
/// and on each turn; the standard tier asks for nothing.
#[test]
fn the_chosen_speed_tier_is_the_service_tier_codex_runs() {
    let mut config = artisan_api::EngineConfig::new(artisan_api::EngineId::Codex);
    config.model = Some("gpt-6.1-sol".into());
    config.speed = Some("priority".into());
    let mut turn = crate::Turn {
        thread_id: artisan_api::ThreadId::new(),
        engine: artisan_api::EngineId::Codex,
        binary: "codex".into(),
        home: "/home".into(),
        cwd: "/work".into(),
        config,
        session_id: None,
        input: crate::UserInput::default(),
        launch: crate::Launch::default(),
    };
    let (_, params) = super::thread_params(&turn);
    assert_eq!(params["serviceTier"], "priority");
    let mut overrides = json!({});
    super::turn_overrides(&turn.config, &mut overrides);
    assert_eq!(overrides["serviceTier"], "priority");

    turn.config.speed = Some("standard".into());
    let (_, params) = super::thread_params(&turn);
    assert!(params.get("serviceTier").is_none(), "{params}");
}

/// Auto is Codex's sandbox plus its automatic reviewer; Full has neither.
#[test]
fn permissions_pick_the_reviewed_sandbox_or_full_access() {
    let mut turn = crate::Turn {
        thread_id: artisan_api::ThreadId::new(),
        engine: artisan_api::EngineId::Codex,
        binary: "codex".into(),
        home: "/home".into(),
        cwd: "/work".into(),
        config: artisan_api::EngineConfig::new(artisan_api::EngineId::Codex),
        session_id: None,
        input: crate::UserInput::default(),
        launch: crate::Launch::default(),
    };
    let (_, params) = super::thread_params(&turn);
    assert_eq!(
        (
            &params["sandbox"],
            &params["approvalPolicy"],
            &params["approvalsReviewer"]
        ),
        (
            &json!("workspace-write"),
            &json!("on-request"),
            &json!("auto_review")
        )
    );

    turn.config.permissions = artisan_api::Permissions::Full;
    let (_, params) = super::thread_params(&turn);
    assert_eq!(params["sandbox"], "danger-full-access");
    assert_eq!(params["approvalPolicy"], "never");
    assert!(params.get("approvalsReviewer").is_none(), "{params}");
}

/// Codex sends `thread/goal/cleared` at every turn, goal or not: a thread
/// without a goal shows no "Goal cleared:" line; one with a goal shows it
/// set, then cleared.
#[test]
fn a_goal_is_shown_only_when_one_was_set() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("transcripts/codex/command.jsonl");
    let transcript = std::fs::read_to_string(path).unwrap();
    let mut decoder = Decoder::default();
    decoder.request("initialize", json!({}));
    let mut lines = transcript.lines();
    let start: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    decoder.request("thread/start", start["client"]["params"].clone());
    decoder.feed(lines.next().unwrap());
    decoder.take_outbox();
    let thread = "01a12bc9-9151-7921-a2d6-1e7b08366259";
    let mut goals = |frame: Value| -> Vec<String> {
        decoder
            .feed(&frame.to_string())
            .into_iter()
            .filter_map(|event| match event {
                EngineEvent::Item {
                    kind:
                        artisan_api::ItemKind::Notice {
                            kind: artisan_api::NoticeKind::Goal { state },
                            text,
                        },
                    ..
                } => Some(format!("{state}: {text}")),
                _ => None,
            })
            .collect()
    };
    let cleared = json!({"method": "thread/goal/cleared", "params": {"threadId": thread}});
    assert_eq!(
        goals(cleared.clone()),
        Vec::<String>::new(),
        "no goal, no line"
    );
    let set = json!({"method": "thread/goal/updated", "params": {"threadId": thread, "goal": {"objective": "Ship the parser", "status": "active"}}});
    assert_eq!(goals(set), ["active: Ship the parser"]);
    assert_eq!(goals(cleared), ["cleared: Ship the parser"]);
}
