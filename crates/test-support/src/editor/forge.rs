use artisan_api::tls::{Identity, fingerprint, server_config};
use artisan_api::{Envelope, Event, Seq, State};
use axum::body::{Body, Bytes};
use axum::http::{Method, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, watch};

const TAKE_WITHIN: Duration = crate::wait::LIVENESS;
pub(crate) const BARRIER: &str = "test-barrier";

pub struct HeldRequest {
    method: Method,
    path: String,
    query: Option<String>,
    body: Bytes,
    reply: oneshot::Sender<Response>,
}

impl HeldRequest {
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    pub(crate) fn hold(
        method: Method,
        path: String,
        body: Bytes,
    ) -> (Self, oneshot::Receiver<Response>) {
        let (reply, answer) = oneshot::channel();
        (
            Self {
                method,
                path,
                query: None,
                body,
                reply,
            },
            answer,
        )
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "the {} {} body is not the expected JSON: {error}",
                self.method, self.path
            )
        })
    }

    pub fn respond(self, value: impl serde::Serialize) {
        self.send(
            axum::Json(serde_json::to_value(value).expect("encode a stub answer")).into_response(),
        );
    }

    pub fn no_content(self) {
        self.send(StatusCode::NO_CONTENT.into_response());
    }

    pub fn respond_bytes(self, content_type: &'static str, bytes: Vec<u8>) {
        self.send(([(header::CONTENT_TYPE, content_type)], bytes).into_response());
    }

    pub fn fail(self, status: StatusCode, code: &str, message: &str) {
        self.respond_with(
            status,
            serde_json::json!({"code": code, "message": message}),
        );
    }

    pub fn respond_with(self, status: StatusCode, value: impl serde::Serialize) {
        self.send(
            (
                status,
                axum::Json(serde_json::to_value(value).expect("encode a stub answer")),
            )
                .into_response(),
        );
    }

    fn send(self, response: Response) {
        match self.reply.send(response) {
            Ok(()) | Err(_) => {}
        }
    }
}

impl std::fmt::Debug for HeldRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldRequest")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &self.query)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct Held {
    requests: Mutex<VecDeque<HeldRequest>>,
    arrived: Condvar,
}

struct Log {
    state: State,
    events: Vec<Envelope>,
}

struct Shared {
    held: Held,
    log: Mutex<Log>,
    head: watch::Sender<Seq>,
    streams: watch::Sender<Streams>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Streams {
    Open { epoch: u64 },
    Stopped,
}

pub struct TlsServer {
    address: String,
    fingerprint: String,
    task: tokio::task::JoinHandle<()>,
}

impl TlsServer {
    pub async fn serve(router: axum::Router) -> Self {
        let identity =
            Identity::generate("stub-forge").expect("generate the stub Forge's identity");
        let certificate = identity
            .certificate_der()
            .expect("decode the stub Forge's certificate");
        let fingerprint = fingerprint(certificate.as_ref());
        let config = server_config(&identity).expect("configure the stub Forge's TLS");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the stub Forge");
        let address = listener
            .local_addr()
            .expect("read the stub Forge's address")
            .to_string();
        let tls = TlsListener {
            listener,
            acceptor: tokio_rustls::TlsAcceptor::from(Arc::new(config)),
        };
        let task = tokio::spawn(async move {
            axum::serve(tls, router)
                .await
                .expect("the stub Forge stopped serving");
        });
        Self {
            address,
            fingerprint,
            task,
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn client(&self) -> editor_core::ForgeClient {
        let identity = Identity::generate("stub-editor").expect("generate an Editor identity");
        editor_core::ForgeClient::new(&self.address, &self.fingerprint, &identity)
            .expect("build a client for the stub Forge")
    }
}

impl Drop for TlsServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl std::fmt::Debug for TlsServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsServer")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

pub struct StubForge {
    server: TlsServer,
    shared: Arc<Shared>,
}

pub fn inconsistent(path: &str, check: &'static str) -> editor_core::client::CallError {
    editor_core::client::CallError {
        method: Method::GET,
        path: path.to_owned(),
        failure: Box::new(editor_core::client::CallFailure::Inconsistent { check }),
    }
}

pub fn png(shade: u8) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(1, 1, image::Rgba([shade, shade, shade, 255]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode a one-pixel PNG");
    bytes.into_inner()
}

pub fn empty_state() -> State {
    State {
        head: 0,
        forge_version: "test".into(),
        protocol: artisan_api::PROTOCOL_VERSION,
        projects: Vec::new(),
        threads: Vec::new(),
        threads_next: None,
        runs: Vec::new(),
        settings: serde_json::Map::new(),
        engines: Vec::new(),
        waiting: Vec::new(),
        account: None,
        database_rollback: None,
    }
}

impl StubForge {
    pub async fn serve(state: State, routes: axum::Router) -> Self {
        let head = state.head;
        let shared = Arc::new(Shared {
            held: Held::default(),
            log: Mutex::new(Log {
                state,
                events: Vec::new(),
            }),
            head: watch::Sender::new(head),
            streams: watch::Sender::new(Streams::Open { epoch: 0 }),
        });
        let app = routes.merge(
            axum::Router::new()
                .route("/state", get(answer_state))
                .route("/events", get(stream_events))
                .fallback(hold)
                .with_state(Arc::clone(&shared)),
        );
        let server = TlsServer::serve(app).await;
        Self { server, shared }
    }

    pub fn address(&self) -> &str {
        self.server.address()
    }

    pub fn fingerprint(&self) -> &str {
        self.server.fingerprint()
    }

    pub fn head(&self) -> Seq {
        self.log().state.head
    }

    pub fn publish(&self, event: Event) -> Seq {
        let mut log = self.log();
        let seq = log.state.head + 1;
        log.state.head = seq;
        log.events.push(Envelope {
            seq: Some(seq),
            live_revision: 0,
            event,
        });
        drop(log);
        self.shared.head.send_replace(seq);
        seq
    }

    pub fn list_thread(&self, thread: artisan_api::Thread) {
        let mut log = self.log();
        log.state.threads.retain(|listed| listed.id != thread.id);
        log.state.threads.push(thread);
    }

    pub(crate) fn publish_barrier(&self) -> Seq {
        let seq = self.log().state.head + 1;
        self.publish(Event::Setting {
            key: BARRIER.into(),
            value: serde_json::json!(seq),
        })
    }

    pub fn close_streams(&self) {
        self.shared.streams.send_modify(|streams| match streams {
            Streams::Open { epoch } => *epoch += 1,
            Streams::Stopped => {}
        });
    }

    pub fn stop_streaming(&self) {
        self.shared.streams.send_replace(Streams::Stopped);
    }

    #[track_caller]
    pub fn take(&self, method: &str, path: &str) -> HeldRequest {
        self.take_where(&format!("{method} {path}"), |request| {
            request.method.as_str() == method && request.path == path
        })
    }

    #[track_caller]
    fn take_where(&self, what: &str, matches: impl Fn(&HeldRequest) -> bool) -> HeldRequest {
        let deadline = Instant::now() + TAKE_WITHIN;
        let mut requests = self.held();
        loop {
            if let Some(index) = requests.iter().position(&matches) {
                return requests
                    .remove(index)
                    .expect("invariant: the matched request is still queued");
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "the Editor never asked the stub Forge for {what}; it asked for {:?}",
                requests
                    .iter()
                    .map(|request| format!("{} {}", request.method, request.path))
                    .collect::<Vec<_>>()
            );
            requests = self
                .shared
                .held
                .arrived
                .wait_timeout(requests, remaining)
                .expect("invariant: the held-request lock is never poisoned")
                .0;
        }
    }

    pub fn try_take(&self, method: &str, path: &str) -> Option<HeldRequest> {
        let mut requests = self.held();
        let index = requests
            .iter()
            .position(|request| request.method.as_str() == method && request.path == path)?;
        requests.remove(index)
    }

    pub fn asked(&self, method: &str, path: &str) -> bool {
        self.held()
            .iter()
            .any(|request| request.method.as_str() == method && request.path == path)
    }

    fn held(&self) -> std::sync::MutexGuard<'_, VecDeque<HeldRequest>> {
        self.shared
            .held
            .requests
            .lock()
            .expect("invariant: the held-request lock is never poisoned")
    }

    fn log(&self) -> std::sync::MutexGuard<'_, Log> {
        self.shared
            .log
            .lock()
            .expect("invariant: the stub Forge's log lock is never poisoned")
    }
}

impl std::fmt::Debug for StubForge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StubForge")
            .field("address", &self.server.address)
            .finish_non_exhaustive()
    }
}

async fn answer_state(axum::extract::State(shared): axum::extract::State<Arc<Shared>>) -> Response {
    let state = shared
        .log
        .lock()
        .expect("invariant: the stub Forge's log lock is never poisoned")
        .state
        .clone();
    axum::Json(state).into_response()
}

#[derive(serde::Deserialize)]
struct Since {
    since: Seq,
}

async fn stream_events(
    axum::extract::State(shared): axum::extract::State<Arc<Shared>>,
    axum::extract::Query(Since { since }): axum::extract::Query<Since>,
) -> Response {
    let mut streams = shared.streams.subscribe();
    if *streams.borrow_and_update() == Streams::Stopped {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let head = shared.head.subscribe();
    let sent = {
        let log = shared
            .log
            .lock()
            .expect("invariant: the stub Forge's log lock is never poisoned");
        log.events
            .iter()
            .position(|envelope| envelope.seq.is_some_and(|seq| seq > since))
            .unwrap_or(log.events.len())
    };
    let stream = futures::stream::unfold(
        (shared, head, streams, sent),
        |(shared, mut head, mut streams, mut sent)| async move {
            loop {
                let next = shared
                    .log
                    .lock()
                    .expect("invariant: the stub Forge's log lock is never poisoned")
                    .events
                    .get(sent)
                    .cloned();
                if let Some(envelope) = next {
                    sent += 1;
                    let frame = format!(
                        "data: {}\n\n",
                        serde_json::to_string(&envelope).expect("encode a stub event")
                    );
                    return Some((
                        Ok::<_, std::convert::Infallible>(Bytes::from(frame)),
                        (shared, head, streams, sent),
                    ));
                }
                tokio::select! {
                    changed = head.changed() => if changed.is_err() { return None },
                    _ = streams.changed() => return None,
                    () = tokio::time::sleep(Duration::from_secs(5)) => {
                        return Some((Ok(Bytes::from_static(b": keep-alive\n\n")), (shared, head, streams, sent)));
                    }
                }
            }
        },
    );
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(stream),
    )
        .into_response()
}

async fn hold(
    axum::extract::State(shared): axum::extract::State<Arc<Shared>>,
    request: Request<Body>,
) -> Response {
    let (parts, body) = request.into_parts();
    let Ok(body) = axum::body::to_bytes(body, usize::MAX).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let (reply, answer) = oneshot::channel();
    shared
        .held
        .requests
        .lock()
        .expect("invariant: the held-request lock is never poisoned")
        .push_back(HeldRequest {
            method: parts.method,
            path: parts.uri.path().to_owned(),
            query: parts.uri.query().map(str::to_owned),
            body,
            reply,
        });
    shared.held.arrived.notify_all();
    match answer.await {
        Ok(response) => response,
        Err(_dropped) => std::future::pending::<Response>().await,
    }
}

struct TlsListener {
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let Ok((socket, address)) = self.listener.accept().await else {
                continue;
            };
            if let Ok(stream) = self.acceptor.accept(socket).await {
                return (stream, address);
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}
