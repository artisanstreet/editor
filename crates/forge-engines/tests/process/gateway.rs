use super::*;
use std::os::unix::fs::PermissionsExt as _;

const GATEWAY: &str = "https://127.0.0.1:4100/mcp/artisan/gateway-token-0123456789";

fn claude_turn(home: &std::path::Path) -> Turn {
    let mut launch = crate::Launch::default();
    crate::with_gateway_mcp(
        artisan_api::EngineId::Claude,
        &mut launch,
        "artisan",
        GATEWAY,
    );
    Turn {
        thread_id: artisan_api::ThreadId::from("gateway-file"),
        engine: artisan_api::EngineId::Claude,
        binary: "/bin/sh".into(),
        home: home.into(),
        cwd: home.into(),
        config: artisan_api::EngineConfig::new(artisan_api::EngineId::Claude),
        session_id: None,
        input: crate::UserInput::default(),
        launch,
        trace_dir: None,
    }
}

async fn line(process: &mut EngineProcess) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(5), process.next_line())
        .await
        .expect("the engine answers within the bound")
        .unwrap()
        .expect("the engine prints a line")
}

#[tokio::test]
async fn claude_reads_the_gateway_credential_from_a_private_file_removed_when_it_ends() {
    let home = tempfile::tempdir().unwrap();
    let turn = claude_turn(home.path());
    let script = r#"printf '%s\n' "$#" "$@"; tr -d '\n' < "$2"; printf '\n'; read _"#;
    let mut process = spawn(&turn, &["-c".into(), script.into(), "sh".into()]).unwrap();
    let count: usize = line(&mut process).await.parse().unwrap();
    let mut args = Vec::new();
    for _ in 0..count {
        args.push(line(&mut process).await);
    }
    let config: serde_json::Value = serde_json::from_str(&line(&mut process).await).unwrap();
    assert!(
        args.iter().all(|arg| !arg.contains("gateway-token")),
        "other local users can read the gateway credential in the process arguments: {args:?}"
    );
    assert_eq!(args[0], "--mcp-config");
    assert_eq!(config["mcpServers"]["artisan"]["url"], GATEWAY);
    let file = std::path::PathBuf::from(&args[1]);
    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "other local users can read the gateway file");
    process.close_stdin("the test is done");
    process.wait().await;
    drop(process);
    assert!(!file.exists(), "the gateway credential outlives its launch");
}
