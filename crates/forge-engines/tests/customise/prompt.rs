use super::*;
use crate::customise::{Customisation, MaterializeError, materialize};

#[test]
fn an_unreadable_ownership_record_keeps_an_external_edit() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("claude");
    let file = home.join(".claude/CLAUDE.md");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "edited outside Artisan").unwrap();
    fs::create_dir_all(record_path(&home)).unwrap();
    let applied = apply_prompt(EngineId::Claude, &home, "canonical", false);
    assert!(
        matches!(&applied, Some(PromptApplied::Failed(reason)) if reason.contains("global-prompt.sha256")),
        "{applied:?}"
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "edited outside Artisan");
    assert!(!crate::sandbox::forge_records(&home).join(BACKUPS).exists());
}

#[cfg(unix)]
#[test]
fn links_planted_where_forge_once_kept_its_prompt_records_lead_nowhere() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("claude");
    let key = root.path().join("id_ed25519");
    let elsewhere = root.path().join("elsewhere");
    fs::write(&key, "PRIVATE KEY").unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::write(home.join(".claude/CLAUDE.md"), "the user's own prompt").unwrap();
    fs::create_dir_all(home.join(".artisan")).unwrap();
    std::os::unix::fs::symlink(&key, home.join(".artisan/global-prompt.artisan-tmp")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, home.join(".artisan/backups")).unwrap();

    let applied = apply_prompt(EngineId::Claude, &home, "canonical", true);

    assert_eq!(applied, Some(PromptApplied::Synced));
    assert_eq!(fs::read_to_string(&key).unwrap(), "PRIVATE KEY");
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    assert_eq!(
        fs::read_to_string(home.join(".claude/CLAUDE.md")).unwrap(),
        "canonical"
    );
    let backups: Vec<_> = fs::read_dir(crate::sandbox::forge_records(&home).join(BACKUPS))
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    assert_eq!(backups, ["the user's own prompt"]);
}

#[cfg(unix)]
#[test]
fn a_prompt_file_replaced_with_a_link_is_not_read_through() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("claude");
    let key = root.path().join("id_ed25519");
    fs::write(&key, "PRIVATE KEY").unwrap();
    fs::create_dir_all(home.join(".claude")).unwrap();
    std::os::unix::fs::symlink(&key, home.join(".claude/CLAUDE.md")).unwrap();

    let file = read_prompt_file(EngineId::Claude, &home).unwrap();

    assert!(
        matches!(&file.text, Err(reason) if !reason.contains("PRIVATE KEY")),
        "{:?}",
        file.text
    );
}

#[test]
fn a_prompt_that_cannot_be_written_fails_the_launch() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("claude");
    fs::create_dir_all(home.join(".claude/CLAUDE.md")).unwrap();
    let settings = Customisation {
        prompt: Some("Be brief.".to_owned()),
        skills_dir: root.path().join("skills"),
        ..Customisation::default()
    };
    let launched = materialize(EngineId::Claude, &home, &settings);
    assert!(
        matches!(
            &launched,
            Err(MaterializeError::Prompt {
                engine: EngineId::Claude,
                source: ApplyPromptError::Read { .. },
            })
        ),
        "{launched:?}"
    );
}
