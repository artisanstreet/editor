//! Transcript snapshots of the ACP decoder.
//!
//! Each transcript line is `{"dir": "in", "line": <engine message>}`,
//! `{"dir": "out", "line": <our request>}`, `{"dir": "answer", "key", "answer"}`
//! (the user answering) or `{"dir": "cancel"}` (a steer or cancel). Lines
//! with `"dir": "note"` say where the transcript came from.

use super::Profile;
use super::decode::{AcpDecoder, needs_sign_in};
use crate::EngineEvent;
use serde_json::Value;
use std::fmt::Write as _;

#[derive(Debug)]
#[expect(dead_code, reason = "read through the Debug snapshot")]
enum Step {
    Event(Box<EngineEvent>),
    /// What we wrote back to the engine.
    Reply(String),
    Response {
        method: String,
        result: Result<String, String>,
        sign_in: bool,
    },
}

fn replay(profile: Profile, case: &str) -> Vec<Step> {
    let engine = match profile {
        Profile::Grok => "grok",
        Profile::Cursor => "cursor",
    };
    let transcript = crate::transcript_tests::read(engine, case);
    let mut decoder = AcpDecoder::new(profile);
    let mut steps = Vec::new();
    // Older recordings identify the approval by the native Tool target.
    let mut approval_keys = std::collections::HashMap::new();
    for line in transcript.lines().filter(|line| !line.trim().is_empty()) {
        let entry: Value =
            serde_json::from_str(line).unwrap_or_else(|error| panic!("{case}: {error}: {line}"));
        let events = match entry["dir"].as_str() {
            Some("in") => decoder.feed(&entry["line"].to_string()).unwrap(),
            Some("out") => {
                decoder.sent(&entry["line"]);
                Vec::new()
            }
            Some("answer") => {
                let answer = serde_json::from_value(entry["answer"].clone()).expect("answer");
                let recorded = entry["key"].as_str().expect("key");
                let key = approval_keys
                    .get(recorded)
                    .map(String::as_str)
                    .unwrap_or(recorded);
                decoder.answer(key, &answer)
            }
            Some("cancel") => decoder.cancel_pending(),
            _ => Vec::new(),
        };
        for event in &events {
            if let EngineEvent::Approval {
                key,
                target: Some(target),
                status: artisan_api::Status::AwaitingUser,
                ..
            } = event
            {
                approval_keys.insert(target.clone(), key.clone());
            }
        }
        steps.extend(events.into_iter().map(|event| Step::Event(Box::new(event))));
        steps.extend(
            decoder
                .take_outbox()
                .into_iter()
                .map(|reply| Step::Reply(reply.to_string())),
        );
        steps.extend(decoder.take_responses().into_iter().map(|response| {
            Step::Response {
                sign_in: response
                    .result
                    .as_ref()
                    .is_err_and(|error| needs_sign_in(error)),
                method: response.method,
                result: response.result.map(|value| abbreviate(&value)),
            }
        }));
    }
    let cancelled = decoder.stop_reason.as_deref() != Some("end_turn");
    steps.extend(
        decoder
            .end_run(cancelled)
            .into_iter()
            .map(|event| Step::Event(Box::new(event))),
    );
    steps
}

/// Responses are snapshotted by their keys only; their content is the engine's.
fn abbreviate(value: &Value) -> String {
    match value.as_object() {
        Some(object) => object.keys().fold(String::new(), |mut keys, key| {
            let _ = write!(keys, "{}{key}", if keys.is_empty() { "" } else { "," });
            keys
        }),
        None => value.to_string(),
    }
}

#[test]
fn grok_signed_out() {
    insta::assert_debug_snapshot!(replay(Profile::Grok, "signed-out"));
}

#[test]
fn grok_plain_reply() {
    insta::assert_debug_snapshot!(replay(Profile::Grok, "plain-reply"));
}

#[test]
fn grok_tool_permission() {
    insta::assert_debug_snapshot!(replay(Profile::Grok, "tool-permission"));
}

#[test]
fn grok_question_and_steer() {
    insta::assert_debug_snapshot!(replay(Profile::Grok, "question-steer"));
}

#[test]
fn cursor_signed_out() {
    insta::assert_debug_snapshot!(replay(Profile::Cursor, "signed-out"));
}

#[test]
fn cursor_plain_reply() {
    insta::assert_debug_snapshot!(replay(Profile::Cursor, "plain-reply"));
}

#[test]
fn cursor_tool_permission() {
    insta::assert_debug_snapshot!(replay(Profile::Cursor, "tool-permission"));
}

#[test]
fn cursor_plan_and_question() {
    insta::assert_debug_snapshot!(replay(Profile::Cursor, "plan-question"));
}

/// Cursor runs its fast tier as the model's `-fast` id.
#[test]
fn cursor_fast_speed_runs_the_fast_model_id() {
    let mut config = artisan_api::EngineConfig::new(artisan_api::EngineId::Cursor);
    config.model = Some("composer-2.5".into());
    let model_arg = |config: &artisan_api::EngineConfig| {
        let args = Profile::Cursor.args(config);
        let at = args
            .iter()
            .position(|arg| arg == "--model")
            .expect("--model");
        args[at + 1].clone()
    };
    assert_eq!(model_arg(&config), "composer-2.5");
    config.speed = Some("fast".into());
    assert_eq!(model_arg(&config), "composer-2.5-fast");
}

/// Grok runs Auto in its own auto mode; Cursor has none and asks as it
/// always does. Full approves everything on both.
#[test]
fn permissions_become_launch_flags() {
    let args = |profile: Profile, engine, permissions| {
        let mut config = artisan_api::EngineConfig::new(engine);
        config.permissions = permissions;
        profile.args(&config)
    };
    let auto = args(
        Profile::Grok,
        artisan_api::EngineId::Grok,
        artisan_api::Permissions::Auto,
    );
    let mode = auto.iter().position(|arg| arg == "--permission-mode");
    assert_eq!(mode.map(|at| auto[at + 1].as_str()), Some("auto"));
    assert!(!auto.iter().any(|arg| arg == "--always-approve"));
    let full = args(
        Profile::Grok,
        artisan_api::EngineId::Grok,
        artisan_api::Permissions::Full,
    );
    assert!(full.iter().any(|arg| arg == "--always-approve"));
    assert!(!full.iter().any(|arg| arg == "--permission-mode"));

    let auto = args(
        Profile::Cursor,
        artisan_api::EngineId::Cursor,
        artisan_api::Permissions::Auto,
    );
    assert_eq!(auto, ["acp"]);
    let full = args(
        Profile::Cursor,
        artisan_api::EngineId::Cursor,
        artisan_api::Permissions::Full,
    );
    assert_eq!(full, ["--force", "acp"]);
}
