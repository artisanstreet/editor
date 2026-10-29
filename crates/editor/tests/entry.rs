//! The entry gate as a user meets it: resume, setup, sign-out and deep
//! links, against a stub Core and a scripted `ae`. Path-included into
//! `src/entry.rs`, so `super::` reaches the private root.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::time::Duration;

use super::EntryRoot;
use editor_core::{
    Account, Core, EditorFiles, EntryGate, Screen, Services, SettingsPage,
    save_onboarding_for_tests,
};
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use serde_json::json;

const ACCOUNT_ID: &str = "acct_test000000000000000000000001";
const MACHINE_ID: &str = "mch_test00000000000000000000001";

fn profile() -> serde_json::Value {
    json!({
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
}

/// Answers `/v1/me` with [`profile`] and session revokes with success.
fn stub_core() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                head.push_str(&line);
            }
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0);
            let mut body = vec![0_u8; length];
            reader.read_exact(&mut body).ok();
            let request = head.lines().next().unwrap_or_default();
            let (status, body) = if request.starts_with("GET /v1/me ") {
                (200, profile().to_string())
            } else if request.starts_with("POST /v1/sessions/current/revoke ") {
                (200, json!({"revoked": true}).to_string())
            } else {
                let error = json!({"error": {"code": "not_found", "message": "stub"}});
                (404, error.to_string())
            };
            let mut stream = reader.into_inner();
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .ok();
        }
    });
    format!("http://{address}")
}

struct Fixture {
    _dir: tempfile::TempDir,
    core: Entity<Core>,
}

impl Fixture {
    /// A device with a saved session for `core_url`; `profile` is the one an
    /// earlier run saved, and `onboarded` whether setup finished here.
    fn new(
        cx: &mut TestAppContext,
        core_url: &str,
        profile: Option<serde_json::Value>,
        onboarded: bool,
    ) -> Self {
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let files = EditorFiles {
            dir: dir.path().join("editor"),
        };
        std::fs::create_dir_all(&files.dir).unwrap();
        let mut account = json!({"base_url": core_url, "session_token": "fixture-session"});
        if let Some(profile) = profile {
            account["profile"] = profile;
        }
        std::fs::write(files.dir.join("account.json"), account.to_string()).unwrap();
        if onboarded {
            save_onboarding_for_tests(&files, core_url, ACCOUNT_ID, MACHINE_ID).unwrap();
        }
        let services = Services::scripted(
            core_url,
            std::sync::Arc::new(|_| Ok(r#"{"enrolled":false,"status":"none"}"#.to_owned())),
            None,
        );
        let core = cx.new(|cx| Core::with_services(files, services, cx).unwrap());
        Self { _dir: dir, core }
    }

    fn open<'a>(&self, cx: &'a mut TestAppContext) -> &'a mut VisualTestContext {
        let core = self.core.clone();
        cx.add_window_view(move |window, cx| EntryRoot::new(core, None, window, cx))
            .1
    }

    fn gate(&self, cx: &VisualTestContext) -> EntryGate {
        self.core.read_with(cx, |core, cx| core.entry_gate(cx))
    }

    fn account(
        &self,
        cx: &mut VisualTestContext,
        action: impl FnOnce(&mut Account, &mut gpui::Context<Account>),
    ) {
        let account = self.core.read_with(cx, |core, _| core.account().clone());
        account.update(cx, action);
    }
}

/// Runs the Editor's tasks, and gives the stub Core's thread time, until
/// `check` holds.
async fn wait_for(
    cx: &mut VisualTestContext,
    what: &str,
    mut check: impl FnMut(&mut VisualTestContext) -> bool,
) {
    for _ in 0..500 {
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        cx.executor().timer(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

#[gpui::test]
async fn resume_validates_then_ready_mounts_workspace(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx, &stub_core(), None, true);
    let cx = fixture.open(cx);
    assert!(cx.debug_bounds("artisan-desktop-workspace").is_none());
    wait_for(cx, "the workspace", |cx| {
        cx.debug_bounds("artisan-desktop-workspace").is_some()
    })
    .await;
    assert!(cx.debug_bounds("entry-root").is_none());
}

#[gpui::test]
async fn offline_resume_with_a_saved_profile_mounts_the_workspace(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx, "http://127.0.0.1:1", Some(profile()), true);
    let cx = fixture.open(cx);
    cx.run_until_parked();
    assert!(cx.debug_bounds("artisan-desktop-workspace").is_some());
    assert!(matches!(fixture.gate(cx), EntryGate::Ready { .. }));
}

#[gpui::test]
async fn sign_out_drops_mounted_workspace_back_to_entry(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx, &stub_core(), None, true);
    let cx = fixture.open(cx);
    wait_for(cx, "the workspace", |cx| {
        cx.debug_bounds("artisan-desktop-workspace").is_some()
    })
    .await;
    fixture.account(cx, Account::sign_out);
    cx.run_until_parked();
    assert!(cx.debug_bounds("artisan-desktop-workspace").is_none());
    assert!(cx.debug_bounds("sign-in-submit").is_some());
    assert_eq!(fixture.gate(cx), EntryGate::SignedOut);
}

#[gpui::test]
async fn setup_can_switch_to_another_account(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx, &stub_core(), None, false);
    let cx = fixture.open(cx);
    wait_for(cx, "the setup screen", |cx| {
        cx.debug_bounds("forges-use-another").is_some()
    })
    .await;
    let switch = cx.debug_bounds("forges-use-another").unwrap();
    cx.simulate_click(switch.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("sign-in-submit").is_some());
    assert!(cx.debug_bounds("artisan-desktop-workspace").is_none());
    assert_eq!(fixture.gate(cx), EntryGate::SignedOut);
}

#[gpui::test]
async fn a_cold_start_deep_link_opens_once_ready(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx, &stub_core(), None, true);
    let link = Screen::Settings(SettingsPage::General);
    fixture
        .core
        .update(cx, |core, cx| core.navigate(link.clone(), cx));
    let cx = fixture.open(cx);
    assert!(cx.debug_bounds("settings-dialog").is_none());
    wait_for(cx, "the linked screen", |cx| {
        cx.debug_bounds("settings-dialog").is_some()
    })
    .await;
    assert_eq!(
        fixture.core.read_with(cx, |core, _| core.screen().clone()),
        link
    );
}
