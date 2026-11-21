use super::*;

#[tokio::test]
async fn installs_started_together_never_clean_up_each_others_download() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("marketplace");
    let listing = "skills.sh:owner/repo/review";
    let (first, second) = tokio::join!(
        marketplace_staging(root.clone(), listing),
        marketplace_staging(root.clone(), listing)
    );
    let (first, second) = (first.unwrap(), second.unwrap());
    let unpacked = second.path().join("repository/review/SKILL.md");
    std::fs::create_dir_all(unpacked.parent().unwrap()).unwrap();
    std::fs::write(&unpacked, "---\nname: review\n---\n").unwrap();

    first.close().unwrap();

    assert!(
        unpacked.is_file(),
        "the first install's cleanup removed the second install's download"
    );
}
