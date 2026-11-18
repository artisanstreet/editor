use super::*;

fn skill(folder: &Path, description: &str) {
    fs::create_dir_all(folder).unwrap();
    fs::write(
        folder.join("SKILL.md"),
        format!("---\nname: release-notes\ndescription: {description}\n---\n"),
    )
    .unwrap();
}

#[test]
fn an_installed_skill_reaches_every_engine_and_replace_keeps_a_backup() {
    let dir = tempfile::tempdir().unwrap();
    let homes = [
        (EngineId::Claude, Some(dir.path().join("claude"))),
        (EngineId::Codex, Some(dir.path().join("codex"))),
    ];
    let config = Customisation {
        skills_dir: dir.path().join("extensions/skills"),
        ..Default::default()
    };
    let draft = dir.path().join("project/release-notes");
    skill(&draft, "First");

    assert_eq!(
        install_skill(&homes, &config, &draft, false).unwrap(),
        "release-notes"
    );
    super::super::reconcile_skills(&homes, &config).unwrap();
    for linked in [
        dir.path()
            .join("claude/.claude/skills/release-notes/SKILL.md"),
        dir.path()
            .join("codex/.codex/skills/release-notes/SKILL.md"),
    ] {
        assert!(fs::read_to_string(linked).unwrap().contains("First"));
    }

    skill(&draft, "Second");
    let error = install_skill(&homes, &config, &draft, false).unwrap_err();
    assert!(matches!(error, InstallSkillError::Exists { .. }), "{error}");
    install_skill(&homes, &config, &draft, true).unwrap();
    let installed = config.skills_dir.join("release-notes/SKILL.md");
    assert!(fs::read_to_string(installed).unwrap().contains("Second"));
    assert_eq!(
        fs::read_dir(backups_root(&config.skills_dir))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn a_skill_written_into_an_engine_folder_is_refused_with_the_way_out() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("claude");
    let config = Customisation {
        skills_dir: dir.path().join("extensions/skills"),
        ..Default::default()
    };
    let native = home.join(".claude/skills/release-notes");
    skill(&native, "Written in place");

    let error =
        install_skill(&[(EngineId::Claude, Some(home))], &config, &native, false).unwrap_err();
    assert!(
        matches!(error, InstallSkillError::InsideEngine { .. }),
        "{error}"
    );
    assert!(!config.skills_dir.join("release-notes").exists());
}
