use super::*;

#[test]
fn status_probes_classify_recorded_output() {
    let claude_in = r#"{
  "loggedIn": true,
  "authMethod": "claude.ai",
  "apiProvider": "firstParty",
  "email": "user@example.com",
  "subscriptionType": "max"
}"#;
    let claude_out = r#"{
  "loggedIn": false,
  "authMethod": "none",
  "apiProvider": "firstParty"
}"#;
    let cases = [
        (EngineId::Claude, true, claude_in, ""),
        (EngineId::Claude, false, claude_out, ""),
        (EngineId::Claude, false, "", "error: unknown command 'auth'"),
        (EngineId::Codex, true, "", "Logged in using ChatGPT\n"),
        (
            EngineId::Codex,
            false,
            "",
            "WARNING: proceeding, even though we could not create PATH aliases\nNot logged in\n",
        ),
    ];
    let results: Vec<_> = cases
        .iter()
        .map(|(engine, success, stdout, stderr)| {
            (
                engine.as_str(),
                parse_status(*engine, *success, stdout, stderr),
            )
        })
        .collect();
    insta::assert_debug_snapshot!(results);
}

#[test]
fn login_output_yields_link_and_code() {
    let claude = "Opening browser to sign in…\nIf the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&client_id=x&state=y\nPaste code here if prompted > ";
    let codex = "\nWelcome to Codex [v\u{1b}[90m0.162.1\u{1b}[0m]\n\nFollow these steps to sign in with ChatGPT using device code authorization:\n\n1. Open this link in your browser and sign in to your account\n   \u{1b}[94mhttps://auth.openai.com/codex/device\u{1b}[0m\n\n2. Enter this one-time code \u{1b}[90m(expires in 15 minutes)\u{1b}[0m\n   \u{1b}[94mQES3-C7POE\u{1b}[0m\n";
    let results: Vec<_> = [("claude", claude), ("codex", codex)]
        .into_iter()
        .map(|(engine, text)| (engine, parse_login_output(text)))
        .collect();
    insta::assert_debug_snapshot!(results);
}

#[test]
fn opencode_device_login_yields_link_and_code() {
    let opencode = "\u{250c}  Connect an integration\n\u{25cf}  Enter code: HNCP-VWCS\n\u{2502}\n\u{25cf}  https://opencode.ai/console/device?user_code=HNCP-VWCS&client_id=opencode-cli\n\u{25c7}  Waiting for authorization...\n";
    assert_eq!(
        parse_login_output(opencode),
        (
            Some(
                "https://opencode.ai/console/device?user_code=HNCP-VWCS&client_id=opencode-cli"
                    .to_owned()
            ),
            Some("HNCP-VWCS".to_owned())
        )
    );
}

#[cfg(unix)]
#[tokio::test]
async fn opencode_is_signed_in_by_its_logins_and_signs_out_of_all() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let binary = fixture.path().join("opencode");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
case "$*" in
 "api --standalone integration.list")
  if [ -e "$PWD/console" ]; then login='[{"type":"credential","id":"cred_x","label":"Default","method":"oauth"}]'; else login='[]'; fi
  printf '{"data":[{"id":"opencode","name":"OpenCode Console","connections":%s},{"id":"opencode-go","name":"OpenCode Go","connections":[]}]}' "$login" ;;
 "auth logout --standalone opencode cred_x") rm "$PWD/console" ;;
 *) exit 2 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sign_in = SignIn::new(EngineId::OpenCode, home.clone());
    sign_in.check(&binary).await;
    assert_eq!(sign_in.state(), SignInState::SignedOut);
    std::fs::write(home.join("console"), "").unwrap();
    sign_in.check(&binary).await;
    assert_eq!(
        sign_in.state(),
        SignInState::SignedIn {
            account: Some("OpenCode Console".to_owned())
        }
    );
    sign_in.logout(&binary);
    tokio::time::timeout(Duration::from_secs(5), async {
        while sign_in.state() != SignInState::SignedOut {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("signed out of every login");
    assert!(!home.join("console").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn stale_status_probes_cannot_restore_an_account_after_logout() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = tempfile::tempdir().unwrap();
    let home = fixture.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let binary = fixture.path().join("claude");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
if [ "$2" = "logout" ]; then touch "$PWD/out"; exit 0; fi
if [ -e "$PWD/out" ]; then
  echo '{"loggedIn":false}'
else
  if [ -e "$PWD/slow" ]; then touch "$PWD/check-started"; sleep 0.2; fi
  echo '{"loggedIn":true,"email":"old"}'
fi
"#,
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sign_in = SignIn::new(EngineId::Claude, home.clone());
    sign_in.check(&binary).await;
    assert!(matches!(sign_in.state(), SignInState::SignedIn { .. }));
    std::fs::write(home.join("slow"), "").unwrap();
    let probe_owner = Arc::clone(&sign_in);
    let probe_binary = binary.clone();
    let old_probe = tokio::spawn(async move { probe_owner.check(&probe_binary).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !home.join("check-started").exists() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    sign_in.logout(&binary);
    tokio::time::timeout(Duration::from_secs(2), async {
        while sign_in.state() != SignInState::SignedOut {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    old_probe.await.unwrap();
    assert_eq!(sign_in.state(), SignInState::SignedOut);
}

#[test]
fn login_walks_from_signed_out_to_signed_in() {
    let mut machine = Machine::default();
    let mut effects = Vec::new();
    let mut seen = Vec::new();
    for event in [
        Event::Checked(SignInState::SignedOut),
        Event::Login,
        Event::Prompt {
            url: Some("https://auth.example/device".into()),
            code: None,
        },
        Event::Prompt {
            url: None,
            code: Some("ABCD-1234".into()),
        },
        Event::Checked(SignInState::SignedOut),
        Event::LoginEnded { error: None },
        Event::Checked(SignInState::SignedIn {
            account: Some("user@example.com".into()),
        }),
        Event::Logout,
        Event::Checked(SignInState::SignedOut),
        Event::Login,
        Event::LoginEnded {
            error: Some("Codex sign-in failed: denied".into()),
        },
    ] {
        machine.handle_with_context(&event, &mut effects);
        seen.push((
            public_state(machine.state(), EngineId::Codex),
            std::mem::take(&mut effects),
        ));
    }
    insta::assert_debug_snapshot!(seen);
}
