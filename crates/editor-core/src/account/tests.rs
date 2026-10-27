//! Account controller tests against a strict stub Core.
//!
//! The stub enforces the wire rules the real Core enforces: unknown JSON
//! fields are rejected with the Core error envelope, required camelCase
//! fields are mandatory, and the session token only travels in
//! `Set-Cookie`. A test that passes here would fail loudly against
//! drifted DTOs instead of silently.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{AppContext as _, TestAppContext};

use super::AccountPhase;
use crate::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, Screen};

type Handler = Box<dyn Fn(&str, &str, &str) -> (u16, Vec<(String, String)>, String) + Send>;

struct StubCore {
    _thread: std::thread::JoinHandle<()>,
    pub base_url: String,
    pub requests: Arc<Mutex<Vec<(String, String, String)>>>,
}

impl StubCore {
    fn new(handler: Handler) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming().take(50) {
                let Ok(stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err()
                        || line == "\r\n"
                        || line == "\n"
                        || line.is_empty()
                    {
                        break;
                    }
                    head.push_str(&line);
                }
                let mut parts = head.lines().next().unwrap_or_default().split_whitespace();
                let method = parts.next().unwrap_or_default().to_owned();
                let path = parts.next().unwrap_or_default().to_owned();
                let content_length: usize = head
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("Content-Length:")
                            .or_else(|| line.strip_prefix("content-length:"))
                            .map(|value| value.trim().parse().unwrap_or(0))
                    })
                    .unwrap_or(0);
                let mut raw = vec![0_u8; content_length];
                reader.read_exact(&mut raw).unwrap_or(());
                let body = String::from_utf8_lossy(&raw).into_owned();
                recorded
                    .lock()
                    .unwrap()
                    .push((method.clone(), path.clone(), body.clone()));
                let (status, headers, response_body) = handler(&method, &path, &body);
                let mut response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response_body.len()
                );
                for (name, value) in headers {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                response.push_str("\r\n");
                response.push_str(&response_body);
                let mut stream = reader.into_inner();
                if stream.write_all(response.as_bytes()).is_err() {
                    break;
                }
                let _ = stream.flush();
            }
        });
        Self {
            _thread: thread,
            base_url: format!("http://{address}"),
            requests,
        }
    }
}

fn envelope(code: &str) -> (u16, Vec<(String, String)>, String) {
    (
        400,
        vec![],
        serde_json::json!({"error": {"code": code, "message": "stub", "requestId": "req-test"}})
            .to_string(),
    )
}

/// Strict Core mock: exact camelCase shapes in, Core error envelope out.
fn strict_core() -> Handler {
    Box::new(|method: &str, path: &str, body: &str| {
        let ok = |value: serde_json::Value| (200, vec![], value.to_string());
        let parsed: serde_json::Value = if body.is_empty() {
            serde_json::json!({})
        } else {
            match serde_json::from_str(body) {
                Ok(value) => value,
                Err(_) => return envelope("bad_request"),
            }
        };
        let check_keys = |allowed: &[&str]| -> bool {
            parsed
                .as_object()
                .map(|object| object.keys().all(|key| allowed.contains(&key.as_str())))
                .unwrap_or(false)
        };
        match (method, path) {
            ("POST", "/v1/auth/magic/request") => {
                if !check_keys(&["email", "displayName", "deviceName"]) {
                    return envelope("bad_request");
                }
                if parsed
                    .get("email")
                    .and_then(|value| value.as_str())
                    .is_none()
                {
                    return envelope("bad_request");
                }
                ok(serde_json::json!({
                    "requestId": "logreq_00000000000000000000000001",
                    "requestSecret": "SECRET-ONE",
                    "expiresAt": "2026-10-26T00:15:00.000Z",
                }))
            }
            ("POST", "/v1/auth/magic/status") => {
                if !check_keys(&["requestId", "requestSecret"]) {
                    return envelope("bad_request");
                }
                ok(serde_json::json!({
                    "requestId": "logreq_00000000000000000000000001",
                    "status": "email_approved",
                    "totpRequired": false,
                    "email": "op@example.invalid",
                    "expiresAt": "2026-10-26T00:15:00.000Z",
                }))
            }
            ("POST", "/v1/auth/magic/finalize") => {
                if !check_keys(&["requestId", "requestSecret", "totpCode", "recoveryCode"]) {
                    return envelope("bad_request");
                }
                (
                    200,
                    vec![(
                        "Set-Cookie".to_owned(),
                        "ast_session=STUB-SESSION; Path=/; HttpOnly".to_owned(),
                    )],
                    serde_json::json!({
                        "sessionId": "sess_00000000000000000000000001",
                        "accountId": "acct_00000000000000000000000000",
                        "expiresAt": "2026-10-26T01:15:00.000Z",
                        "totpVerified": false,
                    })
                    .to_string(),
                )
            }
            ("GET", "/v1/me") => ok(serde_json::json!({
                "accountId": "acct_00000000000000000000000000",
                "email": "op@example.invalid",
                "displayName": "Op",
                "totpEnrolled": false,
                "machines": [
                    {"id": "mch_11111111111111111111111111", "label": "server",
                     "publicKey": {"kty": "OKP", "crv": "Ed25519",
                                  "x": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"},
                     "createdAt": "2026-10-26T00:00:00.000Z",
                     "revokedAt": null, "lastSeenAt": null}
                ],
                "providerConnections": [],
                "latestConfigVersion": 3,
            })),
            ("GET", "/v1/sessions") => ok(serde_json::json!({"sessions": []})),
            ("POST", "/v1/sessions/current/revoke") => ok(serde_json::json!({"revoked": true})),
            ("POST", "/v1/machines/enrollments/claim") => {
                if !check_keys(&["code"]) {
                    return envelope("bad_request");
                }
                ok(serde_json::json!({
                    "enrollmentId": "enr_22222222222222222222222222",
                    "status": "claimed",
                }))
            }
            ("POST", "/v1/grants/issue") => {
                if !check_keys(&["audience", "scope", "ttlSeconds", "targetMachineId", "cnf"]) {
                    return envelope("bad_request");
                }
                ok(serde_json::json!({
                    "token": "grant.jwt.stub",
                    "expiresAt": "2026-10-26T00:05:00.000Z",
                }))
            }
            ("POST", "/v1/machines/enrollments") => {
                if !check_keys(&["publicKey", "label", "timestamp", "proof"]) {
                    return envelope("bad_request");
                }
                (
                    201,
                    vec![],
                    serde_json::json!({
                        "enrollmentId": "enr_22222222222222222222222222",
                        "code": "ABCDEFGH",
                        "expiresAt": "2026-10-26T00:10:00.000Z",
                        "keyFingerprint": "fp",
                    })
                    .to_string(),
                )
            }
            _ => (404, vec![], envelope("not_found").2),
        }
    })
}

fn wait_for<T>(
    cx: &mut TestAppContext,
    what: &str,
    check: impl Fn(&TestAppContext) -> Option<T>,
) -> T {
    for _ in 0..400 {
        // The status poll waits on a 3s executor timer; the test executor
        // holds virtual time, so advance it or the poll never fires.
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        if let Some(value) = check(cx) {
            return value;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

/// Full sign-in against the stub: request → poll approves → finalize →
/// session cookie → `/v1/me` → Authenticated with machines.
#[gpui::test]
fn sign_in_flow_reaches_authenticated(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let stub = StubCore::new(strict_core());
    let dir = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let base_url = stub.base_url.clone();
    let core = cx.new(|cx| {
        let core = Core::new(files, cx).unwrap();
        core.account.update(cx, |account, cx| {
            *account = super::Account::with_base_url(
                EditorFiles {
                    dir: dir.path().join("editor"),
                },
                Some(base_url),
                cx,
            );
        });
        core
    });
    let account = core.read_with(cx, |core, _| core.account().clone());
    account.update(cx, |account, cx| {
        account.request_link("op@example.invalid".to_owned(), cx);
    });
    // The controller pauses at EmailApproved for explicit finish (TOTP or
    // plain confirm); the test drives that second step, then waits for
    // the session-backed profile.
    wait_for(cx, "the email approval", |cx| {
        let account = core.read_with(cx, |core, _| core.account().clone());
        account.read_with(cx, |account, _| {
            matches!(account.phase(), AccountPhase::EmailApproved { .. }).then_some(())
        })
    });
    account.update(cx, |account, cx| {
        account.finish_sign_in(None, None, cx);
    });
    wait_for(cx, "the sign-in to complete", |cx| {
        let account = core.read_with(cx, |core, _| core.account().clone());
        account.read_with(cx, |account, _| {
            account.profile().map(|profile| profile.email.clone())
        })
    });
    let account = core.read_with(cx, |core, _| core.account().clone());
    let email = account
        .read_with(cx, |account, _| {
            account.profile().map(|profile| profile.email.clone())
        })
        .unwrap();
    assert_eq!(email, "op@example.invalid");
    let phase = account.read_with(cx, |account, _| account.phase().clone());
    assert!(matches!(phase, AccountPhase::Authenticated { .. }));
    let machines = account.read_with(cx, |account, _| {
        account.profile().map(|profile| profile.machines.len())
    });
    assert_eq!(machines, Some(1));
    // The session file holds only the opaque token, never the profile.
    let stored: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("editor").join("account.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        stored.get("session_token").and_then(|value| value.as_str()),
        Some("STUB-SESSION")
    );
    assert!(stored.get("accountId").is_none());
    // The stub saw the full request → status → finalize → me sequence.
    let paths: Vec<String> = stub
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|(_, path, _)| path.clone())
        .collect();
    for expected in [
        "/v1/auth/magic/request",
        "/v1/auth/magic/status",
        "/v1/auth/magic/finalize",
        "/v1/me",
    ] {
        assert!(paths.contains(&expected.to_owned()), "{paths:?}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dir.path().join("editor").join("account.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the session file is owner-only");
    }
}

/// Debug output never carries secrets, even though the state holds them.
#[test]
fn debug_output_redacts_secrets() {
    let pending = AccountPhase::Pending {
        request_id: "logreq_1".to_owned(),
        request_secret: "SECRET-ONE".to_owned(),
        email: "op@example.invalid".to_owned(),
        expires_at: "t".to_owned(),
    };
    let rendered = format!("{pending:?}");
    assert!(!rendered.contains("SECRET-ONE"), "{rendered}");
    assert!(rendered.contains("(redacted)"));
}

/// Snake-case requests are rejected before they reach Core semantics: the
/// stub enforces Core's strict shape, so drifted DTOs fail here first.
#[gpui::test]
fn snake_case_requests_are_rejected(_cx: &mut TestAppContext) {
    let stub = StubCore::new(strict_core());
    let base_url = stub.base_url.clone();
    let (status, body) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let response = reqwest::Client::new()
                .post(format!("{base_url}/v1/auth/magic/status"))
                .json(&serde_json::json!({"request_id": "logreq_1", "request_secret": "s"}))
                .send()
                .await
                .unwrap();
            let status = response.status().as_u16();
            let body: serde_json::Value = response.json().await.unwrap();
            (status, body)
        });
    assert_eq!(status, 400);
    assert_eq!(body["error"]["code"], "bad_request");
    assert!(body["error"]["requestId"].is_string());
}

/// Sign-out immediately drops account Relay inventory while a direct host
/// stays put: cached account hosts and their screens must not outlive the
/// session, and grant expiry is never the cleanup path, so this asserts
/// right after the sign-out transition.
#[gpui::test]
fn sign_out_removes_account_hosts_but_keeps_direct(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let stub = StubCore::new(strict_core());
    let dir = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let base_url = stub.base_url.clone();
    let core = cx.new(|cx| {
        let core = Core::new(files, cx).unwrap();
        core.account.update(cx, |account, cx| {
            *account = super::Account::with_base_url(
                EditorFiles {
                    dir: dir.path().join("editor"),
                },
                Some(base_url),
                cx,
            );
        });
        core
    });
    // Full sign-in against the stub: request → approval → finish → profile.
    let account = core.read_with(cx, |core, _| core.account().clone());
    account.update(cx, |account, cx| {
        account.request_link("op@example.invalid".to_owned(), cx);
    });
    wait_for(cx, "the email approval", |cx| {
        core.read_with(cx, |core, _| core.account().clone())
            .read_with(cx, |account, _| {
                matches!(account.phase(), AccountPhase::EmailApproved { .. }).then_some(())
            })
    });
    account.update(cx, |account, cx| {
        account.finish_sign_in(None, None, cx);
    });
    let owner = wait_for(cx, "the sign-in to complete", |cx| {
        core.read_with(cx, |core, _| core.account().clone())
            .read_with(cx, |account, _| {
                account.profile().map(|profile| profile.account_id.clone())
            })
    });
    assert_eq!(owner, "acct_00000000000000000000000000");

    // A direct host through the public pairing path (persists to hosts.json).
    let identity = core.read_with(cx, |core, _| core.identity.clone().unwrap());
    let direct = HostConfig {
        id: HostId("direct-1".to_owned()),
        name: "Direct".to_owned(),
        address: "127.0.0.1:9".to_owned(),
        fingerprint: "00".repeat(32),
        kind: HostKind::Local,
    };
    core.update(cx, |core, cx| {
        core.add_host(direct.clone(), cx).unwrap();
    });

    // An account Relay entry with a harmless unconnected client: building
    // it performs no traffic, and Vilde's transport owns the production
    // runtime-cancellation proof in the real harness.
    let relay = ForgeClient::new("127.0.0.1:9", &"00".repeat(32), &identity).unwrap();
    let machine = HostConfig {
        id: HostId("mch_11111111111111111111111111".to_owned()),
        name: "server".to_owned(),
        address: String::new(),
        fingerprint: String::new(),
        kind: HostKind::Account {
            account: owner.clone(),
            machine: "mch_11111111111111111111111111".to_owned(),
        },
    };
    core.update(cx, |core, cx| {
        core.connect_with_client(machine.clone(), relay, cx);
        core.navigate(
            Screen::NewThread {
                host: machine.id.clone(),
                project: None,
            },
            cx,
        );
    });
    assert!(core.read_with(cx, |core, _| core.host(&machine.id).is_some()));

    // Sign out: the account entry, its screens, and its bookkeeping go
    // immediately; the direct host and its persisted pairing stay.
    account.update(cx, |account, cx| account.sign_out(cx));
    wait_for(cx, "the account inventory to clear", |cx| {
        core.read_with(cx, |core, _| core.host(&machine.id).is_none().then_some(()))
    });
    core.read_with(cx, |core, _| {
        assert!(core.host(&direct.id).is_some(), "direct host stays");
        assert!(core.host(&machine.id).is_none(), "account host is gone");
        assert_eq!(
            core.screen().host(),
            Some(&direct.id),
            "navigated away to the surviving host"
        );
        assert!(
            core.back
                .iter()
                .all(|screen| screen.host() != Some(&machine.id)),
            "no back-stack route points at the removed account host"
        );
        assert!(
            core.account_hosts.is_empty(),
            "no stale account bookkeeping"
        );
    });
    let file: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("editor").join("hosts.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<String> = file
        .get("hosts")
        .and_then(serde_json::Value::as_array)
        .map(|hosts| {
            hosts
                .iter()
                .filter_map(|host| {
                    host.get("id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(ids, vec!["direct-1".to_owned()]);
    assert_ne!(
        file.get("last").and_then(serde_json::Value::as_str),
        Some("mch_11111111111111111111111111"),
        "no saved last pointing at the removed account host"
    );
}
