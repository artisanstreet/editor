use super::*;

fn skill(root: &Path, name: &str, description: &str) {
    let folder = root.join(".claude/skills").join(name);
    std::fs::create_dir_all(folder.join("references")).unwrap();
    std::fs::write(
        folder.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nDo the {name} thing.\n"),
    )
    .unwrap();
    std::fs::write(folder.join("references/blocks.md"), "block catalog\n").unwrap();
}

#[test]
fn skills_come_from_the_home_and_the_project_with_the_project_winning() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    skill(home.path(), "review", "Review code");
    skill(home.path(), "deploy", "Deploy things");
    skill(project.path(), "deploy", "Deploy this project");
    let skills = discover(home.path(), project.path());
    assert_eq!(skills.len(), 2);
    let deploy = skills
        .iter()
        .find(|skill| skill.name == "deploy")
        .expect("the project's deploy");
    assert_eq!(deploy.description, "Deploy this project");
    assert!(deploy.folder.starts_with(project.path()));
}

#[test]
fn a_skill_loads_with_its_body_and_bundled_files() {
    let home = tempfile::tempdir().unwrap();
    skill(home.path(), "review", "Review code");
    let skills = discover(home.path(), home.path());
    let loaded = load(&skills, "review").expect("the skill loads");
    assert!(loaded.contains("Do the review thing."));
    assert!(loaded.contains("references/blocks.md"));
    assert!(
        !loaded.contains("description: Review code"),
        "front matter is not part of the instructions"
    );
    assert!(load(&skills, "missing").unwrap_err().contains("no skill"));
}

#[test]
fn an_invalid_or_hidden_skill_is_not_listed() {
    let home = tempfile::tempdir().unwrap();
    let skills_root = home.path().join(".claude/skills");
    std::fs::create_dir_all(skills_root.join("broken")).unwrap();
    std::fs::write(skills_root.join("broken/SKILL.md"), "no front matter").unwrap();
    std::fs::create_dir_all(skills_root.join(".hidden")).unwrap();
    std::fs::write(skills_root.join(".hidden/SKILL.md"), "---\nname: x\n---\n").unwrap();
    assert!(discover(home.path(), home.path()).is_empty());
}

#[test]
fn a_skill_with_a_bom_no_name_or_loose_yaml_is_still_listed() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".claude/skills");
    for (folder, text) in [
        (
            "bom",
            "\u{feff}---\nname: bom\ndescription: Saved by Notepad\n---\nBody.\n",
        ),
        (
            "nameless",
            "---\ndescription: Named for its folder\n---\nBody.\n",
        ),
        (
            "loose",
            "---\nname: loose\ndescription: Use when: the YAML is loose\n---\nBody.\n",
        ),
    ] {
        std::fs::create_dir_all(root.join(folder)).unwrap();
        std::fs::write(root.join(folder).join("SKILL.md"), text).unwrap();
    }
    let skills = discover(home.path(), home.path());
    let listed: Vec<(&str, &str)> = skills
        .iter()
        .map(|skill| (skill.name.as_str(), skill.description.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            ("bom", "Saved by Notepad"),
            ("loose", "Use when: the YAML is loose"),
            ("nameless", "Named for its folder"),
        ]
    );
    let loaded = load(&skills, "bom").unwrap();
    assert!(
        loaded.contains("Body.") && !loaded.contains("Saved by Notepad"),
        "{loaded}"
    );
}

#[test]
fn the_users_home_outside_a_repository_is_not_a_project() {
    let engine_home = tempfile::tempdir().unwrap();
    let user_home = tempfile::tempdir().unwrap();
    skill(user_home.path(), "disabled", "Turned off in Forge");
    skill(engine_home.path(), "enabled", "Turned on in Forge");
    let names = |skills: Vec<Skill>| {
        skills
            .into_iter()
            .map(|skill| skill.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(discover_with(
            engine_home.path(),
            user_home.path(),
            Some(user_home.path())
        )),
        ["enabled"]
    );
    std::fs::create_dir_all(user_home.path().join(".git")).unwrap();
    assert_eq!(
        names(discover_with(
            engine_home.path(),
            user_home.path(),
            Some(user_home.path())
        )),
        ["disabled", "enabled"],
        "a home that is a repository is that repository's project"
    );
}

#[test]
fn a_crowded_home_does_not_crowd_out_the_projects_skills() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    for index in 0..MAX_SKILLS {
        skill(home.path(), &format!("home-{index:03}"), "From the home");
    }
    skill(project.path(), "zz-project", "From the project");
    let skills = discover(home.path(), project.path());
    assert_eq!(skills.len(), MAX_SKILLS);
    assert!(skills.iter().any(|skill| skill.name == "zz-project"));
}

#[test]
fn an_exact_name_wins_over_one_that_differs_in_case() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".claude/skills");
    for (folder, name) in [("upper", "Review"), ("lower", "review")] {
        std::fs::create_dir_all(root.join(folder)).unwrap();
        std::fs::write(
            root.join(folder).join("SKILL.md"),
            format!("---\nname: {name}\n---\nThe {folder} one.\n"),
        )
        .unwrap();
    }
    let skills = discover(home.path(), home.path());
    assert!(load(&skills, "review").unwrap().contains("The lower one."));
    assert!(load(&skills, "Review").unwrap().contains("The upper one."));
}
