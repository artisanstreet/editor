use artisan_api::{Envelope, Event};
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use editor_core::connection::{self, ConnectionStatus, Deadlines, SessionError, Update};
use futures::StreamExt as _;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_support::editor::forge::TlsServer;
use test_support::wait::within;

async fn state() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "head": 0,
        "forge_version": "test",
        "protocol": artisan_api::PROTOCOL_VERSION,
        "projects": [],
        "threads": [],
        "runs": [],
        "settings": {},
        "engines": [],
    }))
}

fn hello() -> String {
    let envelope = Envelope {
        seq: None,
        live_revision: 0,
        event: Event::Hello {
            head: 0,
            protocol: artisan_api::PROTOCOL_VERSION,
            forge_version: "test".into(),
        },
    };
    format!(
        "data: {}\n\n",
        serde_json::to_string(&envelope).expect("encode Hello")
    )
}

fn event_stream(body: Body) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(body)
        .expect("build the event stream response")
}

async fn hello_then_silence() -> Response {
    event_stream(Body::from_stream(
        futures::stream::iter([Ok::<_, Infallible>(hello())]).chain(futures::stream::pending()),
    ))
}

async fn hello_then_close() -> Response {
    event_stream(Body::from(hello()))
}

async fn refusal_that_never_finishes() -> Response {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .body(Body::from_stream(futures::stream::pending::<
            Result<String, Infallible>,
        >()))
        .expect("build the refusal")
}

struct Follower {
    updates: tokio::sync::mpsc::Receiver<Update>,
    retry: Arc<tokio::sync::Notify>,
    _server: TlsServer,
    observe: tokio::sync::watch::Sender<Option<artisan_api::ThreadId>>,
    task: tokio::task::JoinHandle<()>,
}

impl Follower {
    async fn start(router: axum::Router, deadlines: Deadlines) -> Self {
        let server = TlsServer::serve(router.route("/state", axum::routing::get(state))).await;
        let (sender, updates) = tokio::sync::mpsc::channel(64);
        let (observe, observed) = tokio::sync::watch::channel(None);
        let retry = Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn(connection::run(
            server.client(),
            sender,
            Arc::clone(&retry),
            observed,
            deadlines,
        ));
        Self {
            updates,
            retry,
            _server: server,
            observe,
            task,
        }
    }

    async fn status(&mut self) -> ConnectionStatus {
        loop {
            match self
                .updates
                .recv()
                .await
                .expect("the connection keeps running")
            {
                Update::Status(status) => return status,
                Update::State(_) | Update::Event(..) => {}
            }
        }
    }

    async fn offline(&mut self) -> (u32, String) {
        loop {
            if let ConnectionStatus::Offline {
                attempt, reason, ..
            } = self.status().await
            {
                return (attempt, reason);
            }
        }
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn current_thread<T>(limit: Duration, test: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build a current-thread runtime")
        .block_on(within("the connection scenario", limit, test))
}

#[test]
fn a_stream_that_stops_sending_heartbeats_goes_offline() {
    current_thread(Duration::from_secs(600), async {
        let mut follower = Follower::start(
            axum::Router::new().route("/events", axum::routing::get(hello_then_silence)),
            Deadlines::FORGE,
        )
        .await;
        while follower.status().await != ConnectionStatus::Live {}
        tokio::time::pause();
        let (_, reason) = follower.offline().await;
        assert!(
            reason.contains("heartbeat"),
            "the reason names the silence: {reason}"
        );
    });
}

#[test]
fn a_stream_that_keeps_closing_backs_off_further_each_time() {
    current_thread(Duration::from_secs(20), async {
        let mut follower = Follower::start(
            axum::Router::new().route("/events", axum::routing::get(hello_then_close)),
            Deadlines::FORGE,
        )
        .await;
        let mut attempts = Vec::new();
        for _ in 0..3 {
            attempts.push(follower.offline().await.0);
            follower.retry.notify_one();
        }
        assert_eq!(attempts, [1, 2, 3]);
    });
}

#[test]
fn a_refusal_whose_body_never_arrives_still_goes_offline() {
    current_thread(Duration::from_secs(20), async {
        let mut follower = Follower::start(
            axum::Router::new().route("/events", axum::routing::get(refusal_that_never_finishes)),
            Deadlines {
                open: Duration::from_millis(300),
                ..Deadlines::FORGE
            },
        )
        .await;
        let (_, reason) = follower.offline().await;
        assert!(
            reason.contains("did not open"),
            "the reason names the opening deadline: {reason}"
        );
    });
}

#[test]
fn an_event_larger_than_the_frame_limit_is_refused_instead_of_waiting_forever() {
    assert!(matches!(
        connection::admission_bytes(20 * 1024 * 1024),
        Err(SessionError::FrameTooLarge { .. })
    ));
    assert!(matches!(connection::admission_bytes(1024), Ok(1024)));
}

#[test]
fn only_a_connection_that_was_lost_reports_reconnecting() {
    current_thread(Duration::from_secs(20), async {
        let mut follower = Follower::start(
            axum::Router::new().route("/events", axum::routing::get(hello_then_close)),
            Deadlines::FORGE,
        )
        .await;
        assert_eq!(follower.status().await, ConnectionStatus::Connecting);
        assert_eq!(follower.status().await, ConnectionStatus::Live);
        assert_eq!(follower.offline().await.0, 1);
        follower.retry.notify_one();
        assert_eq!(
            follower.status().await,
            ConnectionStatus::Reconnecting { attempt: 1 },
            "a dropped stream reconnects as a reconnection"
        );
    });
}

#[test]
fn opening_another_thread_after_a_recovered_drop_does_not_report_a_lost_connection() {
    current_thread(Duration::from_secs(20), async {
        let opened = Arc::new(AtomicUsize::new(0));
        let mut follower = Follower::start(
            axum::Router::new().route(
                "/events",
                axum::routing::get(move || {
                    let first = opened.fetch_add(1, Ordering::SeqCst) == 0;
                    async move {
                        if first {
                            hello_then_close().await
                        } else {
                            hello_then_silence().await
                        }
                    }
                }),
            ),
            Deadlines::FORGE,
        )
        .await;
        assert_eq!(follower.status().await, ConnectionStatus::Connecting);
        assert_eq!(follower.status().await, ConnectionStatus::Live);
        assert_eq!(follower.offline().await.0, 1);
        follower.retry.notify_one();
        assert_eq!(
            follower.status().await,
            ConnectionStatus::Reconnecting { attempt: 1 }
        );
        assert_eq!(follower.status().await, ConnectionStatus::Live);
        follower
            .observe
            .send(Some(artisan_api::ThreadId::from("thread-b")))
            .expect("the connection observes the thread");
        assert_eq!(
            follower.status().await,
            ConnectionStatus::Live,
            "following another thread on a healthy stream is not a reconnection"
        );
    });
}
