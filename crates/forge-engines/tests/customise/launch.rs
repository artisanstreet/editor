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

fn through_a_users_link(launched: &Result<Launch, LaunchError>) -> bool {
    matches!(
        launched,
        Err(LaunchError::Guard {
            source: crate::sandbox::GuardError::ThroughLink { .. },
            ..
        })
    )
}

#[test]
fn a_codex_config_the_user_linked_is_left_alone_and_the_launch_says_why() {
    let root = tempfile::tempdir().unwrap();
    let dotfile = root.path().join("dotfiles/config.toml");
    fs::create_dir_all(dotfile.parent().unwrap()).unwrap();
    fs::write(&dotfile, "model = \"mine\"\n").unwrap();
    let home = root.path().join("codex");
    fs::create_dir_all(home.join(".codex")).unwrap();
    std::os::unix::fs::symlink(&dotfile, home.join(".codex/config.toml")).unwrap();

    let launched = launch(EngineId::Codex, &home, &servers());

    assert_eq!(
        fs::read_link(home.join(".codex/config.toml"))
            .ok()
            .as_deref(),
        Some(dotfile.as_path()),
        "Forge replaced the user's linked Codex config with its own file"
    );
    assert_eq!(fs::read_to_string(&dotfile).unwrap(), "model = \"mine\"\n");
    assert!(through_a_users_link(&launched), "{launched:?}");
}

#[test]
fn a_codex_folder_the_user_linked_is_left_alone_and_the_launch_says_why() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("profiles/codex");
    fs::create_dir_all(&profile).unwrap();
    fs::write(profile.join("auth.json"), "{}").unwrap();
    let home = root.path().join("codex");
    fs::create_dir_all(&home).unwrap();
    std::os::unix::fs::symlink(&profile, home.join(".codex")).unwrap();

    let launched = launch(EngineId::Codex, &home, &servers());

    assert_eq!(
        fs::read_link(home.join(".codex")).ok().as_deref(),
        Some(profile.as_path()),
        "Forge replaced the user's linked Codex folder with an empty one"
    );
    assert_eq!(
        fs::read_dir(&profile).unwrap().count(),
        1,
        "Forge wrote into the folder the user's link points to"
    );
    assert!(through_a_users_link(&launched), "{launched:?}");
}

#[test]
fn a_full_access_launch_keeps_an_engine_folder_the_user_linked() {
    let root = tempfile::tempdir().unwrap();
    let users = root.path().join("dotfiles-claude");
    fs::create_dir_all(&users).unwrap();
    fs::write(users.join("settings.json"), r#"{"theme":"mine"}"#).unwrap();
    let home = root.path().join("claude");
    fs::create_dir_all(&home).unwrap();
    std::os::unix::fs::symlink(&users, home.join(".claude")).unwrap();
    let customisation = Customisation {
        prompt: Some("Be brief.".to_owned()),
        ..Customisation::default()
    };

    let launched = materialize(EngineId::Claude, &home, &customisation);

    assert_eq!(
        fs::read_link(home.join(".claude")).ok().as_deref(),
        Some(users.as_path()),
        "Forge replaced the user's own .claude link with an empty folder"
    );
    assert_eq!(
        fs::read_dir(&users).unwrap().count(),
        1,
        "Forge wrote into the folder the user's link points to"
    );
    assert!(
        launched.is_err(),
        "a launch that can't write its prompt says so"
    );
}
