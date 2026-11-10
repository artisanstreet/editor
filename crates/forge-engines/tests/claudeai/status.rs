use super::*;

#[cfg(unix)]
#[tokio::test]
async fn timed_out_commands_fail_while_normal_nonzero_exits_complete() {
    for (input, expected) in [
        (json!({"command": "sleep 30", "timeout": 1}), Status::Failed),
        (json!({"command": "exit 3"}), Status::Done),
    ] {
        let work = tempfile::tempdir().expect("a dir");
        let backend = script(vec![
            calls("m1", &[("command", "Bash", input)]),
            answer("m2", "Finished."),
        ]);
        let (outcome, log, _) = run(
            backend,
            turn(work.path(), None, Permissions::Full),
            nobody(),
        )
        .await;
        assert_eq!(outcome.status, RunStatus::Done);
        let status = log.iter().rev().find_map(|event| match event {
            EngineEvent::Item { key, status, .. } if key == "command" => Some(*status),
            _ => None,
        });
        assert_eq!(status, Some(expected));
    }
}
