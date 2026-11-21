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
