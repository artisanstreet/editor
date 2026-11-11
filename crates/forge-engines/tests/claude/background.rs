use super::*;
use crate::{Input, Launch, UserInput};
use artisan_api::{EngineId, RunStatus, ThreadId};
use std::os::unix::fs::PermissionsExt as _;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_steer_is_answered_while_a_monitor_outlives_the_reply() {
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("claude");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
read -r first
echo '{"type":"user","isReplay":true,"message":{"content":"go"}}'
echo '{"type":"system","subtype":"init","session_id":"s1","model":"m"}'
echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"m1","task_type":"local_bash","description":"Tail a log"}]}'
echo '{"type":"result","subtype":"success","result":"All done.","usage":{}}'
touch replied
if IFS= read -r steer; then
  printf '%s' "$steer" > steer
  echo '{"type":"user","isReplay":true,"message":{"content":"steer"}}'
  echo '{"type":"result","subtype":"success","result":"Changed course.","usage":{}}'
  echo '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
  cat > /dev/null
else
  touch input_closed_under_the_monitor
  sleep 30
fi
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
        session_id: Some("s1".to_owned()),
        input: UserInput {
            text: "go".to_owned(),
            ..Default::default()
        },
        launch: Launch::default(),
        trace_dir: None,
    };
    let (inputs, inbox) = mpsc::channel(8);
    let (events, mut received) = mpsc::channel(256);
    let running = tokio::spawn(ClaudeHarness.run(
        turn,
        RunIo {
            inbox,
            events,
            cancel: CancellationToken::new(),
        },
    ));
    let drain = tokio::spawn(async move { while received.recv().await.is_some() {} });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !dir.path().join("replied").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the first reply arrives");
    tokio::time::sleep(Duration::from_millis(100)).await;
    inputs
        .send(Input::Steer(UserInput {
            text: "change course".to_owned(),
            ..Default::default()
        }))
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("the run ends once the steer is answered and the monitor is gone")
        .unwrap();
    drain.abort();
    assert_eq!(outcome.status, RunStatus::Done, "{:?}", outcome.error);
    assert!(
        !dir.path().join("input_closed_under_the_monitor").exists(),
        "input must stay open while the monitor runs"
    );
    assert!(
        std::fs::read_to_string(dir.path().join("steer"))
            .unwrap()
            .contains("change course"),
        "the steer reaches the running session"
    );
}
