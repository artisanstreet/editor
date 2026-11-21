use super::*;
#[test]
fn readonly_inspection_never_creates_or_reconciles_prompt_files() {
    let home = tempfile::tempdir().unwrap();
    assert!(matches!(
        inspect_prompt(EngineId::Claude, home.path(), ""),
        Some(PromptApplied::Synced)
    ));
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
    let file = home.path().join(".claude/CLAUDE.md");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "external prompt").unwrap();
    assert!(
        matches!(inspect_prompt(EngineId::Claude, home.path(), "canonical"), Some(PromptApplied::Drifted(text)) if text == "external prompt")
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "external prompt");
    assert!(!home.path().join(".artisan").exists());
}
