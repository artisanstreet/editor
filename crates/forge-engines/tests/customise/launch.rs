use super::*;
use std::os::unix::fs::PermissionsExt as _;

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn servers() -> Vec<McpServer> {
    vec![
        McpServer {
            name: "docs".to_owned(),
            url: Some("https://docs.example/mcp".to_owned()),
            headers: [("Authorization".to_owned(), "Bearer docs-secret".to_owned())].into(),
            ..McpServer::default()
        },
        McpServer {
            name: "local".to_owned(),
            command: "local-mcp".to_owned(),
            env: [("API_KEY".to_owned(), "local-secret".to_owned())].into(),
            ..McpServer::default()
        },
    ]
}

#[test]
fn mcp_credentials_are_written_readable_only_by_this_user() {
    let root = tempfile::tempdir().unwrap();
    for (engine, file) in [
        (EngineId::Claude, ".artisan/claude-mcp.json"),
        (EngineId::Codex, ".codex/config.toml"),
    ] {
        let home = root.path().join(engine.as_str());
        launch(engine, &home, &servers()).unwrap();
        let path = home.join(file);
        assert!(fs::read_to_string(&path).unwrap().contains("docs-secret"));
        assert_eq!(
            mode(&path),
            0o600,
            "other local users can read {engine}'s MCP credentials"
        );
        assert_eq!(mode(path.parent().unwrap()), 0o700);
    }
}

#[test]
fn an_unchanged_readable_mcp_file_is_made_private() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("codex");
    launch(EngineId::Codex, &home, &servers()).unwrap();
    let path = home.join(".codex/config.toml");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    launch(EngineId::Codex, &home, &servers()).unwrap();
    assert_eq!(
        mode(&path),
        0o600,
        "an existing config keeps its credentials readable by other local users"
    );
}
