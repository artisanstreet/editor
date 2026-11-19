use super::*;

#[tokio::test]
async fn engine_stderr_stays_out_of_the_logs() {
    let (captured, _logging) = test_support::logs::capture();
    let base = tempfile::tempdir().unwrap();
    let home = base.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let turn = Turn {
        thread_id: ThreadId::from("stderr-logs"),
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
            "echo 'Authorization: Bearer engine-secret' >&2".into(),
        ],
    )
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), process.wait())
        .await
        .expect("the engine exits within the bound");
    if let Some(stderr) = process.stderr_task.take() {
        stderr.await.unwrap();
    }
    assert!(process.stderr_tail().contains("engine-secret"));
    let logged = captured.text();
    assert!(
        logged.contains("stderr"),
        "the stderr read is still logged as metadata: {logged}"
    );
    assert!(
        !logged.contains("engine-secret"),
        "what the engine wrote to stderr reached the logs: {logged}"
    );
}
