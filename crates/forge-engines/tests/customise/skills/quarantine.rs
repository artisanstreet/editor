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
