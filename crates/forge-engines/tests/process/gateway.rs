use super::*;

const GATEWAY: &str = "https://127.0.0.1:4100/mcp/artisan/gateway-token-0123456789";

fn claude_turn(home: &std::path::Path, permissions: artisan_api::Permissions) -> Turn {
    let mut launch = crate::Launch::default();
    crate::with_gateway_mcp(EngineId::Claude, &mut launch, "artisan", GATEWAY);
    let mut config = artisan_api::EngineConfig::new(EngineId::Claude);
    config.permissions = permissions;
    Turn {
        thread_id: ThreadId::from("gateway"),
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

async fn line(process: &mut EngineProcess) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(5), process.next_line())
        .await
        .expect("the engine answers within the bound")
        .unwrap()
        .expect("the engine prints a line")
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
    let process = spawn(&turn, &["-c".into(), script.into(), "sh".into()]).unwrap();
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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_sandboxed_run_cannot_read_another_runs_gateway_credential() {
    if let forge_isolation::Support::Unavailable(reason) = forge_isolation::support() {
        test_support::skipped(format_args!(
            "this kernel can't enforce the sandbox ({reason:?})"
        ));
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let home = base.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let holder = claude_turn(&home, artisan_api::Permissions::Full);
    let mut holder = spawn(
        &holder,
        &["-c".into(), "echo $$; read _".into(), "sh".into()],
    )
    .unwrap();
    let holder_pid = line(&mut holder).await;
    let mut reader = claude_turn(&home, artisan_api::Permissions::Auto);
    reader.launch = crate::Launch::default();
    let probe = r#"tr '\0' '\n' < "/proc/$1/environ" 2>&1 | grep -c gateway-token; grep -rl gateway-token . ; echo done"#;
    let read = |permissions| {
        let mut reader = reader.clone();
        reader.config.permissions = permissions;
        let process = spawn(
            &reader,
            &["-c".into(), probe.into(), "sh".into(), holder_pid.clone()],
        )
        .unwrap();
        lines(process)
    };
    let unconfined = read(artisan_api::Permissions::Full).await;
    let sandboxed = read(artisan_api::Permissions::Auto).await;
    holder.close_stdin("the test is done");
    holder.wait().await;
    assert_eq!(
        unconfined.first().map(String::as_str),
        Some("1"),
        "an unsandboxed process of the same user reads the holder's environment: {unconfined:?}"
    );
    assert_eq!(
        sandboxed,
        ["0", "done"],
        "a sandboxed run read another run's gateway credential"
    );
}
