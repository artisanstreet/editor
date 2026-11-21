use super::*;
use axum::{
    Json, Router,
    body::{Body, Bytes},
    http::StatusCode as Code,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use test_support::{http::StubServer, wait::within};

#[derive(Clone, Default)]
struct Base(Arc<OnceLock<String>>);

impl Base {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.0.get().expect("stub address is known"))
    }
}

async fn serve(base: &Base, router: Router) -> StubServer {
    let stub = StubServer::serve(router).await;
    base.0
        .set(stub.url().to_owned())
        .expect("stub address is set once");
    stub
}

fn json_route(
    answer: impl Fn(&Base) -> (Code, Value) + Clone + Send + Sync + 'static,
    base: &Base,
) -> axum::routing::MethodRouter {
    let base = base.clone();
    get(move || {
        let (code, body) = answer(&base);
        async move { (code, Json(body)) }
    })
}

fn issuer_metadata(base: &Base) -> Value {
    serde_json::json!({
        "issuer": base.url(""),
        "authorization_endpoint": base.url("/authorize"),
        "token_endpoint": base.url("/token"),
        "registration_endpoint": base.url("/register"),
        "code_challenge_methods_supported": ["S256"],
    })
}

fn resource_metadata(base: &Base) -> Value {
    serde_json::json!({
        "resource": base.url("/mcp"),
        "authorization_servers": [base.url("")],
    })
}

fn signing_in_server(
    base: &Base,
    resource: impl Fn(&Base) -> (Code, Value) + Clone + Send + Sync + 'static,
    issuer: impl Fn(&Base) -> (Code, Value) + Clone + Send + Sync + 'static,
) -> Router {
    Router::new()
        .route(
            "/mcp",
            post(|| async { (Code::UNAUTHORIZED, [("www-authenticate", "Bearer")]) }),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            json_route(resource, base),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            json_route(issuer, base),
        )
}

async fn probe_stub(base: &Base) -> Result<Probe, ProbeError> {
    probe(&Http::new(), "linear", &base.url("/mcp"), &[]).await
}

#[tokio::test]
async fn a_server_that_answers_without_a_token_is_open() {
    let base = Base::default();
    let _stub = serve(&base, Router::new().route("/mcp", post(|| async { "{}" }))).await;
    assert_eq!(probe_stub(&base).await.expect("probe"), Probe::Open);
}

#[tokio::test]
async fn an_unavailable_server_is_not_mistaken_for_one_without_sign_in() {
    for status in [403_u16, 404, 500, 503] {
        let base = Base::default();
        let _stub = serve(
            &base,
            Router::new().route(
                "/mcp",
                post(move || async move { Code::from_u16(status).expect("status") }),
            ),
        )
        .await;
        let probed = probe_stub(&base).await;
        assert!(
            matches!(probed, Err(ProbeError::Unavailable { status: answered, .. }) if answered == status),
            "HTTP {status} was taken as {probed:?}"
        );
    }
}

#[tokio::test]
async fn a_sign_in_is_found_through_the_resource_metadata() {
    let base = Base::default();
    let _stub = serve(
        &base,
        signing_in_server(
            &base,
            |base| (Code::OK, resource_metadata(base)),
            |base| (Code::OK, issuer_metadata(base)),
        ),
    )
    .await;
    let Probe::SignIn(discovery) = probe_stub(&base).await.expect("probe") else {
        panic!("a 401 with OAuth metadata offers a sign-in");
    };
    assert_eq!(discovery.issuer.as_str(), base.url("/"));
    assert_eq!(discovery.resource.as_str(), base.url("/mcp"));
    assert_eq!(discovery.token_endpoint.as_str(), base.url("/token"));
}

#[tokio::test]
async fn a_server_without_oauth_metadata_needs_a_token() {
    let base = Base::default();
    let _stub = serve(
        &base,
        signing_in_server(
            &base,
            |_| (Code::NOT_FOUND, Value::Null),
            |_| (Code::NOT_FOUND, Value::Null),
        ),
    )
    .await;
    assert_eq!(probe_stub(&base).await.expect("probe"), Probe::NeedsToken);
}

#[tokio::test]
async fn failing_metadata_is_an_error_rather_than_no_sign_in() {
    let base = Base::default();
    let _stub = serve(
        &base,
        signing_in_server(
            &base,
            |_| (Code::SERVICE_UNAVAILABLE, Value::Null),
            |base| (Code::OK, issuer_metadata(base)),
        ),
    )
    .await;
    let probed = probe_stub(&base).await;
    assert!(
        matches!(
            probed,
            Err(ProbeError::Discovery(DiscoveryError::Fetch {
                source: ResponseError::Status { status: 503, .. },
                ..
            }))
        ),
        "an outage of the metadata was taken as {probed:?}"
    );
}

#[tokio::test]
async fn plain_http_endpoints_are_refused() {
    let base = Base::default();
    let _stub = serve(
        &base,
        signing_in_server(
            &base,
            |base| (Code::OK, resource_metadata(base)),
            |base| {
                let mut metadata = issuer_metadata(base);
                metadata["token_endpoint"] = "http://auth.example.com/token".into();
                (Code::OK, metadata)
            },
        ),
    )
    .await;
    let probed = probe_stub(&base).await;
    assert!(
        matches!(
            probed,
            Err(ProbeError::Discovery(DiscoveryError::Endpoint {
                field: "token_endpoint",
                source: InsecureUrl::NotHttps { .. },
                ..
            }))
        ),
        "a plain-HTTP token endpoint was accepted: {probed:?}"
    );
}

#[tokio::test]
async fn metadata_claiming_another_issuer_is_refused() {
    let base = Base::default();
    let _stub = serve(
        &base,
        signing_in_server(
            &base,
            |base| (Code::OK, resource_metadata(base)),
            |base| {
                let mut metadata = issuer_metadata(base);
                metadata["issuer"] = "https://attacker.example".into();
                (Code::OK, metadata)
            },
        ),
    )
    .await;
    let probed = probe_stub(&base).await;
    assert!(
        matches!(
            probed,
            Err(ProbeError::Discovery(DiscoveryError::IssuerMismatch { .. }))
        ),
        "metadata for another issuer was accepted: {probed:?}"
    );
}

#[tokio::test]
async fn metadata_for_another_resource_is_refused() {
    let base = Base::default();
    let _stub = serve(
        &base,
        signing_in_server(
            &base,
            |base| {
                let mut metadata = resource_metadata(base);
                metadata["resource"] = "https://other.example/mcp".into();
                (Code::OK, metadata)
            },
            |base| (Code::OK, issuer_metadata(base)),
        ),
    )
    .await;
    let probed = probe_stub(&base).await;
    assert!(
        matches!(
            probed,
            Err(ProbeError::Discovery(
                DiscoveryError::ForeignResource { .. }
            ))
        ),
        "tokens would be bound to another resource: {probed:?}"
    );
}

fn endless() -> Response {
    Body::from_stream(futures::stream::repeat_with(|| {
        Ok::<_, std::io::Error>(Bytes::from(vec![b' '; 64 * 1024]))
    }))
    .into_response()
}

#[tokio::test]
async fn an_endless_metadata_answer_stops_at_the_limit() {
    let base = Base::default();
    let _stub = serve(
        &base,
        Router::new()
            .route(
                "/mcp",
                post(|| async { (Code::UNAUTHORIZED, [("www-authenticate", "Bearer")]) }),
            )
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                get(|| async { endless() }),
            ),
    )
    .await;
    let probed = within(
        "an endless metadata answer",
        Duration::from_secs(10),
        probe_stub(&base),
    )
    .await;
    assert!(
        matches!(
            probed,
            Err(ProbeError::Discovery(DiscoveryError::Fetch {
                source: ResponseError::TooLarge { .. },
                ..
            }))
        ),
        "an endless metadata answer gave {probed:?}"
    );
}

fn discovery_at(base: &Base) -> Discovery {
    let secure = |path: &str| SecureUrl::try_from(base.url(path)).expect("loopback is secure");
    Discovery {
        issuer: secure("/"),
        resource: secure("/mcp"),
        authorization_endpoint: secure("/authorize"),
        token_endpoint: secure("/token"),
        registration_endpoint: Some(secure("/register")),
        scope: None,
    }
}

fn client_for(discovery: &Discovery) -> Client {
    Client {
        id: "abc".into(),
        secret: None,
        redirect_uri: "http://127.0.0.1:33418/callback".into(),
        issuer: discovery.issuer.clone(),
        resource: discovery.resource.clone(),
    }
}

async fn token_stub(
    answer: impl Fn() -> Response + Clone + Send + Sync + 'static,
) -> (Base, StubServer) {
    let base = Base::default();
    let stub = serve(
        &base,
        Router::new().route(
            "/token",
            post(move || {
                let answer = answer.clone();
                async move { answer() }
            }),
        ),
    )
    .await;
    (base, stub)
}

async fn refreshed(base: &Base) -> Result<Tokens, TokenError> {
    let discovery = discovery_at(base);
    refresh(
        &Http::new(),
        "linear",
        &discovery,
        &client_for(&discovery),
        &Secret::from("refresh-1"),
        1_000,
    )
    .await
}

#[tokio::test]
async fn an_endless_token_answer_stops_at_the_limit() {
    let (base, _stub) = token_stub(endless).await;
    let answer = within(
        "an endless token answer",
        Duration::from_secs(10),
        refreshed(&base),
    )
    .await;
    assert!(
        matches!(
            answer,
            Err(TokenError::Request {
                source: ResponseError::TooLarge { .. },
                ..
            })
        ),
        "an endless token answer gave {answer:?}"
    );
}

#[tokio::test]
async fn an_outage_of_the_token_endpoint_is_not_a_revoked_grant() {
    for status in [
        Code::TOO_MANY_REQUESTS,
        Code::SERVICE_UNAVAILABLE,
        Code::INTERNAL_SERVER_ERROR,
    ] {
        let (base, _stub) = token_stub(move || (status, "upstream down").into_response()).await;
        let answer = refreshed(&base).await;
        assert!(
            matches!(answer, Err(TokenError::Request { .. })),
            "HTTP {status} was taken as {answer:?}"
        );
    }
    let (base, _stub) = token_stub(|| {
        (
            Code::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_request" })),
        )
            .into_response()
    })
    .await;
    let answer = refreshed(&base).await;
    assert!(
        matches!(answer, Err(TokenError::Request { .. })),
        "a malformed request was taken as a revocation: {answer:?}"
    );
}

#[tokio::test]
async fn a_refused_grant_is_a_revocation() {
    let (base, _stub) = token_stub(|| {
        (
            Code::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_grant" })),
        )
            .into_response()
    })
    .await;
    let answer = refreshed(&base).await;
    assert!(
        matches!(
            answer,
            Err(TokenError::Revoked {
                revocation: Revocation::InvalidGrant,
                ..
            })
        ),
        "a refused grant gave {answer:?}"
    );
}

async fn refused_with(code: &'static str) -> String {
    let (base, _stub) = token_stub(move || {
        (
            Code::BAD_REQUEST,
            Json(serde_json::json!({ "error": code, "error_description": "token sk-secret" })),
        )
            .into_response()
    })
    .await;
    let answer = refreshed(&base).await;
    let Err(failure @ TokenError::Request { .. }) = answer else {
        panic!("an unknown refusal gave {answer:?}");
    };
    diagnostics::chain(&failure)
}

#[tokio::test]
async fn an_unknown_oauth_code_is_shown_but_never_the_providers_description() {
    let told = refused_with("login_required").await;
    assert!(told.contains("(login_required)"), "{told}");
    assert!(!told.contains("sk-secret"), "{told}");

    let told = refused_with("<script>alert(1)</script>").await;
    assert!(!told.contains("script"), "{told}");
    assert!(told.contains("can't show"), "{told}");
}

async fn token_answer(body: Value) -> Result<Tokens, TokenError> {
    let (base, _stub) = token_stub(move || Json(body.clone()).into_response()).await;
    refreshed(&base).await
}

#[tokio::test]
async fn token_answers_must_carry_a_usable_bearer_token() {
    let empty =
        token_answer(serde_json::json!({ "access_token": "", "token_type": "Bearer" })).await;
    assert!(
        matches!(empty, Err(TokenError::EmptyAccessToken { .. })),
        "an empty access token gave {empty:?}"
    );
    let mac = token_answer(serde_json::json!({ "access_token": "a", "token_type": "mac" })).await;
    assert!(
        matches!(mac, Err(TokenError::UnsupportedTokenType { ref token_type, .. }) if token_type == "mac"),
        "a MAC token gave {mac:?}"
    );
    for expires_in in [
        serde_json::json!(i64::MAX),
        serde_json::json!(-1),
        serde_json::json!(1e30),
        serde_json::json!("soon"),
    ] {
        let answer = token_answer(serde_json::json!({
            "access_token": "a",
            "token_type": "bearer",
            "expires_in": expires_in,
        }))
        .await;
        assert!(
            matches!(answer, Err(TokenError::InvalidExpiry { .. })),
            "expires_in {expires_in} gave {answer:?}"
        );
    }
    let tokens = token_answer(serde_json::json!({
        "access_token": "a",
        "token_type": "Bearer",
        "expires_in": "3600",
    }))
    .await
    .expect("a usable answer");
    assert_eq!(tokens.access_token().expose(), "a");
    assert_eq!(
        tokens.refresh_token().map(Secret::expose),
        Some("refresh-1"),
        "a rotation-less answer keeps the refresh token"
    );
    assert!(!tokens.stale(3_540_000));
    assert!(tokens.stale(3_541_001));
}

#[tokio::test]
async fn a_redirect_to_plain_http_is_not_followed() {
    let (base, _stub) = token_stub(|| {
        (
            Code::TEMPORARY_REDIRECT,
            [("location", "http://auth.example.com/token")],
        )
            .into_response()
    })
    .await;
    let answer = refreshed(&base).await;
    assert!(
        matches!(
            answer,
            Err(TokenError::Request {
                source: ResponseError::InsecureRedirect { .. },
                ..
            })
        ),
        "a downgrade redirect gave {answer:?}"
    );
}

#[tokio::test]
async fn a_token_request_is_not_sent_again_to_another_origin() {
    let received = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let elsewhere = Base::default();
    let counted = Arc::clone(&received);
    let _elsewhere = serve(
        &elsewhere,
        Router::new().route(
            "/token",
            post(move |form: String| {
                let counted = Arc::clone(&counted);
                async move {
                    assert!(form.contains("refresh-1"), "{form}");
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Code::BAD_REQUEST
                }
            }),
        ),
    )
    .await;
    let target = elsewhere.url("/token");
    let (base, _stub) = token_stub(move || {
        (Code::TEMPORARY_REDIRECT, [("location", target.clone())]).into_response()
    })
    .await;
    let answer = refreshed(&base).await;
    assert!(
        matches!(
            answer,
            Err(TokenError::Request {
                source: ResponseError::CrossOriginRedirect { .. },
                ..
            })
        ),
        "a cross-origin redirect gave {answer:?}"
    );
    assert_eq!(
        received.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the refresh token was sent to another origin"
    );
}

#[test]
fn challenge_parameters() {
    let challenge = r#"Bearer error="invalid_token", resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource/mcp", scope="read write""#;
    assert_eq!(
        param(challenge, "resource_metadata").as_deref(),
        Some("https://mcp.example.com/.well-known/oauth-protected-resource/mcp")
    );
    assert_eq!(param(challenge, "scope").as_deref(), Some("read write"));
    assert_eq!(
        param("Bearer realm=x, scope=a", "scope").as_deref(),
        Some("a")
    );
    assert_eq!(param("Bearer xscope=a", "scope"), None);
    assert_eq!(param("Bearer", "scope"), None);
}

#[test]
fn authorize_url_carries_pkce_resource_and_scope() {
    let base = Base::default();
    base.0
        .set("https://auth.example.com".to_owned())
        .expect("base is set once");
    let mut discovery = discovery_at(&base);
    discovery.authorization_endpoint =
        SecureUrl::try_from("https://auth.example.com/authorize?x=1".to_owned()).expect("https");
    discovery.scope = Some("read".into());
    let client = client_for(&discovery);
    let pkce = Pkce::new().expect("random bytes");
    let url = Url::parse(&authorize_url(&discovery, &client, &pkce)).expect("a URL");
    let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(pairs["x"], "1");
    assert_eq!(pairs["response_type"], "code");
    assert_eq!(pairs["client_id"], "abc");
    assert_eq!(pairs["code_challenge_method"], "S256");
    assert_eq!(pairs["code_challenge"], pkce.challenge);
    assert_eq!(pairs["state"], pkce.state);
    assert_eq!(pairs["resource"], "https://auth.example.com/mcp");
    assert_eq!(pairs["scope"], "read");
    assert_eq!(pairs["redirect_uri"], "http://127.0.0.1:33418/callback");
}

#[test]
fn pkce_challenge_matches_the_rfc_7636_vector() {
    assert_eq!(
        s256_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    let pkce = Pkce::new().expect("random bytes");
    assert_eq!(pkce.challenge, s256_challenge(pkce.verifier.expose()));
    assert_eq!(pkce.verifier.expose().len(), 43, "a 32-byte verifier");
    assert_ne!(pkce.state, Pkce::new().expect("random bytes").state);
}
