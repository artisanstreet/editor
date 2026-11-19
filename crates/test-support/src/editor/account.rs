use super::forge::HeldRequest;
use artisan_account::network::{
    AccountProfile, CoreSessionClient, Jwk, MachineEntry, SessionToken,
};
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use editor_core::client::RelayClientError;
use editor_core::{AeError, DialRelay, ForgeClient, RelayDialRequest, RunAe};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

struct Session {
    profile: AccountProfile,
    clients: HashMap<String, ForgeClient>,
    sync: SyncReply,
}

#[derive(Clone)]
enum SyncReply {
    Answer,
    Fail(StatusCode),
    Hold(std::sync::Arc<std::sync::mpsc::SyncSender<HeldRequest>>),
}

struct Accounts {
    url: String,
    sessions: Mutex<HashMap<String, Session>>,
}

static ACCOUNTS: LazyLock<Accounts> = LazyLock::new(|| {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("bind the stub account service");
    listener
        .set_nonblocking(true)
        .expect("make the stub account service's socket non-blocking");
    let url = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("read the stub account service's address")
    );
    let router = axum::Router::new()
        .route("/v1/me", get(me))
        .route("/v1/grants/issue", post(grant))
        .route("/v1/auth/magic/request", post(magic_request))
        .route("/v1/auth/magic/status", post(magic_status))
        .route("/v1/auth/magic/finalize", post(magic_finalize));
    editor_core::runtime::runtime().spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener)
            .expect("hand the stub account socket to tokio");
        axum::serve(listener, router)
            .await
            .expect("the stub account service stopped serving");
    });
    Accounts {
        url,
        sessions: Mutex::new(HashMap::new()),
    }
});

static NEXT_ACCOUNT: AtomicU64 = AtomicU64::new(1);

fn sessions() -> MutexGuard<'static, HashMap<String, Session>> {
    ACCOUNTS
        .sessions
        .lock()
        .expect("invariant: the stub account lock is never poisoned")
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_owned)
}

async fn me(headers: HeaderMap) -> Response {
    let session = bearer(&headers).and_then(|token| {
        sessions()
            .get(&token)
            .map(|session| (session.profile.clone(), session.sync.clone()))
    });
    match session {
        None => StatusCode::UNAUTHORIZED.into_response(),
        Some((profile, SyncReply::Answer)) => axum::Json(profile).into_response(),
        Some((_, SyncReply::Fail(status))) => (
            status,
            axum::Json(
                serde_json::json!({"error": {"code": "internal", "message": "stub failure"}}),
            ),
        )
            .into_response(),
        Some((_, SyncReply::Hold(held))) => {
            let (request, answer) =
                HeldRequest::hold(axum::http::Method::GET, "/v1/me".to_owned(), Bytes::new());
            assert!(
                held.send(request).is_ok(),
                "the test stopped holding account syncs"
            );
            match answer.await {
                Ok(response) => response,
                Err(_) => std::future::pending::<Response>().await,
            }
        }
    }
}

fn account_of_email(email: &str) -> Option<String> {
    email
        .trim()
        .strip_suffix("@example.invalid")
        .filter(|account| sessions().contains_key(*account))
        .map(str::to_owned)
}

async fn magic_request(axum::Json(body): axum::Json<serde_json::Value>) -> Response {
    match body["email"].as_str().and_then(account_of_email) {
        Some(account) => axum::Json(serde_json::json!({
            "request_id": account,
            "request_secret": "stub-secret",
            "expires_at": "2099-01-01T00:00:00.000Z",
        }))
        .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn magic_status(axum::Json(body): axum::Json<serde_json::Value>) -> Response {
    let Some(account) = body["requestId"].as_str().map(str::to_owned) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    axum::Json(serde_json::json!({
        "request_id": account,
        "status": "email_approved",
        "totp_required": false,
        "email": email_of(&account),
        "expires_at": "2099-01-01T00:00:00.000Z",
    }))
    .into_response()
}

async fn magic_finalize(axum::Json(body): axum::Json<serde_json::Value>) -> Response {
    let Some(account) = body["requestId"].as_str().map(str::to_owned) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    (
        [(
            header::SET_COOKIE,
            format!("ast_session={account}; Path=/; HttpOnly"),
        )],
        axum::Json(serde_json::json!({
            "session_id": account,
            "account_id": account,
            "expires_at": "2099-01-01T00:00:00.000Z",
            "totp_verified": false,
        })),
    )
        .into_response()
}

#[must_use]
pub fn email_of(account_id: &str) -> String {
    format!("{account_id}@example.invalid")
}

fn session<R>(account_id: &str, change: impl FnOnce(&mut Session) -> R) -> R {
    let mut sessions = sessions();
    let session = sessions
        .get_mut(account_id)
        .unwrap_or_else(|| panic!("invariant: account {account_id} was opened by the fixture"));
    change(session)
}

pub fn fail_syncs(account_id: &str, status: StatusCode) {
    session(account_id, |session| session.sync = SyncReply::Fail(status));
}

pub fn answer_syncs(account_id: &str) {
    session(account_id, |session| session.sync = SyncReply::Answer);
}

pub fn hold_syncs(account_id: &str) -> HeldSyncs {
    let (sender, receiver) = std::sync::mpsc::sync_channel(64);
    session(account_id, |session| {
        session.sync = SyncReply::Hold(std::sync::Arc::new(sender));
    });
    HeldSyncs(receiver)
}

#[derive(Debug)]
pub struct HeldSyncs(std::sync::mpsc::Receiver<HeldRequest>);

impl HeldSyncs {
    #[track_caller]
    pub fn take(&self) -> HeldRequest {
        self.0
            .recv_timeout(crate::wait::LIVENESS)
            .expect("the Editor never synced the account")
    }

    pub fn try_take(&self) -> Option<HeldRequest> {
        self.0.try_recv().ok()
    }

    pub fn wait(&self, limit: std::time::Duration) -> Option<HeldRequest> {
        self.0.recv_timeout(limit).ok()
    }
}

async fn grant(headers: HeaderMap) -> Response {
    match bearer(&headers).filter(|token| sessions().contains_key(token)) {
        Some(_) => axum::Json(serde_json::json!({"token": "stub-grant"})).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

#[must_use]
pub(crate) fn account_url() -> &'static str {
    &ACCOUNTS.url
}

#[must_use]
pub(crate) fn machine(id: &str, label: &str) -> MachineEntry {
    MachineEntry {
        id: id.to_owned(),
        label: label.to_owned(),
        public_key: Jwk {
            kty: "OKP".to_owned(),
            crv: "Ed25519".to_owned(),
            x: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
        },
        created_at: "2026-01-01T00:00:00.000Z".to_owned(),
        revoked_at: None,
        last_seen_at: None,
    }
}

#[must_use]
pub(crate) fn open_account(machines: Vec<MachineEntry>) -> AccountProfile {
    let account_id = format!(
        "acct_test{:020}",
        NEXT_ACCOUNT.fetch_add(1, Ordering::Relaxed)
    );
    let profile = AccountProfile {
        account_id: account_id.clone(),
        email: email_of(&account_id),
        display_name: "Test".to_owned(),
        username: None,
        avatar_url: None,
        totp_enrolled: false,
        machines,
        latest_config_version: None,
    };
    sessions().insert(
        account_id,
        Session {
            profile: profile.clone(),
            clients: HashMap::new(),
            sync: SyncReply::Answer,
        },
    );
    profile
}

pub(crate) fn add_machine(account_id: &str, entry: MachineEntry, client: ForgeClient) {
    let mut sessions = sessions();
    let session = sessions
        .get_mut(account_id)
        .unwrap_or_else(|| panic!("invariant: account {account_id} was opened by the fixture"));
    session
        .profile
        .machines
        .retain(|known| known.id != entry.id);
    session.clients.insert(entry.id.clone(), client);
    session.profile.machines.push(entry);
}

pub fn revoke_machine(account_id: &str, machine_id: &str) {
    let mut sessions = sessions();
    let session = sessions
        .get_mut(account_id)
        .unwrap_or_else(|| panic!("invariant: account {account_id} was opened by the fixture"));
    session
        .profile
        .machines
        .iter_mut()
        .filter(|machine| machine.id == machine_id)
        .for_each(|machine| machine.revoked_at = Some("2026-01-02T00:00:00.000Z".to_owned()));
}

#[derive(Debug, Clone, Copy)]
pub struct StubRelay;

impl DialRelay for StubRelay {
    fn dial(
        &self,
        request: RelayDialRequest,
        _core: CoreSessionClient,
        session: SessionToken,
    ) -> BoxFuture<'static, Result<ForgeClient, RelayClientError>> {
        let client = sessions()
            .get(session.as_str())
            .and_then(|known| known.clients.get(request.machine_id()).cloned())
            .unwrap_or_else(|| {
                panic!(
                    "invariant: the fixture registered a Forge for machine {}",
                    request.machine_id()
                )
            });
        Box::pin(std::future::ready(Ok(client)))
    }
}

pub struct ScriptedAe<F>(pub(crate) F);

impl<F> ScriptedAe<F>
where
    F: Fn(&[&str]) -> Result<String, AeError> + Send + Sync,
{
    pub const fn new(script: F) -> Self {
        Self(script)
    }
}

impl<F> std::fmt::Debug for ScriptedAe<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScriptedAe")
    }
}

impl<F> RunAe for ScriptedAe<F>
where
    F: Fn(&[&str]) -> Result<String, AeError> + Send + Sync,
{
    fn run(&self, args: Vec<String>) -> BoxFuture<'static, Result<String, AeError>> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        Box::pin(std::future::ready((self.0)(&args)))
    }
}

pub(crate) fn unenrolled_ae(args: &[&str]) -> Result<String, AeError> {
    match args {
        ["account", .., "status", "--json"] => {
            Ok(r#"{"enrolled":false,"status":"none"}"#.to_owned())
        }
        _ => Err(AeError::Failed {
            command: args.join(" "),
            detail: "component tests have no `ae`".to_owned(),
        }),
    }
}
