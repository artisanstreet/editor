//! Direct, dependency-free parity tests for the thread environment card.
//!
//! The production module is included by path so this harness does not depend
//! on Cargo registration, the frontend crate, Svelte, a browser, or any
//! controller and transport runtime.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/thread_environment_presentation.rs"]
mod thread_environment_presentation;

use thread_environment_presentation::{
    GitBranchState, GitDiffSummary, GitWorkspace, GitWorktree, HomeHostMemory,
    HostIdentitySnapshot, HostMachineKind, HostMachineSnapshot, HostMachinesSnapshot,
    MachineSwitchError, ProjectRepository, RepositoryRemote, RepositoryState,
    ThreadEnvironmentInput, branch_label, present_thread_environment, project_thread_environment,
    worktree_label,
};

fn machine(id: &str, kind: HostMachineKind, label: &str) -> HostMachineSnapshot {
    HostMachineSnapshot::new(id, kind, label)
}

fn machine_with_detail(
    id: &str,
    kind: HostMachineKind,
    label: &str,
    detail: &str,
) -> HostMachineSnapshot {
    machine(id, kind, label).with_detail(detail)
}

fn attached(name: &str) -> GitBranchState {
    GitBranchState::attached(name)
}

fn unborn(name: &str) -> GitBranchState {
    GitBranchState::unborn(name)
}

fn repository(
    state: RepositoryState,
    branch: Option<GitBranchState>,
    default_remote: Option<&str>,
    remotes: Vec<RepositoryRemote>,
) -> ProjectRepository {
    ProjectRepository::new(state, branch, default_remote.map(str::to_owned), remotes)
}

fn remote(name: &str, web_url: Option<&str>) -> RepositoryRemote {
    let remote = RepositoryRemote::new(name);
    match web_url {
        Some(url) => remote.with_web_url(url),
        None => remote,
    }
}

fn worktree(path: &str, is_current: bool, branch: Option<GitBranchState>) -> GitWorktree {
    GitWorktree::new(path, is_current, branch)
}

// Taking ownership keeps the many temporary fixture literals concise while
// the production function itself remains a borrowed, pure projection.
#[allow(clippy::needless_pass_by_value)]
fn projection(
    input: ThreadEnvironmentInput,
) -> thread_environment_presentation::ThreadEnvironmentProjection {
    present_thread_environment(&input)
}

#[test]
fn machine_snapshot_absence_and_empty_snapshot_are_empty_choices() {
    let mut absent = ThreadEnvironmentInput {
        identity: Some(HostIdentitySnapshot::new("raw-host")),
        ..ThreadEnvironmentInput::default()
    };
    let absent_projection = projection(absent.clone());
    assert!(absent_projection.machine_rows.is_empty());
    assert_eq!(absent_projection.current_machine, None);
    assert_eq!(absent_projection.machine_label, "raw-host");
    assert!(!absent_projection.machine_menu_required);

    absent.machines = Some(HostMachinesSnapshot::new(Vec::new()));
    let empty_projection = projection(absent);
    assert!(empty_projection.machine_rows.is_empty());
    assert_eq!(empty_projection.current_machine, None);
    assert_eq!(empty_projection.machine_label, "raw-host");
    assert!(!empty_projection.machine_menu_required);
}

#[test]
fn machine_label_uses_first_machine_then_identity_then_not_connected() {
    let no_facts = projection(ThreadEnvironmentInput::default());
    assert_eq!(no_facts.machine_label, "Not connected");

    let identity_only = projection(ThreadEnvironmentInput {
        identity: Some(HostIdentitySnapshot::new("HOSTNAME")),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(identity_only.machine_label, "HOSTNAME");

    let machine_wins = projection(ThreadEnvironmentInput {
        identity: Some(HostIdentitySnapshot::new("ignored-host")),
        machines: Some(HostMachinesSnapshot::new(vec![machine(
            "first",
            HostMachineKind::Local,
            "First machine",
        )])),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(machine_wins.machine_label, "First machine");

    let empty_label_is_present_and_wins = projection(ThreadEnvironmentInput {
        identity: Some(HostIdentitySnapshot::new("ignored-host")),
        machines: Some(HostMachinesSnapshot::new(vec![machine(
            "first",
            HostMachineKind::Local,
            "",
        )])),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(empty_label_is_present_and_wins.machine_label, "");
}

#[test]
fn machine_rows_preserve_order_and_menu_threshold() {
    let one = projection(ThreadEnvironmentInput {
        machines: Some(HostMachinesSnapshot::new(vec![machine_with_detail(
            "local",
            HostMachineKind::Local,
            "Local",
            "host",
        )])),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(one.machine_rows.len(), 1);
    assert!(!one.machine_menu_required);
    assert_eq!(one.machine_rows[0].id, "local");
    assert!(one.machine_rows[0].informational);
    assert_eq!(one.machine_rows[0].detail.as_deref(), Some("host"));

    let many = projection(ThreadEnvironmentInput {
        machines: Some(HostMachinesSnapshot::new(vec![
            machine("first", HostMachineKind::Local, "First"),
            machine("second", HostMachineKind::Wsl, "Second"),
            machine(
                "third",
                HostMachineKind::Other("future".to_owned()),
                "Third",
            ),
        ])),
        ..ThreadEnvironmentInput::default()
    });
    assert!(many.machine_menu_required);
    assert_eq!(
        many.machine_rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["first", "second", "third"]
    );
    assert_eq!(many.machine_rows[1].kind, HostMachineKind::Wsl);
    assert!(!many.machine_rows[1].informational);
    assert!(many.machine_rows[2].informational);
    assert_eq!(
        many.current_machine
            .as_ref()
            .map(|machine| machine.id.as_str()),
        Some("first")
    );
}

#[test]
fn home_row_requires_desktop_and_an_exact_first_wsl_label() {
    let wsl_machines = Some(HostMachinesSnapshot::new(vec![machine(
        "local",
        HostMachineKind::Local,
        "This computer on WSL2",
    )]));

    for desktop in [false, true] {
        let result = projection(ThreadEnvironmentInput {
            desktop,
            machines: wsl_machines.clone(),
            ..ThreadEnvironmentInput::default()
        });
        if desktop {
            assert!(result.home_row.is_some());
            assert!(result.machine_menu_required);
        } else {
            assert_eq!(result.home_row, None);
            assert!(!result.machine_menu_required);
        }
    }

    for label in [
        "This computer",
        "This computer on WSL2 ",
        "this computer on WSL2",
    ] {
        let result = projection(ThreadEnvironmentInput {
            desktop: true,
            machines: Some(HostMachinesSnapshot::new(vec![machine(
                "local",
                HostMachineKind::Local,
                label,
            )])),
            ..ThreadEnvironmentInput::default()
        });
        assert_eq!(result.home_row, None, "home row should not match {label:?}");
    }

    let not_first = projection(ThreadEnvironmentInput {
        desktop: true,
        machines: Some(HostMachinesSnapshot::new(vec![
            machine("local", HostMachineKind::Local, "This computer"),
            machine("wsl", HostMachineKind::Wsl, "This computer on WSL2"),
        ])),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(not_first.home_row, None);
}

#[test]
fn home_row_uses_memory_default_detail_and_switching_precedence() {
    let base = ThreadEnvironmentInput {
        desktop: true,
        machines: Some(HostMachinesSnapshot::new(vec![machine(
            "local",
            HostMachineKind::Local,
            "This computer on WSL2",
        )])),
        ..ThreadEnvironmentInput::default()
    };

    let fallback = projection(base.clone());
    let fallback_home = fallback.home_row.expect("desktop WSL has a home row");
    assert_eq!(fallback_home.id, "home");
    assert_eq!(fallback_home.label, "This computer");
    assert_eq!(
        fallback_home.detail.as_deref(),
        Some("Return to this desktop's Forge")
    );
    assert!(!fallback_home.disabled);

    let remembered = ThreadEnvironmentInput {
        home_host: Some(HomeHostMemory::new("Remembered desktop").with_detail("DESKTOP")),
        ..base.clone()
    };
    let remembered_projection = projection(remembered.clone());
    let remembered_home = remembered_projection.home_row.expect("remembered home row");
    assert_eq!(remembered_home.label, "Remembered desktop");
    assert_eq!(remembered_home.detail.as_deref(), Some("DESKTOP"));

    let missing_detail = projection(ThreadEnvironmentInput {
        home_host: Some(HomeHostMemory::new("Remembered desktop")),
        ..remembered.clone()
    });
    assert_eq!(
        missing_detail
            .home_row
            .as_ref()
            .and_then(|home| home.detail.as_deref()),
        Some("Return to this desktop's Forge")
    );

    let switching_home = projection(ThreadEnvironmentInput {
        switching: Some("home".to_owned()),
        ..remembered
    });
    let switching_home = switching_home
        .home_row
        .expect("home row while switching home");
    assert_eq!(switching_home.detail.as_deref(), Some("Returning…"));
    assert!(switching_home.disabled);

    let switching_machine = projection(ThreadEnvironmentInput {
        switching: Some("wsl".to_owned()),
        ..base
    });
    let switching_machine = switching_machine
        .home_row
        .expect("home row while switching machine");
    assert_eq!(
        switching_machine.detail.as_deref(),
        Some("Return to this desktop's Forge")
    );
    assert!(switching_machine.disabled);
}

#[test]
fn machine_row_details_cover_wsl_switch_error_and_informational_states() {
    let machines = HostMachinesSnapshot::new(vec![
        machine_with_detail("local", HostMachineKind::Local, "Local", "local detail"),
        machine_with_detail("wsl-a", HostMachineKind::Wsl, "WSL A", "distro A"),
        machine_with_detail("wsl-b", HostMachineKind::Wsl, "WSL B", "distro B"),
    ]);

    let ordinary = projection(ThreadEnvironmentInput {
        machines: Some(machines.clone()),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(
        ordinary.machine_rows[0].detail.as_deref(),
        Some("local detail")
    );
    assert_eq!(ordinary.machine_rows[1].detail.as_deref(), Some("distro A"));
    assert!(!ordinary.machine_rows[1].informational);

    let error = projection(ThreadEnvironmentInput {
        machines: Some(machines.clone()),
        switch_error: Some(MachineSwitchError::new("wsl-a", "could not connect")),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(
        error.machine_rows[1].detail.as_deref(),
        Some("could not connect")
    );
    assert_eq!(error.machine_rows[2].detail.as_deref(), Some("distro B"));
    assert_eq!(
        error.machine_rows[0].detail.as_deref(),
        Some("local detail")
    );
    assert!(error.machine_rows.iter().all(|row| !row.disabled));

    let switching = projection(ThreadEnvironmentInput {
        machines: Some(machines),
        switching: Some("wsl-a".to_owned()),
        switch_error: Some(MachineSwitchError::new("wsl-a", "old error")),
        ..ThreadEnvironmentInput::default()
    });
    assert!(switching.machine_rows.iter().all(|row| row.disabled));
    assert_eq!(
        switching.machine_rows[0].detail.as_deref(),
        Some("local detail")
    );
    assert_eq!(
        switching.machine_rows[1].detail.as_deref(),
        Some("Starting…")
    );
    assert_eq!(
        switching.machine_rows[2].detail.as_deref(),
        Some("distro B")
    );

    let unrelated_switch = projection(ThreadEnvironmentInput {
        machines: Some(HostMachinesSnapshot::new(vec![machine(
            "wsl-a",
            HostMachineKind::Wsl,
            "WSL A",
        )])),
        switching: Some("other".to_owned()),
        switch_error: Some(MachineSwitchError::new("wsl-a", "exact error")),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(
        unrelated_switch.machine_rows[0].detail.as_deref(),
        Some("exact error")
    );
    assert!(unrelated_switch.machine_rows[0].disabled);

    let no_detail = projection(ThreadEnvironmentInput {
        machines: Some(HostMachinesSnapshot::new(vec![machine(
            "wsl",
            HostMachineKind::Wsl,
            "WSL",
        )])),
        switch_error: Some(MachineSwitchError::new("different", "ignored")),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(no_detail.machine_rows[0].detail, None);
}

#[test]
fn branch_labels_cover_absent_detached_attached_unborn_empty_and_unicode() {
    assert_eq!(branch_label(None), "No branch");
    assert_eq!(
        branch_label(Some(&GitBranchState::detached())),
        "Detached HEAD"
    );
    assert_eq!(branch_label(Some(&attached("main"))), "main");
    assert_eq!(branch_label(Some(&unborn(""))), "");
    assert_eq!(branch_label(Some(&attached("分支/é 🦀"))), "分支/é 🦀");
}

#[test]
fn raw_kind_state_and_named_branch_helpers_preserve_exact_values() {
    let machine_kind = HostMachineKind::from_raw(" future kind ");
    assert_eq!(machine_kind.as_raw(), " future kind ");
    assert!(!machine_kind.is_wsl());
    assert_eq!(HostMachineKind::from("wsl"), HostMachineKind::Wsl);

    let repository_state = RepositoryState::from_raw(" future state ");
    assert_eq!(repository_state.as_raw(), " future state ");
    assert!(!repository_state.is_repository());
    assert_eq!(
        RepositoryState::from(String::from("repository")),
        RepositoryState::Repository
    );

    let named = GitBranchState::named("named");
    assert_eq!(named.name(), Some("named"));
    assert_eq!(GitBranchState::detached().name(), None);

    let input = ThreadEnvironmentInput::default();
    assert_eq!(
        project_thread_environment(&input),
        present_thread_environment(&input)
    );
}

#[test]
fn worktree_label_matches_split_filter_last_segment_for_all_path_shapes() {
    let cases = [
        ("/one/two/three", "three"),
        (r"C:\one\\two\three", "three"),
        (r"/one\\two///three\\", "three"),
        ("one//two///three", "three"),
        ("/", "/"),
        (r"\\\\", r"\\\\"),
        ("", ""),
        ("///\\\\", "///\\\\"),
        ("/世界/é 🦀", "é 🦀"),
        ("世界", "世界"),
        ("/one/", "one"),
    ];

    for (path, expected) in cases {
        assert_eq!(worktree_label(path), expected, "path {path:?}");
    }
}

#[test]
fn absent_workspace_uses_project_root_but_present_empty_workspace_does_not() {
    let absent = projection(ThreadEnvironmentInput {
        project_root_path: Some("/project/root".to_owned()),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(absent.current_worktree, None);
    assert_eq!(
        absent.current_worktree_path.as_deref(),
        Some("/project/root")
    );
    assert_eq!(absent.current_worktree_label.as_deref(), Some("root"));
    assert_eq!(absent.worktree_paths, vec![String::from("/project/root")]);
    assert_eq!(absent.worktree_choices[0].label, "root");

    let no_root = projection(ThreadEnvironmentInput::default());
    assert_eq!(no_root.current_worktree_path, None);
    assert!(no_root.worktree_paths.is_empty());

    let present_empty = projection(ThreadEnvironmentInput {
        project_root_path: Some("/project/root".to_owned()),
        workspace: Some(GitWorkspace::default()),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(present_empty.current_worktree, None);
    assert_eq!(
        present_empty.current_worktree_path.as_deref(),
        Some("/project/root")
    );
    assert_eq!(present_empty.worktree_paths, Vec::<String>::new());
    assert!(present_empty.worktree_choices.is_empty());

    let empty_path_is_present = projection(ThreadEnvironmentInput {
        project_root_path: Some("/project/root".to_owned()),
        workspace: Some(GitWorkspace::new(
            None,
            vec![worktree("", true, None)],
            None,
        )),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(
        empty_path_is_present.current_worktree_path.as_deref(),
        Some("")
    );
    assert_eq!(
        empty_path_is_present.current_worktree_label.as_deref(),
        Some("")
    );
    assert_eq!(empty_path_is_present.worktree_paths, vec![String::new()]);
}

#[test]
fn current_worktree_is_the_first_current_entry_and_paths_keep_order() {
    let result = projection(ThreadEnvironmentInput {
        project_root_path: Some("/root".to_owned()),
        workspace: Some(GitWorkspace::new(
            None,
            vec![
                worktree("/not/current", false, None),
                worktree("/first/current", true, Some(attached("first"))),
                worktree("/second/current", true, Some(attached("second"))),
            ],
            None,
        )),
        ..ThreadEnvironmentInput::default()
    });

    assert_eq!(
        result
            .current_worktree
            .as_ref()
            .map(|worktree| worktree.path.as_str()),
        Some("/first/current")
    );
    assert_eq!(
        result.current_worktree_path.as_deref(),
        Some("/first/current")
    );
    assert_eq!(result.current_worktree_label.as_deref(), Some("current"));
    assert_eq!(
        result.worktree_paths,
        vec![
            String::from("/not/current"),
            String::from("/first/current"),
            String::from("/second/current"),
        ]
    );
}

#[test]
fn workspace_branch_precedes_repository_branch_but_absent_branch_falls_back() {
    let repo_branch = attached("repository");
    let repo = repository(
        RepositoryState::Repository,
        Some(repo_branch.clone()),
        None,
        Vec::new(),
    );

    let workspace_wins = projection(ThreadEnvironmentInput {
        repository: Some(repo.clone()),
        workspace: Some(GitWorkspace::new(
            Some(attached("workspace")),
            Vec::new(),
            None,
        )),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(workspace_wins.current_branch, Some(attached("workspace")));
    assert_eq!(
        workspace_wins.current_branch_label.as_deref(),
        Some("workspace")
    );

    let repository_fallback = projection(ThreadEnvironmentInput {
        repository: Some(repo),
        workspace: Some(GitWorkspace::default()),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(repository_fallback.current_branch, Some(repo_branch));

    let no_repo_fallback = projection(ThreadEnvironmentInput {
        repository: Some(repository(
            RepositoryState::NotRepository,
            Some(attached("ignored")),
            None,
            Vec::new(),
        )),
        workspace: Some(GitWorkspace::default()),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(no_repo_fallback.current_branch, None);

    let no_workspace_repository_branch = projection(ThreadEnvironmentInput {
        repository: Some(repository(
            RepositoryState::Repository,
            Some(attached("root")),
            None,
            Vec::new(),
        )),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(
        no_workspace_repository_branch
            .current_branch_label
            .as_deref(),
        Some("root")
    );
}

#[test]
fn aggregate_changes_are_workspace_only_and_optional() {
    let repository_summary = GitDiffSummary::new(90, 80);
    let workspace_summary = GitDiffSummary::new(3, 2);
    let repository = repository(RepositoryState::Repository, None, None, Vec::new());

    let no_workspace = projection(ThreadEnvironmentInput {
        repository: Some(repository.clone()),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(no_workspace.change_summary, None);

    let present_without_summary = projection(ThreadEnvironmentInput {
        repository: Some(repository.clone()),
        workspace: Some(GitWorkspace::default()),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(present_without_summary.change_summary, None);

    let present_with_summary = projection(ThreadEnvironmentInput {
        repository: Some(repository),
        workspace: Some(GitWorkspace::new(None, Vec::new(), Some(workspace_summary))),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(present_with_summary.change_summary, Some(workspace_summary));
    assert_ne!(
        present_with_summary.change_summary,
        Some(repository_summary)
    );
}

#[test]
fn default_remote_is_the_first_matching_remote_only_for_repository_state() {
    let first = remote("origin", Some("https://first.example"));
    let second = remote("origin", Some("https://second.example"));
    let other = remote("upstream", None);

    let repository_projection = projection(ThreadEnvironmentInput {
        repository: Some(repository(
            RepositoryState::Repository,
            None,
            Some("origin"),
            vec![other.clone(), first.clone(), second],
        )),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(repository_projection.default_remote, Some(first));

    let no_name = projection(ThreadEnvironmentInput {
        repository: Some(repository(
            RepositoryState::Repository,
            None,
            None,
            vec![other.clone()],
        )),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(no_name.default_remote, None);

    for state in [
        RepositoryState::NotRepository,
        RepositoryState::Other("future".to_owned()),
    ] {
        let ignored = projection(ThreadEnvironmentInput {
            repository: Some(repository(
                state,
                Some(attached("ignored")),
                Some("upstream"),
                vec![other.clone()],
            )),
            ..ThreadEnvironmentInput::default()
        });
        assert_eq!(ignored.default_remote, None);
        assert_eq!(ignored.current_branch, None);
    }

    let url_absent_is_preserved = projection(ThreadEnvironmentInput {
        repository: Some(repository(
            RepositoryState::Repository,
            None,
            Some("upstream"),
            vec![other.clone()],
        )),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(url_absent_is_preserved.default_remote, Some(other));
}

#[test]
fn branch_choices_skip_absent_branches_and_replace_collisions_in_place() {
    let result = projection(ThreadEnvironmentInput {
        workspace: Some(GitWorkspace::new(
            Some(attached("main")),
            vec![
                worktree("/one", false, None),
                worktree("/two", false, Some(attached("main"))),
                worktree("/three", false, Some(unborn("main"))),
                worktree("/four", false, Some(GitBranchState::detached())),
                worktree("/five", false, Some(attached("feature"))),
                worktree("/six", false, Some(unborn("feature"))),
            ],
            None,
        )),
        ..ThreadEnvironmentInput::default()
    });

    assert_eq!(
        result
            .branch_choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>(),
        ["main", "Detached HEAD", "feature"]
    );
    assert_eq!(result.branch_choices[0].branch, attached("main"));
    assert_eq!(result.branch_choices[1].branch, GitBranchState::detached());
    assert_eq!(result.branch_choices[2].branch, unborn("feature"));
}

#[test]
fn current_branch_is_inserted_last_and_replaces_without_moving_position() {
    let result = projection(ThreadEnvironmentInput {
        workspace: Some(GitWorkspace::new(
            Some(unborn("main")),
            vec![
                worktree("/one", false, Some(attached("main"))),
                worktree("/two", false, Some(attached("other"))),
            ],
            None,
        )),
        ..ThreadEnvironmentInput::default()
    });

    assert_eq!(
        result
            .branch_choices
            .iter()
            .map(|choice| choice.label.as_str())
            .collect::<Vec<_>>(),
        ["main", "other"]
    );
    assert_eq!(result.branch_choices[0].branch, unborn("main"));

    let absent_workspace = projection(ThreadEnvironmentInput {
        repository: Some(repository(
            RepositoryState::Repository,
            Some(attached("repo")),
            None,
            Vec::new(),
        )),
        ..ThreadEnvironmentInput::default()
    });
    assert_eq!(absent_workspace.branch_choices.len(), 1);
    assert_eq!(absent_workspace.branch_choices[0].label, "repo");
}

#[test]
fn projection_is_owned_and_repeated_evaluation_is_deterministic() {
    let input = ThreadEnvironmentInput {
        desktop: true,
        identity: Some(HostIdentitySnapshot::new("host")),
        machines: Some(HostMachinesSnapshot::new(vec![
            machine_with_detail(
                "local",
                HostMachineKind::Local,
                "This computer on WSL2",
                "host",
            ),
            machine_with_detail("wsl", HostMachineKind::Wsl, "Linux", "distro"),
        ])),
        home_host: Some(HomeHostMemory::new("desktop").with_detail("host")),
        repository: Some(repository(
            RepositoryState::Repository,
            Some(attached("main")),
            Some("origin"),
            vec![remote("origin", Some("https://example.test"))],
        )),
        workspace: Some(GitWorkspace::new(
            Some(attached("feature")),
            vec![worktree("/root/feature", true, Some(attached("feature")))],
            Some(GitDiffSummary::new(1, 2)),
        )),
        project_root_path: Some("/root".to_owned()),
        switching: Some("wsl".to_owned()),
        switch_error: Some(MachineSwitchError::new("other", "ignored")),
    };
    let first = present_thread_environment(&input);
    let second = present_thread_environment(&input);
    assert_eq!(first, second);
    assert_eq!(first.machine_label, "This computer on WSL2");
    assert_eq!(first.current_branch_label.as_deref(), Some("feature"));
    assert_eq!(first.current_worktree_label.as_deref(), Some("feature"));
    assert_eq!(
        first
            .default_remote
            .as_ref()
            .and_then(|remote| remote.web_url.as_deref()),
        Some("https://example.test")
    );
}
