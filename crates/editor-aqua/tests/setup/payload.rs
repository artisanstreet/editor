use super::*;

#[test]
fn shipped_notices_carry_the_dependency_licences_of_the_editor_and_the_programs_it_bundles() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("NOTICES.txt");
    NOTICES.write(&path).unwrap();
    let notices = std::fs::read_to_string(&path).unwrap();
    for section in [
        "LICENSE.md",
        "crates/editor-assets/licenses/spline-sans-OFL.txt",
        "crates/editor-assets/licenses/sigurd-NOTES.md",
        "notices/editor.txt",
        "notices/ae.txt",
        "notices/forge.txt",
        "notices/fff-MIT.txt",
    ] {
        assert!(
            notices.contains(&format!("==== {section} ====")),
            "{section}"
        );
    }
    assert!(notices.contains("Copyright (c) Tokio Contributors"));
}
