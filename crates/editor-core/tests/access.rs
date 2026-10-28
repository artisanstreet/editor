//! The account-only entry, end to end through `Core`: a stub Core over
//! HTTP, a scripted `ae`, a scripted Relay, and a real Forge for the direct
//! route.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::future_not_send,
    reason = "test fixtures: a failure fails the test; GPUI test futures are !Send"
)]

use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use editor_core::hosts::HostsFile;
use editor_core::{
    Account, AccountPhase, Core, EditorFiles, EntryGate, ForgeClient, HostConfig, HostId, HostKind,
    LocalForge, RelayDialRequest, RelayScript, Services,
};
use gpui::{AppContext as _, Entity, TestAppContext};
use serde_json::{Value, json};

const ACCOUNT: &str = "acct_aaaaaaaaaaaaaaaaaaaaaaaaaa";
const EMAIL: &str = "op@example.invalid";
const LOCAL: &str = "mch_11111111111111111111111111";
const REMOTE: &str = "mch_22222222222222222222222222";

// -- stub Core ---------------------------------------------------------

#[derive(Default)]
struct Stub {
    me: (u16, String),
    statuses: VecDeque<(u16, String)>,
    finalize_delay: Duration,
    finalized: bool,
    seen: Vec<String>,
}

struct StubCore {
    base_url: String,
    state: Arc<Mutex<Stub>>,
}

impl StubCore {
    fn new(me: &Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(Stub {
            me: (200, me.to_string()),
            ..Stub::default()
        }));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &shared);
            }
        });
        Self { base_url, state }
    }

    fn saw(&self, request: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .seen
            .iter()
            .any(|seen| seen == request)
    }
}

fn serve(stream: TcpStream, state: &Mutex<Stub>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let (status, headers, reply) = respond(method, path, state);
    let response = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{reply}",
        reply.len()
    );
    let mut stream = reader.into_inner();
    if stream.write_all(response.as_bytes()).is_ok() {
        stream.flush().ok();
    }
}

fn respond(method: &str, path: &str, state: &Mutex<Stub>) -> (u16, &'static str, String) {
    let mut stub = state.lock().unwrap();
    stub.seen.push(format!("{method} {path}"));
    match (method, path) {
        ("GET", "/v1/me") => (stub.me.0, "", stub.me.1.clone()),
        ("POST", "/v1/auth/magic/request") => (
            200,
            "",
            json!({"request_id": "logreq_1", "request_secret": "SECRET", "expires_at": "2026-10-28T01:00:00.000Z"})
                .to_string(),
        ),
        ("POST", "/v1/auth/magic/status") => {
            let (status, body) = stub.statuses.pop_front().unwrap_or_else(|| {
                (
                    200,
                    json!({"request_id": "logreq_1", "status": "email_approved", "totp_required": false,
                           "email": EMAIL, "expires_at": "2026-10-28T01:00:00.000Z"})
                    .to_string(),
                )
            });
            (status, "", body)
        }
        ("POST", "/v1/auth/magic/finalize") => {
            let delay = stub.finalize_delay;
            drop(stub);
            std::thread::sleep(delay);
            state.lock().unwrap().finalized = true;
            (
                200,
                "Set-Cookie: ast_session=SESSION; Path=/; HttpOnly\r\n",
                json!({"session_id": "sess_1", "account_id": ACCOUNT,
                       "expires_at": "2026-10-29T00:00:00.000Z", "totp_verified": false})
                .to_string(),
            )
        }
        ("POST", "/v1/machines/enrollments/claim") => {
            (200, "", json!({"enrollment_id": "enr_1"}).to_string())
        }
        ("POST", "/v1/grants/issue") => (
            200,
            "",
            json!({"token": "grant.jwt", "expires_at": "2026-10-28T00:05:00.000Z"}).to_string(),
        ),
        ("POST", "/v1/sessions/current/revoke") => (200, "", json!({"revoked": true}).to_string()),
        _ => (404, "", core_error("not_found")),
    }
}

fn core_error(code: &str) -> String {
    json!({"error": {"code": code, "message": "stub", "request_id": "req_1"}}).to_string()
}

fn machine(id: &str, revoked: bool) -> Value {
    json!({
        "id": id,
        "label": id,
        "public_key": {"kty": "OKP", "crv": "Ed25519", "x": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"},
        "created_at": "2026-10-01T00:00:00.000Z",
        "revoked_at": revoked.then_some("2026-10-02T00:00:00.000Z"),
        "last_seen_at": null,
    })
}

fn profile(machines: &[Value]) -> Value {
    json!({
        "account_id": ACCOUNT,
        "email": EMAIL,
        "display_name": "Op",
        "totp_enrolled": false,
        "machines": machines,
        "latest_config_version": null,
    })
}

// -- scripted `ae` and a real Forge -------------------------------------

type PairScript = Arc<dyn Fn() -> anyhow::Result<String> + Send + Sync>;

const UNENROLLED: &str = r#"{"enrolled":false,"status":"none"}"#;

fn claimed() -> String {
    json!({"enrolled": false, "status": "claimed", "account_id": ACCOUNT, "email": EMAIL})
        .to_string()
}

fn enrolled(machine_id: &str) -> String {
    json!({"enrolled": true, "machine_id": machine_id, "account_id": ACCOUNT}).to_string()
}

#[derive(Clone)]
struct ScriptedAe {
    status: Arc<Mutex<String>>,
    calls: Arc<Mutex<Vec<String>>>,
    pair: Option<PairScript>,
}

impl ScriptedAe {
    fn new(status: &str, pair: Option<PairScript>) -> Self {
        Self {
            status: Arc::new(Mutex::new(status.to_owned())),
            calls: Arc::default(),
            pair,
        }
    }

    fn script(&self) -> editor_core::AeScript {
        let ae = self.clone();
        Arc::new(move |args: &[&str]| {
            ae.calls.lock().unwrap().push(args.join(" "));
            match args {
                ["pair", "--json"] => ae
                    .pair
                    .as_ref()
                    .map_or_else(|| Err(anyhow::anyhow!("no Forge here")), |pair| pair()),
                [.., "status", "--json"] => Ok(ae.status.lock().unwrap().clone()),
                [.., "enroll", "--json"] | [.., "enroll", "--json", "--replace"] => {
                    *ae.status.lock().unwrap() = claimed();
                    Ok(r#"{"code":"ABCD-2345"}"#.to_owned())
                }
                [.., "confirm", "--yes", "--account-id", _] => {
                    *ae.status.lock().unwrap() = enrolled(LOCAL);
                    Ok("Enrolled.".to_owned())
                }
                _ => anyhow::bail!("unexpected `ae {}`", args.join(" ")),
            }
        })
    }

    fn called(&self, command: &str) -> bool {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.contains(command))
    }
}

struct Forge {
    runtime: tokio::runtime::Runtime,
    store: forge_store::Store,
    address: String,
    _data: tempfile::TempDir,
}

impl Forge {
    fn start() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let data = tempfile::tempdir().unwrap();
        let server = runtime
            .block_on(forge_http::start(forge_http::ServeConfig {
                data_dir: data.path().to_owned(),
                listen: "127.0.0.1:0".parse().unwrap(),
            }))
            .unwrap();
        let address = server.local_addr().unwrap().to_string();
        let store = server.store().clone();
        runtime.spawn(async move { server.run().await.ok() });
        Self {
            runtime,
            store,
            address,
            _data: data,
        }
    }

    /// What `ae pair --json` prints on this machine.
    fn pair_script(&self) -> PairScript {
        let (handle, store, address) = (
            self.runtime.handle().clone(),
            self.store.clone(),
            self.address.clone(),
        );
        Arc::new(move || {
            let invite = handle.block_on(forge_http::invite(&store))?;
            Ok(json!({"code": invite.code, "forge_certificate": invite.forge_certificate, "address": address})
                .to_string())
        })
    }

    /// A pairing saved earlier, as the direct route leaves it in `hosts.json`.
    fn saved_pairing(&self, files: &EditorFiles, id: &str) -> HostConfig {
        let identity = files.identity().unwrap();
        let invite = self
            .runtime
            .block_on(forge_http::invite(&self.store))
            .unwrap();
        let fingerprint = artisan_api::tls::fingerprint_of(&invite.forge_certificate).unwrap();
        let config = self
            .runtime
            .block_on(editor_core::hosts::pair(
                &self.address,
                &invite.code,
                &fingerprint,
                &identity,
                "test",
                id.to_owned(),
                HostKind::Local,
            ))
            .unwrap();
        HostConfig {
            id: HostId(id.to_owned()),
            ..config
        }
    }
}

// -- this device's files ------------------------------------------------

fn save_session(dir: &Path, base_url: &str, profile: Option<&Value>) {
    std::fs::create_dir_all(dir).unwrap();
    let mut file = json!({"base_url": base_url, "session_token": "SESSION"});
    if let Some(profile) = profile {
        file["profile"] = profile.clone();
    }
    std::fs::write(dir.join("account.json"), file.to_string()).unwrap();
}

fn save_onboarding(dir: &Path, base_url: &str, machine_id: &str) {
    use base64::Engine as _;
    let origin = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(base_url);
    let accounts = dir.join("accounts");
    std::fs::create_dir_all(&accounts).unwrap();
    std::fs::write(
        accounts.join(format!("{origin}-{ACCOUNT}.json")),
        json!({"account_id": ACCOUNT, "base_url": base_url, "machine_id": machine_id}).to_string(),
    )
    .unwrap();
}

fn save_hosts(dir: &Path, hosts: Vec<HostConfig>) {
    EditorFiles {
        dir: dir.to_owned(),
    }
    .save_hosts(&HostsFile { hosts, last: None })
    .unwrap();
}

fn saved_session(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("account.json"))
        .unwrap_or_default()
        .contains("session_token")
}

// -- driving the Editor -------------------------------------------------

fn open_core(
    cx: &mut TestAppContext,
    dir: &Path,
    core_url: &str,
    ae: &ScriptedAe,
    relay: Option<RelayScript>,
) -> Entity<Core> {
    cx.executor().allow_parking();
    let services = Services::scripted(core_url, ae.script(), relay);
    let files = EditorFiles {
        dir: dir.to_owned(),
    };
    cx.new(|cx| Core::with_services(files, services, cx).unwrap())
}

/// Lets the Editor's tasks and the real network make progress until
/// `check` holds.
async fn wait_for<T>(
    cx: &mut TestAppContext,
    what: &str,
    mut check: impl FnMut(&mut TestAppContext) -> Option<T>,
) -> T {
    for _ in 0..500 {
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        if let Some(value) = check(cx) {
            return value;
        }
        cx.executor().timer(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

fn account(cx: &TestAppContext, core: &Entity<Core>) -> Entity<Account> {
    core.read_with(cx, |core, _| core.account().clone())
}

fn gate(cx: &TestAppContext, core: &Entity<Core>) -> EntryGate {
    core.read_with(cx, |core, cx| core.entry_gate(cx))
}

fn ready() -> EntryGate {
    EntryGate::Ready {
        account_id: ACCOUNT.to_owned(),
    }
}

fn local_forge(cx: &TestAppContext, core: &Entity<Core>) -> LocalForge {
    account(cx, core).read_with(cx, |account, _| account.local_forge())
}

/// `(id, kind, through the Relay)` for each host the workspace may show.
fn visible_hosts(cx: &TestAppContext, core: &Entity<Core>) -> Vec<(String, HostKind, bool)> {
    core.read_with(cx, |core, cx| {
        core.hosts()
            .map(|(config, host)| {
                (
                    config.id.0.clone(),
                    config.kind.clone(),
                    host.read(cx).client.is_relay(),
                )
            })
            .collect()
    })
}

async fn wait_approved(cx: &mut TestAppContext, core: &Entity<Core>) {
    let account = account(cx, core);
    account.update(cx, |account, cx| account.request_link(EMAIL, cx));
    wait_for(cx, "the email approval", |cx| {
        account
            .read_with(cx, |account, _| {
                matches!(account.phase(), AccountPhase::EmailApproved { .. })
            })
            .then_some(())
    })
    .await;
}

async fn sign_in(cx: &mut TestAppContext, core: &Entity<Core>) {
    wait_approved(cx, core).await;
    let account = account(cx, core);
    account.update(cx, |account, cx| account.finish_sign_in(None, None, cx));
    wait_for(cx, "the sign-in", |cx| {
        account
            .read_with(cx, |account, _| account.profile().is_some())
            .then_some(())
    })
    .await;
}

// -- scenarios ----------------------------------------------------------

#[gpui::test]
async fn sign_in_then_setup_connects_this_forge_directly(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let stub = StubCore::new(&profile(&[machine(LOCAL, false)]));
    let forge = Forge::start();
    let ae = ScriptedAe::new(UNENROLLED, Some(forge.pair_script()));
    let core = open_core(cx, dir.path(), &stub.base_url, &ae, None);
    assert_eq!(
        gate(cx, &core),
        EntryGate::SignedOut,
        "a fresh device starts signed out"
    );

    sign_in(cx, &core).await;
    assert!(
        matches!(gate(cx, &core), EntryGate::NeedsSetup { .. }),
        "signed in without setup"
    );

    let account = account(cx, &core);
    account.update(cx, Account::enroll_this_forge);
    wait_for(cx, "the claim", |cx| {
        matches!(local_forge(cx, &core), LocalForge::Claimed { .. }).then_some(())
    })
    .await;
    assert!(
        stub.saw("POST /v1/machines/enrollments/claim"),
        "the code went to Core"
    );

    account.update(cx, Account::confirm_forge);
    wait_for(cx, "the direct route", |cx| {
        core.read_with(cx, |core, cx| core.machine_live(LOCAL, cx))
            .then_some(())
    })
    .await;
    assert!(
        visible_hosts(cx, &core).is_empty(),
        "nothing shows before setup finishes"
    );

    core.update(cx, |core, cx| core.complete_setup(LOCAL.to_owned(), cx))
        .unwrap();
    assert_eq!(gate(cx, &core), ready(), "setup opens the gate");
    assert_eq!(
        visible_hosts(cx, &core),
        vec![(LOCAL.to_owned(), HostKind::Local, false)],
        "this machine's Forge is reached directly, not through the Relay"
    );
    assert!(
        EditorFiles {
            dir: dir.path().to_owned()
        }
        .hosts()
        .unwrap()
        .hosts
        .iter()
        .any(|config| config.id.0 == LOCAL),
        "the direct pairing is saved for offline starts"
    );
}

#[gpui::test]
async fn resume_online_reaches_ready_and_pairs_the_enrolled_forge(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let stub = StubCore::new(&profile(&[machine(LOCAL, false)]));
    save_session(dir.path(), &stub.base_url, None);
    save_onboarding(dir.path(), &stub.base_url, LOCAL);
    let forge = Forge::start();
    let ae = ScriptedAe::new(&enrolled(LOCAL), Some(forge.pair_script()));
    let core = open_core(cx, dir.path(), &stub.base_url, &ae, None);
    assert_eq!(
        gate(cx, &core),
        EntryGate::Restoring,
        "no workspace flash before Core answers"
    );

    wait_for(cx, "the direct route", |cx| {
        visible_hosts(cx, &core).first().cloned()
    })
    .await;
    assert_eq!(
        gate(cx, &core),
        ready(),
        "a known session with setup done opens"
    );
    assert!(
        ae.called("pair --json"),
        "the Editor paired with its Forge through `ae`"
    );
}

#[gpui::test]
async fn resume_offline_opens_the_direct_forge_from_the_saved_profile(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let offline = "http://127.0.0.1:1";
    let forge = Forge::start();
    let files = EditorFiles {
        dir: dir.path().to_owned(),
    };
    save_hosts(dir.path(), vec![forge.saved_pairing(&files, LOCAL)]);
    save_session(
        dir.path(),
        offline,
        Some(&profile(&[machine(LOCAL, false)])),
    );
    save_onboarding(dir.path(), offline, LOCAL);
    let ae = ScriptedAe::new(&enrolled(LOCAL), None);
    let core = open_core(cx, dir.path(), offline, &ae, None);
    assert_eq!(
        gate(cx, &core),
        ready(),
        "the saved profile opens the gate without Core"
    );

    wait_for(cx, "the direct route", |cx| {
        core.read_with(cx, |core, cx| core.machine_live(LOCAL, cx))
            .then_some(())
    })
    .await;
    assert_eq!(
        gate(cx, &core),
        ready(),
        "an unreachable Core never signs out"
    );
    assert!(
        saved_session(dir.path()),
        "the session stays saved for when Core is back"
    );
}

#[gpui::test]
async fn a_session_core_rejects_signs_out(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let stub = StubCore::new(&profile(&[machine(REMOTE, false)]));
    stub.state.lock().unwrap().me = (401, core_error("unauthorized"));
    save_session(
        dir.path(),
        &stub.base_url,
        Some(&profile(&[machine(REMOTE, false)])),
    );
    save_onboarding(dir.path(), &stub.base_url, REMOTE);
    let ae = ScriptedAe::new(UNENROLLED, None);
    let core = open_core(cx, dir.path(), &stub.base_url, &ae, None);

    wait_for(cx, "the sign-out", |cx| {
        (gate(cx, &core) == EntryGate::SignedOut).then_some(())
    })
    .await;
    assert!(
        visible_hosts(cx, &core).is_empty(),
        "no host outlives the session"
    );
    assert!(!saved_session(dir.path()), "the dead session is forgotten");
}

#[gpui::test]
async fn sign_out_hides_every_host_and_signing_in_again_reconnects(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let stub = StubCore::new(&profile(&[machine(LOCAL, false)]));
    let forge = Forge::start();
    let files = EditorFiles {
        dir: dir.path().to_owned(),
    };
    save_hosts(dir.path(), vec![forge.saved_pairing(&files, LOCAL)]);
    save_session(dir.path(), &stub.base_url, None);
    save_onboarding(dir.path(), &stub.base_url, LOCAL);
    let ae = ScriptedAe::new(&enrolled(LOCAL), None);
    let core = open_core(cx, dir.path(), &stub.base_url, &ae, None);
    wait_for(cx, "the direct route", |cx| {
        visible_hosts(cx, &core).first().cloned()
    })
    .await;

    account(cx, &core).update(cx, Account::sign_out);
    assert_eq!(gate(cx, &core), EntryGate::SignedOut, "signed out");
    assert!(
        visible_hosts(cx, &core).is_empty(),
        "sign-out hides every host"
    );
    assert!(
        !core.read_with(cx, |core, cx| core.machine_live(LOCAL, cx)),
        "the route is closed, not just hidden"
    );
    assert_eq!(files.hosts().unwrap().hosts.len(), 1, "hosts.json is kept");

    sign_in(cx, &core).await;
    wait_for(cx, "the route to reopen", |cx| {
        visible_hosts(cx, &core).first().cloned()
    })
    .await;
    assert_eq!(gate(cx, &core), ready(), "setup done once stays done");
    assert!(!ae.called("pair --json"), "the saved pairing is reused");
}

#[gpui::test]
async fn another_machine_of_the_account_is_reached_through_the_relay(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let stub = StubCore::new(&profile(&[machine(REMOTE, false)]));
    let files = EditorFiles {
        dir: dir.path().to_owned(),
    };
    let identity = files.identity().unwrap();
    save_hosts(
        dir.path(),
        vec![HostConfig {
            id: HostId("legacy".to_owned()),
            name: "Legacy".to_owned(),
            address: "127.0.0.1:9".to_owned(),
            fingerprint: "00".repeat(32),
            kind: HostKind::Local,
        }],
    );
    save_session(dir.path(), &stub.base_url, None);
    save_onboarding(dir.path(), &stub.base_url, REMOTE);
    let dialed = Arc::new(Mutex::new(Vec::new()));
    let relay: RelayScript = {
        let dialed = Arc::clone(&dialed);
        Arc::new(move |request: &RelayDialRequest| {
            dialed.lock().unwrap().push(request.machine_id.clone());
            ForgeClient::new("127.0.0.1:9", &"00".repeat(32), &identity)
        })
    };
    let ae = ScriptedAe::new(UNENROLLED, None);
    let core = open_core(cx, dir.path(), &stub.base_url, &ae, Some(relay));

    let hosts = wait_for(cx, "the Relay host", |cx| {
        let hosts = visible_hosts(cx, &core);
        (!hosts.is_empty()).then_some(hosts)
    })
    .await;
    assert_eq!(
        hosts,
        vec![(
            REMOTE.to_owned(),
            HostKind::Account {
                account: ACCOUNT.to_owned(),
                machine: REMOTE.to_owned()
            },
            false
        )],
        "only the account's machine shows; the legacy pairing stays hidden"
    );
    assert_eq!(
        *dialed.lock().unwrap(),
        vec![REMOTE.to_owned()],
        "dialed through the Relay"
    );
    assert!(
        stub.saw("POST /v1/grants/issue"),
        "with a fresh grant from Core"
    );
}

#[gpui::test]
async fn a_removed_local_forge_can_pair_again(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let stub = StubCore::new(&profile(&[machine(LOCAL, true)]));
    save_session(dir.path(), &stub.base_url, None);
    let ae = ScriptedAe::new(&enrolled(LOCAL), None);
    let core = open_core(cx, dir.path(), &stub.base_url, &ae, None);

    wait_for(cx, "the binding", |cx| {
        (local_forge(cx, &core)
            == LocalForge::Revoked {
                machine_id: LOCAL.to_owned(),
            })
        .then_some(())
    })
    .await;
    assert!(
        !core.read_with(cx, |core, cx| core.machine_live(LOCAL, cx)),
        "a removed Forge is never dialed"
    );

    account(cx, &core).update(cx, Account::enroll_this_forge);
    wait_for(cx, "the new claim", |cx| {
        matches!(local_forge(cx, &core), LocalForge::Claimed { .. }).then_some(())
    })
    .await;
    assert!(
        ae.called("enroll --json --replace"),
        "re-pairing replaces the machine key"
    );
}

#[gpui::test]
async fn a_network_blip_while_waiting_for_the_link_keeps_waiting(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let stub = StubCore::new(&profile(&[]));
    stub.state
        .lock()
        .unwrap()
        .statuses
        .push_back((503, "upstream hiccup".to_owned()));
    let ae = ScriptedAe::new(UNENROLLED, None);
    let core = open_core(cx, dir.path(), &stub.base_url, &ae, None);

    wait_approved(cx, &core).await;
    assert!(
        stub.state.lock().unwrap().statuses.is_empty(),
        "the failed poll happened before the approval"
    );
}

#[gpui::test]
async fn a_superseded_sign_in_never_completes(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let stub = StubCore::new(&profile(&[]));
    stub.state.lock().unwrap().finalize_delay = Duration::from_millis(300);
    let ae = ScriptedAe::new(UNENROLLED, None);
    let core = open_core(cx, dir.path(), &stub.base_url, &ae, None);
    wait_approved(cx, &core).await;

    let account = account(cx, &core);
    account.update(cx, |account, cx| account.finish_sign_in(None, None, cx));
    wait_for(cx, "the finish to reach Core", |_| {
        stub.saw("POST /v1/auth/magic/finalize").then_some(())
    })
    .await;
    account.update(cx, Account::dismiss_error);
    wait_for(cx, "Core to answer the abandoned finish", |_| {
        stub.state.lock().unwrap().finalized.then_some(())
    })
    .await;
    for _ in 0..10 {
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        cx.executor().timer(Duration::from_millis(20)).await;
    }
    assert_eq!(
        gate(cx, &core),
        EntryGate::SignedOut,
        "the abandoned sign-in stays abandoned"
    );
    assert!(!saved_session(dir.path()), "its session is never saved");
}
