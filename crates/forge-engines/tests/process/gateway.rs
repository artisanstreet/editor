use super::*;

const GATEWAY: &str = "https://127.0.0.1:4100/mcp/artisan/gateway-token-0123456789";

fn claude_turn(home: &std::path::Path, permissions: artisan_api::Permissions) -> Turn {
    let mut launch = crate::Launch::default();
    crate::with_gateway_mcp(EngineId::Claude, &mut launch, "artisan", GATEWAY);
    let mut config = artisan_api::EngineConfig::new(EngineId::Claude);
    config.permissions = permissions;
    Turn {
        thread_id: artisan_api::ThreadId::from("gateway"),
        engine: EngineId::Claude,
        binary: "/bin/sh".into(),
        home: home.into(),
        cwd: home.into(),
        config,
        session_id: None,
        input: crate::UserInput::default(),
        launch,
        trace_dir: None,
    }
}

async fn lines(mut process: EngineProcess) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(line) =
        tokio::time::timeout(std::time::Duration::from_secs(10), process.next_line())
            .await
            .expect("the engine finishes within the bound")
            .unwrap()
    {
        lines.push(line);
    }
    process.wait().await;
    lines
}

#[tokio::test]
async fn claude_reaches_the_gateway_through_its_own_environment_not_a_shared_file() {
    let home = tempfile::tempdir().unwrap();
    let turn = claude_turn(home.path(), artisan_api::Permissions::Full);
    let script = r#"printf '%s\n' "$#" "$@" "$ARTISAN_GATEWAY_ARTISAN"; grep -rl gateway-token . || echo "no file holds it""#;
    let process = spawn(&turn, &["-c".into(), script.into(), "sh".into()])
        .await
        .unwrap();
    let lines = lines(process).await;
    let count: usize = lines[0].parse().unwrap();
    let args = &lines[1..=count];
    assert!(
        args.iter().all(|arg| !arg.contains("gateway-token")),
        "other local users can read the gateway credential in the process arguments: {args:?}"
    );
    assert_eq!(args[0], "--mcp-config");
    let config: serde_json::Value = serde_json::from_str(&args[1]).unwrap();
    assert_eq!(
        config["mcpServers"]["artisan"]["url"],
        "${ARTISAN_GATEWAY_ARTISAN}"
    );
    assert_eq!(
        lines[count + 1],
        GATEWAY,
        "Claude can't expand the gateway address"
    );
    assert_eq!(
        lines[count + 2..],
        ["no file holds it"],
        "the gateway credential sits in the engine home every thread shares"
    );
}
