use super::*;
#[test]
fn unsupported_managed_tree_does_not_publish_fabricated_zero_counts() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("skills");
    let skill = root.join("demo");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: demo\ndescription: Demonstration\n---\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(".", skill.join("cycle")).unwrap();
    let settings = Customisation {
        skills_dir: root,
        ..Customisation::default()
    };
    let error = skill_catalog(&[], &settings).unwrap_err();
    assert!(matches!(error, ReadSkillsError::Inspect { .. }), "{error}");
}
