//! Focused, dependency-free coverage for host identity policy.

#![forbid(unsafe_code)]
#![allow(dead_code)]

#[path = "../../modules/backend/src/host_identity_policy.rs"]
mod host_identity_policy;

use host_identity_policy::{
    CommandRunnerOutcome, DisplayNameCommand, HostIdentityPolicy, HostIdentitySnapshot,
    HostPlatform, build_display_name_command, is_safe_windows_username, map_host_platform,
    parse_display_name, project_snapshot, project_snapshot_from_runner, resolve_display_name,
};

#[test]
fn platform_mapping_is_closed_and_exhaustive() {
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
fn windows_username_grammar_accepts_only_the_exact_ascii_allow_list() {
    for username in ["A", "Alice Doe_9-.", "0", " ._-", "name with spaces"] {
        assert!(
            is_safe_windows_username(username),
            "expected safe Windows username: {username:?}"
        );
    }

    for username in [
        "",
        "alice' -and $true",
        "alice; Get-Process",
        "alice\"",
        "alice/path",
        "alice\\path",
        "alice\tname",
        "alice\nname",
        "Ålice",
        "🦀",
    ] {
        assert!(
            !is_safe_windows_username(username),
            "expected unsafe Windows username: {username:?}"
        );
    }
}

#[test]
fn windows_command_preserves_the_exact_script_and_arguments() {
    let username = "Alice Doe_9-.";
    let script = "(Get-CimInstance Win32_UserAccount -Filter \"Name='Alice Doe_9-.'\" | "
        .to_owned()
        + "Select-Object -First 1).FullName";

    assert_eq!(
        build_display_name_command(HostPlatform::Win32, Some(username)),
        Some(DisplayNameCommand {
            command: "powershell.exe".to_owned(),
            args: vec![
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                "-Command".to_owned(),
                script,
            ],
        })
    );
}

#[test]
fn missing_unsafe_and_unknown_inputs_do_not_build_commands() {
    assert_eq!(build_display_name_command(HostPlatform::Win32, None), None);
    assert_eq!(
        build_display_name_command(HostPlatform::Win32, Some("alice'")),
        None
    );
    assert_eq!(
        build_display_name_command(HostPlatform::Unknown, Some("alice")),
        None
    );

    assert_eq!(
        resolve_display_name(
            HostPlatform::Win32,
            None,
            &CommandRunnerOutcome::succeeded("Alice")
        ),
        None
    );
    assert_eq!(
        resolve_display_name(
            HostPlatform::Win32,
            Some("alice'"),
            &CommandRunnerOutcome::succeeded("Alice")
        ),
        None
    );
    assert_eq!(
        resolve_display_name(
            HostPlatform::Unknown,
            Some("alice"),
            &CommandRunnerOutcome::succeeded("Alice")
        ),
        None
    );
}

#[test]
fn darwin_and_linux_commands_keep_their_exact_shapes() {
    assert_eq!(
        build_display_name_command(HostPlatform::Darwin, Some("ignored")),
        Some(DisplayNameCommand {
            command: "id".to_owned(),
            args: vec!["-F".to_owned()],
        })
    );

    let exact_username = "account name;still-one-argv";
    assert_eq!(
        build_display_name_command(HostPlatform::Linux, Some(exact_username)),
        Some(DisplayNameCommand {
            command: "getent".to_owned(),
            args: vec!["passwd".to_owned(), exact_username.to_owned()],
        })
    );
}

#[test]
fn direct_output_is_trimmed_for_windows_and_darwin() {
    assert_eq!(
        parse_display_name(HostPlatform::Win32, " \r\nAda Lovelace\t "),
        Some("Ada Lovelace".to_owned())
    );
    assert_eq!(
        parse_display_name(HostPlatform::Darwin, " \nGrace Hopper\r\n"),
        Some("Grace Hopper".to_owned())
    );
}

#[test]
fn linux_parser_selects_the_fifth_field_before_the_first_gecos_comma() {
    let output = "ada:x:1000:1000: Ada Lovelace,Room 42,555-0100:/home/ada:/bin/bash\n";

    assert_eq!(
        parse_display_name(HostPlatform::Linux, output),
        Some("Ada Lovelace".to_owned())
    );
}

#[test]
fn empty_whitespace_and_malformed_outputs_are_absent() {
    for platform in [
        HostPlatform::Win32,
        HostPlatform::Darwin,
        HostPlatform::Linux,
    ] {
        assert_eq!(parse_display_name(platform, ""), None);
        assert_eq!(parse_display_name(platform, " \r\n\t "), None);
    }

    for malformed in [
        "ada:x:1000:1000",
        "ada:x:1000:1000:",
        "ada:x:1000:1000:,Room 42:/home/ada:/bin/bash",
        "ada:x:1000:1000:   ,Room 42:/home/ada:/bin/bash",
    ] {
        assert_eq!(parse_display_name(HostPlatform::Linux, malformed), None);
    }
}

#[test]
fn runner_failures_and_timeouts_are_contained_as_absent_display_names() {
    let output = CommandRunnerOutcome::succeeded("Ada Lovelace\n");
    assert_eq!(
        resolve_display_name(HostPlatform::Darwin, Some("ada"), &output),
        Some("Ada Lovelace".to_owned())
    );

    for outcome in [
        CommandRunnerOutcome::failed(),
        CommandRunnerOutcome::timed_out(),
    ] {
        assert_eq!(
            resolve_display_name(HostPlatform::Darwin, Some("ada"), &outcome),
            None
        );
    }

    assert_eq!(
        resolve_display_name(
            HostPlatform::Linux,
            Some("ada"),
            &CommandRunnerOutcome::succeeded("ada:x:1000:1000:\n")
        ),
        None
    );
}

#[test]
fn snapshot_projection_keeps_optional_fields_and_exact_identity_custody() {
    let snapshot = project_snapshot(
        "\tHost/é/🦀 ",
        HostPlatform::Linux,
        Some(" user/é/🦀 ".to_owned()),
        Some(" display/é/🦀 ".to_owned()),
    );

    assert_eq!(
        snapshot,
        HostIdentitySnapshot {
            display_name: Some(" display/é/🦀 ".to_owned()),
            hostname: "\tHost/é/🦀 ".to_owned(),
            platform: HostPlatform::Linux,
            username: Some(" user/é/🦀 ".to_owned()),
        }
    );

    let absent = project_snapshot(
        "host",
        HostPlatform::Unknown,
        None,
        Some("ignored".to_owned()),
    );
    assert_eq!(absent.display_name, None);
    assert_eq!(absent.username, None);
    assert_eq!(absent.platform, HostPlatform::Unknown);
}

#[test]
fn runner_projection_retains_username_when_lookup_is_unavailable() {
    let username = "unsafe'user";
    let failure = project_snapshot_from_runner(
        "machine",
        HostPlatform::Win32,
        Some(username),
        &CommandRunnerOutcome::failed(),
    );

    assert_eq!(failure.hostname, "machine");
    assert_eq!(failure.platform, HostPlatform::Win32);
    assert_eq!(failure.username.as_deref(), Some(username));
    assert_eq!(failure.display_name, None);
}

#[test]
fn policy_evaluations_are_repeated_and_independent_without_caching() {
    let policy = HostIdentityPolicy::new();
    assert_eq!(policy, HostIdentityPolicy);

    let successful =
        CommandRunnerOutcome::succeeded("ada:x:1000:1000: Ada Lovelace,Room:/home/ada:/bin/bash\n");
    let first = HostIdentityPolicy::evaluate("machine", "linux", Some("ada"), &successful);
    let second = HostIdentityPolicy::evaluate("machine", "linux", Some("ada"), &successful);
    assert_eq!(first, second);

    let failed = HostIdentityPolicy::evaluate(
        "machine",
        "linux",
        Some("ada"),
        &CommandRunnerOutcome::timed_out(),
    );
    assert_eq!(first.display_name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(failed.display_name, None);
    assert_eq!(failed.username.as_deref(), Some("ada"));
}
