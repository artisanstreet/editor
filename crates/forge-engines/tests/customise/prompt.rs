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
    assert!(!crate::home::forge_records(&home).join(BACKUPS).exists());
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
    let launched = materialize(EngineId::Claude, &home, &settings, <[_]>::to_vec);
    assert!(
        matches!(
            &launched,
            Err(MaterializeError::Prompt {
                engine: EngineId::Claude,
                source,
            }) if matches!(**source, ApplyPromptError::Read { .. })
        ),
        "{launched:?}"
    );
}
