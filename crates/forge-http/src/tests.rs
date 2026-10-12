//! A client's view over real mutual TLS: pairing, auth, sending, and the
//! event stream resuming after a seq.

use super::*;
use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, Envelope, Event, ItemId, PairRequest,
    ProjectId, RunStatus, SendMessage, State, ThreadId,
};
use std::time::Duration;

struct Device {
    identity: Identity,
    http: reqwest::Client,
    base: String,
}

fn device(forge_certificate: &str, address: SocketAddr) -> Device {
    let identity = Identity::generate("artisan-editor").unwrap();
    let config =
        tls::client_config(&identity, &tls::fingerprint_of(forge_certificate).unwrap()).unwrap();
    let http = reqwest::Client::builder()
        .use_preconfigured_tls(config)
        .build()
        .unwrap();
    Device {
        identity,
        http,
        base: format!("https://localhost:{}", address.port()),
    }
}

impl Device {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
}

async fn forge() -> (tempfile::TempDir, SocketAddr, Store) {
    forge_reaching(false).await
}

/// A Forge whose link titles may (`true`) come from local pages.
async fn forge_reaching(private_links: bool) -> (tempfile::TempDir, SocketAddr, Store) {
    let dir = tempfile::tempdir().unwrap();
    let server = start(ServeConfig {
        data_dir: dir.path().join("forge"),
        listen: "127.0.0.1:0".parse().unwrap(),
    })
    .await
    .unwrap();
    if private_links {
        server.reach_private_links();
    }
    let address = server.local_addr().unwrap();
    let store = server.store().clone();
    tokio::spawn(server.run());
    (dir, address, store)
}

/// Reads SSE envelopes until `stop` says so.
async fn read_events(
    response: reqwest::Response,
    stop: impl Fn(&Envelope) -> bool,
) -> Vec<Envelope> {
    let mut response = response;
    let mut buffer = String::new();
    let mut envelopes = Vec::new();
    let read = async {
        while let Some(chunk) = response.chunk().await.unwrap() {
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(end) = buffer.find("\n\n") {
                let block: String = buffer.drain(..end + 2).collect();
                let data: String = block
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(str::trim)
                    .collect();
                if data.is_empty() {
                    continue;
                }
                let envelope: Envelope = serde_json::from_str(&data).unwrap();
                let done = stop(&envelope);
                envelopes.push(envelope);
                if done {
                    return;
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .expect("events arrive");
    envelopes
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paired_device_sends_a_message_and_follows_the_run() {
    let (_dir, address, store) = forge().await;
    let invite = invite(&store).await.unwrap();
    let editor = device(&invite.forge_certificate, address);

    // Not paired yet.
    let refused = editor.http.get(editor.url("/state")).send().await.unwrap();
    assert_eq!(refused.status(), 401);

    let pair = PairRequest {
        code: invite.code.to_lowercase(),
        device_name: "laptop".into(),
        public_key: editor.identity.certificate.clone(),
    };
    let paired = editor
        .http
        .post(editor.url("/pair"))
        .json(&pair)
        .send()
        .await
        .unwrap();
    assert_eq!(paired.status(), 200);
    let again = editor
        .http
        .post(editor.url("/pair"))
        .json(&pair)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 401, "a code works once");

    let body = editor
        .http
        .get(editor.url("/state"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let forge_key = crate::identity(&store).await.unwrap().key;
    assert!(
        !body.contains(&forge_key),
        "Forge's private key never reaches a client"
    );
    let state: State = serde_json::from_str(&body).unwrap();
    let since = state.head;

    let project = CreateProject {
        id: ProjectId::new(),
        path: std::env::temp_dir().display().to_string(),
        name: None,
    };
    assert!(
        editor
            .http
            .post(editor.url("/projects"))
            .json(&project)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let thread = CreateThread {
        id: ThreadId::new(),
        project_id: project.id.clone(),
        config: EngineConfig::new(EngineId::Claude),
        title: None,
        parent_thread_id: None,
    };
    assert!(
        editor
            .http
            .post(editor.url("/threads"))
            .json(&thread)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let message = SendMessage {
        id: ItemId::new(),
        text: "hello".into(),
        images: Vec::new(),
        config: None,
    };
    let sent = editor
        .http
        .post(editor.url(&format!("/threads/{}/messages", thread.id)))
        .json(&message)
        .send()
        .await
        .unwrap();
    assert!(sent.status().is_success());

    // Claude Code isn't installed in this Forge, so the run fails — and says why.
    let stream = editor
        .http
        .get(editor.url(&format!("/events?since={since}")))
        .send()
        .await
        .unwrap();
    let events = read_events(
        stream,
        |envelope| matches!(&envelope.event, Event::Run { run } if run.status == RunStatus::Failed),
    )
    .await;
    assert!(matches!(events[0].event, Event::Hello { .. }));
    let failed = events.iter().find_map(|envelope| match &envelope.event {
        Event::Run { run } if run.status == RunStatus::Failed => run.error.clone(),
        _ => None,
    });
    assert_eq!(failed.as_deref(), Some("Claude Code is not installed"));
    let seqs: Vec<_> = events.iter().filter_map(|envelope| envelope.seq).collect();
    assert!(
        seqs.windows(2).all(|pair| pair[0] < pair[1]),
        "stored changes arrive in seq order"
    );
    assert!(seqs.first().is_some_and(|first| *first > since));

    // Resuming after the last seq replays nothing old.
    let last = *seqs.last().unwrap();
    let stream = editor
        .http
        .get(editor.url(&format!("/events?since={last}")))
        .send()
        .await
        .unwrap();
    let events = read_events(stream, |envelope| {
        matches!(envelope.event, Event::Engine { .. })
    })
    .await;
    assert!(events.iter().all(|envelope| envelope.seq.is_none()));
}

/// A paired device, as `a_paired_device_sends_a_message_and_follows_the_run`
/// pairs one.
async fn paired(store: &Store, address: SocketAddr) -> Device {
    let invite = invite(store).await.unwrap();
    let editor = device(&invite.forge_certificate, address);
    let pair = PairRequest {
        code: invite.code,
        device_name: "laptop".into(),
        public_key: editor.identity.certificate.clone(),
    };
    let paired = editor
        .http
        .post(editor.url("/pair"))
        .json(&pair)
        .send()
        .await
        .unwrap();
    assert_eq!(paired.status(), 200);
    editor
}

/// A web server with a few pages, on plain HTTP.
async fn website() -> SocketAddr {
    use axum::response::{Html, Redirect};
    let site = axum::Router::new()
        .route(
            "/post",
            axum::routing::get(|| async {
                Html(
                    "<!doctype html><html><head>\
                     <title>Fallback</title>\
                     <meta property=\"og:title\" content=\"Rust &amp; Friends &#8212; Blog\">\
                     </head><body><h1>Hi</h1></body></html>",
                )
            }),
        )
        .route(
            "/book",
            axum::routing::get(|| async {
                Html("<html><head><!-- <title>Not this</title> -->\n<TITLE>\n  The   Book &mdash; Rust\n</TITLE></head></html>")
            }),
        )
        .route(
            "/moved",
            axum::routing::get(|| async { Redirect::temporary("/book") }),
        )
        .route(
            "/data.json",
            axum::routing::get(|| async { axum::Json(serde_json::json!({"title": "no"})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, site).await });
    address
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_reads_as_its_page_title() {
    let (_dir, address, store) = forge_reaching(true).await;
    let editor = paired(&store, address).await;
    let site = website().await;
    let title = |url: String| {
        let request = editor
            .http
            .get(editor.url("/links/title"))
            .query(&[("url", url)]);
        async move { request.send().await.unwrap() }
    };
    let read = |response: reqwest::Response| async move {
        assert_eq!(response.status(), 200);
        response
            .json::<artisan_api::LinkTitle>()
            .await
            .unwrap()
            .title
    };

    let post = format!("http://{site}/post#comments");
    assert_eq!(
        read(title(post).await).await.as_deref(),
        Some("Rust & Friends \u{2014} Blog"),
        "og:title wins over <title>, entities decoded"
    );
    assert_eq!(
        read(title(format!("http://{site}/moved")).await)
            .await
            .as_deref(),
        Some("The Book \u{2014} Rust"),
        "redirects are followed; whitespace collapses"
    );
    assert_eq!(
        read(title(format!("http://{site}/data.json")).await).await,
        None,
        "a page that isn't HTML keeps the link's own label"
    );
    assert_eq!(
        read(title(format!("http://{site}/missing")).await).await,
        None
    );

    for refused in ["file:///etc/passwd", "ftp://example.com/", "not a url"] {
        assert_eq!(
            title(refused.to_owned()).await.status(),
            400,
            "{refused} is refused"
        );
    }

    // A real Forge never reads a page on its own machine or network, even
    // through a redirect.
    let (_guarded_dir, guarded_address, guarded_store) = forge().await;
    let guarded = paired(&guarded_store, guarded_address).await;
    for private in [
        format!("http://{site}/post"),
        format!("http://localhost:{}/post", site.port()),
        "http://[::1]/".to_owned(),
        "http://10.0.0.1/".to_owned(),
        "http://169.254.169.254/latest/meta-data/".to_owned(),
    ] {
        let response = guarded
            .http
            .get(guarded.url("/links/title"))
            .query(&[("url", &private)])
            .send()
            .await
            .unwrap();
        assert_eq!(read(response).await, None, "{private} is not fetched");
    }
}
