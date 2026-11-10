use super::*;
use crate::{Launch, UserInput};
use artisan_api::{EngineId, RunStatus, ThreadId};
use std::os::unix::fs::PermissionsExt as _;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cancellation_sends_the_interrupt_and_reaps_even_an_unresponsive_cli() {
    for ignore_interrupt in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("claude");
        std::fs::write(
            &binary,
            r#"#!/bin/sh
echo $$ > pid
read -r input
touch ready
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"interrupt"'*)
      touch interrupted
      if [ "$IGNORE_INTERRUPT" = true ]; then sleep 30; else sleep 0.03; exit 0; fi
      ;;
  esac
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let turn = Turn {
            thread_id: ThreadId::new(),
            engine: EngineId::Claude,
            binary,
            home: dir.path().join("home"),
            cwd: dir.path().to_owned(),
            config: EngineConfig::new(EngineId::Claude),
            session_id: Some("existing".to_owned()),
            input: UserInput {
                text: "work".to_owned(),
                ..Default::default()
            },
            launch: Launch {
                env: vec![("IGNORE_INTERRUPT".to_owned(), ignore_interrupt.to_string())],
                ..Default::default()
            },
            trace_dir: None,
        };
        let (_inputs, inbox) = mpsc::channel(8);
        let (events, mut received) = mpsc::channel(256);
        let cancel = CancellationToken::new();
        let running = tokio::spawn(ClaudeHarness.run(
            turn,
            RunIo {
                inbox,
                events,
                cancel: cancel.clone(),
            },
        ));
        let drain = tokio::spawn(async move { while received.recv().await.is_some() {} });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !dir.path().join("ready").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        let outcome = tokio::time::timeout(Duration::from_secs(10), running)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.status, RunStatus::Cancelled, "{:?}", outcome.error);
        assert!(dir.path().join("interrupted").exists());
        let pid = std::fs::read_to_string(dir.path().join("pid")).unwrap();
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success(),
            "cancelled CLI process must have been reaped"
        );
        drain.await.unwrap();
    }
}
