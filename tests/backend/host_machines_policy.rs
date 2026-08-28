//! Exhaustive focused coverage for the dependency-free host-machines policy.

#![forbid(unsafe_code)]
#![allow(dead_code)]

#[path = "../../modules/backend/src/host_machines_policy.rs"]
mod host_machines_policy;

use std::time::Duration;

use host_machines_policy::{
    CommandRunnerOutcome, HostMachineKind, HostMachineSnapshot, HostMachinesAction,
    HostMachinesPolicy, HostMachinesSnapshot, HostPlatform, WSL_ENUMERATION_ARGS,
    WSL_ENUMERATION_COMMAND, WSL_ENUMERATION_TIMEOUT, WSL_ENUMERATION_TIMEOUT_MS,
    WslEnumerationCommand, build_machines_snapshot, build_wsl_enumeration_command,
    enumeration_action, map_host_platform, parse_wsl_distributions, project_snapshot,
    project_snapshot_from_runner, resolve_wsl_distributions,
};

fn machine(id: &str, kind: HostMachineKind, label: &str, detail: &str) -> HostMachineSnapshot {
    HostMachineSnapshot::new(id, kind, label).with_detail(detail)
}

fn snapshot(machines: Vec<HostMachineSnapshot>) -> HostMachinesSnapshot {
    HostMachinesSnapshot::new(machines)
}

#[test]
fn platform_mapping_is_closed_and_exact() {
    let cases = [
        ("win32", HostPlatform::Win32),
        ("darwin", HostPlatform::Darwin),
        ("linux", HostPlatform::Linux),
        ("freebsd", HostPlatform::Unknown),
        ("", HostPlatform::Unknown),
        ("Win32", HostPlatform::Unknown),
        ("linux ", HostPlatform::Unknown),
    ];

    for (runtime_platform, expected) in cases {
        assert_eq!(map_host_platform(runtime_platform), expected);
        assert_eq!(HostPlatform::from_runtime(runtime_platform), expected);
    }

    assert_eq!(HostPlatform::Win32.as_str(), "win32");
    assert_eq!(HostPlatform::Darwin.as_str(), "darwin");
    assert_eq!(HostPlatform::Linux.as_str(), "linux");
    assert_eq!(HostPlatform::Unknown.as_str(), "unknown");
}

#[test]
fn enumeration_descriptor_preserves_command_arguments_and_five_second_bound() {
    let expected = WslEnumerationCommand {
        args: vec!["-l".to_owned(), "-q".to_owned()],
        command: "wsl.exe".to_owned(),
        timeout_ms: 5_000,
    };

    assert_eq!(WSL_ENUMERATION_COMMAND, "wsl.exe");
    assert_eq!(WSL_ENUMERATION_ARGS, ["-l", "-q"]);
    assert_eq!(WSL_ENUMERATION_TIMEOUT_MS, 5_000);
    assert_eq!(WSL_ENUMERATION_TIMEOUT, Duration::from_secs(5));
    assert_eq!(build_wsl_enumeration_command(), expected);
    assert_eq!(
        WslEnumerationCommand::default().timeout(),
        Duration::from_secs(5)
    );
}

#[test]
fn platform_action_runs_only_the_windows_enumeration() {
    let expected = HostMachinesAction::EnumerateWsl {
        command: build_wsl_enumeration_command(),
    };

    assert_eq!(enumeration_action(HostPlatform::Win32), expected);
    assert!(enumeration_action(HostPlatform::Win32).command().is_some());
    for platform in [
        HostPlatform::Darwin,
        HostPlatform::Linux,
        HostPlatform::Unknown,
    ] {
        assert_eq!(
            enumeration_action(platform),
            HostMachinesAction::SkipWslEnumeration
        );
        assert!(enumeration_action(platform).command().is_none());
    }
    assert_eq!(
        HostMachinesPolicy::enumeration_action(HostPlatform::Win32),
        expected
    );
}

#[test]
fn parser_removes_nuls_bom_carriage_returns_and_blank_lines() {
    let raw = "\u{feff}U\0b\0u\0n\0t\0u\0\r\0\n\0 \r\n\r\nD\0e\0b\0i\0a\0n\0\r\0\n";

    assert_eq!(parse_wsl_distributions(raw), ["Ubuntu", "Debian"]);
    assert_eq!(parse_wsl_distributions(""), Vec::<String>::new());
    assert_eq!(
        parse_wsl_distributions("\r\n \r\n\t\n"),
        Vec::<String>::new()
    );
}

#[test]
fn parser_excludes_all_four_utility_distributions_case_insensitively() {
    let parsed = parse_wsl_distributions(
        &("Ubuntu\nDOCKER-DESKTOP\nDOCKER-DESKTOP-DATA\nRANCHER-DESKTOP\n".to_owned()
            + "RANCHER-DESKTOP-DATA\nubuntu\nUbuntu\n"),
    );

    assert_eq!(parsed, ["Ubuntu", "ubuntu", "Ubuntu"]);
}

#[test]
fn parser_preserves_retained_order_duplicates_and_spelling() {
    assert_eq!(
        parse_wsl_distributions("  Fedora  \r\nUbuntu\nFedora\n"),
        ["Fedora", "Ubuntu", "Fedora"]
    );
}

#[test]
fn windows_snapshot_keeps_local_first_and_projects_wsl_order_duplicates() {
    let distributions = ["Ubuntu", "Ubuntu", "artisan"];
    let actual = build_machines_snapshot(HostPlatform::Win32, "DESKTOP-1", None, distributions);

    assert_eq!(
        actual,
        snapshot(vec![
            machine(
                "local",
                HostMachineKind::Local,
                "This computer",
                "DESKTOP-1"
            ),
            machine(
                "wsl:Ubuntu",
                HostMachineKind::Wsl,
                "This computer on WSL2",
                "Ubuntu",
            ),
            machine(
                "wsl:Ubuntu",
                HostMachineKind::Wsl,
                "This computer on WSL2",
                "Ubuntu",
            ),
            machine(
                "wsl:artisan",
                HostMachineKind::Wsl,
                "This computer on WSL2",
                "artisan",
            ),
        ])
    );
}

#[test]
fn linux_wsl_environment_changes_only_local_label_and_detail() {
    let actual = build_machines_snapshot(
        HostPlatform::Linux,
        "wsl-host",
        Some("artisan"),
        ["Ubuntu", "Debian"],
    );

    assert_eq!(
        actual,
        snapshot(vec![machine(
            "local",
            HostMachineKind::Local,
            "This computer on WSL2",
            "artisan",
        )])
    );
}

#[test]
fn only_linux_with_present_environment_uses_wsl_local_label() {
    let empty_environment = build_machines_snapshot(
        HostPlatform::Linux,
        "linux-host",
        Some(""),
        std::iter::empty::<&str>(),
    );
    assert_eq!(
        empty_environment.machines[0],
        HostMachineSnapshot {
            detail: Some(String::new()),
            id: "local".to_owned(),
            kind: HostMachineKind::Local,
            label: "This computer on WSL2".to_owned(),
        }
    );

    for platform in [
        HostPlatform::Win32,
        HostPlatform::Darwin,
        HostPlatform::Unknown,
    ] {
        let actual = build_machines_snapshot(platform, "host", Some("ignored"), ["Ubuntu"]);
        assert_eq!(actual.machines[0].label, "This computer");
        assert_eq!(actual.machines[0].detail.as_deref(), Some("host"));
    }
}

#[test]
fn non_windows_hosts_never_project_wsl_entries() {
    for platform in [
        HostPlatform::Darwin,
        HostPlatform::Linux,
        HostPlatform::Unknown,
    ] {
        let actual = build_machines_snapshot(platform, "host", None, ["Ubuntu", "Debian"]);
        assert_eq!(
            actual,
            snapshot(vec![machine(
                "local",
                HostMachineKind::Local,
                "This computer",
                "host",
            )])
        );
    }
}

#[test]
fn command_failure_and_timeout_fall_back_to_empty_windows_wsl_list() {
    for outcome in [
        CommandRunnerOutcome::failed(),
        CommandRunnerOutcome::timed_out(),
    ] {
        assert_eq!(
            resolve_wsl_distributions(HostPlatform::Win32, &outcome),
            Vec::<String>::new()
        );

        let actual = project_snapshot_from_runner("DESKTOP-1", HostPlatform::Win32, None, &outcome);
        assert_eq!(
            actual,
            snapshot(vec![machine(
                "local",
                HostMachineKind::Local,
                "This computer",
                "DESKTOP-1",
            )])
        );
    }
}

#[test]
fn successful_completion_is_parsed_only_for_windows() {
    let output = CommandRunnerOutcome::succeeded("Ubuntu\ndocker-desktop\n");
    assert_eq!(
        resolve_wsl_distributions(HostPlatform::Win32, &output),
        ["Ubuntu"]
    );
    assert_eq!(
        resolve_wsl_distributions(HostPlatform::Linux, &output),
        Vec::<String>::new()
    );

    let actual = HostMachinesPolicy::evaluate("DESKTOP-1", "win32", None, &output);
    assert_eq!(actual.machines[0].id, "local");
    assert_eq!(actual.machines[1].id, "wsl:Ubuntu");
    assert_eq!(actual.machines.len(), 2);
}

#[test]
fn projection_facades_are_stateless_and_preserve_supplied_values() {
    let direct = project_snapshot("host", HostPlatform::Win32, None, ["Ubuntu", "Ubuntu"]);
    let completed = CommandRunnerOutcome::succeeded("Ubuntu\nUbuntu\n");
    let from_runner = project_snapshot_from_runner("host", HostPlatform::Win32, None, &completed);

    assert_eq!(direct, from_runner);
    assert_eq!(HostMachinesPolicy::new(), HostMachinesPolicy);
    assert_eq!(HostMachineKind::Local.as_str(), "local");
    assert_eq!(HostMachineKind::Wsl.as_str(), "wsl");
    assert_eq!(from_runner.as_slice(), &from_runner.machines);
}
