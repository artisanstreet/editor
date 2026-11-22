use super::*;

#[test]
fn each_server_reads_its_own_agents_and_they_go_with_it() {
    let home = tempfile::tempdir().unwrap();
    let first = home.path().join("run-images/first");
    let second = home.path().join("run-images/second");
    let allowed = |agents: &AgentsDir| {
        let config: Value =
            serde_json::from_slice(&std::fs::read(agents.path().join("opencode.json")).unwrap())
                .unwrap();
        config["agent"][agent_name(Permissions::Auto)]["permission"]["external_directory"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|pattern| *pattern != "*")
            .cloned()
            .collect::<Vec<_>>()
    };

    let one = write_agents(home.path(), std::slice::from_ref(&first)).unwrap();
    let two = write_agents(home.path(), std::slice::from_ref(&second)).unwrap();

    assert_eq!(allowed(&one), [folder_pattern(&first)]);
    assert_eq!(allowed(&two), [folder_pattern(&second)]);
    let folder = one.path().to_path_buf();
    drop(one);
    assert!(!folder.exists(), "a finished server's agents are removed");
}

#[test]
fn the_sweep_removes_server_folders_this_process_does_not_own() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path().join("opencode");
    let live = write_agents(&home, &[]).unwrap();
    let parent = servers_folder(&home);
    let leftover = parent.join(format!("{SERVER_PREFIX}killed"));
    std::fs::create_dir_all(&leftover).unwrap();
    std::fs::write(leftover.join("opencode.json"), b"{}").unwrap();
    let unrelated = parent.join("kept");
    std::fs::create_dir_all(&unrelated).unwrap();

    assert_eq!(sweep_servers(&home).unwrap(), 1);

    assert!(
        !leftover.exists(),
        "a killed Forge's server folder survived"
    );
    assert!(
        live.path().join("opencode.json").exists(),
        "the sweep removed a live server's agents"
    );
    assert!(unrelated.exists());
}

#[cfg(unix)]
#[test]
fn the_sweep_counts_only_removals_and_goes_on_past_a_folder_it_cannot_remove() {
    use std::os::unix::fs::PermissionsExt as _;
    let home = tempfile::tempdir().unwrap();
    let home = home.path().join("opencode");
    let parent = servers_folder(&home);
    let stuck = parent.join(format!("{SERVER_PREFIX}stuck"));
    let locked = stuck.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("opencode.json"), b"{}").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    let leftovers: Vec<_> = (0..4)
        .map(|index| parent.join(format!("{SERVER_PREFIX}killed-{index}")))
        .collect();
    for leftover in &leftovers {
        std::fs::create_dir_all(leftover).unwrap();
    }
    let stray = parent.join(format!("{SERVER_PREFIX}stray"));
    std::fs::write(&stray, b"not a folder").unwrap();

    let removed = sweep_servers(&home).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(removed, 5, "four folders and the stray file");
    assert!(leftovers.iter().all(|leftover| !leftover.exists()));
    assert!(!stray.exists(), "the stray file survived");
    assert!(
        stuck.exists(),
        "the locked folder could not have been removed"
    );
}
