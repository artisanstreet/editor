use super::*;
fn settings(root: &Path) -> Customisation {
    Customisation {
        skills_dir: root.to_owned(),
        ..Customisation::default()
    }
}
#[test]
fn empty_native_directories_do_not_consume_metadata_admission() {
    let folder = tempfile::tempdir().unwrap();
    let home = folder.path().join("home");
    let native = home.join(".claude/skills");
    for i in 0..=MAX_CATALOG_RECORDS {
        fs::create_dir_all(native.join(format!("empty-{i}"))).unwrap();
    }
    assert!(
        skill_catalog(
            &[(EngineId::Claude, Some(home))],
            &settings(&folder.path().join("managed"))
        )
        .unwrap()
        .is_empty()
    );
}
#[test]
fn cumulative_native_metadata_is_an_explicit_error_not_a_truncated_catalog() {
    let folder = tempfile::tempdir().unwrap();
    let home = folder.path().join("home");
    let native = home.join(".claude/skills");
    for i in 0..100 {
        let skill = native.join(format!("skill-{i}"));
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            format!(
                "---\nname: skill-{i}\ndescription: {}\n---\n",
                "x".repeat(48 * 1024)
            ),
        )
        .unwrap();
    }
    let error = skill_catalog(
        &[(EngineId::Claude, Some(home))],
        &settings(&folder.path().join("managed")),
    )
    .unwrap_err();
    assert!(matches!(error, ReadSkillsError::Paging { .. }), "{error}");
    assert!(native.join("skill-99/SKILL.md").exists());
}
#[test]
fn selecting_native_folder_avoids_hashing_unrelated_catalogs() {
    let folder = tempfile::tempdir().unwrap();
    let home = folder.path().join("home");
    let native = home.join(".claude/skills");
    for name in ["selected", "unrelated"] {
        let skill = native.join(name);
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Native\n---\n"),
        )
        .unwrap();
    }
    fs::write(
        native.join("unrelated/content.bin"),
        b"unselected native content",
    )
    .unwrap();
    let homes = [(EngineId::Claude, Some(home))];
    assert_eq!(discovered(&homes, None, &[], false, None).unwrap().len(), 2);
    let selected = discovered(&homes, None, &[], false, Some("selected")).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].folder_name, "selected");
}
