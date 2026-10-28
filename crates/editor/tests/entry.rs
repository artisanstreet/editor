//! Entry-guard behavior through the real controller path: a stub Core
//! validates the saved session, the durable onboarding record flips the
//! gate to `Ready`, the workspace mounts with no startup flash, and
//! sign-out drops it again. Path-included into `src/entry.rs`, so `super::`
//! reaches the private root. Not an integration target (`autotests = false`).

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::time::Duration;

use super::EntryRoot;
use editor_core::{Core, EditorFiles, EntryGate, Services};
use editor_settings::account::controller;
use gpui::AppContext as _;

pub(crate) const ACCOUNT_ID: &str = "acct_test000000000000000000000001";
pub(crate) const MACHINE_ID: &str = "mch_test00000000000000000000001";
const SESSION: &str = "fixture-opaque-session";

struct StubCore {
    _thread: std::thread::JoinHandle<()>,
    base_url: String,
}

impl StubCore {
    fn new() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming().take(50) {
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
                let method = parts.next().unwrap_or_default();
                let path = parts.next().unwrap_or_default();
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
                let body = match (method, path) {
                    ("GET", "/v1/me") => serde_json::json!({
                        "account_id": ACCOUNT_ID,
                        "email": "op@example.invalid",
                        "display_name": "Op",
                        "totp_enrolled": false,
                        "machines": [{
                            "id": MACHINE_ID,
                            "label": "test-forge",
                            "public_key": {
                                "kty": "OKP",
                                "crv": "Ed25519",
                                "x": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
                            },
                            "created_at": "2026-10-26T00:00:00.000Z",
                            "revoked_at": null,
                            "last_seen_at": null
                        }],
                        "latest_config_version": 3,
                    })
                    .to_string(),
                    ("POST", "/v1/sessions/current/revoke") => {
                        serde_json::json!({"revoked": true}).to_string()
                    }
                    _ => serde_json::json!({
                        "error": {
                            "code": "not_found",
                            "message": "stub",
                            "request_id": "req-test"
                        }
                    })
                    .to_string(),
                };
                let status = if body.contains("not_found") { 404 } else { 200 };
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
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
        }
    }
}

fn base64_url_no_pad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let mut word = 0_u32;
        for (index, byte) in chunk.iter().enumerate() {
            word |= (*byte as u32) << (16 - 8 * index);
        }
        let digits = match chunk.len() {
            3 => 4,
            2 => 3,
            _ => 2,
        };
        for index in 0..digits {
            let sextet = ((word >> (18 - 6 * index)) & 0x3f) as usize;
            out.push(ALPHABET[sextet] as char);
        }
    }
    out
}

pub(crate) struct EntryFixture {
    _dir: tempfile::TempDir,
    _stub: StubCore,
    pub(crate) core: gpui::Entity<Core>,
}

impl EntryFixture {
    pub(crate) fn new(cx: &mut gpui::TestAppContext) -> Self {
        Self::new_impl(cx, true)
    }

    /// Same session but no durable onboarding record: resume lands in
    /// NeedsSetup instead of Ready.
    pub(crate) fn new_without_record(cx: &mut gpui::TestAppContext) -> Self {
        Self::new_impl(cx, false)
    }

    fn new_impl(cx: &mut gpui::TestAppContext, with_record: bool) -> Self {
        let stub = StubCore::new();
        let dir = tempfile::tempdir().unwrap();
        let editor_dir = dir.path().join("editor");
        std::fs::create_dir_all(&editor_dir).unwrap();
        std::fs::write(
            editor_dir.join("account.json"),
            serde_json::json!({
                "base_url": stub.base_url,
                "session_token": SESSION,
            })
            .to_string(),
        )
        .unwrap();
        if with_record {
            let record = serde_json::json!({
                "account_id": ACCOUNT_ID,
                "base_url": stub.base_url,
                "machine_id": MACHINE_ID,
                "method": "Relay",
                "completed_at": "2026-10-26T00:00:00.000Z",
            });
            let name = format!(
                "{}-{ACCOUNT_ID}.json",
                base64_url_no_pad(stub.base_url.as_bytes())
            );
            let accounts = editor_dir.join("accounts");
            std::fs::create_dir_all(&accounts).unwrap();
            std::fs::write(accounts.join(name), record.to_string()).unwrap();
        }
        let services = Services::scripted(
            &stub.base_url,
            std::sync::Arc::new(|_| {
                Ok(r#"{"enrolled":false,"enrollment_id":null,"status":"none"}"#.to_owned())
            }),
            None,
        );
        let core = cx
            .new(|cx| Core::with_services(EditorFiles { dir: editor_dir }, services, cx).unwrap());
        Self {
            _dir: dir,
            _stub: stub,
            core,
        }
    }
}

fn wait_for(
    cx: &mut gpui::VisualTestContext,
    what: &str,
    mut check: impl FnMut(&mut gpui::VisualTestContext) -> bool,
) {
    for _ in 0..400 {
        cx.executor().allow_parking();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

/// Blocks until the fixture account validates and the gate reports NeedsSetup.
pub(crate) fn wait_needs_setup(cx: &mut gpui::TestAppContext, core: &gpui::Entity<Core>) {
    for _ in 0..400 {
        cx.executor().allow_parking();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        if core.read_with(cx, |core, cx| {
            matches!(core.entry_gate(cx), EntryGate::NeedsSetup { .. })
        }) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for the entry gate to report NeedsSetup");
}

/// Blocks until the fixture account validates and the gate reports Ready.
pub(crate) fn wait_ready(cx: &mut gpui::TestAppContext, core: &gpui::Entity<Core>) {
    for _ in 0..400 {
        cx.executor().allow_parking();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        if core.read_with(cx, |core, cx| {
            matches!(core.entry_gate(cx), EntryGate::Ready { .. })
        }) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for the entry gate to report Ready");
}

#[gpui::test]
fn resume_validates_then_ready_mounts_workspace(cx: &mut gpui::TestAppContext) {
    cx.executor().allow_parking();
    let fixture = EntryFixture::new(cx);
    let gate = fixture.core.read_with(cx, |core, cx| core.entry_gate(cx));
    assert!(
        !matches!(gate, EntryGate::Ready { .. }),
        "no workspace flash before resume validates, got {gate:?}"
    );
    let core = fixture.core.clone();
    let (_entry, cx) = cx.add_window_view(move |window, cx| EntryRoot::new(core, None, window, cx));
    assert!(cx.debug_bounds("entry-root").is_some());
    assert!(cx.debug_bounds("artisan-desktop-workspace").is_none());
    wait_for(cx, "resume to validate and mount the workspace", |cx| {
        cx.debug_bounds("artisan-desktop-workspace").is_some()
    });
    assert!(cx.debug_bounds("entry-root").is_none());
}

#[gpui::test]
fn sign_out_drops_mounted_workspace_back_to_entry(cx: &mut gpui::TestAppContext) {
    cx.executor().allow_parking();
    let fixture = EntryFixture::new(cx);
    let core = fixture.core.clone();
    let (_entry, cx) = cx.add_window_view(move |window, cx| EntryRoot::new(core, None, window, cx));
    wait_for(cx, "resume to validate and mount the workspace", |cx| {
        cx.debug_bounds("artisan-desktop-workspace").is_some()
    });
    let core = fixture.core.clone();
    cx.update(|_, cx| controller::sign_out(&core, false, cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("artisan-desktop-workspace").is_none());
    assert!(cx.debug_bounds("entry-root").is_some());
    let gate = fixture.core.read_with(cx, |core, cx| core.entry_gate(cx));
    assert!(
        matches!(gate, EntryGate::SignedOut),
        "sign-out must relock the gate, got {gate:?}"
    );
}

/// A signed-in account without onboarding stays gated: retry revalidates
/// without unlocking, and the setup surface offers switching accounts.
#[gpui::test]
fn needs_setup_offers_sign_out_and_retry_without_unlocking(cx: &mut gpui::TestAppContext) {
    cx.executor().allow_parking();
    let fixture = EntryFixture::new_without_record(cx);
    wait_needs_setup(cx, &fixture.core);
    let core = fixture.core.clone();
    let (_entry, cx) = cx.add_window_view(move |window, cx| EntryRoot::new(core, None, window, cx));
    wait_for(cx, "the setup surface to render", |cx| {
        cx.debug_bounds("entry-setup").is_some()
    });
    assert!(cx.debug_bounds("artisan-desktop-workspace").is_none());
    let core = fixture.core.clone();
    cx.update(|_, cx| controller::refresh(&core, cx));
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("entry-setup").is_some(),
        "retry must keep the setup surface, not unlock"
    );
    assert!(cx.debug_bounds("artisan-desktop-workspace").is_none());
    let switcher = cx
        .debug_bounds("entry-setup-use-another")
        .expect("the setup surface offers switching accounts");
    cx.simulate_click(switcher.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("artisan-desktop-workspace").is_none());
    assert!(
        cx.debug_bounds("sign-in-submit").is_some(),
        "switching accounts returns to sign-in"
    );
    let gate = fixture.core.read_with(cx, |core, cx| core.entry_gate(cx));
    assert!(
        matches!(gate, EntryGate::SignedOut),
        "switching accounts must relock the gate, got {gate:?}"
    );
}

#[gpui::test]
fn deep_links_do_not_mount_workspace_while_locked(cx: &mut gpui::TestAppContext) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let core = cx.new(|cx| {
        Core::new(
            EditorFiles {
                dir: dir.path().join("editor"),
            },
            cx,
        )
        .unwrap()
    });
    core.update(cx, |core, cx| {
        core.navigate(
            editor_core::Screen::Settings(editor_core::SettingsPage::Account),
            cx,
        );
    });
    let (_entry, cx) =
        cx.add_window_view(|window, cx| EntryRoot::new(core.clone(), None, window, cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("entry-root").is_some());
    assert!(cx.debug_bounds("artisan-desktop-workspace").is_none());
}
