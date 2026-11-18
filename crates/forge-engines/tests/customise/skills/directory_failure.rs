use super::*;
#[test]
fn inaccessible_skill_roots_cannot_look_like_empty_desired_state() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("managed");
    fs::write(&root, b"not a directory").unwrap();
    let settings = Customisation {
        skills_dir: root,
        ..Customisation::default()
    };
    assert!(skill_catalog(&[], &settings).is_err());
    assert!(
        wanted(&settings).is_err(),
        "reconciliation must not delete links after a failed source read"
    );
    let home = folder.path().join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::write(home.join(".claude/skills"), b"not a directory").unwrap();
    let settings = Customisation {
        skills_dir: folder.path().join("missing"),
        ..Customisation::default()
    };
    assert!(skill_catalog(&[(EngineId::Claude, Some(home))], &settings).is_err());
}
