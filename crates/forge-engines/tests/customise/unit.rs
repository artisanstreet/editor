use super::*;

fn skill(dir: &Path, name: &str) {
    fs::create_dir_all(dir.join(name)).expect("a skill folder");
    fs::write(
        dir.join(name).join("SKILL.md"),
        format!("---\nname: {name}\n---\nbody"),
    )
    .expect("a skill file");
}

fn listing(dir: &Path, root: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| {
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().into_owned();
                    fs::read_link(&path).map_or_else(
                        |_| format!("{name}/"),
                        |target| {
                            format!(
                                "{name} -> {}",
                                target.strip_prefix(root).unwrap_or(&target).display()
                            )
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

#[test]
fn materialize_writes_prompt_links_skills_and_passes_mcp() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let skills_dir = root.join("data/extensions/skills");
    skill(&skills_dir, "review");
    skill(&skills_dir, "deploy");
    let c = Customisation {
        prompt: Some("Be brief.".into()),
        mcp_servers: vec![
            McpServer {
                name: "files".into(),
                command: "npx".into(),
                args: vec!["-y".into(), "@mcp/files".into()],
                env: BTreeMap::from([("ROOT".into(), "/srv \"x\"".into())]),
                ..McpServer::default()
            },
            McpServer {
                name: "docs.web".into(),
                url: Some("https://mcp.example/docs".into()),
                ..McpServer::default()
            },
        ],
        skills_dir: skills_dir.clone(),
        ..Customisation::default()
    };

    let mut report = Vec::new();
    for engine in EngineId::ALL {
        let home = root.join(engine.as_str());
        let paths = paths(engine, &home);
        if let Some(paths) = &paths {
            skill(&paths.skills, "mine");
            skill(&paths.skills, "deploy");
            fs::create_dir_all(&paths.skills).unwrap();
            std::os::unix::fs::symlink(skills_dir.join("gone"), paths.skills.join("gone")).unwrap();
            std::os::unix::fs::symlink(
                root.join("data/extensions/store/abc"),
                paths.skills.join("old"),
            )
            .unwrap();
            for shadow in &paths.shadows {
                fs::write(shadow, "override").unwrap();
            }
        }
        let launch = materialize(engine, &home, &c, <[_]>::to_vec).unwrap();
        let files = paths.map(|paths| {
            (
                paths
                    .prompt
                    .strip_prefix(&home)
                    .unwrap()
                    .display()
                    .to_string(),
                fs::read_to_string(&paths.prompt).unwrap(),
                paths.shadows.iter().all(|shadow| !shadow.exists()),
                listing(&paths.skills, root),
            )
        });
        let mcp_file = fs::read_to_string(home.join(".artisan/claude-mcp.json")).ok();
        let args: Vec<String> = launch
            .args
            .iter()
            .map(|arg| arg.replace(&root.display().to_string(), "<root>"))
            .collect();
        report.push((engine.as_str(), files, args, launch.env, mcp_file));
    }
    insta::assert_debug_snapshot!(report);

    fs::remove_dir_all(skills_dir.join("review")).unwrap();
    let cleared = Customisation {
        prompt: Some(String::new()),
        mcp_servers: Vec::new(),
        skills_dir,
        ..Customisation::default()
    };
    let home = root.join("claude");
    let launch = materialize(EngineId::Claude, &home, &cleared, <[_]>::to_vec).unwrap();
    assert_eq!(
        listing(&home.join(".claude/skills"), root),
        ["deploy -> data/extensions/skills/deploy"]
    );
    assert!(!home.join(".claude/CLAUDE.md").exists());
    assert!(launch.args.is_empty() && !home.join(".artisan/claude-mcp.json").exists());
}
