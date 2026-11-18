use super::*;
#[test]
fn yaml_frontmatter_understands_multiline_and_quoted_values() {
    let text = "---\nname: 'review'\ndescription: >\n  First line\n  second line\n---\nbody";
    assert_eq!(frontmatter_name(text).as_deref(), Some("review"));
    assert_eq!(
        frontmatter_field(text, "description:").as_deref(),
        Some("First line second line")
    );
    assert_eq!(frontmatter_name("---\nname: '../outside'\n---"), None);
}
#[test]
fn a_bom_or_loose_yaml_does_not_hide_a_skill() {
    assert_eq!(
        skill_metadata("\u{feff}---\nname: review\ndescription: Saved by Notepad\n---\n"),
        Some(("review".to_owned(), "Saved by Notepad".to_owned()))
    );
    assert_eq!(
        skill_metadata("---\nname: review\ndescription: Use when: X\n---\n"),
        Some(("review".to_owned(), "Use when: X".to_owned()))
    );
    assert_eq!(
        skill_metadata_in("---\ndescription: Reviews code\n---\n", Some("review")),
        Some(("review".to_owned(), "Reviews code".to_owned()))
    );
}
#[test]
fn codex_mcp_preserves_distinct_names_and_keeps_secrets_out_of_args() {
    let directory = tempfile::tempdir().unwrap();
    let server = |name: &str| McpServer {
        name: name.into(),
        command: "tool".into(),
        args: vec![],
        env: std::collections::BTreeMap::from([("TOKEN".into(), "private".into())]),
        ..McpServer::default()
    };
    let result = launch(
        EngineId::Codex,
        directory.path(),
        &[server("docs.web"), server("docs_web")],
    )
    .unwrap();
    assert!(result.args.is_empty());
    let config: toml::Table =
        toml::from_str(&fs::read_to_string(directory.path().join(".codex/config.toml")).unwrap())
            .unwrap();
    let servers = config["mcp_servers"].as_table().unwrap();
    assert_eq!(servers.len(), 2);
    assert_eq!(
        servers["docs.web"]["env"]["TOKEN"].as_str(),
        Some("private")
    );
}
#[cfg(unix)]
#[test]
fn unsupported_skill_links_are_rejected_before_copying() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("SKILL.md"), "---\nname: linked\n---").unwrap();
    std::os::unix::fs::symlink(&source, source.join("cycle")).unwrap();
    let destination = directory.path().join("copy");
    assert!(copy_dir(&source, &destination).is_err());
    assert!(!destination.exists());
    fs::remove_file(source.join("cycle")).unwrap();
    let root_link = directory.path().join("root-link");
    std::os::unix::fs::symlink(&source, &root_link).unwrap();
    assert!(copy_dir(&root_link, &destination).is_err());
    assert!(!destination.exists());
}
