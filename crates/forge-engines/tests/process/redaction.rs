use super::*;

const CHILD: &str = "ARTISAN_TEST_REDACTION_CHILD";
const INHERITED: &str = "SYNTHETIC_SERVICE_TOKEN";
const INHERITED_VALUE: &str = "inherited-0123456789abcdef";
const SHORT_PASSWORD: &str = "pw9";
const TEST: &str = "process::redaction_tests::credentials_forge_inherits_or_configures_short_never_reach_the_trace";

#[test]
fn credentials_forge_inherits_or_configures_short_never_reach_the_trace() {
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .env(INHERITED, INHERITED_VALUE)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "the check in a process that inherits {INHERITED} failed:\n{stdout}\n{stderr}"
        );
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(traced_child());
}

async fn traced_child() {
    let base = tempfile::tempdir().unwrap();
    let home = base.path().join("home");
    let traces = base.path().join("traces");
    std::fs::create_dir_all(&home).unwrap();
    let mut launch = crate::Launch::default();
    launch.mcp_servers.push(crate::McpServer {
        name: "db".to_owned(),
        command: "db-server".to_owned(),
        env: [("DB_PASSWORD".to_owned(), SHORT_PASSWORD.to_owned())].into(),
        ..crate::McpServer::default()
    });
    let turn = Turn {
        thread_id: ThreadId::from("redaction"),
        engine: EngineId::Codex,
        binary: "/bin/sh".into(),
        home: home.clone(),
        cwd: home,
        config: artisan_api::EngineConfig::new(EngineId::Codex),
        session_id: None,
        input: crate::UserInput::default(),
        launch,
        trace_dir: Some(traces.clone()),
    };
    let script = format!(
        "echo \"token $SYNTHETIC_SERVICE_TOKEN\" >&2; echo 'login {SHORT_PASSWORD} rejected; pw9x is another word' >&2; echo \"{{\\\"token\\\":\\\"$SYNTHETIC_SERVICE_TOKEN\\\"}}\""
    );
    let mut process = spawn(&turn, &["-c".into(), script]).unwrap();
    let line = process.next_line().await.unwrap();
    assert!(
        line.as_deref()
            .is_some_and(|line| line.contains(INHERITED_VALUE)),
        "the engine inherited the credential: {line:?}"
    );
    process.finish().await;
    if let Some(stderr) = process.stderr_task.take() {
        stderr.await.unwrap();
    }
    drop(process);

    let trace = test_support::wait::wait_for(
        "the finished engine trace",
        test_support::wait::LIVENESS,
        || {
            let entry = std::fs::read_dir(&traces).ok()?.next()?.ok()?;
            let text = std::fs::read_to_string(entry.path()).ok()?;
            text.contains("\"kind\":\"end\"").then_some(text)
        },
    );
    assert!(
        trace.contains("token [redacted]"),
        "stderr is traced: {trace}"
    );
    assert!(
        trace.contains("login [redacted] rejected; pw9x is another word"),
        "the short password is redacted where it stands alone: {trace}"
    );
    assert!(!trace.contains(INHERITED_VALUE), "{trace}");
    assert!(!trace.contains(" pw9 "), "{trace}");
}
