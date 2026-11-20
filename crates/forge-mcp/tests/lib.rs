use artisan_api::{CreateProject, ProjectId, ThreadId};
use forge_engines::Engines;
use forge_mcp::{ArtisanMcp, server};
use forge_runs::Hub;
use forge_store::Store;
use serde_json::json;

#[tokio::test]
async fn an_address_issued_to_a_removed_thread_reaches_no_tool() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("store")).unwrap();
    store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/repo".into(),
            name: Some("editor".into()),
        })
        .await
        .unwrap();
    let hub = Hub::new(
        store,
        Engines::new(directory.path()),
        directory.path().to_path_buf(),
    );
    let mcp = ArtisanMcp::new(hub.clone());
    let reply = server::handle(
        &mcp,
        &forge_runs::GatewayCaller { thread: ThreadId::new(), ended: tokio_util::sync::CancellationToken::new(), acting: forge_runs::Acting::Run { stopped: tokio_util::sync::CancellationToken::new() } },
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_projects","arguments":{}}}),
    )
    .await
    .unwrap();
    hub.shutdown();
    let output = reply["result"].to_string();
    assert_eq!(reply["result"]["isError"], true, "{output}");
    assert!(!output.contains("/repo"), "{output}");
}
