use super::tests::{MACHINE, SECRET, mcp_servers, stub_server, test_client, test_hub};
use super::*;

fn statuses(stub: &tests::Stub) -> Vec<serde_json::Value> {
    stub.report_statuses()
}

#[tokio::test]
async fn an_applied_bundle_whose_report_failed_is_reported_on_the_next_sync() {
    let (base, stub, _server) = stub_server(false, None).await;
    stub.refuse_reports(1);
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    let first = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path())
        .await
        .unwrap();
    assert!(matches!(first, ConfigOutcome::Updated { .. }));
    assert!(statuses(&stub).is_empty(), "Core was down for the report");
    let second = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path())
        .await
        .unwrap();
    assert!(matches!(second, ConfigOutcome::Current(_)));
    assert_eq!(
        statuses(&stub),
        [serde_json::json!("applied")],
        "Core learns the bundle was applied once it answers again"
    );
    sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path())
        .await
        .unwrap();
    assert_eq!(
        statuses(&stub).len(),
        1,
        "a delivered report is not repeated"
    );
}

#[tokio::test]
async fn a_bundle_with_one_invalid_mcp_server_changes_nothing() {
    let manifest = serde_json::to_vec(&serde_json::json!({
        "format": "ast-editor-config/1",
        "mcp_servers": [
            {"name": "first", "command": "first-server"},
            {"name": "second"},
        ],
        "global_prompt": "synced prompt",
    }))
    .unwrap();
    let (base, stub, _server) = stub_server(false, Some(manifest)).await;
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    let result = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path()).await;
    assert!(
        result.is_err(),
        "a server without a target fails the bundle"
    );
    assert!(
        mcp_servers(hub.store()).await.is_empty(),
        "the valid server before it is not placed on its own"
    );
    assert_eq!(hub.store().setting("global_prompt").await.unwrap(), None);
    assert!(!sync_state_path(dir.path()).exists());
    assert_eq!(statuses(&stub), [serde_json::json!("failed")]);
}

#[tokio::test]
async fn a_state_file_from_before_pending_reports_keeps_its_synced_servers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        sync_state_path(dir.path()),
        serde_json::to_vec(&serde_json::json!({
            "bundle_id": "bundle_000000000000000000000000",
            "version": 2,
            "content_hash": "old",
            "etag": "",
            "mcp_servers": ["s"],
        }))
        .unwrap(),
    )
    .unwrap();
    let hub = test_hub(dir.path());
    hub.store()
        .mutate_mcp(artisan_api::McpMutation::Upsert {
            previous: None,
            server: artisan_api::McpServer {
                name: "s".into(),
                command: "old-server".into(),
                ..artisan_api::McpServer::default()
            },
        })
        .await
        .unwrap();
    let (base, _stub, _server) = stub_server(false, None).await;
    let outcome = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path())
        .await
        .unwrap();
    let ConfigOutcome::Updated { kept_local, .. } = outcome else {
        panic!("a new bundle applies");
    };
    assert!(kept_local.is_empty(), "the synced server is still ours");
    let servers = mcp_servers(hub.store()).await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].command, "srv");
}

#[tokio::test]
async fn a_bundle_with_too_many_skill_files_installs_nothing() {
    let files: Vec<serde_json::Value> = (0..5000)
        .map(|n| serde_json::json!({ "path": format!("f{n}.md"), "content_base64": "eA" }))
        .collect();
    let manifest = serde_json::to_vec(&serde_json::json!({
        "format": "ast-editor-config/1",
        "skills": [{ "name": "wide", "bundle": { "files": files } }],
        "global_prompt": "synced prompt",
    }))
    .unwrap();
    let (base, stub, _server) = stub_server(false, Some(manifest)).await;
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    let result = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path()).await;
    assert!(
        matches!(&result, Err(SyncConfigError::Apply { source, .. }) if matches!(**source, ApplyManifestError::TooManyFiles { .. })),
        "{result:?}"
    );
    assert_eq!(hub.store().setting("global_prompt").await.unwrap(), None);
    assert_eq!(statuses(&stub), [serde_json::json!("failed")]);
}

#[tokio::test]
async fn an_oversized_manifest_is_refused_before_it_is_read_whole() {
    let manifest = serde_json::to_vec(&serde_json::json!({
        "format": "ast-editor-config/1",
        "global_prompt": "p".repeat(65 * 1024 * 1024),
    }))
    .unwrap();
    let (base, _stub, _server) = stub_server(false, Some(manifest)).await;
    let dir = tempfile::tempdir().unwrap();
    let hub = test_hub(dir.path());
    let result = sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, dir.path()).await;
    assert!(
        matches!(result, Err(SyncConfigError::ManifestTooLarge { .. })),
        "{result:?}"
    );
    assert_eq!(hub.store().setting("global_prompt").await.unwrap(), None);
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
enum StateRelease {
    WhileWaiting,
    AfterGivingUp,
}

#[cfg(unix)]
#[test]
fn other_work_runs_while_the_sync_state_is_slow_to_read() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let state = sync_state_path(dir.path());
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&state)
            .status()
            .unwrap()
            .success()
    );
    let (opened_tx, opened) = tokio::sync::oneshot::channel();
    let (release_tx, release) = std::sync::mpsc::channel::<()>();
    let slow_disk = std::thread::spawn(move || {
        let mut fifo = std::fs::OpenOptions::new()
            .write(true)
            .open(&state)
            .unwrap();
        opened_tx.send(()).unwrap();
        let released = match release.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(()) => StateRelease::WhileWaiting,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => StateRelease::AfterGivingUp,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                unreachable!("the test holds the release sender until it releases")
            }
        };
        fifo.write_all(br#"{"applied":null,"claimed_mcp_servers":[]}"#)
            .unwrap();
        released
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (base, _stub, _server) = stub_server(false, None).await;
        let data_dir = dir.path().to_path_buf();
        let hub = test_hub(&data_dir);
        let sync = tokio::spawn(async move {
            sync_config(&test_client(), &base, MACHINE, &SECRET, &hub, &data_dir).await
        });
        opened.await.unwrap();
        tokio::spawn(async {}).await.unwrap();
        release_tx.send(()).unwrap();
        let outcome = sync.await.unwrap().unwrap();
        assert!(
            matches!(outcome, ConfigOutcome::Updated { .. }),
            "{outcome:?}"
        );
    });
    assert_eq!(slow_disk.join().unwrap(), StateRelease::WhileWaiting);
}
