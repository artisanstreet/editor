use super::*;
use crate::{UserInput, fake_engine};
use artisan_api::{EngineConfig, EngineId, RunStatus, ThreadId};
use test_support::wait::{eventually, within};
use tokio_util::sync::CancellationToken;

const SESSION_OPENS: Duration = READY_TIMEOUT.saturating_add(REQUEST_TIMEOUT);

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
        within("the run opens its OpenCode session", SESSION_OPENS, async {
            loop {
                match received.recv().await {
                    Some(EngineEvent::Session { .. }) => break,
                    Some(_) => {}
                    None => panic!("the run ended before it opened a session"),
                }
            }
        })
        .await;
        let drain = tokio::spawn(async move { while received.recv().await.is_some() {} });
        let prompt = dir.path().join("prompt");
        eventually("the prompt reaches OpenCode", REQUEST_TIMEOUT, || {
            std::future::ready(prompt.exists())
        })
        .await;
        cancel.cancel();
        let outcome = within("the cancelled run ends", REQUEST_TIMEOUT, running)
            .await
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
