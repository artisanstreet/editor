use super::*;
#[test]
fn browsing_catalog_does_not_materialize_engine_homes() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("engine-home");
    let settings = Customisation {
        skills_dir: directory.path().join("skills"),
        first_party: true,
        ..Customisation::default()
    };
    let _ = skill_catalog(&[(EngineId::Claude, Some(home.clone()))], &settings).unwrap();
    assert!(!home.exists());
    assert!(!settings.skills_dir.exists());
}
