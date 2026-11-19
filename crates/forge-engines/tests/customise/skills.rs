use super::*;
use crate::customise::materialize;
#[cfg(unix)]
use crate::customise::publish_copy;

fn skill(folder: &Path, name: &str, body: &str) {
    fs::create_dir_all(folder).unwrap();
    fs::write(
        folder.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name} things\n---\n{body}\n"),
    )
    .unwrap();
}

fn body(folder: &Path) -> String {
    fs::read_to_string(folder.join("SKILL.md")).unwrap()
}

fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn an_interrupted_replacement_restores_the_installed_skill_at_the_next_launch() {
    let dir = tempfile::tempdir().unwrap();
    let config = Customisation {
        skills_dir: dir.path().join("extensions/skills"),
        ..Customisation::default()
    };
    let source = dir.path().join("source");
    skill(&source, "review", "first");
    install_skill(&[], &config, &source, false).unwrap();
    let installed = config.skills_dir.join("review");
    let backup = backups_root(&config.skills_dir).join("review-crashed");
    fs::write(
        replacement_record(&config.skills_dir, "review"),
        "review-crashed",
    )
    .unwrap();
    fs::create_dir_all(backup.parent().unwrap()).unwrap();
    fs::rename(&installed, &backup).unwrap();
    skill(
        &config.skills_dir.join(".install-review-crashed"),
        "review",
        "half",
    );

    let home = dir.path().join("claude");
    materialize(EngineId::Claude, &home, &config).unwrap();

    assert!(body(&installed).contains("first"));
    assert!(body(&home.join(".claude/skills/review")).contains("first"));
    assert_eq!(names(&config.skills_dir), ["review"]);
}

#[test]
fn replacing_an_installed_skill_leaves_no_replacement_record() {
    let dir = tempfile::tempdir().unwrap();
    let config = Customisation {
        skills_dir: dir.path().join("extensions/skills"),
        ..Customisation::default()
    };
    let source = dir.path().join("source");
    skill(&source, "review", "first");
    install_skill(&[], &config, &source, false).unwrap();
    skill(&source, "review", "second");
    install_skill(&[], &config, &source, true).unwrap();
    assert!(body(&config.skills_dir.join("review")).contains("second"));
    assert_eq!(names(&config.skills_dir), ["review"]);
}

#[cfg(unix)]
#[test]
fn a_failed_copy_keeps_the_engine_reading_the_previous_copy() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("skills/review");
    let engine_skills = dir.path().join("claude/.claude/skills");
    let link = engine_skills.join("review");
    let staging = super::super::staging_root(&engine_skills);
    skill(&target, "review", "first");
    fs::create_dir_all(&engine_skills).unwrap();
    publish_copy(&target, &link, &staging).unwrap();
    assert!(body(&link).contains("first"));

    skill(&target, "review", "second");
    std::os::unix::fs::symlink(&target, target.join("loop")).unwrap();
    assert!(publish_copy(&target, &link, &staging).is_err());
    assert!(body(&link).contains("first"));
    assert!(link.join(COPY_MARKER).is_file());
    assert!(names(&staging).is_empty());

    fs::remove_file(target.join("loop")).unwrap();
    publish_copy(&target, &link, &staging).unwrap();
    assert!(body(&link).contains("second"));
    assert!(super::super::copied_skill_matches(&link, &target));
    assert!(names(&staging).is_empty());
}

#[cfg(unix)]
#[test]
fn a_failed_quarantine_keeps_the_previously_quarantined_copy() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join("extensions/skills");
    let engine_skills = dir.path().join("claude/.claude/skills");
    let held = quarantine_root(&skills_dir).join("claude/animation");
    skill(&held, "animation", "kept");
    skill(&engine_skills.join("animation"), "animation", "newer");
    fs::set_permissions(&engine_skills, fs::Permissions::from_mode(0o555)).unwrap();
    quarantine_foreign(EngineId::Claude, &engine_skills, &skills_dir, &[]);
    fs::set_permissions(&engine_skills, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(body(&held).contains("kept"));
    assert!(body(&engine_skills.join("animation")).contains("newer"));

    quarantine_foreign(EngineId::Claude, &engine_skills, &skills_dir, &[]);
    assert!(body(&held).contains("newer"));
    assert!(!engine_skills.join("animation").exists());
    assert_eq!(
        names(held.parent().unwrap()),
        [".animation.at", "animation"]
    );
}

#[test]
fn an_interrupted_quarantine_is_finished_at_the_next_launch() {
    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join("extensions/skills");
    let engine_skills = dir.path().join("claude/.claude/skills");
    let root = quarantine_root(&skills_dir).join("claude");
    skill(
        &root.join(".incoming-4294967295-0-animation"),
        "animation",
        "moved",
    );
    skill(&root.join(".retired-4294967295-1-pdf"), "pdf", "old pdf");
    quarantine_foreign(EngineId::Claude, &engine_skills, &skills_dir, &[]);
    assert!(body(&root.join("animation")).contains("moved"));
    assert!(body(&root.join("pdf")).contains("old pdf"));
    assert_eq!(names(&root), ["animation", "pdf"]);
}

#[test]
fn verbatim_unc_repository_paths_keep_their_share_prefix() {
    assert_eq!(
        without_verbatim_prefix(Path::new(r"\\?\UNC\server\share\editor\skills")),
        PathBuf::from(r"\\server\share\editor\skills")
    );
    assert_eq!(
        without_verbatim_prefix(Path::new(r"\\?\C:\editor\skills")),
        PathBuf::from(r"C:\editor\skills")
    );
}
