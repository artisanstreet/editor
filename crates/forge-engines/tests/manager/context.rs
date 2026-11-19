use super::*;
use artisan_api::SignInState;
use std::os::unix::fs::PermissionsExt as _;

fn codex_manager(script: &str) -> (tempfile::TempDir, Engines, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let layout = Layout::new(directory.path());
    let engine = EngineId::Codex;
    let home = layout.home(engine);
    std::fs::create_dir_all(&home).unwrap();
    let version = layout.version_dir(engine, "1.0.0");
    let binary = version.join(host_spec(engine).artifact.unwrap().entry);
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    let script = script.replacen(
        '\n',
        "\napp_server() {\n read -r _; echo '{\"id\":1,\"result\":{}}'\n read -r _\n read -r _; printf '{\"id\":2,\"result\":{\"data\":[{\"id\":\"%s\",\"isDefault\":true}]}}\\n' \"$1\"\n}\n",
        1,
    );
    std::fs::write(&binary, script).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    install::switch(&layout, engine, "1.0.0").unwrap();
    let engines = Engines::new(directory.path());
    (directory, engines, home)
}

async fn wait_until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn login_links_are_still_parsed_after_the_retained_log_budget() {
    let (_directory, engines, _home) = codex_manager(
        r#"#!/bin/sh
case "$*" in
 "login --device-auth") yes 'noise' | head -n 15000; echo 'https://login.example.test/login'; sleep 30 ;;
 "login status") echo 'Logged in' ;;
 "app-server --stdio") app_server model1 ;;
esac
"#,
    );
    engines.login(EngineId::Codex).unwrap();
    wait_until(|| matches!(engines.status(EngineId::Codex).sign_in, SignInState::LoggingIn { url: Some(url), .. } if url == "https://login.example.test/login")).await;
    engines.cancel_login(EngineId::Codex);
    wait_until(|| {
        matches!(
            engines.status(EngineId::Codex).sign_in,
            SignInState::SignedIn { .. }
        )
    })
    .await;
}

#[tokio::test]
async fn overlapping_model_refreshes_share_one_native_probe() {
    let (_directory, engines, home) = codex_manager(
        r#"#!/bin/sh
printf 'probe\n' >> "$PWD/calls"
touch "$PWD/started"
while [ ! -e "$PWD/release" ]; do sleep 0.01; done
app_server model1
"#,
    );
    let owner = engines.clone();
    let first = tokio::spawn(async move { owner.refresh_models(EngineId::Codex).await.unwrap() });
    wait_until(|| home.join("started").exists()).await;
    let owner = engines.clone();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let second = tokio::spawn(async move {
        let _peer_gone = started.send(());
        owner.refresh_models(EngineId::Codex).await.unwrap()
    });
    waiting.await.unwrap();
    std::fs::write(home.join("release"), "").unwrap();
    assert_eq!(first.await.unwrap(), second.await.unwrap());
    assert_eq!(
        std::fs::read_to_string(home.join("calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[tokio::test]
async fn old_models_cannot_return_after_an_account_aba() {
    let (_directory, engines, home) = codex_manager(
        r#"#!/bin/sh
case "$*" in
 "login status") if [ -e "$PWD/out" ]; then echo 'Not logged in'; else echo 'Logged in'; fi ;;
 logout) touch "$PWD/out" ;;
 "login --device-auth") rm -f "$PWD/out" ;;
 "app-server --stdio") model=$(cat "$PWD/model"); touch "$PWD/started"; while [ ! -e "$PWD/release" ]; do sleep 0.01; done; app_server "$model" ;;
esac
"#,
    );
    std::fs::write(home.join("model"), "old-model1").unwrap();
    assert_eq!(
        engines.check_sign_in(EngineId::Codex).await,
        SignInState::SignedIn { account: None }
    );
    let owner = engines.clone();
    let old = tokio::spawn(async move { owner.refresh_models(EngineId::Codex).await.unwrap() });
    wait_until(|| home.join("started").exists()).await;
    engines.logout(EngineId::Codex).unwrap();
    wait_until(|| engines.status(EngineId::Codex).sign_in == SignInState::SignedOut).await;
    std::fs::write(home.join("model"), "new-model2").unwrap();
    engines.login(EngineId::Codex).unwrap();
    wait_until(|| {
        engines.status(EngineId::Codex).sign_in == SignInState::SignedIn { account: None }
    })
    .await;
    std::fs::write(home.join("release"), "").unwrap();
    assert!(
        old.await.unwrap().is_empty(),
        "old context must not commit even though public account state is identical"
    );
    wait_until(|| {
        engines
            .status(EngineId::Codex)
            .models
            .iter()
            .any(|model| model.id == "new-model2")
    })
    .await;
    assert!(
        !engines
            .status(EngineId::Codex)
            .models
            .iter()
            .any(|model| model.id == "old-model1")
    );
}

#[tokio::test]
async fn login_pins_the_selected_version_until_cancellation_and_probe_finish() {
    let (directory, engines, home) = codex_manager(
        r#"#!/bin/sh
case "$*" in
 "login --device-auth") touch "$PWD/started"; while :; do sleep 1; done ;;
 "login status") echo 'Logged in' ;;
 "app-server --stdio") app_server model1 ;;
esac
"#,
    );
    let old_binary = engines.binary(EngineId::Codex).unwrap();
    engines.login(EngineId::Codex).unwrap();
    wait_until(|| home.join("started").exists()).await;
    let layout = Layout::new(directory.path());
    for version in ["2.0.0", "3.0.0"] {
        let next = layout
            .version_dir(EngineId::Codex, version)
            .join(host_spec(EngineId::Codex).artifact.unwrap().entry);
        std::fs::create_dir_all(next.parent().unwrap()).unwrap();
        std::fs::copy(&old_binary, next).unwrap();
        install::switch(&layout, EngineId::Codex, version).unwrap();
    }
    install::prune(&layout, EngineId::Codex).unwrap();
    assert!(old_binary.is_file());
    engines.cancel_login(EngineId::Codex);
    wait_until(|| {
        install::prune(&layout, EngineId::Codex).unwrap();
        !old_binary.exists()
    })
    .await;
}

#[tokio::test]
async fn present_install_reconciles_a_failed_update_without_another_download() {
    let (_directory, engines, _home) = codex_manager("#!/bin/sh\necho 'Logged in'\n");
    engines.apply(
        EngineId::Codex,
        &Event::Start {
            version: "2.0.0".into(),
        },
    );
    engines.apply(
        EngineId::Codex,
        &Event::Fail {
            message: "download failed".into(),
        },
    );
    assert!(matches!(
        engines.status(EngineId::Codex).install,
        InstallState::Failed { .. }
    ));
    assert_eq!(engines.install(EngineId::Codex).await.unwrap(), "1.0.0");
    assert!(matches!(
        engines.status(EngineId::Codex).install,
        InstallState::Installed { .. }
    ));
}
