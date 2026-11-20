use artisan_api::{
    CreateProject, CreateThread, EditorReply, EngineConfig, EngineId, Event, ProjectId, ThreadId,
};
use forge_engines::Engines;
use forge_mcp::{ArtisanMcp, server};
use forge_runs::Hub;
use forge_store::Store;
use serde_json::json;
use std::time::Duration;

fn listed_servers() -> serde_json::Value {
    json!([
        {
            "name": "search",
            "url": "https://alice:hunter2@api.example.com:8443/mcp?api_key=query-secret#fragment-secret"
        },
        {
            "name": "zap",
            "url": "https://mcp.example.com/api/mcp/s/9f8e7d6c5b4a39281706f5e4d3c2b1a0/sse"
        },
        {
            "name": "bare",
            "command": "uvx",
            "args": ["mcp-server-search", "AbCd1234EfGh5678IjKl9012MnOp"]
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
    ])
}

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
        .set_setting("mcp_servers".to_owned(), listed_servers())
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
        &forge_runs::GatewayCaller { thread: thread.clone(), ended: tokio_util::sync::CancellationToken::new(), acting: forge_runs::Acting::Run { stopped: tokio_util::sync::CancellationToken::new() } },
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
        "9f8e7d6c5b4a39281706f5e4d3c2b1a0",
        "AbCd1234EfGh5678IjKl9012MnOp",
    ] {
        assert!(!output.contains(secret), "{secret} leaked: {output}");
    }
    for shown in [
        "https://api.example.com:8443/mcp",
        "npx -y @example/server --api-key=<redacted> --token <redacted> GITHUB_TOKEN=<redacted>",
        "https://hooks.example.com/in --port 8080",
        "env API_KEY",
        "https://mcp.example.com/api/mcp/s/<redacted>/sse",
        "uvx mcp-server-search <redacted>",
    ] {
        assert!(output.contains(shown), "{shown} missing: {output}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_the_editor_started_without_a_link_fails_instead_of_offering_an_empty_one() {
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
            config: EngineConfig::new(EngineId::Claude),
            title: Some("Sign in".into()),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap();
    let hub = Hub::new(
        store,
        Engines::new(directory.path()),
        directory.path().to_path_buf(),
    );
    let mcp = ArtisanMcp::new(hub.clone());
    let calling = tokio::spawn(async move {
        server::handle(
            &mcp,
            &forge_runs::GatewayCaller { thread: thread.clone(), ended: tokio_util::sync::CancellationToken::new(), acting: forge_runs::Acting::Run { stopped: tokio_util::sync::CancellationToken::new() } },
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"sign_in_mcp_server","arguments":{"name":"notion"}}}),
        )
        .await
    });

    let call = std::sync::Arc::new(std::sync::Mutex::new(None));
    test_support::wait::eventually(
        "the sign-in to reach the Editor",
        Duration::from_secs(10),
        || {
            let hub = hub.clone();
            let call = std::sync::Arc::clone(&call);
            async move {
                let found = hub
                    .distributed_live_snapshot()
                    .await
                    .unwrap()
                    .1
                    .into_iter()
                    .find_map(|event| {
                        if let Event::EditorCall { id, .. } = event {
                            Some(id)
                        } else {
                            None
                        }
                    });
                let reached = found.is_some();
                *call.lock().unwrap() = found;
                reached
            }
        },
    )
    .await;
    let call = call.lock().unwrap().take().unwrap();
    hub.answer_distributed_editor_call(&call, EditorReply::Ok(json!({"state": "abc"})))
        .await
        .unwrap();

    let reply = calling.await.unwrap().unwrap();
    hub.shutdown();
    let result = &reply["result"];
    assert_eq!(result["isError"], json!(true), "{reply}");
    assert!(!result.to_string().contains("the link is"), "{reply}");
}
