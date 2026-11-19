use super::*;

fn skill(folder: &Path, name: &str) {
    fs::create_dir_all(folder).unwrap();
    fs::write(
        folder.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} things\n---\n"),
    )
    .unwrap();
}

#[test]
fn a_skill_artisan_didnt_install_is_quarantined_then_installed_or_discarded() {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    let codex = dir.path().join("codex");
    let homes = [
        (EngineId::Claude, Some(claude.clone())),
        (EngineId::Codex, Some(codex.clone())),
    ];
    let config = Customisation {
        skills_dir: dir.path().join("extensions/skills"),
        ..Default::default()
    };
    skill(&config.skills_dir.join("pdf"), "pdf");
    let stray = claude.join(".claude/skills/animation");
    skill(&stray, "animation");
    skill(&codex.join(".codex/skills/animation"), "animation");
    skill(&claude.join(".claude/skills/scratch"), "scratch");
    let system = codex.join(".codex/skills/.system");
    skill(&system, "system");

    super::super::reconcile_skills(&homes, &config).unwrap();

    assert!(!stray.exists());
    assert!(claude.join(".claude/skills/pdf/SKILL.md").exists());
    assert!(system.exists());
    let quarantine = quarantine_root(&config.skills_dir);
    assert!(quarantine.join("claude/animation/SKILL.md").exists());
    assert!(quarantine.join("codex/animation/SKILL.md").exists());

    let catalog = skill_catalog(&homes, &config).unwrap();
    let found: Vec<&CatalogSkill> = catalog
        .iter()
        .filter(|skill| skill.origin == SkillOrigin::Discovered)
        .collect();
    assert_eq!(found.len(), 2);
    let animation = found
        .iter()
        .find(|skill| skill.name == "animation")
        .unwrap();
    assert!(
        animation
            .engines
            .iter()
            .all(|state| state.status == SkillEngineStatus::Native)
    );

    assert_eq!(
        adopt_skill(&homes, &config, &animation.id).unwrap(),
        "animation"
    );
    super::super::reconcile_skills(&homes, &config).unwrap();
    assert!(claude.join(".claude/skills/animation/SKILL.md").exists());
    assert!(!quarantine.join("claude/animation").exists());

    let scratch = skill_catalog(&homes, &config)
        .unwrap()
        .into_iter()
        .find(|skill| skill.name == "scratch")
        .unwrap();
    discard_skill(&homes, &config, &scratch.id).unwrap();
    assert!(!quarantine.join("claude/scratch").exists());
    assert!(
        skill_catalog(&homes, &config)
            .unwrap()
            .iter()
            .all(|skill| skill.origin != SkillOrigin::Discovered)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_skills_folder_replaced_with_a_link_leaves_what_it_points_at_alone() {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    let projects = dir.path().join("projects");
    fs::create_dir_all(projects.join("work")).unwrap();
    fs::write(projects.join("work/main.rs"), "fn main() {}").unwrap();
    fs::write(projects.join("notes.txt"), "notes").unwrap();
    fs::create_dir_all(claude.join(".claude")).unwrap();
    std::os::unix::fs::symlink(&projects, claude.join(".claude/skills")).unwrap();
    let config = Customisation {
        skills_dir: dir.path().join("extensions/skills"),
        ..Default::default()
    };
    skill(&config.skills_dir.join("pdf"), "pdf");

    super::super::reconcile_skills(&[(EngineId::Claude, Some(claude.clone()))], &config).unwrap();

    let mut left: Vec<String> = fs::read_dir(&projects)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["notes.txt", "work"]);
    assert!(!quarantine_root(&config.skills_dir).join("claude").exists());
    let skills = fs::symlink_metadata(claude.join(".claude/skills")).unwrap();
    assert!(skills.is_dir());
    assert!(claude.join(".claude/skills/pdf/SKILL.md").exists());
}
