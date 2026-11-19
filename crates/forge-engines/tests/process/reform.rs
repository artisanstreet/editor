use super::*;
#[tokio::test]
async fn dropping_the_owner_closes_pipes_held_by_descendants() {
    let mut command = Command::new("sh");
    command
        .args(["-c", "sleep 30 & echo ready; wait"])
        .stdout(Stdio::piped());
    let mut child = owned_command(command, Sandbox::Open).unwrap();
    let mut pipe = BufReader::new(child.stdout().take().unwrap());
    let mut ready = String::new();
    pipe.read_line(&mut ready).await.unwrap();
    assert_eq!(ready.trim(), "ready");
    drop(child);
    let mut rest = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        pipe.read_to_end(&mut rest),
    )
    .await
    .expect("descendant inherited stdout must close with the owner")
    .unwrap();
}

#[tokio::test]
async fn oversized_probe_fails_without_waiting_for_a_blocked_pipe() {
    let mut command = Command::new("sh");
    command.args(["-c", "head -c 1048576 /dev/zero; sleep 30"]);
    let result = probe(command, std::time::Duration::from_secs(2), 4096).await;
    assert!(matches!(
        result,
        Err(ProbeError::Oversized { limit: 4096, .. })
    ));
}

#[tokio::test]
async fn oversized_engine_line_is_a_protocol_error() {
    let base = tempfile::tempdir().unwrap();
    let home = base.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let turn = Turn {
        thread_id: ThreadId::from("bounded-output"),
        engine: EngineId::Codex,
        binary: "/bin/sh".into(),
        home: home.clone(),
        cwd: home.clone(),
        config: artisan_api::EngineConfig::new(EngineId::Codex),
        session_id: None,
        input: crate::UserInput::default(),
        launch: crate::Launch::default(),
        trace_dir: None,
    };
    let mut process = spawn(
        &turn,
        &[
            "-c".into(),
            format!("head -c {} /dev/zero; printf '\\n'", MAX_LINE_BYTES + 1),
        ],
    )
    .await
    .unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(5), process.next_line())
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, ReadError::Oversized { .. }));
}
