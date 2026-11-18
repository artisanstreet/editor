use super::*;
use crate::{UserInput, fake_engine};
use artisan_api::{EngineConfig, EngineId, RunStatus, ThreadId};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn an_unreadable_stdout_line_fails_the_run_with_its_cause_not_as_an_exit() {
    let dir = tempfile::tempdir().unwrap();
    let turn = Turn {
        thread_id: ThreadId::new(),
        engine: EngineId::OpenCode,
        binary: fake_engine::binary(),
        home: dir.path().join("home"),
        cwd: dir.path().to_owned(),
        config: EngineConfig::new(EngineId::OpenCode),
        session_id: None,
        input: UserInput {
            text: "work".to_owned(),
            ..Default::default()
        },
        launch: fake_engine::launch("opencode-oversized-output", &[]),
        trace_dir: None,
    };
    let (_inputs, inbox) = mpsc::channel(8);
    let (events, mut received) = mpsc::channel(256);
    let running = tokio::spawn(OpenCodeHarness.run(
        turn,
        RunIo {
            inbox,
            events,
            cancel: CancellationToken::new(),
        },
    ));
    let drain = tokio::spawn(async move { while received.recv().await.is_some() {} });
    let outcome = tokio::time::timeout(Duration::from_secs(20), running)
        .await
        .expect("the run ends on the unreadable line")
        .unwrap();
    drain.await.unwrap();
    assert_eq!(outcome.status(), RunStatus::Failed);
    let error = outcome.error().unwrap();
    assert!(error.contains("read OpenCode's output"), "{error}");
    assert!(error.contains("wrote a line over"), "{error}");
    assert!(!error.contains("exited"), "{error}");
}
