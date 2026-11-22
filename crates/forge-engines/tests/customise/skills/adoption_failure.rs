use super::*;
use std::os::unix::fs::PermissionsExt as _;

const SKILL: &str =
    "---\nname: recoverable\ndescription: adoption fixture\n---\nKeep every instruction.\n";

fn require_enforced_permissions() {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    assert!(
        !status.lines().any(|line| line.starts_with("Uid:\t0\t")),
        "this fixture makes a folder read-only, which root ignores; run it as an unprivileged user"
    );
}

fn other_filesystem_than(path: &Path) -> tempfile::TempDir {
    use std::os::unix::fs::MetadataExt as _;
    let other = tempfile::tempdir_in("/dev/shm")
        .expect("this fixture needs /dev/shm for a second filesystem");
    assert_ne!(
        fs::metadata(other.path()).unwrap().dev(),
        fs::metadata(path).unwrap().dev(),
        "this fixture needs /dev/shm on a different filesystem than the temporary directory"
    );
    other
}

#[test]
fn a_later_backup_failure_restores_every_original_and_allows_normal_retry() {
    require_enforced_permissions();
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    let codex = dir.path().join("codex");
    let claude_copy = claude.join(".claude/skills/recoverable");
    let codex_copy = codex.join(".codex/skills/recoverable");
    for copy in [&claude_copy, &codex_copy] {
        fs::create_dir_all(copy).unwrap();
        fs::write(copy.join("SKILL.md"), SKILL).unwrap();
    }
    let config = Customisation {
        skills_dir: dir.path().join("extensions/skills"),
        ..Default::default()
    };
    let homes = [
        (EngineId::Claude, Some(claude)),
        (EngineId::Codex, Some(codex)),
    ];
    let digest = measure_inner(&claude_copy, false).unwrap().0;
    let id = discovered_id("recoverable", &digest);
    let parent = codex_copy.parent().unwrap();
    let permissions = fs::metadata(parent).unwrap().permissions();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o555)).unwrap();
    let result = adopt_skill(&homes, &config, &id);
    fs::set_permissions(parent, permissions).unwrap();
    assert!(
        result.is_err(),
        "the second backup must fail after the first moved"
    );
    assert!(!config.skills_dir.join("recoverable").exists());
    for copy in [&claude_copy, &codex_copy] {
        assert_eq!(fs::read_to_string(copy.join("SKILL.md")).unwrap(), SKILL);
    }
    assert_eq!(
        fs::read_dir(backups_root(&config.skills_dir))
            .unwrap()
            .count(),
        0,
        "successful rollback leaves no stranded backup"
    );
    assert_eq!(adopt_skill(&homes, &config, &id).unwrap(), "recoverable");
    assert_eq!(
        fs::read_to_string(config.skills_dir.join("recoverable/SKILL.md")).unwrap(),
        SKILL
    );
    assert!(!claude_copy.exists() && !codex_copy.exists());
    assert_eq!(
        fs::read_dir(backups_root(&config.skills_dir))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn cross_device_partial_delete_failure_restores_content_before_retry() {
    require_enforced_permissions();
    let source_root = tempfile::tempdir().unwrap();
    let backup_root = other_filesystem_than(source_root.path());
    let source = source_root.path().join("recoverable");
    let backup = backup_root.path().join("backup");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("SKILL.md"), SKILL).unwrap();
    let permissions = fs::metadata(source_root.path()).unwrap().permissions();
    fs::set_permissions(source_root.path(), fs::Permissions::from_mode(0o555)).unwrap();
    let result = move_folder(&source, &backup);
    fs::set_permissions(source_root.path(), permissions).unwrap();
    let error = result.unwrap_err();
    assert!(
        error.to_string().contains("original source restored"),
        "must exercise cross-device copy followed by failed deletion and successful restoration: {error}"
    );
    assert_eq!(fs::read_to_string(source.join("SKILL.md")).unwrap(), SKILL);
    assert!(!backup.exists());
    move_folder(&source, &backup).unwrap();
    assert!(!source.exists());
    assert_eq!(fs::read_to_string(backup.join("SKILL.md")).unwrap(), SKILL);
}
