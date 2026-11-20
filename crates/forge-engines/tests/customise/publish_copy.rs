use super::*;

fn skill(folder: &Path, body: &str) {
    fs::create_dir_all(folder).unwrap();
    fs::write(
        folder.join("SKILL.md"),
        format!("---\nname: review\ndescription: review things\n---\n{body}\n"),
    )
    .unwrap();
}

fn body(folder: &Path) -> String {
    fs::read_to_string(folder.join("SKILL.md")).unwrap()
}

fn names(folder: &Path) -> Vec<String> {
    fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn too_deep(folder: &Path) -> PathBuf {
    let mut deepest = folder.to_path_buf();
    for _ in 0..65 {
        deepest.push("d");
    }
    fs::create_dir_all(&deepest).unwrap();
    folder.join("d")
}

#[test]
fn a_failed_copy_keeps_the_engine_reading_the_previous_copy() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("skills/review");
    let engine_skills = dir.path().join("claude/.claude/skills");
    let link = engine_skills.join("review");
    let staging = staging_root(&engine_skills);
    skill(&target, "first");
    fs::create_dir_all(&engine_skills).unwrap();
    publish_copy(&target, &link, &staging).unwrap();
    assert!(body(&link).contains("first"));

    skill(&target, "second");
    let unreadable = too_deep(&target);
    assert!(publish_copy(&target, &link, &staging).is_err());
    assert!(body(&link).contains("first"));
    assert!(link.join(COPY_MARKER).is_file());
    assert!(names(&staging).is_empty());

    fs::remove_dir_all(unreadable).unwrap();
    publish_copy(&target, &link, &staging).unwrap();
    assert!(body(&link).contains("second"));
    assert!(copied_skill_matches(&link, &target).unwrap());
    assert!(names(&staging).is_empty());
}
