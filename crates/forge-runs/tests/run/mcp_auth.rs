use super::*;
use artisan_api::{McpAuthStatus, McpServer};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode as Code,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use test_support::{http::StubServer, wait::eventually};
use tokio::sync::Notify;
use url::Url;

const REDIRECT: &str = "http://127.0.0.1:33418/callback";

#[derive(Default)]
struct Gate {
    armed: AtomicBool,
    arrived: Notify,
    release: Notify,
}

impl Gate {
    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    async fn pass(&self) {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.arrived.notify_one();
            self.release.notified().await;
        }
    }
}

#[derive(Default)]
struct Provider {
    base: OnceLock<String>,
    second_issuer: AtomicBool,
    probe_status: AtomicU16,
    probe_gate: Gate,
    token_status: AtomicU16,
    token_failure: Mutex<Value>,
    token_gate: Gate,
    code_expires_in: AtomicU16,
    registrations: Mutex<Vec<String>>,
    refreshes: AtomicUsize,
    rotation: AtomicUsize,
}

impl Provider {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base.get().expect("stub address is known"))
    }

    fn issuer(&self) -> String {
        if self.second_issuer.load(Ordering::SeqCst) {
            self.url("/second")
        } else {
            self.url("")
        }
    }

    fn issuer_metadata(issuer: &str) -> Value {
        serde_json::json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "registration_endpoint": format!("{issuer}/register"),
            "code_challenge_methods_supported": ["S256"],
        })
    }

    fn register(&self, issuer: &str) -> Json<Value> {
        let mut registrations = self.registrations.lock().unwrap();
        registrations.push(issuer.to_owned());
        Json(serde_json::json!({ "client_id": format!("{issuer}#{}", registrations.len()) }))
    }

    fn token(&self, form: &str) -> Response {
        let status = self.token_status.load(Ordering::SeqCst);
        if status != 0 {
            let status = Code::from_u16(status).expect("status");
            let failure = self.token_failure.lock().unwrap().clone();
            return match failure {
                Value::Null => status.into_response(),
                body @ (Value::Bool(_)
                | Value::Number(_)
                | Value::String(_)
                | Value::Array(_)
                | Value::Object(_)) => (status, Json(body)).into_response(),
            };
        }
        let fields: HashMap<String, String> = url::form_urlencoded::parse(form.as_bytes())
            .into_owned()
            .collect();
        let rotation = self.rotation.load(Ordering::SeqCst);
        let expires_in = match fields["grant_type"].as_str() {
            "authorization_code" => self.code_expires_in.load(Ordering::SeqCst),
            "refresh_token" => {
                self.refreshes.fetch_add(1, Ordering::SeqCst);
                if fields["refresh_token"] != format!("r{rotation}") {
                    return (
                        Code::BAD_REQUEST,
                        Json(serde_json::json!({ "error": "invalid_grant" })),
                    )
                        .into_response();
                }
                3600
            }
            other => panic!("unexpected grant {other}"),
        };
        let next = rotation + 1;
        self.rotation.store(next, Ordering::SeqCst);
        Json(serde_json::json!({
            "access_token": format!("a{next}"),
            "token_type": "Bearer",
            "refresh_token": format!("r{next}"),
            "expires_in": expires_in,
        }))
        .into_response()
    }
}

async fn provider() -> (Arc<Provider>, StubServer) {
    let provider = Arc::new(Provider {
        code_expires_in: AtomicU16::new(3600),
        ..Provider::default()
    });
    let router = Router::new()
        .route(
            "/mcp",
            post(|State(provider): State<Arc<Provider>>| async move {
                provider.probe_gate.pass().await;
                match provider.probe_status.load(Ordering::SeqCst) {
                    0 => (Code::UNAUTHORIZED, [("www-authenticate", "Bearer")]).into_response(),
                    status => Code::from_u16(status).expect("status").into_response(),
                }
            }),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(|State(provider): State<Arc<Provider>>| async move {
                Json(serde_json::json!({
                    "resource": provider.url("/mcp"),
                    "authorization_servers": [provider.issuer()],
                }))
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(|State(provider): State<Arc<Provider>>| async move {
                Json(Provider::issuer_metadata(&provider.url("")))
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server/second",
            get(|State(provider): State<Arc<Provider>>| async move {
                Json(Provider::issuer_metadata(&provider.url("/second")))
            }),
        )
        .route(
            "/register",
            post(|State(provider): State<Arc<Provider>>| async move {
                provider.register(&provider.url(""))
            }),
        )
        .route(
            "/second/register",
            post(|State(provider): State<Arc<Provider>>| async move {
                provider.register(&provider.url("/second"))
            }),
        )
        .route(
            "/token",
            post(
                |State(provider): State<Arc<Provider>>, form: String| async move {
                    provider.token_gate.pass().await;
                    provider.token(&form)
                },
            ),
        )
        .with_state(Arc::clone(&provider));
    let stub = StubServer::serve(router).await;
    provider
        .base
        .set(stub.url().to_owned())
        .expect("stub address is set once");
    (provider, stub)
}

async fn mcp_fixture(provider: &Provider) -> Fixture {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    fixture
        .hub
        .store()
        .set_setting(
            "mcp_servers".to_owned(),
            serde_json::json!([{ "name": "linear", "url": provider.url("/mcp") }]),
        )
        .await
        .unwrap();
    fixture
}

async fn sign_in(fixture: &Fixture) {
    let started = fixture
        .hub
        .mcp()
        .sign_in_start("linear", REDIRECT)
        .await
        .unwrap();
    fixture
        .hub
        .mcp()
        .sign_in_finish(&started.state, "code")
        .await
        .unwrap();
}

async fn launch_token(fixture: &Fixture) -> Option<String> {
    let servers: Vec<McpServer> = serde_json::from_value(
        fixture
            .hub
            .store()
            .setting("mcp_servers")
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    fixture
        .hub
        .mcp()
        .with_tokens(servers)
        .await
        .remove(0)
        .headers
        .remove("Authorization")
}

async fn status(fixture: &Fixture) -> Option<McpAuthStatus> {
    fixture.hub.mcp().auth().await.unwrap().remove("linear")
}

fn waiters_on_lifetime(fixture: &Fixture) -> usize {
    fixture.hub.mcp().lifetime_holders("linear")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_that_changes_no_status_writes_nothing() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let store = fixture.hub.store();
    let head = store.head();
    fixture.hub.mcp().check_auth().await;
    assert_eq!(store.head(), head);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_records_a_changed_status_once() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let store = fixture.hub.store();
    store
        .set_setting(
            artisan_api::MCP_AUTH_KEY.to_owned(),
            serde_json::json!({ "removed": "needs_sign_in" }),
        )
        .await
        .unwrap();
    let head = store.head();
    fixture.hub.mcp().check_auth().await;
    assert_eq!(store.head(), head + 1);
    assert!(fixture.hub.mcp().auth().await.unwrap().is_empty());
    fixture.hub.mcp().check_auth().await;
    assert_eq!(store.head(), head + 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refresh_during_an_outage_keeps_the_sign_in() {
    let (provider, _stub) = provider().await;
    let fixture = mcp_fixture(&provider).await;
    provider.code_expires_in.store(0, Ordering::SeqCst);
    sign_in(&fixture).await;
    provider.token_status.store(503, Ordering::SeqCst);
    assert_eq!(launch_token(&fixture).await.as_deref(), Some("Bearer a1"));
    assert_eq!(status(&fixture).await, Some(McpAuthStatus::SignedIn));
    provider.token_status.store(0, Ordering::SeqCst);
    assert_eq!(
        launch_token(&fixture).await.as_deref(),
        Some("Bearer a2"),
        "the refresh token survived the outage"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_refresh_logs_none_of_the_text_the_authorization_server_sent() {
    let (captured, _logging) = test_support::logs::capture();
    let (provider, _stub) = provider().await;
    let fixture = mcp_fixture(&provider).await;
    provider.code_expires_in.store(0, Ordering::SeqCst);
    sign_in(&fixture).await;
    provider.token_status.store(500, Ordering::SeqCst);

    *provider.token_failure.lock().unwrap() = serde_json::json!({
        "error": "temporarily_unavailable",
        "error_description": "retry with client_secret=hunter2",
    });
    assert_eq!(launch_token(&fixture).await.as_deref(), Some("Bearer a1"));
    *provider.token_failure.lock().unwrap() = serde_json::json!({
        "error": "refresh_token r1 was echoed",
    });
    assert_eq!(launch_token(&fixture).await.as_deref(), Some("Bearer a1"));

    let logs = captured.text();
    let failures: Vec<&str> = logs
        .lines()
        .filter(|line| line.contains("refreshing an MCP token"))
        .collect();
    assert_eq!(failures.len(), 2, "{logs}");
    assert!(
        failures[0].contains("HTTP 500 (temporarily_unavailable)"),
        "{logs}"
    );
    assert!(failures[1].contains("HTTP 500"), "{logs}");
    for echoed in ["hunter2", "r1 was echoed"] {
        assert!(
            !logs.contains(echoed),
            "the log holds {echoed:?} from the authorization server: {logs}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_refresh_signs_the_server_out() {
    let (provider, _stub) = provider().await;
    let fixture = mcp_fixture(&provider).await;
    provider.code_expires_in.store(0, Ordering::SeqCst);
    sign_in(&fixture).await;
    provider.rotation.store(7, Ordering::SeqCst);
    assert_eq!(launch_token(&fixture).await, None);
    assert_eq!(status(&fixture).await, Some(McpAuthStatus::NeedsSignIn));
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_launches_refresh_a_rotating_token_once() {
    let (provider, _stub) = provider().await;
    let fixture = mcp_fixture(&provider).await;
    provider.code_expires_in.store(0, Ordering::SeqCst);
    sign_in(&fixture).await;
    let (first, second) = tokio::join!(launch_token(&fixture), launch_token(&fixture));
    assert_eq!(first.as_deref(), Some("Bearer a2"));
    assert_eq!(second.as_deref(), Some("Bearer a2"));
    assert_eq!(provider.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(status(&fixture).await, Some(McpAuthStatus::SignedIn));
    assert_eq!(launch_token(&fixture).await.as_deref(), Some("Bearer a2"));
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_cancels_a_waiting_sign_in() {
    let (provider, _stub) = provider().await;
    let fixture = mcp_fixture(&provider).await;
    let started = fixture
        .hub
        .mcp()
        .sign_in_start("linear", REDIRECT)
        .await
        .unwrap();
    fixture.hub.mcp().sign_out("linear").await.unwrap();
    let finished = fixture
        .hub
        .mcp()
        .sign_in_finish(&started.state, "code")
        .await;
    assert!(
        matches!(finished, Err(McpSignInFinishError::Expired)),
        "a sign-in started before signing out finished: {finished:?}"
    );
    assert_eq!(launch_token(&fixture).await, None);
    assert_eq!(status(&fixture).await, Some(McpAuthStatus::NeedsSignIn));
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_during_a_token_exchange_stays_signed_out() {
    let (provider, _stub) = provider().await;
    let fixture = Arc::new(mcp_fixture(&provider).await);
    let started = fixture
        .hub
        .mcp()
        .sign_in_start("linear", REDIRECT)
        .await
        .unwrap();
    provider.token_gate.arm();
    let finishing = tokio::spawn({
        let fixture = Arc::clone(&fixture);
        async move {
            fixture
                .hub
                .mcp()
                .sign_in_finish(&started.state, "code")
                .await
        }
    });
    provider.token_gate.arrived.notified().await;
    let signing_out = tokio::spawn({
        let fixture = Arc::clone(&fixture);
        async move { fixture.hub.mcp().sign_out("linear").await }
    });
    eventually(
        "sign-out to wait for the exchange",
        test_support::wait::LIVENESS,
        || {
            let ready = signing_out.is_finished() || waiters_on_lifetime(&fixture) == 3;
            async move { ready }
        },
    )
    .await;
    provider.token_gate.release.notify_one();
    drop(finishing.await.unwrap());
    signing_out.await.unwrap().unwrap();
    assert_eq!(launch_token(&fixture).await, None);
    assert_eq!(status(&fixture).await, Some(McpAuthStatus::NeedsSignIn));
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_during_a_refresh_stays_signed_out() {
    let (provider, _stub) = provider().await;
    let fixture = Arc::new(mcp_fixture(&provider).await);
    provider.code_expires_in.store(0, Ordering::SeqCst);
    sign_in(&fixture).await;
    provider.token_gate.arm();
    let launching = tokio::spawn({
        let fixture = Arc::clone(&fixture);
        async move { launch_token(&fixture).await }
    });
    provider.token_gate.arrived.notified().await;
    let signing_out = tokio::spawn({
        let fixture = Arc::clone(&fixture);
        async move { fixture.hub.mcp().sign_out("linear").await }
    });
    eventually(
        "sign-out to wait for the refresh",
        test_support::wait::LIVENESS,
        || {
            let ready = signing_out.is_finished() || waiters_on_lifetime(&fixture) == 3;
            async move { ready }
        },
    )
    .await;
    provider.token_gate.release.notify_one();
    drop(launching.await.unwrap());
    signing_out.await.unwrap().unwrap();
    assert_eq!(launch_token(&fixture).await, None);
    assert_eq!(status(&fixture).await, Some(McpAuthStatus::NeedsSignIn));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_registration_is_not_reused_with_another_authorization_server() {
    let (provider, _stub) = provider().await;
    let fixture = mcp_fixture(&provider).await;
    sign_in(&fixture).await;
    provider.second_issuer.store(true, Ordering::SeqCst);
    let started = fixture
        .hub
        .mcp()
        .sign_in_start("linear", REDIRECT)
        .await
        .unwrap();
    let second = provider.url("/second");
    assert_eq!(
        *provider.registrations.lock().unwrap(),
        [provider.url(""), second.clone()]
    );
    let client_id = Url::parse(&started.authorize_url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "client_id")
        .map(|(_, value)| value.into_owned());
    assert_eq!(client_id, Some(format!("{second}#2")));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unavailable_server_keeps_its_sign_in_state() {
    let (provider, _stub) = provider().await;
    let fixture = mcp_fixture(&provider).await;
    fixture.hub.mcp().check_auth().await;
    assert_eq!(status(&fixture).await, Some(McpAuthStatus::NeedsSignIn));
    for unavailable in [403, 404, 503] {
        provider.probe_status.store(unavailable, Ordering::SeqCst);
        fixture.hub.mcp().check_auth().await;
        assert_eq!(
            status(&fixture).await,
            Some(McpAuthStatus::NeedsSignIn),
            "HTTP {unavailable} dropped the sign-in requirement"
        );
        let started = fixture.hub.mcp().sign_in_start("linear", REDIRECT).await;
        assert!(
            matches!(started, Err(McpSignInStartError::Probe { .. })),
            "HTTP {unavailable} started as {started:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_does_not_overwrite_a_sign_in_that_finished_meanwhile() {
    let (provider, _stub) = provider().await;
    let fixture = Arc::new(mcp_fixture(&provider).await);
    provider.probe_gate.arm();
    let checking = tokio::spawn({
        let fixture = Arc::clone(&fixture);
        async move { fixture.hub.mcp().check_auth().await }
    });
    provider.probe_gate.arrived.notified().await;
    sign_in(&fixture).await;
    assert_eq!(status(&fixture).await, Some(McpAuthStatus::SignedIn));
    provider.probe_gate.release.notify_one();
    checking.await.unwrap();
    assert_eq!(status(&fixture).await, Some(McpAuthStatus::SignedIn));
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_sign_in_starts_share_one_flow() {
    let (provider, _stub) = provider().await;
    let fixture = mcp_fixture(&provider).await;
    let (first, second) = tokio::join!(
        fixture.hub.mcp().sign_in_start("linear", REDIRECT),
        fixture.hub.mcp().sign_in_start("linear", REDIRECT)
    );
    assert_eq!(first.unwrap().state, second.unwrap().state);
    assert_eq!(provider.registrations.lock().unwrap().len(), 1);
}
