use artisan_api::{Envelope, Event, ItemKind, ItemsQuery, ThreadId};
use axum::body::Body;
use axum::http::header;
use axum::response::Response;
use axum::routing::get;
use editor_core::connection::{self, ConnectionStatus, Deadlines, Update};
use futures::StreamExt as _;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use test_support::editor::forge::TlsServer;
use test_support::wait::within;

const THREAD: &str = "01a1f000-0000-7000-8000-000000000002";
const STATE: &str = include_str!("released_forge/state.json");
const THREAD_BODY: &str = include_str!("released_forge/thread.json");
const TRANSCRIPT: &str = include_str!("released_forge/transcript.json");
const DRAFT: &str = include_str!("released_forge/draft.json");
const EVENTS: &str = include_str!("released_forge/events.jsonl");
const FOLLOWED_WITHIN: Duration = Duration::from_secs(10);

fn json(body: &'static str) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("build a recorded answer")
}

async fn events() -> Response {
    let frames = EVENTS
        .lines()
        .map(|line| Ok::<_, Infallible>(format!("data: {line}\n\n")))
        .collect::<Vec<_>>();
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(
            futures::stream::iter(frames).chain(futures::stream::pending()),
        ))
        .expect("build the recorded event stream")
}

async fn released_forge() -> TlsServer {
    TlsServer::serve(
        axum::Router::new()
            .route("/state", get(|| async { json(STATE) }))
            .route("/events", get(events))
            .route(
                &format!("/threads/{THREAD}"),
                get(|| async { json(THREAD_BODY) }),
            )
            .route(
                &format!("/threads/{THREAD}/transcript"),
                get(|| async { json(TRANSCRIPT) }),
            )
            .route(&format!("/drafts/{THREAD}"), get(|| async { json(DRAFT) })),
    )
    .await
}

fn said(kind: &ItemKind) -> Option<String> {
    match kind {
        ItemKind::UserMessage { text, .. } => text.inline().map(|text| format!("you: {text}")),
        ItemKind::AssistantMessage { text, .. } => {
            text.inline().map(|text| format!("reply: {text}"))
        }
        ItemKind::Tool { .. }
        | ItemKind::Agent { .. }
        | ItemKind::Question { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn todays_editor_reads_a_thread_from_the_last_released_forge() {
    let forge = released_forge().await;
    let client = forge.client();
    let thread = ThreadId(THREAD.to_owned());

    let state = client
        .state()
        .await
        .expect("the Editor accepts the released Forge's protocol and state");
    assert!(
        state.threads.iter().any(|listed| listed.id == thread),
        "the released thread is in the sidebar state"
    );
    let opened = client
        .thread(&thread)
        .await
        .expect("open the released thread");
    assert_eq!(opened.title.as_deref(), Some("Released thread"));
    let page = client
        .transcript(
            &thread,
            &ItemsQuery {
                before: None,
                after: None,
                limit: Some(100),
            },
        )
        .await
        .expect("read the released transcript");
    let shown: Vec<String> = page
        .items
        .iter()
        .filter_map(|item| said(&item.kind))
        .collect();
    assert_eq!(
        shown,
        [
            "you: List the files",
            "reply: Answered: List the files",
            "you: Summarise the crate",
            "reply: Answered: Summarise the crate",
        ]
    );
    let draft = client.draft(THREAD).await.expect("read the released draft");
    assert_eq!(draft.body, serde_json::json!({"text": "Unsent words"}));

    let (sender, mut updates) = tokio::sync::mpsc::channel(64);
    let (_observe, observed) = tokio::sync::watch::channel(None);
    let following = tokio::spawn(connection::run(
        forge.client(),
        sender,
        Arc::new(tokio::sync::Notify::new()),
        observed,
        Deadlines::FORGE,
    ));
    let streamed = within(
        "the released event stream to reach the replies",
        FOLLOWED_WITHIN,
        async {
            let mut streamed = Vec::new();
            let mut live = false;
            while !(live && streamed.len() == shown.len()) {
                match updates.recv().await.expect("the connection keeps running") {
                    Update::Status(ConnectionStatus::Offline { reason, .. }) => {
                        panic!("the Editor went offline on the released Forge: {reason}")
                    }
                    Update::Status(status) => live = status == ConnectionStatus::Live,
                    Update::Event(envelope, _permit) => {
                        let Envelope { event, .. } = *envelope;
                        if let Event::Item { item } = event
                            && item.thread_id == thread
                            && let Some(line) = said(&item.kind)
                        {
                            streamed.push(line);
                        }
                    }
                    Update::State(_) => {}
                }
            }
            streamed
        },
    )
    .await;
    following.abort();
    assert_eq!(
        streamed, shown,
        "the stream shows what the transcript shows"
    );
}
