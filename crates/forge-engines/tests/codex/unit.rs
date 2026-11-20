use super::decoder::Decoder;
use crate::{EngineEvent, Outcome};
use artisan_api::{Answer, Decision};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Debug)]
#[expect(dead_code, reason = "read through the Debug snapshot")]
enum Step {
    Event(Box<EngineEvent>),
    Reply(Value),
    Response {
        method: String,
        result: Result<(), String>,
    },
    TurnEnd(Outcome),
    Message(String),
    ProtocolError(String),
}

fn replay(case: &str) -> Vec<Step> {
    let transcript = crate::transcript_tests::read("codex", case);
    let mut decoder = Decoder::default();
    decoder.request("initialize", json!({}));
    decoder.take_outbox();
    let mut asked: HashMap<String, String> = HashMap::new();
    let mut steps = Vec::new();
    for line in transcript.lines().filter(|line| !line.trim().is_empty()) {
        let message: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        if let Some(answer) = message.get("answer") {
            let key = answer["key"].as_str().expect("answered key");
            let answer: Answer =
                serde_json::from_value(answer["answer"].clone()).expect("an Answer");
            let events = decoder.answer(key, &answer);
            steps.extend(events.into_iter().map(|event| Step::Event(Box::new(event))));
            steps.extend(decoder.take_outbox().into_iter().map(Step::Reply));
            steps.extend(decoder.take_messages().into_iter().map(Step::Message));
            continue;
        }
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
        match decoder.feed(line) {
            Ok(events) => {
                steps.extend(events.into_iter().map(|event| Step::Event(Box::new(event))));
            }
            Err(error) => steps.push(Step::ProtocolError(diagnostics::chain(&error))),
        }
        steps.extend(decoder.take_outbox().into_iter().map(Step::Reply));
        for response in decoder.take_responses() {
            steps.push(Step::Response {
                method: response.method,
                result: response.result.map(|_| ()).map_err(|error| {
                    error
                        .said()
                        .map_or_else(|| error.to_string(), str::to_owned)
                }),
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

#[test]
fn unreadable_frames_fail_the_run_only_when_the_turn_depends_on_them() {
    insta::assert_debug_snapshot!(replay("malformed"));
}

#[test]
fn malformed_items_become_errors_not_empty_cards() {
    insta::assert_debug_snapshot!(replay("malformed-items"));
}

#[test]
fn web_actions_show_their_query_or_address() {
    insta::assert_debug_snapshot!(replay("web-search-actions"));
}

#[test]
fn approvals_queue_on_the_item_they_gate() {
    insta::assert_debug_snapshot!(replay("approval-queue"));
}

#[test]
fn user_input_is_answered_by_choice_id() {
    insta::assert_debug_snapshot!(replay("user-input"));
}

#[test]
fn plan_updates_replace_the_plan_and_a_malformed_one_is_an_error() {
    insta::assert_debug_snapshot!(replay("plan"));
}

#[test]
fn async_questions_are_answered_as_a_message() {
    insta::assert_debug_snapshot!(replay("async-questions"));
}

fn turn_end(case: &str) -> Outcome {
    replay(case)
        .into_iter()
        .rev()
        .find_map(|step| match step {
            Step::TurnEnd(outcome) => Some(outcome),
            Step::Event(_)
            | Step::Reply(_)
            | Step::Response { .. }
            | Step::Message(_)
            | Step::ProtocolError(_) => None,
        })
        .expect("the turn ends")
}

#[test]
fn a_turn_failed_at_capacity_can_be_retried() {
    let outcome = turn_end("overloaded");
    assert_eq!(outcome.status(), artisan_api::RunStatus::Failed);
    assert_eq!(outcome.retry(), Some(crate::Retry::Overloaded));
}

#[test]
fn a_turn_failed_on_the_usage_limit_waits_for_the_reset() {
    let outcome = turn_end("usage-limit");
    assert_eq!(outcome.status(), artisan_api::RunStatus::Failed);
    assert_eq!(
        outcome.retry(),
        Some(crate::Retry::UsageLimit { resets_at: None })
    );
}

#[test]
fn a_refused_turn_is_not_retried() {
    assert_eq!(turn_end("notices").retry(), None);
}

#[test]
fn notices_and_a_refused_turn() {
    insta::assert_debug_snapshot!(replay("notices"));
}

#[test]
fn the_chosen_speed_tier_is_the_service_tier_codex_runs() {
    let mut config = artisan_api::EngineConfig::new(artisan_api::EngineId::Codex);
    config.model = Some("gpt-6.1-sol".into());
    config.speed = Some("priority".into());
    config
        .options
        .insert("context_window".to_owned(), json!(1_000_000));
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
        trace_dir: None,
    };
    let (_, params) = super::thread_params(&turn, super::ProjectGuard::Codex);
    assert_eq!(params["serviceTier"], "priority");
    assert_eq!(params["config"]["model_context_window"], 1_000_000);
    turn.session_id = Some("saved-session".into());
    let (method, resumed) = super::thread_params(&turn, super::ProjectGuard::Codex);
    assert_eq!(method, "thread/resume");
    assert_eq!(resumed["config"]["model_context_window"], 1_000_000);
    turn.config.options.remove("context_window");
    let (_, standard) = super::thread_params(&turn, super::ProjectGuard::Codex);
    assert!(standard["config"].get("model_context_window").is_none());
    let mut overrides = json!({});
    super::turn_overrides(&turn, super::ProjectGuard::Codex, &mut overrides);
    assert_eq!(overrides["serviceTierForTurn"], "priority");
    assert_eq!(overrides["model"], "gpt-6.1-sol");

    turn.config.speed = Some("standard".into());
    let (_, params) = super::thread_params(&turn, super::ProjectGuard::Codex);
    assert!(params.get("serviceTier").is_none(), "{params}");
    let mut overrides = json!({});
    super::turn_overrides(&turn, super::ProjectGuard::Codex, &mut overrides);
    assert_eq!(overrides["serviceTierForTurn"], "default");
}

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
        trace_dir: None,
    };
    let (_, params) = super::thread_params(&turn, super::ProjectGuard::Codex);
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

    let mut overrides = json!({});
    super::turn_overrides(&turn, super::ProjectGuard::Codex, &mut overrides);
    assert_eq!(
        (
            &overrides["sandboxPolicy"],
            &overrides["approvalPolicy"],
            &overrides["approvalsReviewer"]
        ),
        (
            &json!({ "type": "workspaceWrite" }),
            &json!("on-request"),
            &json!("auto_review")
        )
    );

    turn.config.permissions = artisan_api::Permissions::Full;
    let (_, params) = super::thread_params(&turn, super::ProjectGuard::Codex);
    assert_eq!(params["sandbox"], "danger-full-access");
    assert_eq!(params["approvalPolicy"], "never");
    assert!(params.get("approvalsReviewer").is_none(), "{params}");
    let mut overrides = json!({});
    super::turn_overrides(&turn, super::ProjectGuard::Codex, &mut overrides);
    assert_eq!(
        overrides["sandboxPolicy"],
        json!({ "type": "dangerFullAccess" })
    );
    assert_eq!(overrides["approvalPolicy"], "never");
}

#[test]
fn inside_forges_sandbox_auto_keeps_the_reviewer_but_not_codexs_own_sandbox() {
    let turn = crate::Turn {
        thread_id: artisan_api::ThreadId::new(),
        engine: artisan_api::EngineId::Codex,
        binary: "codex".into(),
        home: "/home".into(),
        cwd: "/work".into(),
        config: artisan_api::EngineConfig::new(artisan_api::EngineId::Codex),
        session_id: None,
        input: crate::UserInput::default(),
        launch: crate::Launch::default(),
        trace_dir: None,
    };
    let (_, params) = super::thread_params(&turn, super::ProjectGuard::Forge);
    assert_eq!(
        (
            &params["sandbox"],
            &params["approvalPolicy"],
            &params["approvalsReviewer"]
        ),
        (
            &json!("danger-full-access"),
            &json!("on-request"),
            &json!("auto_review")
        ),
        "Codex's bubblewrap can't start inside Landlock, so every command would fail"
    );
    let mut overrides = json!({});
    super::turn_overrides(&turn, super::ProjectGuard::Forge, &mut overrides);
    assert_eq!(
        (
            &overrides["sandboxPolicy"],
            &overrides["approvalPolicy"],
            &overrides["approvalsReviewer"]
        ),
        (
            &json!({ "type": "dangerFullAccess" }),
            &json!("on-request"),
            &json!("auto_review")
        ),
        "each turn keeps Codex's sandbox off while Forge's holds"
    );
}

#[test]
fn a_goal_is_shown_only_when_one_was_set() {
    let transcript = crate::transcript_tests::read("codex", "command");
    let mut decoder = Decoder::default();
    decoder.request("initialize", json!({}));
    let mut lines = transcript.lines();
    let start: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    decoder.request("thread/start", start["client"]["params"].clone());
    decoder.feed(lines.next().unwrap()).unwrap();
    decoder.take_outbox();
    let thread = "01a12bc9-9151-7921-a2d6-1e7b08366259";
    let mut goals = |frame: Value| -> Vec<String> {
        decoder
            .feed(&frame.to_string())
            .unwrap()
            .into_iter()
            .filter_map(|event| match event {
                EngineEvent::Item {
                    kind:
                        artisan_api::ItemKind::Notice {
                            kind: artisan_api::NoticeKind::Goal { state },
                            text,
                        },
                    ..
                } => {
                    let artisan_api::TextContent::Inline(text) = text else {
                        panic!("native goal fixture must be inline before persistence")
                    };
                    Some(format!("{state}: {text}"))
                }
                EngineEvent::Session { .. }
                | EngineEvent::SessionGone
                | EngineEvent::Model { .. }
                | EngineEvent::Title { .. }
                | EngineEvent::Item { .. }
                | EngineEvent::Image { .. }
                | EngineEvent::Status { .. }
                | EngineEvent::Live { .. }
                | EngineEvent::Usage(_)
                | EngineEvent::Commands(_)
                | EngineEvent::InputAccepted { .. } => None,
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

#[test]
fn a_turn_completed_that_is_not_terminal_fails_instead_of_finishing() {
    use serde_json::json;
    for status in ["inProgress", "paused"] {
        let mut decoder = Decoder::default();
        let request = decoder.request("thread/start", json!({}));
        decoder
            .feed(&json!({ "id": request, "result": { "thread": { "id": "root" }, "turn": { "id": "turn-1" } } }).to_string())
            .unwrap();
        let ended = decoder.feed(
            &json!({ "method": "turn/completed", "params": { "threadId": "root", "turn": { "id": "turn-1", "status": status } } })
                .to_string(),
        );
        assert!(
            matches!(
                ended,
                Err(super::decoder::DecodeError::TurnCompleted { .. })
            ),
            "{status}"
        );
        assert_eq!(decoder.take_turn_end(), None, "{status}");
    }
}

#[test]
fn reversed_child_arrival_keeps_explicit_spawn_ownership() {
    use serde_json::json;
    let mut decoder = Decoder::default();
    let request = decoder.request("thread/start", json!({}));
    decoder
        .feed(&json!({ "id": request, "result": { "thread": { "id": "root" } } }).to_string())
        .unwrap();
    for spawn in ["spawn-a", "spawn-b"] {
        decoder.feed(&json!({ "method": "item/started", "params": { "threadId": "root", "item": { "id": spawn, "type": "collabAgentToolCall", "tool": "spawnAgent", "status": "inProgress", "agentsStates": {}, "prompt": spawn, "receiverThreadIds": [] } } }).to_string()).unwrap();
    }
    decoder.feed(&json!({ "method": "thread/started", "params": { "thread": { "id": "child-b", "parentThreadId": "root" } } }).to_string()).unwrap();
    let before = decoder.feed(&json!({ "method": "item/completed", "params": { "threadId": "child-b", "item": { "id": "reply-b", "type": "agentMessage", "text": "B reply", "phase": "final_answer" } } }).to_string()).unwrap();
    assert!(
        before.is_empty(),
        "unbound child content waits for explicit relation"
    );
    let after = decoder.feed(&json!({ "method": "item/completed", "params": { "threadId": "root", "item": { "id": "spawn-b", "type": "collabAgentToolCall", "tool": "spawnAgent", "status": "inProgress", "agentsStates": {}, "receiverThreadIds": ["child-b"] } } }).to_string()).unwrap();
    assert!(after.iter().any(|event| matches!(event, EngineEvent::Item { key, parent: Some(parent), .. } if key == "reply-b" && parent == "spawn-b")));
    assert!(!after.iter().any(|event| matches!(event, EngineEvent::Item { key, parent: Some(parent), .. } if key == "reply-b" && parent == "spawn-a")));
}

#[test]
fn opaque_web_actions_do_not_invent_execution_errors_or_fail_the_turn() {
    let steps = replay("web-search-optional-metadata");
    assert!(!steps.iter().any(|step| matches!(step,
        Step::Event(event) if matches!(event.as_ref(), EngineEvent::Item {
            kind: artisan_api::ItemKind::Error { .. }, ..
        })
    )));
    assert!(steps.iter().any(|step| matches!(step,
        Step::Event(event) if matches!(event.as_ref(), EngineEvent::Item {
            status: artisan_api::Status::Done,
            kind: artisan_api::ItemKind::Tool {
                tool: artisan_api::ToolKind::WebSearch { query }, ..
            }, ..
        } if query == "example query")
    )));
    assert!(
        steps
            .iter()
            .any(|step| matches!(step, Step::TurnEnd(outcome) if outcome.status() == artisan_api::RunStatus::Done && outcome.error().is_none()))
    );
}

#[test]
fn waits_and_sleeps_become_rows() {
    use artisan_api::{AgentAction, ItemKind, ToolKind};
    use serde_json::json;
    let mut decoder = Decoder::default();
    let request = decoder.request("thread/start", json!({}));
    decoder
        .feed(&json!({ "id": request, "result": { "thread": { "id": "root" } } }).to_string())
        .unwrap();
    let wait = decoder.feed(&json!({ "method": "item/completed", "params": { "threadId": "root", "item": { "id": "wait-1", "type": "collabAgentToolCall", "tool": "wait", "status": "completed", "receiverThreadIds": [], "agentsStates": { "child-b": { "message": null }, "child-a": { "message": null } } } } }).to_string()).unwrap();
    assert!(wait.iter().any(|event| matches!(event,
        EngineEvent::Item { key, status: artisan_api::Status::Done, kind: ItemKind::Tool {
            tool: ToolKind::Agents { action: AgentAction::Wait, addresses, .. }, ..
        }, .. } if key == "wait-1" && addresses == &["child-a", "child-b"])));
    let sleep = decoder.feed(&json!({ "method": "item/completed", "params": { "threadId": "root", "item": { "id": "sleep-1", "type": "sleep", "durationMs": 30000 } } }).to_string()).unwrap();
    assert!(sleep.iter().any(|event| matches!(event,
        EngineEvent::Item { kind: ItemKind::Tool {
            tool: ToolKind::Other { name, .. }, title: Some(title), ..
        }, .. } if name == "sleep" && title == "30 s")));
}

#[test]
fn a_question_without_options_is_asked_in_own_words() {
    use artisan_api::ItemKind;
    use serde_json::json;
    let mut decoder = Decoder::default();
    let request = decoder.request("thread/start", json!({}));
    decoder
        .feed(&json!({ "id": request, "result": { "thread": { "id": "root" } } }).to_string())
        .unwrap();
    let events = decoder
        .feed(
            &json!({ "method": "item/completed", "params": { "threadId": "root", "item": {
        "id": "asks", "type": "agentMessage", "text": "Where is it?", "phase": "final_answer",
        "questions": [{ "title": "Where is the Block 17 brief?", "options": null }],
    } } })
            .to_string(),
        )
        .unwrap();
    assert!(!events.iter().any(|event| matches!(
        event,
        EngineEvent::Item {
            kind: ItemKind::Error { .. },
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(event,
        EngineEvent::Item { kind: ItemKind::Question { questions, .. }, .. }
            if questions.inline().is_some_and(|questions| questions.len() == 1
                && questions[0].options.inline().is_some_and(<[_]>::is_empty)))));
}

#[test]
fn a_reply_codex_drops_for_a_steer_ends_empty_when_the_next_starts() {
    use artisan_api::{ItemKind, Status};
    use serde_json::json;
    let mut decoder = Decoder::default();
    let request = decoder.request("thread/start", json!({}));
    decoder
        .feed(&json!({ "id": request, "result": { "thread": { "id": "root" } } }).to_string())
        .unwrap();
    let message = |method: &str, id: &str, text: &str| {
        json!({ "method": method, "params": { "threadId": "root", "item": {
            "id": id, "type": "agentMessage", "text": text, "phase": "final_answer",
        } } })
        .to_string()
    };
    decoder
        .feed(&message("item/started", "dropped", ""))
        .unwrap();
    decoder
        .feed(&json!({ "method": "item/agentMessage/delta", "params": {
            "threadId": "root", "turnId": "t", "itemId": "dropped", "delta": "Yes. **The result does not justify",
        } }).to_string())
        .unwrap();
    let restarted = decoder
        .feed(&message("item/started", "answer", ""))
        .unwrap();
    let ended: Vec<_> = restarted
        .iter()
        .filter_map(|event| match event {
            EngineEvent::Item {
                key,
                status,
                kind: ItemKind::AssistantMessage { text, .. },
                ..
            } => Some((key.as_str(), *status, text.inline().unwrap_or_default())),
            EngineEvent::Session { .. }
            | EngineEvent::SessionGone
            | EngineEvent::Model { .. }
            | EngineEvent::Title { .. }
            | EngineEvent::Item { .. }
            | EngineEvent::Image { .. }
            | EngineEvent::Status { .. }
            | EngineEvent::Live { .. }
            | EngineEvent::Usage(_)
            | EngineEvent::Commands(_)
            | EngineEvent::InputAccepted { .. } => None,
        })
        .collect();
    assert_eq!(
        ended,
        [
            ("dropped", Status::Cancelled, ""),
            ("answer", Status::Running, "")
        ]
    );
    let answered = decoder
        .feed(&message(
            "item/completed",
            "answer",
            "Yes. The delivered result does not justify it.",
        ))
        .unwrap();
    assert!(answered.iter().all(|event| !matches!(event,
        EngineEvent::Item { key, .. } | EngineEvent::Status { key, .. } if key == "dropped")));
    assert!(decoder.finish().iter().all(|event| !matches!(event,
        EngineEvent::Item { key, .. } | EngineEvent::Status { key, .. } if key == "dropped")));
}

#[tokio::test]
async fn generated_title_is_written_to_codex_without_resuming_or_prompting() {
    use crate::{Harness as _, Turn, UserInput};
    use artisan_api::{EngineConfig, EngineId, ThreadId};
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| dir.path().join(name).to_string_lossy().into_owned();
    let (spawns, input) = (path("spawns"), path("input"));
    let turn = Turn {
        thread_id: ThreadId::new(),
        engine: EngineId::Codex,
        binary: crate::fake_engine::binary(),
        home: dir.path().join("home"),
        cwd: dir.path().to_owned(),
        config: EngineConfig::new(EngineId::Codex),
        session_id: Some("working-session".to_owned()),
        input: UserInput::default(),
        launch: crate::fake_engine::launch(
            "codex-app-server",
            &[
                ("ARTISAN_TEST_SPAWNS", &spawns),
                ("ARTISAN_TEST_INPUT", &input),
            ],
        ),
        trace_dir: None,
    };
    super::CodexHarness
        .set_title(turn, "Fix sidebar titles".to_owned())
        .await
        .unwrap();
    let named: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("home/named")).unwrap())
            .unwrap();
    assert_eq!(
        named,
        json!({ "threadId": "working-session", "name": "Fix sidebar titles" })
    );
    let methods: Vec<String> = std::fs::read_to_string(&input)
        .unwrap()
        .lines()
        .filter_map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["method"]
                .as_str()
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(methods, ["initialize", "initialized", "thread/name/set"]);
}
