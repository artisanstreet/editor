use crate::{alive, append, emit, touch};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri, header};
use axum::response::{IntoResponse as _, Response};
use futures::{Stream, StreamExt as _, stream};
use serde_json::{Value, json};
use std::convert::Infallible;
use std::io::Write as _;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy)]
pub(crate) enum Mode {
    Title,
    ClearGoal,
    Compaction { hang: bool },
    Cancellation { hang_prompt: bool },
    OversizedOutput,
}

impl Mode {
    pub(crate) fn compaction() -> Self {
        Self::Compaction {
            hang: std::env::var_os("ARTISAN_TEST_COMPACTION_HANG").is_some(),
        }
    }

    pub(crate) fn cancellation() -> Self {
        Self::Cancellation {
            hang_prompt: std::env::var("HANG_PROMPT").is_ok_and(|value| value == "true"),
        }
    }
}

#[derive(Debug)]
struct Server {
    mode: Mode,
    stopped: CancellationToken,
    queued: mpsc::UnboundedSender<Value>,
    events: Mutex<mpsc::UnboundedReceiver<Value>>,
}

pub(crate) fn serve(mode: Mode) -> ExitCode {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the fake OpenCode server starts its runtime")
        .block_on(listen(mode))
}

async fn listen(mode: Mode) -> ExitCode {
    let _alive = alive("server");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the fake OpenCode server binds a port");
    let port = listener
        .local_addr()
        .expect("the fake OpenCode server has an address")
        .port();
    emit(&format!(
        "opencode server listening on http://127.0.0.1:{port}"
    ));
    let (queued, events) = mpsc::unbounded_channel();
    let server = Arc::new(Server {
        mode,
        stopped: CancellationToken::new(),
        queued,
        events: Mutex::new(events),
    });
    axum::serve(listener, Router::new().fallback(handle).with_state(server))
        .await
        .expect("the fake OpenCode server serves");
    ExitCode::SUCCESS
}

async fn handle(
    State(server): State<Arc<Server>>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    let path = uri
        .path_and_query()
        .map_or_else(|| uri.path().to_owned(), ToString::to_string);
    let body: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    match server.mode {
        Mode::Title => title(&method, &path, &body),
        Mode::ClearGoal => clear_goal(&method, &path, &body),
        Mode::Compaction { hang } => compaction(&server, hang, &method, &path, &body),
        Mode::Cancellation { hang_prompt } => {
            cancellation(&server, hang_prompt, &method, &path).await
        }
        Mode::OversizedOutput => oversized_output(&server, &method, &path),
    }
}

fn title(method: &Method, path: &str, body: &Value) -> Response {
    if *method == Method::PATCH
        && path == "/api/session/working-session"
        && *body == json!({ "title": "Fix sidebar titles" })
    {
        std::fs::write("named", body.to_string()).expect("the fake records the title");
        return StatusCode::NO_CONTENT.into_response();
    }
    unexpected(method, path)
}

fn clear_goal(method: &Method, path: &str, body: &Value) -> Response {
    if *method == Method::POST
        && path == "/api/session/idle-session/command"
        && *body == json!({ "name": "goal", "text": "clear", "files": [], "delivery": "steer" })
    {
        std::fs::write("cleared", "idle-session").expect("the fake records the cleared goal");
        return StatusCode::NO_CONTENT.into_response();
    }
    unexpected(method, path)
}

fn compaction(
    server: &Arc<Server>,
    hang: bool,
    method: &Method,
    path: &str,
    body: &Value,
) -> Response {
    match (method.as_str(), path) {
        ("GET", "/api/event") => events(queued(Arc::clone(server))),
        ("DELETE", "/api/session/fork") => {
            std::fs::write("deleted-fork", path).expect("the fake records the deletion");
            StatusCode::NO_CONTENT.into_response()
        }
        ("POST", _) => {
            append(
                std::path::Path::new("requests"),
                &format!("{}\n", json!([path, body])),
            );
            match path {
                "/api/session/source/fork" => reply(&json!({ "data": { "id": "fork" } })),
                "/api/session/fork/model"
                    if *body
                        == json!({ "model": { "providerID": "opencode", "id": "big-pickle" } }) =>
                {
                    StatusCode::OK.into_response()
                }
                "/api/session/fork/compact" => {
                    if hang {
                        std::fs::write("hanging", "waiting").expect("the fake records the hang");
                    } else {
                        for event in [
                            json!({ "type": "session.compaction.ended", "data": { "sessionID": "source", "text": "wrong session" } }),
                            json!({ "type": "session.compaction.ended", "data": { "sessionID": "fork", "text": "## Objective\n- Keep building", "tokens": { "input": 3, "output": 20, "reasoning": 0, "cache": { "read": 2000, "write": 0 } } } }),
                        ] {
                            server
                                .queued
                                .send(event)
                                .expect("the fake keeps its event queue");
                        }
                    }
                    reply(&json!({ "data": { "id": "compact-request" } }))
                }
                "/api/session/fork/interrupt" => StatusCode::OK.into_response(),
                _ => unexpected(method, path),
            }
        }
        _ => unexpected(method, path),
    }
}

async fn cancellation(
    server: &Arc<Server>,
    hang_prompt: bool,
    method: &Method,
    path: &str,
) -> Response {
    match (method.as_str(), path) {
        ("GET", "/api/event") => events(until_stopped(server.stopped.clone())),
        ("GET", _) => reply(&json!([])),
        ("POST", "/api/session") => reply(&json!({ "data": { "id": "root" } })),
        ("POST", "/api/rpc/opencode.goal/get") => reply(&json!({ "output": null })),
        ("POST", "/api/session/root/prompt") => {
            touch("prompt");
            if hang_prompt {
                wait_for_stop(server.stopped.clone()).await;
            }
            reply(&json!({ "data": { "id": "prompt" } }))
        }
        ("POST", "/api/session/root/interrupt") => {
            tokio::time::sleep(Duration::from_millis(30)).await;
            touch("interrupted");
            let stopped = server.stopped.clone();
            let then_stop = stream::once(async move {
                stopped.cancel();
                None
            })
            .filter_map(std::future::ready);
            Response::new(Body::from_stream(
                stream::once(async { Some(Ok::<_, Infallible>(Bytes::from("null"))) })
                    .filter_map(std::future::ready)
                    .chain(then_stop),
            ))
        }
        _ => unexpected(method, path),
    }
}

fn oversized_output(server: &Arc<Server>, method: &Method, path: &str) -> Response {
    match (method.as_str(), path) {
        ("GET", "/api/event") => events(until_stopped(server.stopped.clone())),
        ("GET", _) => reply(&json!([])),
        ("POST", "/api/session") => reply(&json!({ "data": { "id": "root" } })),
        ("POST", "/api/rpc/opencode.goal/get") => reply(&json!({ "output": null })),
        ("POST", "/api/session/root/prompt") => {
            tokio::task::spawn_blocking(|| {
                let mut stdout = std::io::stdout().lock();
                stdout
                    .write_all(&vec![b'x'; 32 * 1024 * 1024 + 1])
                    .and_then(|()| stdout.write_all(b"\n"))
                    .and_then(|()| stdout.flush())
                    .expect("the harness reads the oversized line");
            });
            reply(&json!({ "data": { "id": "prompt" } }))
        }
        ("POST", "/api/session/root/interrupt") => reply(&Value::Null),
        _ => unexpected(method, path),
    }
}

fn queued(server: Arc<Server>) -> impl Stream<Item = Value> + Send {
    stream::unfold(server, |server| async move {
        let event = server.events.lock().await.recv().await?;
        Some((event, server))
    })
}

fn until_stopped(stopped: CancellationToken) -> impl Stream<Item = Value> + Send {
    stream::once(async move {
        wait_for_stop(stopped).await;
        None
    })
    .filter_map(std::future::ready)
}

async fn wait_for_stop(stopped: CancellationToken) {
    tokio::time::timeout(Duration::from_secs(30), stopped.cancelled())
        .await
        .unwrap_or(());
}

fn events(stream: impl Stream<Item = Value> + Send + 'static) -> Response {
    let frames = stream::once(async { json!({ "type": "server.connected" }) })
        .chain(stream)
        .map(|event| Ok::<_, Infallible>(format!("data: {event}\n\n")));
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(frames),
    )
        .into_response()
}

fn reply(value: &Value) -> Response {
    (
        [(header::CONTENT_TYPE, "application/json")],
        value.to_string(),
    )
        .into_response()
}

fn unexpected(method: &Method, path: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("the fake OpenCode server did not expect {method} {path}"),
    )
        .into_response()
}
