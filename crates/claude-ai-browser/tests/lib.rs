use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::*;

const WAIT: Duration = Duration::from_secs(10);

struct FakeBrowser {
    methods: mpsc::UnboundedReceiver<String>,
}

impl FakeBrowser {
    async fn start(profile: &Path, organization: Option<&'static str>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the fake DevTools port");
        let port = listener.local_addr().expect("fake DevTools address").port();
        std::fs::create_dir_all(profile).expect("create the profile");
        std::fs::write(
            profile.join("DevToolsActivePort"),
            format!("{port}\n/devtools/browser/fake\n"),
        )
        .expect("write the DevTools port file");
        let (sender, methods) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, organization, sender.clone()));
            }
        });
        Self { methods }
    }

    async fn next(&mut self) -> String {
        tokio::time::timeout(WAIT, self.methods.recv())
            .await
            .expect("the browser hears from the capture in time")
            .expect("the fake browser runs")
    }

    async fn wait_for(&mut self, method: &str) {
        while self.next().await != method {}
    }

    fn heard(&mut self) -> Vec<String> {
        std::iter::from_fn(|| self.methods.try_recv().ok()).collect()
    }
}

async fn serve(
    stream: tokio::net::TcpStream,
    organization: Option<&'static str>,
    methods: mpsc::UnboundedSender<String>,
) {
    let mut socket = tokio_tungstenite::accept_async(stream)
        .await
        .expect("accept the DevTools connection");
    let mut clearance = 0;
    while let Some(Ok(Message::Text(text))) = socket.next().await {
        let request: Value = serde_json::from_str(&text).expect("a DevTools request");
        let method = request["method"].as_str().expect("a method").to_owned();
        let result = match method.as_str() {
            "Storage.getCookies" => {
                clearance += 1;
                let mut cookies = vec![
                    json!({ "name": "cf_clearance", "value": clearance.to_string(), "domain": ".claude.ai" }),
                ];
                if let Some(organization) = organization {
                    cookies.push(
                        json!({ "name": "sessionKey", "value": "secret", "domain": ".claude.ai" }),
                    );
                    cookies.push(json!({ "name": "lastActiveOrg", "value": organization, "domain": "claude.ai" }));
                }
                json!({ "cookies": cookies })
            }
            "Browser.getVersion" => json!({ "userAgent": "Fake/1" }),
            _ => json!({}),
        };
        methods.send(method).expect("the test listens");
        let answer = json!({ "id": request["id"], "result": result });
        if socket
            .send(Message::text(answer.to_string()))
            .await
            .is_err()
        {
            return;
        }
    }
}

fn signed_in_to(organization: &str) -> ClaudeAiSession {
    ClaudeAiSession {
        cookies: vec![
            ("sessionKey".to_owned(), "secret".to_owned()),
            ("lastActiveOrg".to_owned(), organization.to_owned()),
        ],
        user_agent: "Fake/1".to_owned(),
    }
}

#[tokio::test]
async fn an_abandoned_sign_in_closes_the_browser() {
    let folder = tempfile::tempdir().expect("temporary folder");
    let profile = folder.path().join("profile");
    let mut browser = FakeBrowser::start(&profile, None).await;
    let task = tokio::spawn({
        let profile = profile.clone();
        async move { sign_in_with(&profile, || None).await }
    });
    browser.wait_for("Storage.getCookies").await;
    task.abort();
    browser.wait_for("Browser.close").await;
}

#[tokio::test]
async fn a_renewal_leaves_a_sign_in_window_alone() {
    let folder = tempfile::tempdir().expect("temporary folder");
    let profile = folder.path().join("profile");
    let mut browser = FakeBrowser::start(&profile, None).await;
    authorization::authorize_in(&profile, "forge", &signed_in_to("org-a")).expect("authorize");
    let sign_in = tokio::spawn({
        let profile = profile.clone();
        async move { sign_in_with(&profile, || None).await }
    });
    browser.wait_for("Storage.getCookies").await;
    let renewal = renew_with(&profile, "forge", || None).await;
    assert!(
        matches!(
            renewal,
            Err(RenewError::Open {
                source: OpenError::Busy,
                ..
            })
        ),
        "{renewal:?}"
    );
    assert!(
        !browser
            .heard()
            .iter()
            .any(|method| method == "Browser.close"),
        "the renewal closed the window the user is signing in in"
    );
    sign_in.abort();
}

#[tokio::test]
async fn a_host_this_device_did_not_sign_in_gets_no_session() {
    let folder = tempfile::tempdir().expect("temporary folder");
    let profile = folder.path().join("profile");
    let mut browser = FakeBrowser::start(&profile, Some("org-a")).await;
    authorization::authorize_in(&profile, "forge", &signed_in_to("org-a")).expect("authorize");
    let renewal = renew_with(&profile, "stranger", || None).await;
    assert!(
        matches!(&renewal, Err(RenewError::NotAuthorized { host }) if host == "stranger"),
        "{renewal:?}"
    );
    assert_eq!(
        browser.heard(),
        Vec::<String>::new(),
        "the browser was opened"
    );
}

#[tokio::test]
async fn a_browser_now_on_another_account_does_not_renew_the_host() {
    let folder = tempfile::tempdir().expect("temporary folder");
    let profile = folder.path().join("profile");
    let _browser = FakeBrowser::start(&profile, Some("org-b")).await;
    authorization::authorize_in(&profile, "forge", &signed_in_to("org-a")).expect("authorize");
    let renewal = renew_with(&profile, "forge", || None).await;
    assert!(
        matches!(&renewal, Err(RenewError::AccountChanged { host }) if host == "forge"),
        "{renewal:?}"
    );
}

#[tokio::test]
async fn the_host_this_device_signed_in_is_renewed() {
    let folder = tempfile::tempdir().expect("temporary folder");
    let profile = folder.path().join("profile");
    let mut browser = FakeBrowser::start(&profile, Some("org-a")).await;
    authorization::authorize_in(&profile, "forge", &signed_in_to("org-a")).expect("authorize");
    let session = renew_with(&profile, "forge", || None)
        .await
        .expect("the session renews");
    assert_eq!(session.cookie("lastActiveOrg"), Some("org-a"));
    browser.wait_for("Browser.close").await;
}

#[test]
fn only_claude_ai_cookies_leave_the_browser() {
    let cookies = json!([
        { "name": "sessionKey", "value": "abc", "domain": ".claude.ai" },
        { "name": "lastActiveOrg", "value": "org", "domain": "claude.ai" },
        { "name": "SID", "value": "google", "domain": ".google.com" },
        { "name": "evil", "value": "x", "domain": "notclaude.ai" },
    ]);
    assert_eq!(
        claude_ai_cookies(&cookies),
        vec![
            ("sessionKey".to_owned(), "abc".to_owned()),
            ("lastActiveOrg".to_owned(), "org".to_owned()),
        ]
    );
}

#[test]
fn the_port_file_names_the_browser_endpoint() {
    assert_eq!(
        devtools::parse_port_file("9222\n/devtools/browser/abc-123\n"),
        Ok((9222, "/devtools/browser/abc-123".to_owned()))
    );
    assert_eq!(
        devtools::parse_port_file("9222\n"),
        Err(PortFileError::NoPath)
    );
    assert!(matches!(
        devtools::parse_port_file("x\n/devtools/browser/a"),
        Err(PortFileError::Port { port, .. }) if port == "x"
    ));
    assert_eq!(
        devtools::parse_port_file("9222\n/json/version"),
        Err(PortFileError::Path {
            path: "/json/version".to_owned()
        })
    );
}
