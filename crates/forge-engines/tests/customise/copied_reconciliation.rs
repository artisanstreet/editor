use super::*;
#[test]
fn unchanged_real_copied_skill_keeps_files_and_timestamps_on_reconciliation() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let engine = dir.path().join("engine");
    let copy = engine.join("skill");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: skill\ndescription: fixture\n---\nUse this skill.\n",
    )
    .unwrap();
    copy_dir(&source, &copy).unwrap();
    let (digest, _, _) = skills::measure(&source).unwrap();
    fs::write(
        copy.join(COPY_MARKER),
        format!("{}\n{digest}", source.display()),
    )
    .unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(12345);
    for name in ["SKILL.md", COPY_MARKER] {
        fs::File::options()
            .write(true)
            .open(copy.join(name))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
    }
    let wanted = BTreeMap::from([("skill".to_owned(), source.clone())]);
    for _ in 0..3 {
        sync_skill_folder(&engine, &wanted, std::slice::from_ref(&source)).unwrap();
    }
    assert!(fs::symlink_metadata(&copy).unwrap().is_dir());
    assert!(
        !fs::symlink_metadata(&copy)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    for name in ["SKILL.md", COPY_MARKER] {
        assert_eq!(
            fs::metadata(copy.join(name)).unwrap().modified().unwrap(),
            old,
            "unchanged copied {name} must not be rewritten"
        );
    }
    fs::write(
        source.join("SKILL.md"),
        "---\nname: skill\ndescription: changed\n---\nChanged instructions.\n",
    )
    .unwrap();
    assert!(!copied_skill_matches(&copy, &source).unwrap());
    sync_skill_folder(&engine, &wanted, std::slice::from_ref(&source)).unwrap();
    assert_eq!(
        fs::read(copy.join("SKILL.md")).unwrap(),
        fs::read(source.join("SKILL.md")).unwrap()
    );
}
