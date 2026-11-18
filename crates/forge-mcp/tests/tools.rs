use artisan_api::{CreateProject, CreateThread, EngineConfig, EngineId, ProjectId, ThreadId};
use forge_engines::Engines;
use forge_mcp::{ArtisanMcp, server};
use forge_runs::Hub;
use forge_store::Store;
use serde_json::json;

#[tokio::test]
async fn listing_mcp_servers_keeps_their_credentials_from_the_model() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("store")).unwrap();
    let project = ProjectId::new();
    store
        .create_project(CreateProject {
            id: project.clone(),
            path: "/repo".into(),
            name: Some("editor".into()),
        })
        .await
        .unwrap();
    let thread = ThreadId::new();
    store
        .create_thread(CreateThread {
            id: thread.clone(),
            project_id: project,
            config: EngineConfig::new(EngineId::Codex),
            title: Some("Servers".into()),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap();
    store
        .set_setting(
            "mcp_servers".to_owned(),
            json!([
                {
                    "name": "search",
                    "url": "https://alice:hunter2@api.example.com:8443/mcp?api_key=query-secret#fragment-secret"
                },
                {
                    "name": "local",
                    "command": "npx",
                    "args": [
                        "-y",
                        "@example/server",
                        "--api-key=flag-secret",
                        "--token",
                        "spaced-secret",
                        "GITHUB_TOKEN=pair-secret",
                        "--header",
                        "Authorization: Bearer header-secret",
                        "sk-bare-secret",
                        "https://bob:arg-secret@hooks.example.com/in?sig=signed-secret",
                        "--port",
                        "8080"
                    ],
                    "env": { "API_KEY": "env-secret" }
                }
            ]),
        )
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
        &thread,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_mcp_servers","arguments":{}}}),
    )
    .await
    .unwrap();
    hub.shutdown();
    let output = reply["result"].to_string();
    for secret in [
        "alice",
        "hunter2",
        "query-secret",
        "fragment-secret",
        "flag-secret",
        "spaced-secret",
        "pair-secret",
        "header-secret",
        "sk-bare-secret",
        "arg-secret",
        "signed-secret",
        "env-secret",
    ] {
        assert!(!output.contains(secret), "{secret} leaked: {output}");
    }
    for shown in [
        "https://api.example.com:8443/mcp",
        "npx -y @example/server --api-key=<redacted> --token <redacted> GITHUB_TOKEN=<redacted>",
        "https://hooks.example.com/in --port 8080",
        "env API_KEY",
    ] {
        assert!(output.contains(shown), "{shown} missing: {output}");
    }
}
