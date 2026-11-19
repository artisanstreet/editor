use super::*;
#[test]
fn invalid_native_metadata_is_never_adoptable() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let native = home.join(".claude/skills/broken");
    fs::create_dir_all(&native).unwrap();
    fs::write(
        native.join("SKILL.md"),
        "---\nname: [not a scalar]\ndescription: bad\n---\n",
    )
    .unwrap();
    let customisation = Customisation {
        skills_dir: directory.path().join("managed"),
        ..Default::default()
    };
    let homes = [(EngineId::Claude, Some(home))];
    assert!(skill_catalog(&homes, &customisation).unwrap().is_empty());
    let digest = measure(&native).unwrap().0;
    adopt_skill(&homes, &customisation, &discovered_id("broken", &digest))
        .expect_err("a broken discovered skill is not adopted");
    assert!(!customisation.skills_dir.exists());
    assert!(native.is_dir());
}

#[test]
fn catalog_generation_reuses_content_but_adoption_always_revalidates_it() {
    let folder = tempfile::tempdir().unwrap();
    let file = folder.path().join("resource.txt");
    fs::write(&file, "abc").unwrap();
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    let first = measure_inner(folder.path(), true).unwrap();
    assert_eq!(measure_inner(folder.path(), true).unwrap(), first);
    fs::write(&file, "xyz").unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_ne!(measure(folder.path()).unwrap().0, first.0);
    fs::write(&file, "a longer resource").unwrap();
    assert_ne!(measure_inner(folder.path(), true).unwrap(), first);
}
