use super::*;

const SECRETS: [&str; 2] = ["sk-ant-synthetic-0123456789abcdef", "Bearer hunter2"];

#[tokio::test]
async fn an_engine_that_dies_with_secrets_on_stderr_keeps_them_out_of_the_log_and_the_thread() {
    use std::os::unix::fs::PermissionsExt as _;
    let (captured, _logging) = test_support::logs::capture();
    let fixture = fixture_with_harness(Arc::new(forge_engines::harness)).await;
    let binary = fixture.hub.engines().binary(EngineId::Claude).unwrap();
    std::fs::write(
        &binary,
        format!(
            "#!/bin/sh\nread -r input\necho 'ANTHROPIC_API_KEY={}' >&2\necho 'Authorization: {}' >&2\nexit 3\n",
            SECRETS[0], SECRETS[1]
        ),
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

    fixture
        .hub
        .send_message(&fixture.thread_id, message("go"))
        .await
        .unwrap();
    let runs = fixture.settled().await;

    assert_eq!(runs[0].status(), RunStatus::Failed);
    let shown: Vec<String> = fixture
        .items()
        .await
        .into_iter()
        .filter_map(|item| {
            if let ItemKind::Error { message, .. } = item.kind {
                message.inline().map(str::to_owned)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(
        shown[0].contains("exited with code 3") && shown[0].contains("bytes on stderr"),
        "the failure says how the engine ended: {}",
        shown[0]
    );
    let logged = captured.text();
    assert!(
        logged.contains("Claude exited without a result"),
        "the failure is logged: {logged}"
    );
    for secret in SECRETS {
        assert!(
            !shown[0].contains(secret),
            "the thread shows {secret}: {}",
            shown[0]
        );
        assert!(!logged.contains(secret), "the log has {secret}: {logged}");
    }
    fixture.hub.shutdown();
}
