use super::*;
#[test]
fn oversized_copy_marker_does_not_read_unbounded_data() {
    let folder = tempfile::tempdir().unwrap();
    let link = folder.path().join("copy");
    let target = folder.path().join("target");
    fs::create_dir_all(&link).unwrap();
    fs::create_dir_all(&target).unwrap();
    let file = fs::File::create(link.join(COPY_MARKER)).unwrap();
    file.set_len(64 * 1024 + 1).unwrap();
    assert!(!copied_skill_matches(&link, &target));
}
