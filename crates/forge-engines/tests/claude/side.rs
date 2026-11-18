use super::*;
use artisan_api::Permissions;

#[test]
fn a_fork_launches_as_its_thread_does() {
    for permissions in [Permissions::Auto, Permissions::Full] {
        let mut config = EngineConfig::new(artisan_api::EngineId::Claude);
        config.permissions = permissions;
        config.model = Some("claude-opus-5-5".to_owned());
        let readable = [PathBuf::from("/data/blobs")];
        let thread = launch_args(&config, Some("s1"), &readable);
        let fork = side_args(&config, Some("s1"), &readable);
        assert_eq!(fork[..thread.len()], thread[..]);
        assert_eq!(
            fork[thread.len()..],
            ["--fork-session", "--max-turns", "1"].map(str::to_owned)
        );
    }
}
