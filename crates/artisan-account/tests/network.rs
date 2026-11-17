#![cfg(test)]

use artisan_account::network::{AccountProfile, CoreSessionClient, SessionToken};
use axum::http::{HeaderMap, StatusCode, Uri, header::AUTHORIZATION};
use axum::routing::get;
use std::sync::{Arc, Mutex};
use test_support::http::StubServer;

#[derive(Clone)]
struct Core {
    details: (StatusCode, String),
    requests: Arc<Mutex<Vec<(String, bool)>>>,
}

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get(AUTHORIZATION)
        .is_some_and(|value| value.as_bytes() == b"Bearer session")
}

async fn read_profile(details_status: u16, details: &str) -> AccountProfile {
    let core = Core {
        details: (
            StatusCode::from_u16(details_status).unwrap(),
            details.to_owned(),
        ),
        requests: Arc::default(),
    };
    let record = |core: &Core, uri: &Uri, headers: &HeaderMap| {
        core.requests
            .lock()
            .unwrap()
            .push((uri.path().to_owned(), authorized(headers)));
    };
    let router = axum::Router::new()
        .route(
            "/v1/me",
            get(
                move |axum::extract::State(core): axum::extract::State<Core>,
                      uri: Uri,
                      headers: HeaderMap| async move {
                    record(&core, &uri, &headers);
                    serde_json::json!({"account_id":"account", "email":"test@example.invalid", "display_name":"Sander", "totp_enrolled":false}).to_string()
                },
            ),
        )
        .route(
            "/v1/account/profile",
            get(
                move |axum::extract::State(core): axum::extract::State<Core>,
                      uri: Uri,
                      headers: HeaderMap| async move {
                    record(&core, &uri, &headers);
                    core.details
                },
            ),
        )
        .with_state(core.clone());
    let server = StubServer::serve(router).await;
    let profile = CoreSessionClient::new(server.url())
        .unwrap()
        .me(&SessionToken::new("session".into()))
        .await
        .unwrap();
    let requests = core.requests.lock().unwrap().clone();
    assert!(
        requests.iter().any(|(path, _)| path == "/v1/me"),
        "the account is read from /v1/me: {requests:?}"
    );
    assert!(
        requests.iter().all(|(_, authorized)| *authorized),
        "every request carries the session token: {requests:?}"
    );
    profile
}

#[tokio::test]
async fn account_reads_the_username_from_the_profile_endpoint() {
    let profile = read_profile(
        200,
        r#"{"username":"ss","display_name":"Sander","avatar_url":null,"phone":null}"#,
    )
    .await;
    assert_eq!(profile.username.as_deref(), Some("ss"));
    let cached = serde_json::to_value(&profile).unwrap();
    assert_eq!(cached["username"], "ss");
}

#[tokio::test]
async fn an_unavailable_profile_does_not_discard_a_valid_account() {
    let profile = read_profile(404, "{}").await;
    assert_eq!(profile.account_id, "account");
    assert_eq!(profile.username, None);
}
