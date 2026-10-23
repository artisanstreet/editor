//! Whole turns against a scripted claude.ai: what the user sees and what
//! claude.ai is sent, from the first message to the answer.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use artisan_api::{
    Answer as UserAnswer, Decision, EngineConfig, EngineId, ItemId, ItemKind, Permissions,
    RunStatus, Status, StepStatus, ThreadId, ToolKind,
};
use futures::StreamExt as _;
use futures::future::BoxFuture;
use futures::stream;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{Backend, Events, Refusal, Request, drive};
use crate::{EngineEvent, Input, Launch, Outcome, RunIo, Turn, UserInput};

/// One completion claude.ai answers with.
enum Reply {
    /// These events, then the stream ends.
    Ends(Vec<Value>),
    /// These events, then the stream waits (pings never come in a test),
    /// as it does after a call to one of the caller's tools.
    Waits(Vec<Value>),
    Refuses(Refusal),
    /// The reply made from what the request sent, for a reply that names
    /// something only the run knows (a shell's id).
    From(Box<dyn FnOnce(&Request) -> Reply + Send>),
}

#[derive(Default)]
struct Script {
    replies: Mutex<Vec<Reply>>,
    sent: Mutex<Vec<Request>>,
    uploads: Mutex<Vec<PathBuf>>,
    /// What each upload held, read when it was uploaded.
    uploaded: Mutex<Vec<Vec<u8>>>,
    created: Mutex<usize>,
    /// The conversation each completion was asked in, in order.
    conversations: Mutex<Vec<String>>,
    active: Arc<Mutex<HashSet<String>>>,
    /// The conversations whose completions were stopped, each recorded once
    /// its stop finished.
    stopped: Mutex<Vec<String>>,
    stop_error: Mutex<Option<Refusal>>,
    /// How long claude.ai takes to confirm a stop.
    stop_delay: Mutex<Option<std::time::Duration>>,
    deleted: Mutex<Vec<String>>,
}

impl Backend for Script {
    fn create_conversation(&self) -> BoxFuture<'_, anyhow::Result<String>> {
        let mut created = self.created.lock().expect("lock");
        *created += 1;
        let id = format!("conv-{created}");
        drop(created);
        Box::pin(async move { Ok(id) })
    }

    fn complete<'a>(
        &'a self,
        conversation: &'a str,
        request: &'a Request,
    ) -> BoxFuture<'a, anyhow::Result<Events>> {
        assert!(
            !self.active.lock().expect("lock").contains(conversation),
            "another completion started before the server stopped its predecessor"
        );
        self.sent.lock().expect("lock").push(request.clone());
        self.conversations
            .lock()
            .expect("lock")
            .push(conversation.to_owned());
        let mut reply = {
            let mut replies = self.replies.lock().expect("lock");
            assert!(!replies.is_empty(), "claude.ai was asked once too often");
            replies.remove(0)
        };
        if let Reply::From(make) = reply {
            reply = make(request);
        }
        Box::pin(async move {
            let events: Events = match reply {
                Reply::Ends(events) => Box::pin(stream::iter(events.into_iter().map(Ok))),
                Reply::Waits(events) => Box::pin(futures::StreamExt::chain(
                    stream::iter(events.into_iter().map(Ok)),
                    stream::pending(),
                )),
                Reply::Refuses(refusal) => return Err(refusal.into()),
                Reply::From(_) => unreachable!("made above"),
            };
            let mut active_set = self.active.lock().expect("lock");
            active_set.insert(conversation.to_owned());
            drop(active_set);
            let active = Arc::clone(&self.active);
            let conversation = conversation.to_owned();
            Ok(events
                .inspect(move |event| {
                    if event.as_ref().is_ok_and(|event| {
                        matches!(event["type"].as_str(), Some("message_stop" | "error"))
                    }) {
                        active.lock().expect("lock").remove(&conversation);
                    }
                })
                .boxed())
        })
    }

    fn upload<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, anyhow::Result<String>> {
        self.uploads.lock().expect("lock").push(path.to_owned());
        self.uploaded
            .lock()
            .expect("lock")
            .push(std::fs::read(path).unwrap_or_default());
        Box::pin(async { Ok("file-1".to_owned()) })
    }

    fn stop<'a>(&'a self, conversation: &'a str) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let delay = *self.stop_delay.lock().expect("lock");
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            self.stopped
                .lock()
                .expect("lock")
                .push(conversation.to_owned());
            let error = self.stop_error.lock().expect("lock").take();
            if let Some(error) = error {
                return Err(error.into());
            }
            self.active.lock().expect("lock").remove(conversation);
            Ok(())
        })
    }

    fn delete_conversation<'a>(
        &'a self,
        conversation: &'a str,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        self.deleted
            .lock()
            .expect("lock")
            .push(conversation.to_owned());
        Box::pin(async { Ok(()) })
    }
}

fn script(replies: Vec<Reply>) -> Arc<Script> {
    Arc::new(Script {
        replies: Mutex::new(replies),
        ..Script::default()
    })
}

fn start(id: &str) -> Value {
    json!({ "type": "message_start", "message": { "id": id, "model": "claude-opus-5-5" } })
}

fn text(index: u64, said: &str) -> Vec<Value> {
    vec![
        json!({ "type": "content_block_start", "index": index, "content_block": { "type": "text" } }),
        json!({ "type": "content_block_delta", "index": index, "delta": { "type": "text_delta", "text": said } }),
        json!({ "type": "content_block_stop", "index": index }),
    ]
}

fn call(index: u64, id: &str, name: &str, input: &Value) -> Vec<Value> {
    vec![
        json!({ "type": "content_block_start", "index": index, "content_block": { "type": "tool_use", "id": id, "name": name, "input": {}, "tool_origin": null } }),
        json!({ "type": "content_block_delta", "index": index, "delta": { "type": "input_json_delta", "partial_json": input.to_string() } }),
        json!({ "type": "content_block_stop", "index": index }),
    ]
}

fn answer(id: &str, said: &str) -> Reply {
    let mut events = vec![start(id)];
    events.extend(text(0, said));
    events.push(json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" } }));
    events.push(json!({ "type": "message_stop" }));
    Reply::Ends(events)
}

fn calls(id: &str, made: &[(&str, &str, Value)]) -> Reply {
    let mut events = vec![start(id)];
    for (index, (call_id, name, input)) in made.iter().enumerate() {
        events.extend(call(index as u64, call_id, name, input));
    }
    Reply::Waits(events)
}

fn turn(cwd: &Path, session: Option<&str>, permissions: Permissions) -> Turn {
    let mut config = EngineConfig::new(EngineId::Claude);
    config.permissions = permissions;
    Turn {
        thread_id: ThreadId::new(),
        engine: EngineId::Claude,
        binary: PathBuf::new(),
        home: cwd.join("home"),
        cwd: cwd.to_owned(),
        config,
        session_id: session.map(str::to_owned),
        input: UserInput {
            id: ItemId::new(),
            text: "What is this project?".to_owned(),
            images: Vec::new(),
        },
        launch: Launch::default(),
        trace_dir: None,
    }
}

/// What the user does when the turn asks: answers each approval, and
/// sends inputs when an event appears.
type User = Box<dyn FnMut(&EngineEvent) -> Vec<Input> + Send>;

/// Runs the turn to its end, with `user` answering, and returns what Forge
/// saw.
async fn run(
    backend: Arc<Script>,
    turn: Turn,
    mut user: User,
) -> (Outcome, Vec<EngineEvent>, CancellationToken) {
    run_with(backend, turn, &mut user, CancellationToken::new()).await
}

async fn run_with(
    backend: Arc<Script>,
    turn: Turn,
    user: &mut User,
    cancel: CancellationToken,
) -> (Outcome, Vec<EngineEvent>, CancellationToken) {
    let (inputs, inbox) = mpsc::channel(16);
    let (events, mut seen) = mpsc::channel(256);
    let io = RunIo {
        inbox,
        events,
        cancel: cancel.clone(),
    };
    let running = tokio::spawn(async move { drive(backend.as_ref(), turn, io).await });
    let mut log = Vec::new();
    while let Some(event) = seen.recv().await {
        for input in user(&event) {
            inputs.send(input).await.ok();
        }
        log.push(event);
    }
    (running.await.expect("the turn"), log, cancel)
}

fn nobody() -> User {
    Box::new(|_| Vec::new())
}

#[tokio::test]
async fn two_threads_release_every_tool_completion_before_continuing() {
    let work = tempfile::tempdir().expect("a dir");
    std::fs::write(work.path().join("a.txt"), "alpha").expect("a file");
    let mut replies = Vec::new();
    for index in 0..8 {
        replies.push(calls(
            &format!("m{index}"),
            &[("read", "Read", json!({"file_path":"a.txt"}))],
        ));
    }
    replies.extend([answer("a1", "Done."), answer("a2", "Done.")]);
    let backend = script(replies);
    let (first, second) = tokio::join!(
        run(
            Arc::clone(&backend),
            turn(work.path(), Some("claude.ai:first"), Permissions::Full),
            nobody()
        ),
        run(
            Arc::clone(&backend),
            turn(work.path(), Some("claude.ai:second"), Permissions::Full),
            nobody()
        ),
    );
    assert_eq!(first.0.status, RunStatus::Done);
    assert_eq!(second.0.status, RunStatus::Done);
    assert_eq!(backend.sent.lock().expect("lock").len(), 10);
    assert_eq!(backend.stopped.lock().expect("lock").len(), 8);
    assert!(backend.active.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn a_failed_stop_prevents_tools_and_another_completion() {
    let work = tempfile::tempdir().expect("a dir");
    let marker = work.path().join("ran");
    let backend = script(vec![calls(
        "m1",
        &[(
            "write",
            "Write",
            json!({"file_path":"ran", "content":"ran"}),
        )],
    )]);
    *backend.stop_error.lock().expect("lock") = Some(Refusal::Overloaded("stop failed".to_owned()));
    let (outcome, _, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Full),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.retry, Some(crate::Retry::Overloaded));
    assert!(!marker.exists(), "the tool must wait for server cleanup");
    assert_eq!(backend.sent.lock().expect("lock").len(), 1);
}

#[tokio::test]
async fn cancellation_releases_the_server_completion() {
    let work = tempfile::tempdir().expect("a dir");
    let backend = script(vec![Reply::Waits(vec![start("m1")])]);
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    let mut user: User = Box::new(move |event| {
        if matches!(event, EngineEvent::Model { .. }) {
            stopper.cancel();
        }
        Vec::new()
    });
    let (outcome, _, _) = run_with(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Full),
        &mut user,
        cancel,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Cancelled);
    assert_eq!(backend.stopped.lock().expect("lock").len(), 1);
    assert!(backend.active.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn a_closed_connection_does_not_leave_the_server_completion_running() {
    let work = tempfile::tempdir().expect("a dir");
    let backend = script(vec![Reply::Ends(vec![start("m1")])]);
    let (outcome, _, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Full),
        nobody(),
    )
    .await;
    assert_eq!(outcome.retry, Some(crate::Retry::Overloaded));
    assert_eq!(backend.stopped.lock().expect("lock").len(), 1);
    assert!(backend.active.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn provider_limits_show_the_correct_error_for_http_and_both_stream_event_shapes() {
    let work = tempfile::tempdir().expect("a dir");
    let unknown = "Claude.ai rejected this request because of a limit, but did not identify which limit. Try again shortly.";
    for (limit, message, retry) in [
        (
            json!({"type":"exceeded_limit", "representativeClaim":"concurrents"}),
            "Claude.ai has too many active responses. Try again shortly.",
            crate::Retry::Overloaded,
        ),
        (
            json!({"type":"exceeded_limit", "resetsAt":100}),
            unknown,
            crate::Retry::Overloaded,
        ),
        (
            json!({"type":"exceeded_limit", "representativeClaim":"unknown", "resetsAt":100}),
            unknown,
            crate::Retry::Overloaded,
        ),
        (
            json!({"type":"exceeded_limit", "representativeClaim":"five_hour", "resetsAt":100}),
            "Claude.ai reported that your session usage limit was reached.",
            crate::Retry::UsageLimit {
                resets_at: Some(100_000),
            },
        ),
        (
            json!({"type":"exceeded_limit", "representativeClaim":"seven_day", "perModelLimit":true, "resetsAt":100}),
            "Claude.ai reported that your weekly usage limit for this model was reached.",
            crate::Retry::UsageLimit {
                resets_at: Some(100_000),
            },
        ),
        (
            json!({"type":"exceeded_limit", "windows":{"7d":{"status":"exceeded_limit"}}, "resetsAt":100}),
            "Claude.ai reported that your weekly usage limit was reached.",
            crate::Retry::UsageLimit {
                resets_at: Some(100_000),
            },
        ),
    ] {
        let body = json!({"error":{"type":"rate_limit_error", "message":limit.to_string()}});
        let http = super::transport::refusal(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            false,
            &body.to_string(),
        );
        for reply in [
            Reply::Refuses(http),
            Reply::Ends(vec![
                start("m1"),
                json!({"type":"message_limit", "message_limit":limit}),
                json!({"type":"message_stop"}),
            ]),
            Reply::Ends(vec![
                start("m1"),
                json!({"type":"error", "error":{"type":"rate_limit_error", "message":limit.to_string()}}),
            ]),
        ] {
            let (outcome, _, _) = run(
                script(vec![reply]),
                turn(work.path(), None, Permissions::Full),
                nobody(),
            )
            .await;
            assert_eq!(outcome.status, RunStatus::Failed);
            assert_eq!(
                outcome.error.as_deref(),
                Some(message),
                "provider limit: {limit}"
            );
            assert_eq!(outcome.retry, Some(retry), "provider limit: {limit}");
        }
    }
}

/// The key of an item that waits on the user.
fn awaiting(event: &EngineEvent) -> Option<&str> {
    if let EngineEvent::Item {
        key,
        status: Status::AwaitingUser,
        ..
    } = event
    {
        Some(key)
    } else {
        None
    }
}

fn decide(decision: Decision) -> User {
    Box::new(move |event| {
        awaiting(event)
            .map(|key| Input::Answer {
                key: key.to_owned(),
                answer: UserAnswer::Approval { decision },
            })
            .into_iter()
            .collect()
    })
}

fn accepted(log: &[EngineEvent]) -> Vec<ItemId> {
    log.iter()
        .filter_map(|event| {
            if let EngineEvent::InputAccepted { id } = event {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect()
}

#[tokio::test]
async fn the_message_is_delivered_once_so_forge_does_not_run_it_again() {
    let work = tempfile::tempdir().expect("a dir");
    let backend = script(vec![answer("m1", "A Rust workspace.")]);
    let turn = turn(work.path(), None, Permissions::Auto);
    let message = turn.input.id.clone();
    let (outcome, log, _) = run(Arc::clone(&backend), turn, nobody()).await;
    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(accepted(&log), vec![message]);
    let sent = backend.sent.lock().expect("lock").clone();
    assert!(
        sent[0].prompt.contains("Working directory:")
            && sent[0].prompt.ends_with("What is this project?"),
        "a new conversation learns where it works first: {}",
        sent[0].prompt
    );
    assert!(log.iter().any(|event| matches!(
        event,
        EngineEvent::Item {
            kind: ItemKind::AssistantMessage {
                phase: artisan_api::Phase::Final,
                ..
            },
            status: Status::Done,
            ..
        }
    )));
}

#[tokio::test]
async fn nothing_runs_before_the_user_allows_it_and_a_denied_call_never_runs() {
    let work = tempfile::tempdir().expect("a dir");
    let marker = work.path().join("ran");
    let command = format!("touch {}", marker.display());
    let backend = script(vec![
        calls("m1", &[("toolu_1", "Bash", json!({ "command": command }))]),
        answer("m2", "I won't."),
    ]);
    let marker_seen = marker.clone();
    let user: User = Box::new(move |event| {
        let Some(key) = awaiting(event) else {
            return Vec::new();
        };
        assert!(
            !marker_seen.exists(),
            "the command ran before the user answered"
        );
        vec![Input::Answer {
            key: key.to_owned(),
            answer: UserAnswer::Approval {
                decision: Decision::DenyOnce,
            },
        }]
    });
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Auto),
        user,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    assert!(!marker.exists(), "a declined command never runs");
    assert!(log.iter().any(|event| matches!(event, EngineEvent::Item { key, status: Status::Declined, .. } if key == "toolu_1")));
    let sent = backend.sent.lock().expect("lock").clone();
    assert!(sent[1].prompt.starts_with("<tool_results>"));
    assert!(sent[1].prompt.contains("status=\"error\""));
    assert!(sent[1].prompt.contains("declined"));
}

#[tokio::test]
async fn an_allowed_call_runs_exactly_once() {
    let work = tempfile::tempdir().expect("a dir");
    let log_file = work.path().join("log");
    let command = format!("echo once >> {}", log_file.display());
    let backend = script(vec![
        calls("m1", &[("toolu_1", "Bash", json!({ "command": command }))]),
        answer("m2", "Done."),
    ]);
    let (outcome, _, _) = run(
        backend,
        turn(work.path(), None, Permissions::Auto),
        decide(Decision::AllowOnce),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(
        std::fs::read_to_string(&log_file).expect("the log"),
        "once\n"
    );
}

#[tokio::test]
async fn parallel_calls_are_all_answered_and_marked_as_tool_output() {
    let work = tempfile::tempdir().expect("a dir");
    std::fs::write(work.path().join("a.txt"), "alpha\n").expect("a");
    std::fs::write(work.path().join("b.txt"), "beta\n").expect("b");
    let mut first = vec![start("m1")];
    first.extend(text(0, "Reading both."));
    first.extend(call(1, "toolu_a", "Read", &json!({ "file_path": "a.txt" })));
    first.extend(call(2, "toolu_b", "Read", &json!({ "file_path": "b.txt" })));
    let backend = script(vec![Reply::Waits(first), answer("m2", "alpha and beta")]);
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Auto),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    let sent = backend.sent.lock().expect("lock").clone();
    assert_eq!(sent.len(), 2, "both calls go back in one message");
    assert!(sent[1].prompt.contains("tool_use_id=\"toolu_a\"") && sent[1].prompt.contains("alpha"));
    assert!(sent[1].prompt.contains("tool_use_id=\"toolu_b\"") && sent[1].prompt.contains("beta"));
    assert!(sent[1].prompt.contains("not a message from the user"));
    assert_eq!(sent[1].tool_states.len(), 2);
    assert!(
        log.iter().any(|event| matches!(
            event,
            EngineEvent::Item { kind: ItemKind::AssistantMessage { text, .. }, status: Status::Done, .. } if text.inline() == Some("Reading both.")
        )),
        "the text before the calls is kept"
    );
}

/// A message that sits in claude.ai's conversation tree, answering `asks`.
fn start_in(id: &str, uuid: &str, asks: &str) -> Value {
    json!({ "type": "message_start", "message": { "id": id, "model": "claude-opus-5-5", "uuid": uuid, "parent_uuid": asks } })
}

#[tokio::test]
async fn tool_results_follow_the_users_message_so_the_model_keeps_the_question() {
    let work = tempfile::tempdir().expect("a dir");
    std::fs::write(work.path().join("a.txt"), "alpha\n").expect("a");
    let mut first = vec![start_in("m1", "a1", "h1")];
    first.extend(call(0, "toolu_a", "Read", &json!({ "file_path": "a.txt" })));
    let mut second = vec![start_in("m2", "a2", "h2")];
    second.extend(text(0, "It says alpha."));
    second.push(json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" } }));
    second.push(json!({ "type": "message_stop" }));
    let backend = script(vec![Reply::Waits(first), Reply::Ends(second)]);
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), Some("claude.ai:conv-1/a0"), Permissions::Auto),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    let sent = backend.sent.lock().expect("lock").clone();
    assert_eq!(
        sent[0].parent.as_deref(),
        Some("a0"),
        "the message follows the thread's last answer"
    );
    assert_eq!(
        sent[1].parent.as_deref(),
        Some("h1"),
        "claude.ai never saves the message with the calls, so the results follow the user's message"
    );
    assert!(
        sent[1]
            .prompt
            .contains(r#"<input>{"file_path":"a.txt"}</input>"#),
        "the results say what was called"
    );
    let session = log.iter().rev().find_map(|event| {
        if let EngineEvent::Session { id } = event {
            Some(id.as_str())
        } else {
            None
        }
    });
    assert_eq!(
        session,
        Some("claude.ai:conv-1/a2"),
        "the next turn follows the saved answer"
    );
}

#[tokio::test]
async fn claude_ai_s_own_tools_are_never_refused_in_the_user_s_name() {
    let work = tempfile::tempdir().expect("a dir");
    let events = vec![
        start("m1"),
        json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "toolu_s", "name": "bash_tool", "input": {}, "tool_origin": "first_party" } }),
        json!({ "type": "content_block_stop", "index": 0 }),
        json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_result", "tool_use_id": "toolu_s", "content": [{ "type": "text", "text": "/" }] } }),
        json!({ "type": "content_block_stop", "index": 1 }),
        json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "text" } }),
        json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "text_delta", "text": "Here it is." } }),
        json!({ "type": "content_block_stop", "index": 2 }),
        json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" } }),
        json!({ "type": "message_stop" }),
    ];
    let backend = script(vec![Reply::Ends(events)]);
    let (outcome, _, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Auto),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(
        backend.sent.lock().expect("lock").len(),
        1,
        "nothing is sent back for the provider's tool"
    );
}

#[tokio::test]
async fn a_connector_tool_stops_the_turn_unless_access_is_full() {
    let work = tempfile::tempdir().expect("a dir");
    let connector = || {
        Reply::Waits(vec![
            start("m1"),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "toolu_c", "name": "send_email", "input": {}, "tool_origin": "mcp", "mcp_server_url": "https://gmail.example" } }),
        ])
    };
    let backend = script(vec![connector()]);
    let (outcome, _, _) = run(
        backend,
        turn(work.path(), None, Permissions::Auto),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Failed);
    assert!(outcome.error.unwrap_or_default().contains("send_email"));
}

#[tokio::test]
async fn a_steer_during_the_tools_goes_with_their_results() {
    let work = tempfile::tempdir().expect("a dir");
    std::fs::write(work.path().join("a.txt"), "alpha\n").expect("a");
    let backend = script(vec![
        calls(
            "m1",
            &[("toolu_a", "Read", json!({ "file_path": "a.txt" }))],
        ),
        answer("m2", "Noted."),
    ]);
    let steer = ItemId::new();
    let steer_id = steer.clone();
    let mut sent_steer = false;
    let user: User = Box::new(move |event| {
        if !sent_steer && matches!(event, EngineEvent::Item { key, .. } if key == "toolu_a") {
            sent_steer = true;
            return vec![Input::Steer(UserInput {
                id: steer_id.clone(),
                text: "Also check b.txt".to_owned(),
                images: Vec::new(),
            })];
        }
        Vec::new()
    });
    let turn = turn(work.path(), None, Permissions::Auto);
    let message = turn.input.id.clone();
    let (outcome, log, _) = run(Arc::clone(&backend), turn, user).await;
    assert_eq!(outcome.status, RunStatus::Done);
    let sent = backend.sent.lock().expect("lock").clone();
    assert!(
        sent[1].prompt.contains("alpha"),
        "the results are not dropped for the steer"
    );
    assert!(sent[1].prompt.contains("Also check b.txt"));
    assert_eq!(accepted(&log), vec![message, steer]);
}

#[tokio::test]
async fn a_gone_conversation_hands_the_message_back_for_a_fresh_session() {
    let work = tempfile::tempdir().expect("a dir");
    let backend = script(vec![Reply::Refuses(Refusal::ConversationGone)]);
    let (outcome, log, _) = run(
        backend,
        turn(work.path(), Some("claude.ai:conv-old"), Permissions::Auto),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    assert!(
        accepted(&log).is_empty(),
        "the message waits for the fresh session"
    );
    assert!(
        log.iter()
            .any(|event| matches!(event, EngineEvent::Session { id } if id.is_empty()))
    );
}

#[tokio::test]
async fn a_browser_check_is_a_short_retryable_error() {
    let work = tempfile::tempdir().expect("a dir");
    let backend = script(vec![Reply::Refuses(Refusal::Challenge)]);
    let (outcome, _, _) = run(
        backend,
        turn(work.path(), None, Permissions::Auto),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Failed);
    assert!(outcome.retry.is_some());
    let error = outcome.error.unwrap_or_default();
    assert!(error.contains("browser check") && !error.contains('<'));
}

#[cfg(unix)]
#[tokio::test]
async fn stopping_the_turn_stops_a_running_command() {
    let work = tempfile::tempdir().expect("a dir");
    let backend = script(vec![calls(
        "m1",
        &[("toolu_1", "Bash", json!({ "command": "sleep 30" }))],
    )]);
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    let mut user: User = Box::new(move |event| {
        if matches!(event, EngineEvent::Item { key, status: Status::Running, kind: ItemKind::Tool { .. }, .. } if key == "toolu_1")
        {
            let stopper = stopper.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                stopper.cancel();
            });
        }
        Vec::new()
    });
    let started = std::time::Instant::now();
    let (outcome, _, _) = run_with(
        backend,
        turn(work.path(), None, Permissions::Full),
        &mut user,
        cancel,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Cancelled);
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[tokio::test]
async fn images_are_uploaded_and_go_with_the_message() {
    let work = tempfile::tempdir().expect("a dir");
    let image = work.path().join("shot.webp");
    std::fs::write(&image, b"RIFF").expect("an image");
    let backend = script(vec![answer("m1", "A red square.")]);
    let mut turn = turn(work.path(), None, Permissions::Auto);
    turn.input.images = vec![image.clone()];
    let (outcome, _, _) = run(Arc::clone(&backend), turn, nobody()).await;
    assert_eq!(outcome.status, RunStatus::Done);
    assert_eq!(*backend.uploads.lock().expect("lock"), vec![image]);
    assert_eq!(
        backend.sent.lock().expect("lock")[0].files,
        vec!["file-1".to_owned()]
    );
}

/// A whole turn against the real claude.ai, as the session in
/// `CLAUDE_AI_HOME` (an engine home holding `.claude-ai/session.json`).
/// The model reads a file with the user's-machine tool and must report
/// what it holds: the result reached it, framed as tool output.
#[tokio::test]
#[ignore = "needs a signed-in claude.ai session"]
async fn a_live_turn_reads_a_file_and_reports_it() {
    let home = PathBuf::from(std::env::var("CLAUDE_AI_HOME").expect("CLAUDE_AI_HOME"));
    let work = tempfile::tempdir().expect("a dir");
    std::fs::write(work.path().join("hostname.txt"), "artisan-live-4417\n").expect("a file");
    let transport = super::Transport::open(&home).expect("a session");
    let mut turn = turn(work.path(), None, Permissions::Full);
    turn.home = home.clone();
    // The code word is only in the question: the answer has it only when
    // the model still sees the question after the tool's result.
    turn.input.text = "My code word is HERON. Read hostname.txt with the Read tool, then tell me exactly what it says, followed by my code word.".to_owned();
    let (events, mut seen) = mpsc::channel(256);
    let (_inputs, inbox) = mpsc::channel(4);
    let io = RunIo {
        inbox,
        events,
        cancel: CancellationToken::new(),
    };
    let running = tokio::spawn(async move { drive(&transport, turn, io).await });
    let mut said = String::new();
    let mut accepted = 0;
    while let Some(event) = seen.recv().await {
        if matches!(event, EngineEvent::InputAccepted { .. }) {
            accepted += 1;
        } else if let EngineEvent::Item {
            kind: ItemKind::AssistantMessage { text, .. },
            status: Status::Done,
            ..
        } = &event
        {
            said.push_str(text.inline().unwrap_or_default());
        } else if !matches!(
            event,
            EngineEvent::Live { .. } | EngineEvent::Session { .. } | EngineEvent::Model { .. }
        ) {
            eprintln!("{event:?}");
        }
    }
    let outcome = running.await.expect("the turn");
    eprintln!("outcome: {outcome:?}\nsaid: {said}");
    assert_eq!(outcome.status, RunStatus::Done, "{outcome:?}");
    assert_eq!(accepted, 1, "the message is delivered once");
    assert!(
        said.contains("artisan-live-4417"),
        "the model saw the tool's result: {said}"
    );
    assert!(
        said.contains("HERON"),
        "the model kept the question: {said}"
    );
}

#[tokio::test]
async fn a_skill_is_listed_and_loaded_only_when_called() {
    let work = tempfile::tempdir().expect("a dir");
    let skill = work.path().join("home/.claude/skills/review");
    std::fs::create_dir_all(&skill).expect("the skill folder");
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: review\ndescription: Review code carefully\n---\nAlways check the tests first.\n",
    )
    .expect("the skill");
    let backend = script(vec![
        calls("m1", &[("toolu_s", "Skill", json!({ "skill": "review" }))]),
        answer("m2", "Loaded."),
    ]);
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Auto),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    let sent = backend.sent.lock().expect("lock").clone();
    assert!(
        sent[0].tools.iter().any(|tool| tool["name"] == "Skill"),
        "the Skill tool is offered"
    );
    assert!(
        sent[0].prompt.contains("<available_skills>")
            && sent[0].prompt.contains("review: Review code carefully"),
        "the skill is listed in the preamble: {}",
        sent[0].prompt
    );
    assert!(
        sent[1].prompt.contains("Always check the tests first."),
        "the skill's body is the tool's result: {}",
        sent[1].prompt
    );
    assert!(log.iter().any(|event| matches!(
        event,
        EngineEvent::Item {
            key,
            status: Status::Done,
            kind: ItemKind::Tool { tool: ToolKind::Other { name, .. }, .. },
            ..
        } if key == "toolu_s" && name == "Skill"
    )));
}

#[tokio::test]
async fn todos_become_one_plan_item_replaced_in_place() {
    let work = tempfile::tempdir().expect("a dir");
    let backend = script(vec![
        calls(
            "m1",
            &[(
                "toolu_t1",
                "TodoWrite",
                json!({ "todos": [
                    { "content": "Read the code", "status": "in_progress" },
                    { "content": "Write the fix", "status": "pending" }
                ] }),
            )],
        ),
        calls(
            "m2",
            &[(
                "toolu_t2",
                "TodoWrite",
                json!({ "todos": [
                    { "content": "Read the code", "status": "completed" },
                    { "content": "Write the fix", "status": "in_progress" }
                ] }),
            )],
        ),
        answer("m3", "Working."),
    ]);
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Auto),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    let plans: Vec<(String, Vec<artisan_api::PlanStep>)> = log
        .iter()
        .filter_map(|event| {
            if let EngineEvent::Item {
                key,
                status: Status::Done,
                kind: ItemKind::Plan { steps, .. },
                ..
            } = event
            {
                Some((key.clone(), steps.inline().expect("inline steps").to_vec()))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(plans.len(), 2);
    assert!(
        plans.iter().all(|(key, _)| key == "plan"),
        "the second list replaces the first: {plans:?}"
    );
    let plan = &plans[1].1;
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].status, StepStatus::Done);
    assert_eq!(plan[1].status, StepStatus::InProgress);
    assert!(
        !log.iter().any(|event| matches!(
            event,
            EngineEvent::Item { key, .. } if key.starts_with("toolu_t")
        )),
        "no item is left running under a call's id"
    );
}

#[tokio::test]
async fn a_task_runs_its_own_conversation_and_reports_back() {
    let work = tempfile::tempdir().expect("a dir");
    let backend = script(vec![
        calls(
            "m1",
            &[(
                "toolu_t",
                "Task",
                json!({
                    "description": "Find alpha",
                    "prompt": "Read a.txt and report its contents."
                }),
            )],
        ),
        Reply::Ends({
            let mut events = vec![start("c1")];
            events.extend(text(0, "a.txt says alpha."));
            events.extend(text(1, "It has one line."));
            events.push(json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" } }));
            events.push(json!({ "type": "message_stop" }));
            events
        }),
        answer("m2", "The agent says alpha."),
    ]);
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Full),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    let sent = backend.sent.lock().expect("lock").clone();
    assert_eq!(
        sent.len(),
        3,
        "the parent, the agent, then the parent again"
    );
    assert_eq!(
        *backend.conversations.lock().expect("lock"),
        ["conv-1", "conv-2", "conv-1"],
        "the agent works in a conversation of its own"
    );
    assert_eq!(
        *backend.deleted.lock().expect("lock"),
        ["conv-2"],
        "the agent's conversation is deleted once it reported, the thread's is kept"
    );
    assert!(
        sent[1]
            .prompt
            .contains("Read a.txt and report its contents.")
            && sent[1].prompt.contains("You are a subagent"),
        "the agent gets its own task: {}",
        sent[1].prompt
    );
    assert!(
        sent[2]
            .prompt
            .contains("a.txt says alpha.\n\nIt has one line."),
        "the agent's whole reply is the tool's result: {}",
        sent[2].prompt
    );
    let (description, result) = log
        .iter()
        .find_map(|event| {
            if let EngineEvent::Item {
                key,
                status: Status::Done,
                kind:
                    ItemKind::Agent {
                        description,
                        result: Some(result),
                        ..
                    },
                ..
            } = event
                && key == "toolu_t"
            {
                Some((
                    description.inline().map(str::to_owned),
                    result.inline().map(str::to_owned),
                ))
            } else {
                None
            }
        })
        .expect("the agent item settles");
    assert_eq!(description.as_deref(), Some("Find alpha"));
    assert_eq!(
        result.as_deref(),
        Some("a.txt says alpha.\n\nIt has one line.")
    );
    assert!(
        log.iter().any(|event| matches!(
            event,
            EngineEvent::Item {
                parent: Some(parent),
                kind: ItemKind::AssistantMessage { .. },
                ..
            } if parent == "toolu_t"
        )),
        "the agent's items hang under its Agent item"
    );
}

#[tokio::test]
async fn a_subagent_cannot_start_subagents_of_its_own() {
    let work = tempfile::tempdir().expect("a dir");
    let backend = script(vec![
        calls(
            "m1",
            &[(
                "toolu_outer",
                "Task",
                json!({ "description": "Outer task", "prompt": "Start the inner task." }),
            )],
        ),
        calls(
            "m2",
            &[(
                "toolu_inner",
                "Task",
                json!({ "description": "Inner task", "prompt": "Find the deep answer." }),
            )],
        ),
        answer("m3", "outer answer"),
        answer("m4", "Done."),
    ]);
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Full),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    let sent = backend.sent.lock().expect("lock").clone();
    assert_eq!(sent.len(), 4, "no third conversation is started");
    assert!(
        sent[0].tools.iter().any(|tool| tool["name"] == "Task")
            && sent[0].prompt.contains("Task, "),
        "the thread's model may start agents"
    );
    assert!(
        !sent[1].tools.iter().any(|tool| tool["name"] == "Task")
            && !sent[1].prompt.contains("Task, "),
        "the agent is neither offered nor told of Task: {}",
        sent[1].prompt
    );
    assert!(
        sent[2].prompt.contains("cannot start subagents"),
        "a Task the agent calls anyway is refused: {}",
        sent[2].prompt
    );
    assert!(log.iter().any(|event| matches!(
        event,
        EngineEvent::Item { key, status: Status::Declined, .. } if key == "toolu_outer:toolu_inner"
    )));
    assert_eq!(*backend.created.lock().expect("lock"), 2);
}

#[tokio::test]
async fn a_subagent_s_approval_reaches_the_user_and_its_tool_runs() {
    let work = tempfile::tempdir().expect("a dir");
    let marker = work.path().join("ran");
    let backend = script(vec![
        calls(
            "m1",
            &[(
                "toolu_t",
                "Task",
                json!({ "description": "Run it", "prompt": "Run the command." }),
            )],
        ),
        calls(
            "m2",
            &[(
                "toolu_b",
                "Bash",
                json!({ "command": format!("touch {}", marker.display()) }),
            )],
        ),
        answer("m3", "It ran."),
        answer("m4", "Done."),
    ]);
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Auto),
        decide(Decision::AllowOnce),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    assert!(marker.exists(), "the approved command ran");
    assert!(
        log.iter().any(|event| matches!(
            event,
            EngineEvent::Item {
                key,
                status: Status::AwaitingUser,
                ..
            } if key == "toolu_t:toolu_b"
        )),
        "the subagent's approval is keyed under its agent"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_background_shell_is_read_and_stopped() {
    let work = tempfile::tempdir().expect("a dir");
    let backend = script(vec![
        calls(
            "m1",
            &[(
                "b1",
                "Bash",
                json!({ "command": "echo first; sleep 30", "run_in_background": true }),
            )],
        ),
        Reply::From(Box::new(|request: &Request| {
            let id = shell_id(&request.prompt);
            calls("m2", &[("o1", "BashOutput", json!({ "bash_id": id }))])
        })),
        Reply::From(Box::new(|request: &Request| {
            let id = shell_id(&request.prompt);
            calls("m3", &[("k1", "KillShell", json!({ "shell_id": id }))])
        })),
        answer("m4", "Done."),
    ]);
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Full),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    let sent = backend.sent.lock().expect("lock").clone();
    assert_eq!(sent.len(), 4);
    assert!(
        sent[1].prompt.contains("with ID bash-"),
        "the model is told the shell's id: {}",
        sent[1].prompt
    );
    // Each round waits out the call grace, so the echo ran long before.
    assert!(
        sent[2].prompt.contains("first\n[still running]"),
        "the shell's output is read while it runs: {}",
        sent[2].prompt
    );
    assert!(
        sent[3].prompt.contains("stopped"),
        "the shell is stopped: {}",
        sent[3].prompt
    );
    assert!(log.iter().any(|event| matches!(
        event,
        EngineEvent::Item {
            key,
            status: Status::Done,
            kind: ItemKind::Tool {
                tool: ToolKind::Shell { background: true, .. },
                ..
            },
            ..
        } if key == "b1"
    )));
}

/// The id a background shell got, from the results that announced it
/// (or, in the next round, that named it).
fn shell_id(prompt: &str) -> String {
    let at = prompt.find("bash-").expect("a shell id");
    prompt[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

#[tokio::test]
async fn an_allow_rule_runs_a_command_without_asking() {
    let work = tempfile::tempdir().expect("a dir");
    let settings = work.path().join("home/.claude");
    std::fs::create_dir_all(&settings).expect("the settings folder");
    std::fs::write(
        settings.join("settings.json"),
        r#"{"permissions": {"allow": ["Bash(echo allowed:*)"]}}"#,
    )
    .expect("the settings");
    let backend = script(vec![
        calls(
            "m1",
            &[("b1", "Bash", json!({ "command": "echo allowed yes" }))],
        ),
        answer("m2", "Done."),
    ]);
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Auto),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    assert!(
        !log.iter().any(|event| matches!(
            event,
            EngineEvent::Item {
                status: Status::AwaitingUser,
                ..
            }
        )),
        "the rule meant no approval was asked"
    );
    let sent = backend.sent.lock().expect("lock").clone();
    assert!(sent[1].prompt.contains("allowed yes"));
}

#[tokio::test]
async fn a_deny_rule_refuses_even_under_full_access() {
    let work = tempfile::tempdir().expect("a dir");
    let settings = work.path().join("home/.claude");
    std::fs::create_dir_all(&settings).expect("the settings folder");
    std::fs::write(
        settings.join("settings.json"),
        r#"{"permissions": {"deny": ["Bash(touch:*)"]}}"#,
    )
    .expect("the settings");
    let marker = work.path().join("ran");
    let backend = script(vec![
        calls(
            "m1",
            &[(
                "b1",
                "Bash",
                json!({ "command": format!("touch {}", marker.display()) }),
            )],
        ),
        answer("m2", "Understood."),
    ]);
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Full),
        nobody(),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    assert!(!marker.exists(), "a denied command never runs");
    assert!(log.iter().any(|event| matches!(
        event,
        EngineEvent::Item {
            key,
            status: Status::Declined,
            ..
        } if key == "b1"
    )));
}

#[tokio::test]
async fn a_launch_mcp_server_is_offered_and_its_tool_runs() {
    let work = tempfile::tempdir().expect("a dir");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a listener");
    let addr = listener.local_addr().expect("an address");
    let server = tokio::spawn(super::mcp::fake_server(listener));
    let backend = script(vec![
        calls(
            "m1",
            &[("x1", "mcp__docs__search", json!({ "query": "rust" }))],
        ),
        answer("m2", "Found it."),
    ]);
    let mut turn = turn(work.path(), None, Permissions::Full);
    turn.launch.mcp_servers = vec![artisan_api::McpServer {
        name: "docs".to_owned(),
        url: Some(format!("http://{addr}/mcp")),
        ..artisan_api::McpServer::default()
    }];
    let (outcome, log, _) = run(Arc::clone(&backend), turn, nobody()).await;
    assert_eq!(outcome.status, RunStatus::Done);
    let sent = backend.sent.lock().expect("lock").clone();
    assert!(
        sent[0]
            .tools
            .iter()
            .any(|tool| tool["name"] == "mcp__docs__search"),
        "the server's tool is offered"
    );
    assert!(
        sent[0].prompt.contains("<mcp_servers>") && sent[0].prompt.contains("docs"),
        "the server is named in the preamble: {}",
        sent[0].prompt
    );
    assert!(
        sent[1].prompt.contains("found it"),
        "the server's answer is the tool's result: {}",
        sent[1].prompt
    );
    assert!(log.iter().any(|event| matches!(
        event,
        EngineEvent::Item {
            key,
            status: Status::Done,
            kind: ItemKind::Tool {
                tool: ToolKind::Mcp { server, tool, .. },
                ..
            },
            ..
        } if key == "x1" && server == "docs" && tool == "search"
    )));
    server.abort();
}

#[tokio::test]
async fn compaction_uses_a_temporary_conversation_and_never_runs_tools() {
    let dir = tempfile::tempdir().unwrap();
    let mut turn = turn(dir.path(), None, Permissions::Full);
    turn.input.text = "Summarize this historical conversation".into();
    turn.config.model = Some("claude-haiku-5-5".into());
    let script = Script::default();
    script
        .replies
        .lock()
        .unwrap()
        .push(answer("summary", "## Objective\n- Build the parser"));
    let reply = super::side::compact_with(&script, &turn, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(reply.text, "## Objective\n- Build the parser");
    assert_eq!(*script.created.lock().unwrap(), 1);
    {
        let sent = script.sent.lock().unwrap();
        assert_eq!(sent[0].model, "claude-haiku-5-5");
        assert!(sent[0].tools.is_empty());
        assert!(sent[0].parent.is_none());
    }
    assert!(script.active.lock().unwrap().is_empty());
    assert_eq!(*script.deleted.lock().unwrap(), vec!["conv-1"]);

    let script = Script::default();
    script.replies.lock().unwrap().push(calls(
        "bad",
        &[(
            "call",
            "Bash",
            json!({ "command": "touch should-not-exist" }),
        )],
    ));
    assert!(
        super::side::compact_with(&script, &turn, CancellationToken::new())
            .await
            .is_err()
    );
    assert!(!dir.path().join("should-not-exist").exists());
    assert!(script.active.lock().unwrap().is_empty());
    assert_eq!(*script.deleted.lock().unwrap(), vec!["conv-1"]);
}

#[tokio::test]
async fn an_mcp_image_goes_to_claude_ai_with_the_results() {
    let work = tempfile::tempdir().expect("a dir");
    let backend = script(vec![
        calls("m1", &[("x1", "mcp__computer__image", json!({}))]),
        answer("m2", "I see the screen."),
    ]);
    let mut turn = turn(work.path(), None, Permissions::Full);
    turn.launch.mcp_servers = vec![super::mcp::fake_stdio(
        "computer",
        &[("FAKE_TOOLS", "image")],
    )];
    let (outcome, _, _) = run(Arc::clone(&backend), turn, nobody()).await;
    assert_eq!(outcome.status, RunStatus::Done);
    let sent = backend.sent.lock().expect("lock").clone();
    assert_eq!(
        sent[1].files,
        vec!["file-1".to_owned()],
        "the screenshot goes with the results"
    );
    assert!(
        sent[1].prompt.contains("attached to this message"),
        "the result says where the image is: {}",
        sent[1].prompt
    );
    let uploaded = backend.uploaded.lock().expect("lock").clone();
    assert_eq!(uploaded.len(), 1);
    assert!(
        uploaded[0].starts_with(b"\x89PNG"),
        "the server's PNG is what was uploaded"
    );
}

/// A turn whose model starts one agent with `prompt`.
fn starts_an_agent(prompt: &str) -> Reply {
    calls(
        "m1",
        &[(
            "toolu_t",
            "Task",
            json!({ "description": "An agent", "prompt": prompt }),
        )],
    )
}

#[tokio::test]
async fn stopping_the_turn_during_a_task_stops_the_agent_s_completion() {
    let work = tempfile::tempdir().expect("a dir");
    let mut working = vec![start("c1")];
    working.extend(text(0, "Looking."));
    let backend = script(vec![
        starts_an_agent("Look around."),
        // The agent's completion streams on; it never ends by itself.
        Reply::Waits(working),
    ]);
    *backend.stop_delay.lock().expect("lock") = Some(std::time::Duration::from_millis(300));
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    let mut user: User = Box::new(move |event| {
        if matches!(event, EngineEvent::Live { target: crate::LiveKind::Text { key }, .. } if key.starts_with("toolu_t:"))
        {
            stopper.cancel();
        }
        Vec::new()
    });
    let (outcome, _, _) = run_with(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Full),
        &mut user,
        cancel,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Cancelled);
    assert!(
        backend
            .stopped
            .lock()
            .expect("lock")
            .contains(&"conv-2".to_owned()),
        "claude.ai confirmed stopping the agent's completion before the turn ended"
    );
    assert!(
        backend.active.lock().expect("lock").is_empty(),
        "no completion is left running"
    );
    assert_eq!(*backend.deleted.lock().expect("lock"), ["conv-2"]);
}

#[tokio::test]
async fn lowering_access_before_a_task_applies_to_the_agent() {
    let work = tempfile::tempdir().expect("a dir");
    let marker = work.path().join("ran");
    let backend = script(vec![
        starts_an_agent("Run the command."),
        calls(
            "c1",
            &[(
                "toolu_b",
                "Bash",
                json!({ "command": format!("touch {}", marker.display()) }),
            )],
        ),
        answer("c2", "It was declined."),
        answer("m2", "Done."),
    ]);
    let mut lowered = false;
    let user: User = Box::new(move |event| {
        if !lowered && matches!(event, EngineEvent::Model { .. }) {
            lowered = true;
            return vec![Input::SetPermissions(Permissions::Auto)];
        }
        awaiting(event)
            .map(|key| Input::Answer {
                key: key.to_owned(),
                answer: UserAnswer::Approval {
                    decision: Decision::DenyOnce,
                },
            })
            .into_iter()
            .collect()
    });
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Full),
        user,
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    assert!(
        log.iter()
            .any(|event| awaiting(event) == Some("toolu_t:toolu_b")),
        "the agent asks, as the thread now does"
    );
    assert!(!marker.exists(), "the declined command never ran");
}

#[tokio::test]
async fn allow_always_holds_for_the_agent_too() {
    let work = tempfile::tempdir().expect("a dir");
    let log_file = work.path().join("log");
    let command = format!("echo ran >> {}", log_file.display());
    let backend = script(vec![
        calls("m1", &[("toolu_1", "Bash", json!({ "command": command }))]),
        calls(
            "m2",
            &[(
                "toolu_t",
                "Task",
                json!({ "description": "Again", "prompt": "Run it again." }),
            )],
        ),
        calls("c1", &[("toolu_2", "Bash", json!({ "command": command }))]),
        answer("c2", "Ran it."),
        answer("m3", "Done."),
    ]);
    let (outcome, log, _) = run(
        Arc::clone(&backend),
        turn(work.path(), None, Permissions::Auto),
        decide(Decision::AllowAlways),
    )
    .await;
    assert_eq!(outcome.status, RunStatus::Done);
    let asked: Vec<&str> = log.iter().filter_map(awaiting).collect();
    assert_eq!(asked, ["toolu_1"], "the agent did not ask again");
    assert_eq!(
        std::fs::read_to_string(&log_file).expect("the log"),
        "ran\nran\n"
    );
}

#[tokio::test]
async fn compaction_branches_from_the_source_leaf_and_resumes_the_original_branch() {
    let work = tempfile::tempdir().unwrap();
    let mut source = turn(
        work.path(),
        Some("claude.ai:source-conversation/original-leaf"),
        Permissions::Full,
    );
    source.config.model = Some("claude-opus-5-5".into());
    source.input.text = crate::compaction_prompt();
    let backend = script(vec![
        answer("summary", "## Objective\n- Continue the parser"),
        answer("continued", "original branch"),
    ]);
    let summary = super::side::compact_with(backend.as_ref(), &source, CancellationToken::new())
        .await
        .unwrap();
    assert!(crate::is_compaction_summary(&summary.text));
    assert_eq!(
        *backend.created.lock().unwrap(),
        0,
        "use the source's server-held history"
    );
    assert!(
        backend.deleted.lock().unwrap().is_empty(),
        "never delete the source"
    );
    assert!(backend.active.lock().unwrap().is_empty());
    source.input.text = "Continue from my original branch".into();
    let (outcome, _, _) = run(Arc::clone(&backend), source.clone(), nobody()).await;
    assert_eq!(outcome.status, RunStatus::Done);
    let sent = backend.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].parent.as_deref(), Some("original-leaf"));
    assert_eq!(
        sent[1].parent, sent[0].parent,
        "the summary must not advance the continuation"
    );
    assert_eq!(sent[0].model, sent[1].model);
    assert_eq!(sent[0].effort, sent[1].effort);
    assert_eq!(
        sent[0].tools, sent[1].tools,
        "keep the tool prefix unchanged"
    );
    assert!(!sent[0].tools.is_empty());
    assert_eq!(
        sent[0].prompt,
        crate::compaction_prompt(),
        "no history is reserialized"
    );
    assert_eq!(
        *backend.conversations.lock().unwrap(),
        vec!["source-conversation", "source-conversation"]
    );
    assert_eq!(
        source.session_id.as_deref(),
        Some("claude.ai:source-conversation/original-leaf")
    );
}

#[tokio::test]
async fn compaction_of_a_source_branch_never_executes_tools_or_deletes_the_source() {
    let work = tempfile::tempdir().unwrap();
    let source = turn(
        work.path(),
        Some("claude.ai:source/leaf"),
        Permissions::Full,
    );
    let backend = script(vec![calls(
        "bad",
        &[("call", "Bash", json!({"command": "touch should-not-exist"}))],
    )]);
    assert!(
        super::side::compact_with(backend.as_ref(), &source, CancellationToken::new())
            .await
            .is_err()
    );
    assert!(!work.path().join("should-not-exist").exists());
    assert!(backend.deleted.lock().unwrap().is_empty());
    assert!(backend.active.lock().unwrap().is_empty());
    assert_eq!(*backend.created.lock().unwrap(), 0);
}

#[tokio::test]
async fn cancelled_branch_compaction_releases_the_source_completion() {
    let work = tempfile::tempdir().unwrap();
    let source = turn(
        work.path(),
        Some("claude.ai:source/leaf"),
        Permissions::Full,
    );
    let backend = script(vec![Reply::Waits(vec![start("summary")])]);
    let cancel = CancellationToken::new();
    let running_backend = Arc::clone(&backend);
    let running_cancel = cancel.clone();
    let running = tokio::spawn(async move {
        super::side::compact_with(running_backend.as_ref(), &source, running_cancel).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while backend.active.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), running)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(backend.active.lock().unwrap().is_empty());
    assert!(backend.deleted.lock().unwrap().is_empty());
    assert_eq!(*backend.stopped.lock().unwrap(), vec!["source"]);
}

#[tokio::test]
async fn native_compaction_without_a_saved_source_leaf_requires_canonical_fallback() {
    let work = tempfile::tempdir().unwrap();
    let source = turn(work.path(), Some("claude.ai:source"), Permissions::Full);
    let backend = Script::default();
    let error = super::side::compact_with(&backend, &source, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("saved message UUID"));
    assert!(backend.sent.lock().unwrap().is_empty());
    assert_eq!(*backend.created.lock().unwrap(), 0);
}
