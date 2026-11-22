use super::*;
use crate::UserInput;
use artisan_api::{EngineConfig, EngineId, ThreadId};

#[tokio::test]
async fn native_compaction_forks_overrides_only_the_fork_and_cleans_it_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = EngineConfig::new(EngineId::OpenCode);
    config.model = Some("opencode/big-pickle".to_owned());
    let turn = Turn {
        thread_id: ThreadId::new(),
        engine: EngineId::OpenCode,
        binary: crate::fake_engine::binary(),
        home: dir.path().join("home"),
        cwd: dir.path().to_owned(),
        config,
        session_id: Some("source".to_owned()),
        input: UserInput {
            text: crate::compaction_prompt(),
            ..Default::default()
        },
        launch: crate::fake_engine::launch("opencode-compaction", &[]),
        trace_dir: None,
    };
    let reply = tokio::time::timeout(
        test_support::wait::LIVENESS,
        compact(turn.clone(), CancellationToken::new()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(reply.text, "## Objective\n- Keep building");
    assert_eq!(reply.usage.cached_input, 2000);
    assert!(dir.path().join("deleted-fork").exists());
    let requests = std::fs::read_to_string(dir.path().join("requests")).unwrap();
    let paths: Vec<String> = requests
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()[0]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        paths,
        [
            "/api/session/source/fork",
            "/api/session/fork/model",
            "/api/session/fork/compact",
            "/api/session/fork/interrupt"
        ]
    );
    let mut hanging = turn;
    hanging
        .launch
        .env
        .push(("ARTISAN_TEST_COMPACTION_HANG".into(), "1".into()));
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move { compact(hanging, task_cancel).await });
    let hanging_marker = dir.path().join("hanging");
    test_support::wait::eventually(
        "the compaction to hang",
        test_support::wait::LIVENESS,
        || std::future::ready(hanging_marker.exists()),
    )
    .await;
    cancel.cancel();
    let error = tokio::time::timeout(test_support::wait::LIVENESS, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, SideQueryError::Cancelled), "{error}");
}
