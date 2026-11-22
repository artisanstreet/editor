use crate::{append, emit};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri, header};
use axum::response::{IntoResponse as _, Response};
use futures::stream;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, mpsc};
use test_support::transcript::{Step, Transcript};
use tokio::sync::Notify;

const TRANSCRIPT: &str = "transcript.jsonl";
const LAUNCHES: &str = "launches";
const LOG: &str = "replay.log";
const CHECK: &str = test_support::transcript::CHECK;
const IDS: [&str; 2] = ["/id", "/request_id"];
const REPLY_IDS: [&str; 2] = ["/id", "/response/request_id"];
const KINDS: [&str; 7] = [
    "/type",
    "/subtype",
    "/method",
    "/request/subtype",
    "/response/subtype",
    "/message/content/0/text",
    "/params/input/0/text",
];

#[derive(Debug, Clone, Copy)]
enum Protocol {
    Stdio,
    Http,
}

impl Protocol {
    fn of(args: &[String]) -> Option<Self> {
        match args.first().map(String::as_str) {
            Some("app-server") if !args.iter().any(|arg| arg == "--stdio") => Some(Self::Stdio),
            Some("serve") => Some(Self::Http),
            _ if args.iter().any(|arg| arg == "--replay-user-messages") => Some(Self::Stdio),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum ReplayError {
    #[error("the replaying engine only runs turns, not {args:?}")]
    NotATurn { args: Vec<String> },
    #[error("read the transcript {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse the transcript {}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: test_support::transcript::ParseError,
    },
    #[error("count launches in {}", path.display())]
    Count {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("launch {launch} is past the transcript's {launches} launches")]
    NoLaunch { launch: usize, launches: usize },
    #[error("input closed while the transcript expects {expected}")]
    InputClosed { expected: Signature },
    #[error("the transcript expects {expected} but Forge sent {actual}")]
    Mismatch {
        expected: Signature,
        actual: Signature,
    },
    #[error("Forge sent {actual} after the transcript's input ended")]
    Unexpected { actual: Signature },
    #[error("a {protocol:?} launch cannot replay {step:?}")]
    WrongStep { protocol: Protocol, step: Step },
    #[error("Forge gave the engine no {server} MCP gateway in {variable}")]
    NoGateway {
        server: String,
        variable: String,
        #[source]
        source: std::env::VarError,
    },
    #[error("the {server} MCP gateway address {url} is not a loopback HTTP address")]
    GatewayAddress { server: String, url: String },
    #[error("call the {server} MCP gateway")]
    Call {
        server: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the {server} MCP gateway answered {answer}")]
    CallRefused { server: String, answer: String },
    #[error("wait at the gate {}", path.display())]
    Gate {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Copy)]
enum Ending {
    Finished,
    Died { code: u8 },
}

pub(crate) fn run() -> ExitCode {
    if std::env::args().skip(1).eq([CHECK]) {
        return match transcript() {
            Ok(_) => ExitCode::SUCCESS,
            Err(failure) => fail(&failure, |_| {}),
        };
    }
    match replay() {
        Ok(Ending::Finished) => ExitCode::SUCCESS,
        Ok(Ending::Died { code }) => {
            log(&format!("died with {code}"));
            ExitCode::from(code)
        }
        Err(failure) => fail(&failure, |error| log(&format!("failed: {error}"))),
    }
}

fn fail(failure: &ReplayError, mut record: impl FnMut(&dyn std::error::Error)) -> ExitCode {
    let mut stderr = std::io::stderr().lock();
    let mut cause: Option<&dyn std::error::Error> = Some(failure);
    while let Some(error) = cause {
        writeln!(stderr, "{error}").expect("the replaying engine writes to stderr");
        record(error);
        cause = error.source();
    }
    ExitCode::from(3)
}

fn log(text: &str) {
    let exe = std::env::current_exe().expect("the replaying engine knows its own path");
    append(
        &exe.with_file_name(LOG),
        &format!("{} {text}\n", std::process::id()),
    );
}

fn transcript() -> Result<(PathBuf, Transcript), ReplayError> {
    let exe = std::env::current_exe().expect("the replaying engine knows its own path");
    let dir = exe
        .parent()
        .expect("the replaying engine lives in a folder")
        .to_path_buf();
    let path = dir.join(TRANSCRIPT);
    let text = std::fs::read_to_string(&path).map_err(|source| ReplayError::Read {
        path: path.clone(),
        source,
    })?;
    let transcript =
        Transcript::parse(&text).map_err(|source| ReplayError::Parse { path, source })?;
    Ok((dir, transcript))
}

fn replay() -> Result<Ending, ReplayError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    log(&format!("started with {args:?}"));
    let protocol = Protocol::of(&args).ok_or(ReplayError::NotATurn { args })?;
    let (dir, transcript) = transcript()?;
    let launch = next_launch(&dir)?;
    log(&format!("replaying launch {launch}"));
    let steps = transcript.launch(launch).ok_or(ReplayError::NoLaunch {
        launch,
        launches: transcript.launches(),
    })?;
    match protocol {
        Protocol::Stdio => stdio(steps, &dir),
        Protocol::Http => http(steps).map(|()| Ending::Finished),
    }
}

fn next_launch(dir: &Path) -> Result<usize, ReplayError> {
    let path = dir.join(LAUNCHES);
    let count = |source| ReplayError::Count {
        path: path.clone(),
        source,
    };
    let launch = match std::fs::read_to_string(&path) {
        Ok(text) => text
            .trim()
            .parse()
            .expect("the launch counter holds a number"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(count(error)),
    };
    std::fs::write(&path, (launch + 1).to_string()).map_err(count)?;
    Ok(launch)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Signature(Vec<(&'static str, Option<Value>)>);

use test_support::transcript::ANY;

impl Signature {
    fn admits(&self, actual: &Self) -> bool {
        self.0.len() == actual.0.len()
            && self
                .0
                .iter()
                .zip(&actual.0)
                .all(|((pointer, wanted), (seen, got))| {
                    pointer == seen
                        && match (wanted, got) {
                            (Some(Value::String(wanted)), Some(Value::String(got)))
                                if wanted.contains(ANY) =>
                            {
                                fits(wanted, got)
                            }
                            _ => wanted == got,
                        }
                })
    }
}

fn fits(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split(ANY);
    let Some(rest) = parts.next().and_then(|first| text.strip_prefix(first)) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    let mut rest = rest;
    for part in middle {
        let Some((_, after)) = rest.split_once(part) else {
            return false;
        };
        rest = after;
    }
    rest.ends_with(last)
}

impl std::fmt::Display for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown: Vec<String> = self
            .0
            .iter()
            .filter_map(|(pointer, value)| match value {
                None | Some(Value::Bool(false)) => None,
                Some(Value::Bool(true)) => Some((*pointer).to_owned()),
                Some(value) => Some(format!("{pointer}={value}")),
            })
            .collect();
        write!(f, "{{{}}}", shown.join(" "))
    }
}

#[derive(Debug, Default)]
struct Ids {
    emitted: HashSet<Value>,
    bound: HashMap<Value, Value>,
}

impl Ids {
    fn outgoing(&mut self, text: &str) -> String {
        let Ok(mut frame) = serde_json::from_str::<Value>(text) else {
            return text.to_owned();
        };
        if frame.get("method").is_some()
            && let Some(id) = frame.get("id")
        {
            self.emitted.insert(id.clone());
        }
        if let Some(id) = frame.get("request_id") {
            self.emitted.insert(id.clone());
        }
        let mut changed = false;
        if frame.get("method").is_none() {
            for pointer in REPLY_IDS {
                if let Some(id) = frame.pointer_mut(pointer)
                    && let Some(actual) = self.bound.get(id)
                {
                    *id = actual.clone();
                    changed = true;
                }
            }
        }
        if changed {
            frame.to_string()
        } else {
            text.to_owned()
        }
    }

    fn signature(&self, frame: &Value, recorded: &Value) -> Signature {
        let kinds = KINDS
            .into_iter()
            .map(|pointer| (pointer, frame.pointer(pointer).cloned()));
        let outcome = ["/result", "/error"]
            .into_iter()
            .map(|pointer| (pointer, Some(Value::Bool(frame.pointer(pointer).is_some()))));
        let ids = ["/id", "/request_id", "/response/request_id"]
            .into_iter()
            .map(|pointer| {
                let answers_us = recorded
                    .pointer(pointer)
                    .is_some_and(|id| self.emitted.contains(id));
                let id = frame.pointer(pointer);
                (
                    pointer,
                    if answers_us {
                        id.cloned()
                    } else {
                        Some(Value::Bool(id.is_some()))
                    },
                )
            });
        Signature(kinds.chain(outcome).chain(ids).collect())
    }

    fn incoming(&mut self, recorded: &str, actual: &str) -> Result<(), ReplayError> {
        let recorded: Value = serde_json::from_str(recorded).unwrap_or(Value::Null);
        let actual_frame: Value = serde_json::from_str(actual).unwrap_or(Value::Null);
        let expected = self.signature(&recorded, &recorded);
        let seen = self.signature(&actual_frame, &recorded);
        if !expected.admits(&seen) {
            return Err(ReplayError::Mismatch {
                expected,
                actual: seen,
            });
        }
        for pointer in IDS {
            if let (Some(from), Some(to)) =
                (recorded.pointer(pointer), actual_frame.pointer(pointer))
                && !self.emitted.contains(from)
            {
                self.bound.insert(from.clone(), to.clone());
            }
        }
        Ok(())
    }

    fn unexpected(&self, actual: &str) -> Signature {
        let frame: Value = serde_json::from_str(actual).unwrap_or(Value::Null);
        self.signature(&frame, &Value::Null)
    }
}

fn stdin_lines() -> mpsc::Receiver<String> {
    let (lines, received) = mpsc::channel();
    std::thread::spawn(move || {
        while let Some(line) = crate::read_line() {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    received
}

fn gateway_variable(server: &str) -> String {
    let name: String = server
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("ARTISAN_GATEWAY_{name}")
}

fn call(server: &str, request: &Value) -> Result<(), ReplayError> {
    use std::io::Read as _;
    let variable = gateway_variable(server);
    let url = std::env::var(&variable).map_err(|source| ReplayError::NoGateway {
        server: server.to_owned(),
        variable,
        source,
    })?;
    let (address, path) = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split_once('/'))
        .ok_or_else(|| ReplayError::GatewayAddress {
            server: server.to_owned(),
            url: url.clone(),
        })?;
    let body = request.to_string();
    let failed = |source| ReplayError::Call {
        server: server.to_owned(),
        source,
    };
    let mut stream = std::net::TcpStream::connect(address).map_err(failed)?;
    write!(
        stream,
        "POST /{path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(failed)?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).map_err(failed)?;
    log(&format!("call {server}: {answer}"));
    let reply = answer
        .split_once("\r\n\r\n")
        .map(|(head, body)| (head, serde_json::from_str::<Value>(body)));
    let succeeded = matches!(&reply, Some((head, Ok(reply)))
        if head.starts_with("HTTP/1.1 200")
            && reply.get("error").is_none()
            && reply.pointer("/result/isError") != Some(&Value::Bool(true)));
    if succeeded {
        Ok(())
    } else {
        Err(ReplayError::CallRefused {
            server: server.to_owned(),
            answer,
        })
    }
}

fn wait_at(dir: &Path, name: &str) -> Result<(), ReplayError> {
    let path = dir.join(format!("{name}.gate"));
    let gate = |source| ReplayError::Gate {
        path: path.clone(),
        source,
    };
    let file = std::fs::File::open(&path).map_err(gate)?;
    file.lock_shared().map_err(gate)?;
    log(&format!("passed the gate {name}"));
    Ok(())
}

fn stdio(steps: &[Step], dir: &Path) -> Result<Ending, ReplayError> {
    let input = stdin_lines();
    let mut ids = Ids::default();
    let mut closed = false;
    for step in steps {
        match step {
            Step::Out { text } => emit(&ids.outgoing(text)),
            Step::In { text } => {
                let recorded: Value = serde_json::from_str(text).unwrap_or(Value::Null);
                let line = input.recv().map_err(|_closed| ReplayError::InputClosed {
                    expected: ids.signature(&recorded, &recorded),
                })?;
                log(&format!("in: {line}"));
                ids.incoming(text, &line)?;
            }
            Step::Closed => {
                if let Ok(line) = input.recv() {
                    return Err(ReplayError::Unexpected {
                        actual: ids.unexpected(&line),
                    });
                }
                closed = true;
            }
            Step::Exit { code } => return Ok(Ending::Died { code: *code }),
            Step::Call { server, request } => call(server, request)?,
            Step::Gate { name } => wait_at(dir, name)?,
            Step::Request { .. } => {
                return Err(ReplayError::WrongStep {
                    protocol: Protocol::Stdio,
                    step: step.clone(),
                });
            }
        }
    }
    if !closed && let Ok(line) = input.recv() {
        return Err(ReplayError::Unexpected {
            actual: ids.unexpected(&line),
        });
    }
    Ok(Ending::Finished)
}

#[derive(Debug)]
struct Slot {
    method: String,
    path: String,
    reply: Value,
    taken: bool,
}

#[derive(Debug)]
struct Server {
    steps: Vec<Step>,
    slots: Mutex<Vec<Slot>>,
    taken: Notify,
}

impl Server {
    fn take(&self, method: &Method, path: &str) -> Option<Value> {
        let mut slots = self
            .slots
            .lock()
            .expect("the replay slots are never poisoned");
        let slot = slots
            .iter_mut()
            .find(|slot| !slot.taken && slot.method == method.as_str() && slot.path == path)?;
        slot.taken = true;
        self.taken.notify_waiters();
        Some(slot.reply.clone())
    }

    fn is_taken(&self, index: usize) -> bool {
        self.slots
            .lock()
            .expect("the replay slots are never poisoned")[index]
            .taken
    }
}

fn http(steps: &[Step]) -> Result<(), ReplayError> {
    let slots = steps
        .iter()
        .map(|step| match step {
            Step::Request {
                method,
                path,
                reply,
            } => Ok(Some(Slot {
                method: method.clone(),
                path: path.clone(),
                reply: reply.clone(),
                taken: false,
            })),
            Step::Out { .. } => Ok(None),
            Step::In { .. }
            | Step::Call { .. }
            | Step::Gate { .. }
            | Step::Closed
            | Step::Exit { .. } => Err(ReplayError::WrongStep {
                protocol: Protocol::Http,
                step: step.clone(),
            }),
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    let server = Arc::new(Server {
        steps: steps.to_vec(),
        slots: Mutex::new(slots),
        taken: Notify::new(),
    });
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the replaying server starts its runtime")
        .block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("the replaying server binds a port");
            let port = listener
                .local_addr()
                .expect("the replaying server has an address")
                .port();
            emit(&format!(
                "opencode server listening on http://127.0.0.1:{port}"
            ));
            axum::serve(listener, Router::new().fallback(handle).with_state(server))
                .await
                .expect("the replaying server serves");
        });
    Ok(())
}

async fn handle(State(server): State<Arc<Server>>, method: Method, uri: Uri) -> Response {
    let path = uri
        .path_and_query()
        .map_or_else(|| uri.path().to_owned(), ToString::to_string);
    log(&format!("request: {method} {path}"));
    if method == Method::GET && uri.path() == "/api/event" {
        return events(server);
    }
    server.take(&method, &path).map_or_else(
        || {
            let missing = format!("the transcript has no request {method} {path}");
            log(&missing);
            (StatusCode::NOT_FOUND, missing).into_response()
        },
        |reply| {
            (
                [(header::CONTENT_TYPE, "application/json")],
                reply.to_string(),
            )
                .into_response()
        },
    )
}

fn events(server: Arc<Server>) -> Response {
    let frames = stream::unfold((server, 0, 0), |(server, step, slot)| async move {
        let mut step = step;
        let mut slot = slot;
        loop {
            let next = server.steps.get(step)?.clone();
            match next {
                Step::Out { text } => {
                    return Some((
                        Ok::<_, Infallible>(format!("data: {text}\n\n")),
                        (server, step + 1, slot),
                    ));
                }
                Step::Request { .. } => {
                    let taken = server.taken.notified();
                    if server.is_taken(slot) {
                        step += 1;
                        slot += 1;
                    } else {
                        taken.await;
                    }
                }
                Step::In { .. }
                | Step::Call { .. }
                | Step::Gate { .. }
                | Step::Closed
                | Step::Exit { .. } => {
                    unreachable!(
                        "invariant: HTTP launches were checked to hold only output and requests"
                    )
                }
            }
        }
    });
    let held = stream::once(std::future::pending::<Result<String, Infallible>>());
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(futures::StreamExt::chain(frames, held)),
    )
        .into_response()
}
