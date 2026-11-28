use super::decoder::{Answered, Decoder, Formed, Permitted};
use super::protocol::{Frame, Opened, Prompted};
use crate::EngineEvent;
use artisan_api::{Answer, Decision, Permissions, QuestionAnswer};
use serde_json::Value;
use std::collections::HashSet;
use std::time::{Duration, Instant};

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
    fn superseded(&self) -> bool {
        !self.open_prompts.is_empty()
    }

    fn track(&mut self, client: &Value, decoder: &mut Decoder, now: Instant) {
        match client["method"].as_str() {
            Some("session/prompt") => {
                decoder.prompted(false, now);
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
        && let Answered::Reply { result, events, .. } = decoder.answer(key, &answer, Instant::now())
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
        let now = Instant::now();
        if let Some(client) = value.get("client") {
            sent.track(client, &mut decoder, now);
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
            Frame::Update { update } => events.extend(decoder.update(*update, now)),
            Frame::Response {
                id,
                result: Ok(result),
            } if sent.prompts.contains(&Value::from(id)) => {
                sent.open_prompts.remove(&Value::from(id));
                match serde_json::from_value::<Prompted>(result) {
                    Ok(prompted) => events.extend(decoder.ended(&prompted, sent.superseded(), now)),
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
                match decoder.permission(id.clone(), *request, permissions, now) {
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
                match decoder.form(id.clone(), *request, now) {
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

struct Replayed {
    dir: tempfile::TempDir,
    running: tokio::task::JoinHandle<crate::Outcome>,
    received: tokio::sync::mpsc::Receiver<EngineEvent>,
}

impl Replayed {
    fn start(case: &str, model: &str, effort: &str, gates: &[&str]) -> (Self, Vec<std::fs::File>) {
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
        let held = gates
            .iter()
            .map(|name| {
                let file = std::fs::File::create(dir.path().join(format!("{name}.gate"))).unwrap();
                file.lock().unwrap();
                file
            })
            .collect();
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
        let (events, received) = tokio::sync::mpsc::channel(256);
        let running = tokio::spawn(super::AntigravityHarness.run(
            turn,
            RunIo {
                inbox,
                events,
                cancel: tokio_util::sync::CancellationToken::new(),
            },
        ));
        (
            Self {
                dir,
                running,
                received,
            },
            held,
        )
    }

    async fn until(
        &mut self,
        what: &str,
        seen: impl Fn(&EngineEvent) -> bool + Send,
    ) -> Vec<EngineEvent> {
        let received = &mut self.received;
        test_support::wait::within(what, test_support::wait::LIVENESS, async move {
            let mut events = Vec::new();
            while let Some(event) = received.recv().await {
                let found = seen(&event);
                events.push(event);
                if found {
                    return events;
                }
            }
            panic!("the run ended before {what}: {events:#?}");
        })
        .await
    }

    async fn finish(self) -> (crate::Outcome, Vec<EngineEvent>) {
        let Self {
            dir,
            running,
            mut received,
        } = self;
        let drain = tokio::spawn(async move {
            let mut events = Vec::new();
            while let Some(event) = received.recv().await {
                events.push(event);
            }
            events
        });
        let outcome = test_support::wait::within(
            "the replayed turn ends",
            test_support::wait::LIVENESS,
            running,
        )
        .await
        .unwrap();
        let events = drain.await.unwrap();
        let log = std::fs::read_to_string(dir.path().join("replay.log"))
            .unwrap_or_else(|error| format!("no replay log: {error}"));
        assert_eq!(
            outcome.status(),
            artisan_api::RunStatus::Done,
            "{:?}\n{log}",
            outcome.error()
        );
        (outcome, events)
    }
}

async fn replayed_turn(case: &str, model: &str, effort: &str) {
    let (replayed, _gates) = Replayed::start(case, model, effort, &[]);
    replayed.finish().await;
}

fn waiting_status(event: &EngineEvent) -> Option<&str> {
    if let EngineEvent::Live {
        target: crate::harness::LiveKind::Status,
        text,
        ..
    } = event
    {
        Some(text)
    } else {
        None
    }
}

#[tokio::test]
async fn a_command_left_running_holds_the_run_until_the_woken_reply_is_the_answer() {
    use artisan_api::{ItemKind, Phase, Status};
    let (mut replayed, gates) =
        Replayed::start("background-wake", "gemini-3.8-flash", "low", &["job"]);
    let mut events = replayed
        .until("Antigravity waits on its background command", |event| {
            waiting_status(event).is_some_and(|text| !text.is_empty())
        })
        .await;
    assert!(
        !replayed.running.is_finished(),
        "the run ended while the command still ran"
    );
    for gate in gates {
        gate.unlock().unwrap();
    }
    let (_outcome, rest) = replayed.finish().await;
    events.extend(rest);
    let mut items: Vec<(String, Status, ItemKind)> = Vec::new();
    for event in &events {
        if let EngineEvent::Item {
            key, status, kind, ..
        } = event
        {
            items.retain(|(seen, ..)| seen != key);
            items.push((key.clone(), *status, kind.clone()));
        }
    }
    let messages: Vec<(String, Phase)> = items
        .iter()
        .filter_map(|(_, _, kind)| {
            if let ItemKind::AssistantMessage { text, phase } = kind {
                Some((text.inline().unwrap_or_default().to_owned(), *phase))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        messages,
        vec![
            (
                "I have launched the command in the background and will report back once it finishes."
                    .to_owned(),
                Phase::Commentary
            ),
            (
                "The background command has finished with output: BGDONE".to_owned(),
                Phase::Final
            ),
        ]
    );
    let command = items
        .iter()
        .find(|(key, ..)| key == "call_10289241707886692278")
        .expect("the command is a row");
    assert_eq!(command.1, Status::Done);
    assert_eq!(
        events.iter().filter_map(waiting_status).next_back(),
        Some(""),
        "the waiting status is cleared once the command finishes"
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

fn change(update: Value) -> super::protocol::Change {
    serde_json::from_value(update).expect("invariant: the test writes a valid session update")
}

fn command(id: &str, status: &str, daemon: bool) -> super::protocol::Change {
    change(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": id,
        "status": status,
        "kind": "execute",
        "rawInput": { "CommandLine": "cargo test", "IsDaemon": daemon },
    }))
}

fn say(text: &str) -> super::protocol::Change {
    change(serde_json::json!({
        "sessionUpdate": "agent_message_chunk",
        "content": { "type": "text", "text": text },
    }))
}

fn step_ended() -> super::protocol::Change {
    change(serde_json::json!({ "sessionUpdate": "usage_update", "used": 1, "size": 2 }))
}

fn end_turn() -> Prompted {
    serde_json::from_value(serde_json::json!({ "stopReason": "end_turn" }))
        .expect("invariant: the test writes a valid prompt result")
}

fn schedule(id: &str, status: &str) -> super::protocol::Change {
    change(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": id,
        "status": status,
        "kind": "other",
        "title": "Run schedule",
        "rawInput": { "DurationSeconds": 600, "Prompt": "Check the deploy." },
    }))
}

fn read(id: &str, status: &str) -> super::protocol::Change {
    change(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": id,
        "status": status,
        "kind": "read",
        "rawInput": { "AbsolutePath": "/work/src/lib.rs" },
    }))
}

#[derive(Debug, PartialEq, Eq)]
enum Run {
    Live,
    Settled,
    Abandoned,
}

struct Turn {
    decoder: Decoder,
    start: Instant,
    now: Instant,
    probes: usize,
    events: Vec<EngineEvent>,
}

impl Turn {
    fn prompt() -> Self {
        let start = Instant::now();
        let mut decoder = Decoder::default();
        let events = decoder.prompted(false, start);
        Self {
            decoder,
            start,
            now: start,
            probes: 0,
            events,
        }
    }

    fn at(&mut self, secs: u64) -> Instant {
        let at = self.start + Duration::from_secs(secs);
        assert!(
            at >= self.now,
            "invariant: the scenario moves forward in time"
        );
        self.now = at;
        at
    }

    fn update(&mut self, secs: u64, change: super::protocol::Change) {
        assert_eq!(
            self.idle(secs, false),
            Run::Live,
            "the run ended before {secs}s"
        );
        let at = self.at(secs);
        self.events.extend(self.decoder.update(change, at));
    }

    fn end_turn(&mut self, secs: u64) {
        let at = self.at(secs);
        self.events
            .extend(self.decoder.ended(&end_turn(), false, at));
    }

    fn idle(&mut self, secs: u64, queued: bool) -> Run {
        let until = self.start + Duration::from_secs(secs);
        loop {
            if self.decoder.wake() == super::wake::Wake::Quiet {
                return Run::Settled;
            }
            match self.decoder.due() {
                Some((at, due)) if at <= until => {
                    self.now = at;
                    match due {
                        super::wake::Due::Probe => {
                            self.probes += 1;
                            self.events.extend(self.decoder.probed(queued, at));
                        }
                        super::wake::Due::Lapse => self.events.extend(self.decoder.lapse(at)),
                        super::wake::Due::Abandon => return Run::Abandoned,
                    }
                }
                Some(_) | None => {
                    self.now = until;
                    return Run::Live;
                }
            }
        }
    }

    fn messages(&mut self) -> Vec<(String, artisan_api::Phase)> {
        self.events.extend(self.decoder.finish());
        let mut latest: Vec<(String, String, artisan_api::Phase)> = Vec::new();
        for event in &self.events {
            if let EngineEvent::Item {
                key,
                kind: artisan_api::ItemKind::AssistantMessage { text, phase },
                ..
            } = event
            {
                latest.retain(|(seen, ..)| seen != key);
                latest.push((
                    key.clone(),
                    text.inline().unwrap_or_default().to_owned(),
                    *phase,
                ));
            }
        }
        latest
            .into_iter()
            .map(|(_, text, phase)| (text, phase))
            .collect()
    }

    fn waiting(&self) -> Option<&str> {
        self.events.iter().filter_map(waiting_status).next_back()
    }
}

fn backgrounded_command() -> Turn {
    let mut turn = Turn::prompt();
    turn.update(1, command("build", "in_progress", false));
    turn.update(11, say("Building in the background."));
    turn.update(11, step_ended());
    turn.end_turn(11);
    turn
}

#[test]
fn a_wake_agy_queued_while_the_last_reply_closed_is_not_cut_off() {
    let mut turn = backgrounded_command();
    turn.update(40, command("build", "completed", false));
    turn.update(45, say("Tests passed; waiting for clippy."));
    turn.update(45, step_ended());
    assert_eq!(
        turn.idle(50, true),
        Run::Live,
        "the queued wake was cut off"
    );
    turn.update(50, say("Clippy is clean too."));
    turn.update(51, step_ended());
    assert_eq!(turn.idle(60, false), Run::Settled);
    assert_eq!(turn.now, turn.start + Duration::from_secs(54));
    assert_eq!(
        turn.messages().last(),
        Some(&("Clippy is clean too.".to_owned(), artisan_api::Phase::Final))
    );
}

#[test]
fn a_closed_reply_with_nothing_queued_settles_after_the_short_quiet() {
    let mut turn = backgrounded_command();
    turn.update(40, command("build", "completed", false));
    turn.update(45, say("The build passed."));
    turn.update(45, step_ended());
    assert_eq!(turn.idle(60, false), Run::Settled);
    assert_eq!(turn.now, turn.start + Duration::from_secs(48));
    assert_eq!(turn.probes, 1);
}

#[test]
fn eighteen_silent_seconds_inside_an_open_woken_reply_do_not_end_the_turn() {
    let mut turn = backgrounded_command();
    turn.update(40, command("build", "completed", false));
    turn.update(42, say("Reading the failure."));
    turn.update(42, read("look", "in_progress"));
    turn.update(43, read("look", "completed"));
    assert_eq!(
        turn.idle(61, false),
        Run::Live,
        "the open reply was cut off"
    );
    turn.update(61, say("Fixed the failing assertion."));
    turn.update(61, step_ended());
    assert_eq!(turn.idle(70, false), Run::Settled);
    assert_eq!(
        turn.messages().last(),
        Some(&(
            "Fixed the failing assertion.".to_owned(),
            artisan_api::Phase::Final
        ))
    );
}

#[test]
fn an_open_reply_that_never_closes_gives_up_after_two_minutes_of_silence() {
    let mut turn = backgrounded_command();
    turn.update(40, command("build", "completed", false));
    turn.update(42, say("Reading the failure."));
    assert_eq!(turn.idle(161, false), Run::Live);
    assert_eq!(turn.idle(163, false), Run::Settled);
}

#[test]
fn a_daemon_left_running_does_not_hold_the_turn() {
    let mut turn = Turn::prompt();
    turn.update(1, command("serve", "in_progress", true));
    turn.update(2, say("The dev server is up."));
    turn.update(2, step_ended());
    turn.end_turn(2);
    assert_eq!(turn.idle(2, false), Run::Settled);
    assert_eq!(turn.waiting(), None);
}

#[test]
fn a_command_left_running_holds_the_turn_until_it_finishes() {
    let mut turn = backgrounded_command();
    assert_eq!(turn.idle(20 * 60, false), Run::Live);
    assert_eq!(
        turn.waiting(),
        Some("Antigravity is waiting on a background job")
    );
    turn.update(20 * 60, command("build", "completed", false));
    assert_eq!(turn.waiting(), Some(""));
}

#[test]
fn a_command_that_goes_silent_for_thirty_minutes_releases_the_turn() {
    let mut turn = backgrounded_command();
    turn.update(10 * 60, command("build", "in_progress", false));
    assert_eq!(turn.idle(40 * 60 - 1, false), Run::Live);
    assert_eq!(turn.idle(40 * 60, false), Run::Settled);
}

#[test]
fn a_pending_schedule_holds_the_turn_past_every_silence_bound() {
    let mut turn = Turn::prompt();
    turn.update(1, schedule("timer", "in_progress"));
    turn.update(2, say("I'll check back in ten minutes."));
    turn.update(2, step_ended());
    turn.end_turn(2);
    assert_eq!(turn.idle(2 * 60 * 60, false), Run::Live);
    assert_eq!(
        turn.waiting(),
        Some("Antigravity is waiting on a background job")
    );
    turn.update(2 * 60 * 60, schedule("timer", "completed"));
    turn.update(2 * 60 * 60 + 1, say("The deploy finished."));
    turn.update(2 * 60 * 60 + 1, step_ended());
    assert_eq!(turn.idle(2 * 60 * 60 + 10, false), Run::Settled);
}

#[test]
fn a_prompt_silent_for_fifteen_minutes_is_abandoned() {
    let mut turn = Turn::prompt();
    turn.update(5, say("Thinking it over."));
    assert_eq!(turn.idle(15 * 60 + 4, false), Run::Live);
    assert_eq!(turn.idle(15 * 60 + 5, false), Run::Abandoned);
}

#[test]
fn a_prompt_waiting_on_a_running_tool_is_never_abandoned() {
    let mut turn = Turn::prompt();
    turn.update(5, read("big", "in_progress"));
    assert_eq!(turn.idle(60 * 60, false), Run::Live);
}

struct Brain {
    dir: tempfile::TempDir,
}

const SESSION: &str = "9b843313-9bdf-4693-b379-66db2d082483";

impl Brain {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn generated(&self) -> std::path::PathBuf {
        self.dir
            .path()
            .join("antigravity-cli/brain")
            .join(SESSION)
            .join(".system_generated")
    }

    fn message(&self, id: &str, priority: &str) -> &Self {
        let inbox = self.generated().join("messages");
        std::fs::create_dir_all(inbox.join("undelivered")).unwrap();
        std::fs::write(
            inbox.join(format!("{id}.json")),
            serde_json::json!({
                "id": id,
                "recipient": SESSION,
                "sender": format!("{SESSION}/task-108"),
                "priority": priority,
                "timestamp": "2026-11-23T08:44:29.112931711Z",
                "content": "Task id finished with result:\nCommand exited with code 0.",
            })
            .to_string(),
        )
        .unwrap();
        self
    }

    fn read(&self, ids: &[&str]) -> &Self {
        let receipts: serde_json::Map<String, Value> = ids
            .iter()
            .map(|id| ((*id).to_owned(), Value::Bool(true)))
            .collect();
        std::fs::write(
            self.generated().join("messages/read.json"),
            Value::Object(receipts).to_string(),
        )
        .unwrap();
        self
    }

    fn transcript(&self, steps: &[(&str, &str)]) -> &Self {
        let logs = self.generated().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let mut lines = String::new();
        for (index, (kind, content)) in steps.iter().enumerate() {
            let step = serde_json::json!({
                "step_index": index.to_string(),
                "source": if *kind == "SYSTEM_MESSAGE" { "SYSTEM" } else { "MODEL" },
                "type": kind,
                "status": "DONE",
                "created_at": "2026-11-23T08:44:34Z",
                "content": content,
            });
            lines.push_str(&step.to_string());
            lines.push('\n');
        }
        std::fs::write(logs.join("transcript.jsonl"), lines).unwrap();
        self
    }

    async fn pending(&self) -> bool {
        super::brain::pending_wake(self.dir.path(), SESSION)
            .await
            .expect("the fixture is readable")
    }
}

const TASK_FINISHED: &str = "<SYSTEM_MESSAGE>\n[Message] timestamp=2026-11-23T08:44:29Z sender=s/task-108 priority=MESSAGE_PRIORITY_HIGH content=Task finished";
const RESTARTED: &str = "<SYSTEM_MESSAGE>\n[Message] timestamp=2026-11-23T08:52:55Z sender=system priority=MESSAGE_PRIORITY_LOW content=[Notice] All your subagents and background tasks have been stopped";

#[tokio::test]
async fn an_unread_task_message_is_a_queued_wake() {
    let brain = Brain::new();
    brain
        .message("a1b3a93f", "MESSAGE_PRIORITY_HIGH")
        .message("ca885867", "MESSAGE_PRIORITY_HIGH")
        .read(&["ca885867"]);
    assert!(brain.pending().await);
    brain.read(&["ca885867", "a1b3a93f"]);
    assert!(!brain.pending().await);
}

#[tokio::test]
async fn an_unread_low_priority_notice_does_not_wake_the_model() {
    let brain = Brain::new();
    brain.message("3e16a40d", "MESSAGE_PRIORITY_LOW");
    assert!(!brain.pending().await);
}

#[tokio::test]
async fn a_task_message_dequeued_without_a_reply_yet_is_a_queued_wake() {
    let brain = Brain::new();
    brain.transcript(&[
        (
            "PLANNER_RESPONSE",
            "Tests passed (12/12); waiting for clippy.",
        ),
        ("SYSTEM_MESSAGE", TASK_FINISHED),
    ]);
    assert!(brain.pending().await);
    brain.transcript(&[
        ("SYSTEM_MESSAGE", TASK_FINISHED),
        ("PLANNER_RESPONSE", "Clippy is clean."),
    ]);
    assert!(!brain.pending().await);
    brain.transcript(&[("PLANNER_RESPONSE", "Done."), ("SYSTEM_MESSAGE", RESTARTED)]);
    assert!(!brain.pending().await);
}

#[tokio::test]
async fn a_session_agy_never_recorded_has_nothing_queued() {
    assert!(!Brain::new().pending().await);
}

#[test]
fn a_compact_prompt_emits_running_compaction_and_settles_before_output() {
    let start = Instant::now();
    let mut decoder = Decoder::default();
    let started = decoder.prompted(true, start);
    assert!(matches!(
        started.as_slice(),
        [EngineEvent::Item {
            key,
            status: artisan_api::Status::Running,
            kind: artisan_api::ItemKind::Compaction { .. },
            ..
        }] if key == "compaction:1"
    ));
    let events = decoder.update(
        super::protocol::Change::AgentThoughtChunk {
            content: super::protocol::Block::Text {
                text: "Checking theme tokens".to_owned(),
            },
        },
        start + Duration::from_secs(15),
    );
    assert!(matches!(
        events.first(),
        Some(EngineEvent::Item {
            key,
            status: artisan_api::Status::Done,
            kind: artisan_api::ItemKind::Compaction { .. },
            ..
        }) if key == "compaction:1"
    ));
}
