use super::*;

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
