use super::*;
use crate::{UserInput, fake_engine};
use artisan_api::{EngineConfig, EngineId, RunStatus, ThreadId};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cancellation_interrupts_the_session_and_reaps_the_server_even_during_a_prompt() {
    for hang_prompt in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let hang_prompt = hang_prompt.to_string();
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
            launch: fake_engine::launch("opencode-cancellation", &[("HANG_PROMPT", &hang_prompt)]),
            trace_dir: None,
        };
        let (_inputs, inbox) = mpsc::channel(8);
        let (events, mut received) = mpsc::channel(256);
        let cancel = CancellationToken::new();
        let running = tokio::spawn(OpenCodeHarness.run(
            turn,
            RunIo {
                inbox,
                events,
                cancel: cancel.clone(),
            },
        ));
        let drain = tokio::spawn(async move { while received.recv().await.is_some() {} });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !dir.path().join("prompt").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        let outcome = tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            outcome.status(),
            RunStatus::Cancelled,
            "{:?}",
            outcome.error()
        );
        assert!(dir.path().join("interrupted").exists());
        assert!(
            fake_engine::ended(dir.path(), "server").await,
            "cancelled CLI process must have ended"
        );
        #[cfg(unix)]
        assert!(
            fake_engine::reaped(dir.path(), "server"),
            "cancelled CLI process must have been reaped"
        );
        drain.await.unwrap();
    }
}
