//! Account-only entry gate behavior: restore vs sign-in vs setup vs ready.
//!
//! Public-surface tests for the owned model (`editor-core`): fresh devices
//! never auto-connect legacy pairings, startup restore never flashes the
//! workspace, sign-in lands in setup with inventory hidden, only a verified
//! live Relay route completes setup, restarts rebind the recorded machine
//! through the account, and sign-out/switch/revocation lock everything with
//! stale callbacks dropped. No wire snapshots: a lean local Core stub plus
//! real localhost roundtrips.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use editor_core::{
    Account, AccountPhase, Core, EditorFiles, EntryGate, ForgeClient, HostConfig, HostId,
    HostKind, RelayDialRequest, Screen,
};
use gpui::{AppContext as _, TestAppContext};

const ACCOUNT_A: &str = "acct_aaaaaaaaaaaaaaaaaaaaaaaaaa";
const ACCOUNT_B: &str = "acct_bbbbbbbbbbbbbbbbbbbbbbbbbb";
const MACHINE_A: &str = "mch_11111111111111111111111111";

struct StubCore {
    _thread: std::thread::JoinHandle<()>,
    pub base_url: String,
    pub me_override: Arc<Mutex<Option<(u16, String)>>>,
}

impl StubCore {
    fn new(account_id: &str) -> Self {
        let account_id = account_id.to_owned();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let me_override: Arc<Mutex<Option<(u16, String)>>> = Arc::new(Mutex::new(None));
        let overridden = Arc::clone(&me_override);
        let thread = std::thread::spawn(move || {
            let me_body = serde_json::json!({
                "account_id": account_id,
                "email": "op@example.invalid",
                "display_name": "Op",
                "totp_enrolled": false,
                "machines": [
                    {"id": MACHINE_A, "label": "server",
                     "public_key": {"kty": "OKP", "crv": "Ed25519",
                                  "x": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"},
                     "created_at": "2026-10-26T00:00:00.000Z",
                     "revoked_at": None::<String>, "last_seen_at": None::<String>}
                ],
                "provider_connections": [],
                "latest_config_version": 3,
            })
            .to_string();
            for stream in listener.incoming().take(200) {
                let Ok(stream) = stream else { break };
                drop(stream.set_read_timeout(Some(Duration::from_secs(10))));
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
                let (status, headers, response_body): (u16, Vec<(String, String)>, String) =
                    match (method.as_str(), path.as_str()) {
                        ("POST", "/v1/auth/magic/request") => (
                            200,
                            vec![],
                            serde_json::json!({
                                "request_id": "logreq_00000000000000000000000001",
                                "request_secret": "SECRET-ONE",
                                "expires_at": "2026-10-26T00:15:00.000Z",
                            })
                            .to_string(),
                        ),
                        ("POST", "/v1/auth/magic/status") => (
                            200,
                            vec![],
                            serde_json::json!({
                                "request_id": "logreq_00000000000000000000000001",
                                "status": "email_approved",
                                "totp_required": false,
                                "email": "op@example.invalid",
                                "expires_at": "2026-10-26T00:15:00.000Z",
                            })
                            .to_string(),
                        ),
                        ("POST", "/v1/auth/magic/finalize") => (
                            200,
                            vec![(
                                "Set-Cookie".to_owned(),
                                "ast_session=STUB-SESSION; Path=/; HttpOnly".to_owned(),
                            )],
                            serde_json::json!({
                                "session_id": "sess_00000000000000000000000001",
                                "account_id": account_id,
                                "expires_at": "2026-10-26T01:15:00.000Z",
                                "totp_verified": false,
                            })
                            .to_string(),
                        ),
                        ("GET", "/v1/me") => {
                            if let Some((status, body)) = overridden.lock().unwrap().clone() {
                                (status, vec![], body)
                            } else {
                                (200, vec![], me_body.clone())
                            }
                        }
                        ("POST", "/v1/grants/issue") => (
                            200,
                            vec![],
                            serde_json::json!({
                                "token": "grant.jwt.stub",
                                "expires_at": "2026-10-26T00:05:00.000Z",
                            })
                            .to_string(),
                        ),
                        ("POST", "/v1/sessions/current/revoke")
                        | ("POST", "/v1/sessions/revoke-all") => (
                            200,
                            vec![],
                            serde_json::json!({"revoked": true}).to_string(),
                        ),
                        _ => (
                            404,
                            vec![],
                            serde_json::json!({"error": {"code": "not_found", "message": "stub", "request_id": "req-test"}})
                                .to_string(),
                        ),
                    };
                let mut response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response_body.len()
                );
                for (name, value) in headers {
                    response.push_str(&name);
                    response.push_str(": ");
                    response.push_str(&value);
                    response.push_str("\r\n");
                }
                response.push_str("\r\n");
                response.push_str(&response_body);
                let mut stream = reader.into_inner();
                if stream.write_all(response.as_bytes()).is_err() {
                    break;
                }
                drop(stream.flush());
            }
        });
        Self {
            _thread: thread,
            base_url: format!("http://{address}"),
            me_override,
        }
    }
}

fn wait_for<T>(
    cx: &TestAppContext,
    what: &str,
    check: impl Fn(&TestAppContext) -> Option<T>,
) -> T {
    for _ in 0..400 {
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        if let Some(value) = check(cx) {
            return value;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

/// Drives a full magic-link sign-in against the stub; returns the account id.
fn sign_in(
    cx: &mut TestAppContext,
    core: &gpui::Entity<Core>,
    email: &str,
) -> (gpui::Entity<Account>, String) {
    let account = core.read_with(cx, |core, _| core.account().clone());
    account.update(cx, |account, cx| {
        account.request_link(email.to_owned(), cx);
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
    let account_id = wait_for(cx, "the sign-in to complete", |cx| {
        core.read_with(cx, |core, _| core.account().clone())
            .read_with(cx, |account, _| {
                account.profile().map(|profile| profile.account_id.clone())
            })
    });
    (account, account_id)
}

/// A dialer that returns a plain (non-Relay) client: insertable through the
/// validated path, but never a live Relay route, so setup stays incomplete.
fn insertable_dialer(
    executor: gpui::BackgroundExecutor,
    identity: artisan_api::tls::Identity,
    machine: String,
    account: String,
) -> editor_core::RelayDialerFactory {
    use std::sync::Arc as _Arc;
    _Arc::new(move |_session: editor_core::SessionToken, _url: String| {
        let executor = executor.clone();
        let identity = identity.clone();
        let (machine, account) = (machine.clone(), account.clone());
        Some(_Arc::new(move |_req: RelayDialRequest| {
            let identity = identity.clone();
            let (machine, account) = (machine.clone(), account.clone());
            executor.spawn(async move {
                let client =
                    ForgeClient::new("127.0.0.1:9", &"00".repeat(32), &identity)?;
                let config = HostConfig {
                    id: HostId(machine.clone()),
                    name: "server".to_owned(),
                    address: String::new(),
                    fingerprint: String::new(),
                    kind: HostKind::Account { account, machine },
                };
                Ok((config, client))
            })
        }) as editor_core::RelayDialer)
    })
}

/// Writes a Relay onboarding record the way a proven setup persists it:
/// `accounts/{base64url(base)}-{account}.json` under the Editor data dir.
fn write_relay_record(data_dir: &std::path::Path, base_url: &str, account_id: &str) {
    use base64::Engine as _;
    let scope = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(base_url.as_bytes());
    let dir = data_dir.join("accounts");
    std::fs::create_dir_all(&dir).unwrap();
    let record = serde_json::json!({
        "account_id": account_id,
        "base_url": base_url,
        "machine_id": MACHINE_A,
        "method": "Relay",
        "completed_at": "1",
        "host_id": MACHINE_A,
    });
    std::fs::write(
        dir.join(format!("{scope}-{account_id}.json")),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
}

fn gate_of(cx: &TestAppContext, core: &gpui::Entity<Core>) -> EntryGate {
    core.read_with(cx, |core, cx| core.entry_gate(cx))
}

#[gpui::test]
fn fresh_device_signed_out_never_autoconnects_legacy(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let identity = files.identity().unwrap();
    let pin = artisan_api::tls::fingerprint(identity.certificate_der().unwrap().as_ref());
    // A legacy OS-user pairing on disk, from before account-only.
    let legacy = HostConfig {
        id: HostId("legacy-1".to_owned()),
        name: "Legacy".to_owned(),
        address: "127.0.0.1:9".to_owned(),
        fingerprint: pin,
        kind: HostKind::Local,
    };
    files
        .save_hosts(&editor_core::hosts::HostsFile {
            hosts: vec![legacy.clone()],
            last: Some(legacy.id.clone()),
        })
        .unwrap();
    let core = cx.new(|cx| Core::new(files, cx).unwrap());
    assert!(matches!(gate_of(cx, &core), EntryGate::SignedOut));
    assert!(core.read_with(cx, |core, _| core.hosts().next().is_none()));
    assert!(core.read_with(cx, |core, _| core.host(&legacy.id).is_none()));
    assert!(core.read_with(cx, |core, _| core.active_host().is_none()));
    // A deeplink while locked defers: Home stays, nothing loads.
    core.update(cx, |core, cx| {
        core.navigate(
            Screen::Thread {
                host: legacy.id.clone(),
                thread: artisan_api::ThreadId("t".to_owned()),
            },
            cx,
        );
    });
    assert_eq!(core.read_with(cx, |core, _| core.screen().clone()), Screen::Home);
    // The pairing survives on disk, disconnected, awaiting verified adoption.
    assert!(dir.path().join("editor").join("hosts.json").exists());
}

#[gpui::test]
fn restoring_holds_without_flash_on_transport(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    // A saved session whose Core is unreachable (closed port).
    std::fs::create_dir_all(dir.path().join("editor")).unwrap();
    std::fs::write(
        dir.path().join("editor").join("account.json"),
        serde_json::json!({
            "base_url": "http://127.0.0.1:1",
            "session_token": "STALE-TOKEN",
        })
        .to_string(),
    )
    .unwrap();
    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let core = cx.new(|cx| Core::new(files, cx).unwrap());
    // Restoring, not SignedOut: no workspace flash, no host connects.
    assert!(matches!(gate_of(cx, &core), EntryGate::Restoring));
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(100));
    cx.run_until_parked();
    assert!(matches!(gate_of(cx, &core), EntryGate::Restoring));
    assert!(core.read_with(cx, |core, _| core.hosts().next().is_none()));
    // The saved session is retained for retry, not dropped.
    let stored: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("editor").join("account.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        stored.get("session_token").and_then(|v| v.as_str()),
        Some("STALE-TOKEN")
    );
}

#[gpui::test]
fn login_reaches_needs_setup_with_hidden_inventory(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let stub = StubCore::new(ACCOUNT_A);
    let dir = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let base_url = stub.base_url.clone();
    let core = cx.new(|cx| {
        let core = Core::new(files, cx).unwrap();
        core.account().update(cx, |account, cx| {
            *account = Account::with_base_url(
                EditorFiles {
                    dir: dir.path().join("editor"),
                },
                Some(base_url),
                cx,
            );
        });
        core
    });
    let identity = core.read_with(cx, |core, _| core.identity.clone().unwrap());
    core.update(cx, |core, cx| {
        core.set_relay_dialer_factory(
            insertable_dialer(cx.background_executor().clone(), identity, MACHINE_A.to_owned(), ACCOUNT_A.to_owned()),
            cx,
        );
    });
    let (_account, account_id) = sign_in(cx, &core, "op@example.invalid");
    assert_eq!(account_id, ACCOUNT_A);
    // Authenticated but not onboarded: setup, with public inventory hidden.
    match gate_of(cx, &core) {
        EntryGate::NeedsSetup { account_id, .. } => assert_eq!(account_id, ACCOUNT_A),
        gate => panic!("expected NeedsSetup, got {gate:?}"),
    }
    // The real stub connect path inserts (grant + dial + guarded insertion,
    // no borrow panic) while staying publicly hidden until Ready.
    let account = core.read_with(cx, |core, _| core.account().clone());
    account.update(cx, |account, cx| {
        account.connect_machine(MACHINE_A.to_owned(), &core, cx);
    });
    wait_for(cx, "the route to open", |cx| {
        core.read_with(cx, |core, _| core.account().clone())
            .read_with(cx, |account, _| {
                account.notice().filter(|notice| notice.contains("is open")).map(str::to_owned)
            })
    });
    assert!(core.read_with(cx, |core, _| core.hosts().next().is_none()));
    assert!(
        core.read_with(cx, |core, cx| core
            .require_host(&HostId(MACHINE_A.to_owned()), cx)
            .is_none())
    );
    assert!(
        !core.read_with(cx, |core, cx| core.connected_account_host(MACHINE_A, cx)),
        "a non-Relay placeholder is not a live route"
    );
    // Workspace navigation while locked defers instead of loading.
    core.update(cx, |core, cx| {
        core.navigate(
            Screen::NewThread {
                host: HostId(MACHINE_A.to_owned()),
                project: None,
            },
            cx,
        );
    });
    assert_eq!(core.read_with(cx, |core, _| core.screen().clone()), Screen::Home);
}

#[gpui::test]
fn setup_rejects_placeholders_without_live_relay(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let stub = StubCore::new(ACCOUNT_A);
    let dir = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let base_url = stub.base_url.clone();
    let core = cx.new(|cx| {
        let core = Core::new(files, cx).unwrap();
        core.account().update(cx, |account, cx| {
            *account = Account::with_base_url(
                EditorFiles {
                    dir: dir.path().join("editor"),
                },
                Some(base_url),
                cx,
            );
        });
        core
    });
    let identity = core.read_with(cx, |core, _| core.identity.clone().unwrap());
    core.update(cx, |core, cx| {
        core.set_relay_dialer_factory(
            insertable_dialer(cx.background_executor().clone(), identity, MACHINE_A.to_owned(), ACCOUNT_A.to_owned()),
            cx,
        );
    });
    sign_in(cx, &core, "op@example.invalid");
    // No route at all: honest rejection, no persisted binding.
    let account = core.read_with(cx, |core, _| core.account().clone());
    let err = core
        .update(cx, |core, cx| {
            core.complete_setup(MACHINE_A.to_owned(), cx)
        })
        .unwrap_err();
    assert!(format!("{err:#}").contains("Relay"), "unexpected: {err:#}");
    // A connected-but-not-Relay placeholder is still insufficient.
    account.update(cx, |account, cx| {
        account.connect_machine(MACHINE_A.to_owned(), &core, cx);
    });
    wait_for(cx, "the route to open", |cx| {
        core.read_with(cx, |core, _| core.account().clone())
            .read_with(cx, |account, _| {
                account.notice().filter(|notice| notice.contains("is open")).map(str::to_owned)
            })
    });
    let err = core
        .update(cx, |core, cx| {
            core.complete_setup(MACHINE_A.to_owned(), cx)
        })
        .unwrap_err();
    assert!(format!("{err:#}").contains("Relay"), "unexpected: {err:#}");
    assert!(matches!(gate_of(cx, &core), EntryGate::NeedsSetup { .. }));
}

/// Unverified direct pairing is rejected in every gate state: production
/// `add_host` never persists or connects a legacy config, and the reply
/// guides to account enrollment. No pairing network runs.
#[gpui::test]
fn add_host_rejects_unverified_direct_in_needssetup(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let stub = StubCore::new(ACCOUNT_A);
    let dir = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let base_url = stub.base_url.clone();
    let core = cx.new(|cx| {
        let core = Core::new(files, cx).unwrap();
        core.account().update(cx, |account, cx| {
            *account = Account::with_base_url(
                EditorFiles {
                    dir: dir.path().join("editor"),
                },
                Some(base_url),
                cx,
            );
        });
        core
    });
    sign_in(cx, &core, "op@example.invalid");
    assert!(matches!(gate_of(cx, &core), EntryGate::NeedsSetup { .. }));
    let err = core
        .update(cx, |core, cx| {
            core.add_host(
                HostConfig {
                    id: HostId("legacy-b".to_owned()),
                    name: "Legacy B".to_owned(),
                    address: "127.0.0.1:9".to_owned(),
                    fingerprint: "00".repeat(32),
                    kind: HostKind::Local,
                },
                cx,
            )
        })
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("enroll this Forge into your account"),
        "unexpected: {err:#}"
    );
    assert!(!dir.path().join("editor").join("hosts.json").exists());
    assert!(core.read_with(cx, |core, _| core.hosts().next().is_none()));
}

/// A proven Relay record restores to Ready on restart and auto-reconnects
/// the recorded machine through the account; legacy pairings stay dark.
#[gpui::test]
fn restart_with_bound_record_ready_and_autoconnects(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let stub = StubCore::new(ACCOUNT_A);
    let dir = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let base_url = stub.base_url.clone();
    let core = cx.new(|cx| {
        let core = Core::new(files, cx).unwrap();
        core.account().update(cx, |account, cx| {
            *account = Account::with_base_url(
                EditorFiles {
                    dir: dir.path().join("editor"),
                },
                Some(base_url.clone()),
                cx,
            );
        });
        core
    });
    let identity = core.read_with(cx, |core, _| core.identity.clone().unwrap());
    core.update(cx, |core, cx| {
        core.set_relay_dialer_factory(
            insertable_dialer(cx.background_executor().clone(), identity.clone(), MACHINE_A.to_owned(), ACCOUNT_A.to_owned()),
            cx,
        );
    });
    sign_in(cx, &core, "op@example.invalid");
    // A legacy pairing on disk must never light up by itself.
    // (Written directly: production `add_host` rejects unverified direct
    // configs, so tests seed the file instead of the bypass.)
    std::fs::write(
        dir.path().join("editor").join("hosts.json"),
        serde_json::json!({
            "hosts": [{
                "id": "old-legacy",
                "name": "Old",
                "address": "127.0.0.1:9",
                "fingerprint": "00".repeat(32),
                "kind": {"type": "local"},
            }],
            "last": "old-legacy",
        })
        .to_string(),
    )
    .unwrap();
    // Simulate the proven setup having persisted its record earlier.
    write_relay_record(&dir.path().join("editor"), &stub.base_url, ACCOUNT_A);
    // Restart: same device dir, same session file.
    let files2 = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let base_url2 = stub.base_url.clone();
    let core2 = cx.new(|cx| {
        let core = Core::new(files2, cx).unwrap();
        core.account().update(cx, |account, cx| {
            *account = Account::with_base_url(
                EditorFiles {
                    dir: dir.path().join("editor"),
                },
                Some(base_url2),
                cx,
            );
        });
        core
    });
    let identity2 = core2.read_with(cx, |core, _| core.identity.clone().unwrap());
    core2.update(cx, |core, cx| {
        core.set_relay_dialer_factory(
            insertable_dialer(cx.background_executor().clone(), identity2, MACHINE_A.to_owned(), ACCOUNT_A.to_owned()),
            cx,
        );
    });
    // Resume validates, the bound record yields Ready, and the recorded
    // machine auto-reconnects through the account after borrow release.
    wait_for(cx, "the restart to reach Ready", |cx| {
        matches!(gate_of(cx, &core2), EntryGate::Ready { .. }).then_some(())
    });
    wait_for(cx, "the recorded route to reconnect", |cx| {
        core2
            .read_with(cx, |core, cx| {
                core.require_host(&HostId(MACHINE_A.to_owned()), cx)
            })
            .map(|_| ())
    });
    // Public inventory shows the Relay route and it is navigable; the
    // legacy pairing stays disconnected and unlisted.
    assert!(
        core2.read_with(cx, |core, _| core
            .hosts()
            .any(|(config, _)| config.id.0 == MACHINE_A))
    );
    assert!(core2.read_with(cx, |core, _| core.active_host().is_some()));
    core2.update(cx, |core, cx| {
        core.navigate(
            Screen::NewThread {
                host: HostId(MACHINE_A.to_owned()),
                project: None,
            },
            cx,
        );
    });
    assert!(
        core2
            .read_with(cx, |core, _| core.screen().clone())
            .host()
            .is_some()
    );
    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("editor").join("hosts.json")).unwrap(),
    )
    .unwrap();
    let ids: Vec<String> = persisted
        .get("hosts")
        .and_then(|hosts| hosts.as_array())
        .map(|hosts| {
            hosts
                .iter()
                .filter_map(|host| host.get("id").and_then(|id| id.as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    assert!(!ids.contains(&MACHINE_A.to_owned()), "Relay routes never persist");
    // Even Ready, an arbitrary legacy pairing is refused, not connected.
    let err = core2
        .update(cx, |core, cx| {
            core.add_host(
                HostConfig {
                    id: HostId("sneaky-b".to_owned()),
                    name: "Sneaky".to_owned(),
                    address: "127.0.0.1:9".to_owned(),
                    fingerprint: "ff".repeat(32),
                    kind: HostKind::Local,
                },
                cx,
            )
        })
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("enroll this Forge into your account"),
        "unexpected: {err:#}"
    );
    assert!(core2.read_with(cx, |core, _| core.host(&HostId("sneaky-b".to_owned())).is_none()));
    // Sign-out locks everything again.
    let account2 = core2.read_with(cx, |core, _| core.account().clone());
    account2.update(cx, Account::sign_out);
    wait_for(cx, "the lock to land", |cx| {
        matches!(gate_of(cx, &core2), EntryGate::SignedOut).then_some(())
    });
    assert!(core2.read_with(cx, |core, _| core.hosts().next().is_none()));
    assert!(
        core2
            .read_with(cx, |core, cx| core
                .require_host(&HostId(MACHINE_A.to_owned()), cx)
                .is_none())
    );
}

#[gpui::test]
fn switch_account_drops_previous_inventory(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let stub_a = StubCore::new(ACCOUNT_A);
    let stub_b = StubCore::new(ACCOUNT_B);
    let dir = tempfile::tempdir().unwrap();
    // A legacy pairing on disk (see restart test: file-seeded, never
    // production-paired).
    std::fs::create_dir_all(dir.path().join("editor")).unwrap();
    std::fs::write(
        dir.path().join("editor").join("hosts.json"),
        serde_json::json!({
            "hosts": [{
                "id": "a-direct",
                "name": "A",
                "address": "127.0.0.1:9",
                "fingerprint": "00".repeat(32),
                "kind": {"type": "local"},
            }],
            "last": null,
        })
        .to_string(),
    )
    .unwrap();
    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let base_a = stub_a.base_url.clone();
    let core = cx.new(|cx| {
        let core = Core::new(files, cx).unwrap();
        core.account().update(cx, |account, cx| {
            *account = Account::with_base_url(
                EditorFiles {
                    dir: dir.path().join("editor"),
                },
                Some(base_a),
                cx,
            );
        });
        core
    });
    let identity = core.read_with(cx, |core, _| core.identity.clone().unwrap());
    core.update(cx, |core, cx| {
        core.set_relay_dialer_factory(
            insertable_dialer(cx.background_executor().clone(), identity, MACHINE_A.to_owned(), ACCOUNT_A.to_owned()),
            cx,
        );
    });
    sign_in(cx, &core, "op@example.invalid");
    let account = core.read_with(cx, |core, _| core.account().clone());
    account.update(cx, Account::sign_out);
    wait_for(cx, "the sign-out to land", |cx| {
        matches!(gate_of(cx, &core), EntryGate::SignedOut).then_some(())
    });
    // Switch identity: a fresh controller on Core B in the same device dir.
    let base_b = stub_b.base_url.clone();
    core.update(cx, |core, cx| {
        core.account().update(cx, |account, cx| {
            *account = Account::with_base_url(
                EditorFiles {
                    dir: dir.path().join("editor"),
                },
                Some(base_b),
                cx,
            );
        });
    });
    let (_account_b, account_id) = sign_in(cx, &core, "op@example.invalid");
    assert_eq!(account_id, ACCOUNT_B);
    match gate_of(cx, &core) {
        EntryGate::NeedsSetup { account_id, .. } => assert_eq!(account_id, ACCOUNT_B),
        gate => panic!("expected NeedsSetup for B, got {gate:?}"),
    }
    assert!(core.read_with(cx, |core, _| core.hosts().next().is_none()));
    assert!(
        core.read_with(cx, |core, _| core.host(&HostId("a-direct".to_owned())).is_none())
    );
}

#[gpui::test]
fn stale_connect_after_signout_never_repopulates(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let stub = StubCore::new(ACCOUNT_A);
    let dir = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let base_url = stub.base_url.clone();
    let core = cx.new(|cx| {
        let core = Core::new(files, cx).unwrap();
        core.account().update(cx, |account, cx| {
            *account = Account::with_base_url(
                EditorFiles {
                    dir: dir.path().join("editor"),
                },
                Some(base_url),
                cx,
            );
        });
        core
    });
    sign_in(cx, &core, "op@example.invalid");
    // A dialer that hangs until released: sign out mid-flight. A oneshot
    // (not `Notify`) carries the release past the grant/dial race.
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let parked: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(Some(rx)));
    let release = Arc::new(Mutex::new(Some(tx)));
    let executor = cx.background_executor.clone();
    let identity = core.read_with(cx, |core, _| core.identity.clone().unwrap());
    core.update(cx, |core, cx| {
        core.set_relay_dialer_factory(
            Arc::new(move |_session: editor_core::SessionToken, _url: String| {
                let executor = executor.clone();
                let identity = identity.clone();
                let parked = Arc::clone(&parked);
                Some(Arc::new(move |req: RelayDialRequest| {
                    let identity = identity.clone();
                    let parked = Arc::clone(&parked);
                    executor.spawn(async move {
                        let rx = parked.lock().unwrap().take();
                        if let Some(rx) = rx {
                            let _ = rx.await;
                        }
                        let client =
                            ForgeClient::new("127.0.0.1:9", &"00".repeat(32), &identity)?;
                        let config = HostConfig {
                            id: HostId(req.machine_id.clone()),
                            name: "server".to_owned(),
                            address: String::new(),
                            fingerprint: String::new(),
                            kind: HostKind::Account {
                                account: req.account_id.clone(),
                                machine: req.machine_id.clone(),
                            },
                        };
                        Ok((config, client))
                    })
                }) as editor_core::RelayDialer)
            }),
            cx,
        );
    });
    let account = core.read_with(cx, |core, _| core.account().clone());
    account.update(cx, |account, cx| {
        account.connect_machine(MACHINE_A.to_owned(), &core, cx);
    });
    account.update(cx, Account::sign_out);
    release.lock().unwrap().take().map(|tx| tx.send(()));
    wait_for(cx, "the stale dial to settle locked", |cx| {
        cx.run_until_parked();
        matches!(gate_of(cx, &core), EntryGate::SignedOut).then_some(())
    });
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(50));
    cx.run_until_parked();
    assert!(core.read_with(cx, |core, _| core.hosts().next().is_none()));
    assert!(
        core.read_with(cx, |core, cx| core
            .require_host(&HostId(MACHINE_A.to_owned()), cx)
            .is_none())
    );
}

#[gpui::test]
fn revocation_locks_and_clears_dead_session(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let stub = StubCore::new(ACCOUNT_A);
    let dir = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let base_url = stub.base_url.clone();
    let core = cx.new(|cx| {
        let core = Core::new(files, cx).unwrap();
        core.account().update(cx, |account, cx| {
            *account = Account::with_base_url(
                EditorFiles {
                    dir: dir.path().join("editor"),
                },
                Some(base_url),
                cx,
            );
        });
        core
    });
    sign_in(cx, &core, "op@example.invalid");
    *stub.me_override.lock().unwrap() = Some((
        401,
        serde_json::json!({"error": {"code": "unauthorized", "message": "stub", "request_id": "req-test"}})
            .to_string(),
    ));
    let account = core.read_with(cx, |core, _| core.account().clone());
    account.update(cx, Account::refresh);
    wait_for(cx, "the revocation to lock", |cx| {
        matches!(gate_of(cx, &core), EntryGate::SignedOut).then_some(())
    });
    assert!(core.read_with(cx, |core, _| core.hosts().next().is_none()));
    let stored: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("editor").join("account.json")).unwrap(),
    )
    .unwrap();
    assert!(stored.get("session_token").is_none(), "dead token cleared");
}

#[test]
fn onboarding_records_scope_by_core_origin() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    let base_a = "https://api.artisan.st";
    let base_b = "http://127.0.0.1:8787";
    for base in [base_a, base_b] {
        let scope = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(base.as_bytes());
        let accounts = dir.path().join("accounts");
        std::fs::create_dir_all(&accounts).unwrap();
        let record = serde_json::json!({
            "account_id": ACCOUNT_A,
            "base_url": base,
            "machine_id": MACHINE_A,
            "method": "Relay",
            "completed_at": "1",
            "host_id": MACHINE_A,
        });
        std::fs::write(
            accounts.join(format!("{scope}-{}.json", ACCOUNT_A)),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .unwrap();
    }
    // Same account id, two origins: two distinct records, each self-scoped.
    let entries: Vec<_> = std::fs::read_dir(dir.path().join("accounts"))
        .unwrap()
        .collect();
    assert_eq!(entries.len(), 2);
    for entry in entries {
        let record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
        assert_eq!(record.get("account_id").and_then(|v| v.as_str()), Some(ACCOUNT_A));
        assert!(record.get("base_url").and_then(|v| v.as_str()).is_some());
    }
}
