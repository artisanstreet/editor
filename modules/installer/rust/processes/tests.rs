//! Process discovery, classification, and retirement ordering tests.

use super::*;

fn process(pid: u32, executable: &str) -> RunningProcess {
    RunningProcess {
        pid,
        executable: PathBuf::from(executable),
    }
}

#[test]
fn discovery_lines_become_processes_and_junk_is_ignored() {
    let parsed = parse_discovery(
        "1234|C:\\Artisan\\versions\\0.2.11\\bin\\editor.exe\n\nnot-a-line\n7|C:\\Artisan\\versions\\0.2.11\\bin\\forge.exe\n",
    );
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].pid, 1234);
    assert_eq!(parsed[1].pid, 7);
}

#[test]
fn native_roles_require_the_exact_three_component_layout() {
    let versions = Path::new("/Artisan/versions");

    assert_eq!(
        classify_executable(
            Path::new("/Artisan/versions/0.2.11/bin/editor.exe"),
            versions,
            true,
        ),
        Some(ProcessRole::Editor)
    );
    assert_eq!(
        classify_executable(
            Path::new("/Artisan/versions/0.2.11/bin/FORGE.EXE"),
            versions,
            true,
        ),
        Some(ProcessRole::Forge)
    );
    assert_eq!(
        classify_executable(
            Path::new("/Artisan/versions/0.2.11/bin/editor"),
            versions,
            false,
        ),
        Some(ProcessRole::Editor)
    );
    assert_eq!(
        classify_executable(
            Path::new("/Artisan/versions/0.2.11/bin/forge"),
            versions,
            false,
        ),
        Some(ProcessRole::Forge)
    );
}

#[test]
fn leaf_case_rules_are_platform_specific() {
    let versions = Path::new("/Artisan/versions");
    assert_eq!(
        classify_executable(
            Path::new("/Artisan/versions/0.2.11/bin/EdItOr.ExE"),
            versions,
            true,
        ),
        Some(ProcessRole::Editor)
    );
    assert_eq!(
        classify_executable(
            Path::new("/Artisan/versions/0.2.11/bin/EdItOr"),
            versions,
            false,
        ),
        None
    );
    assert_eq!(
        classify_executable(
            Path::new("/Artisan/versions/0.2.11/bin/editor.exe"),
            versions,
            false,
        ),
        None
    );
    assert_eq!(
        classify_executable(
            Path::new("/Artisan/versions/0.2.11/bin/EDITOR"),
            versions,
            false,
        ),
        None
    );
}

#[test]
fn non_native_paths_remain_unclassified() {
    let versions = Path::new("/Artisan/versions");
    let windows_rejections = [
        "/Artisan/versions/0.2.11/bin/ae.exe",
        "/Artisan/versions/0.2.11/bin/installer.exe",
        "/Artisan/versions/0.2.11/bin/readme.exe",
        "/Artisan/versions/0.2.11/bin/editor/child.exe",
        "/Artisan/versions/0.2.11/editor/Artisan Editor.exe",
        "/Artisan/versions/0.2.11/forge/forge.exe",
        "/Artisan/versions/0.2.11/bin/Artisan Editor.exe",
        "/Artisan/versions/0.2.11/bin/node.exe",
        "/Artisan/versions/0.2.11/bin/broker.exe",
        "/Artisan/versions-old/0.2.11/bin/editor.exe",
        "/Artisan/versions-sibling/0.2.11/bin/forge.exe",
        "/Other/versions/0.2.11/bin/editor.exe",
    ];
    for path in windows_rejections {
        assert_eq!(
            classify_executable(Path::new(path), versions, true),
            None,
            "classified non-native Windows path: {path}"
        );
    }

    let non_windows_rejections = [
        "/Artisan/versions/0.2.11/bin/ae",
        "/Artisan/versions/0.2.11/bin/installer",
        "/Artisan/versions/0.2.11/bin/readme",
        "/Artisan/versions/0.2.11/bin/editor/child",
        "/Artisan/versions/0.2.11/editor/display-name",
        "/Artisan/versions/0.2.11/forge/node",
        "/Artisan/versions/0.2.11/bin/Artisan Editor",
        "/Artisan/versions/0.2.11/bin/node",
        "/Artisan/versions/0.2.11/bin/broker",
        "/Artisan/versions-old/0.2.11/bin/editor",
        "/Artisan/versions-sibling/0.2.11/bin/forge",
        "/Other/versions/0.2.11/bin/editor",
    ];
    for path in non_windows_rejections {
        assert_eq!(
            classify_executable(Path::new(path), versions, false),
            None,
            "classified non-native non-Windows path: {path}"
        );
    }
}

/// The incoming release's own processes are not staleness; retiring them
/// would kill the very instance an update is about to hand back to.
#[test]
fn the_incoming_release_is_never_superseded() {
    let release = Path::new("/Artisan/versions/0.2.14");
    let current = process(3, "/Artisan/versions/0.2.14/bin/editor");
    let stale = process(4, "/Artisan/versions/0.2.11/bin/editor");
    assert!(current.executable.starts_with(release));
    assert!(!stale.executable.starts_with(release));
    assert!(!Path::new("/Artisan/versions/0.2.140").starts_with(release));
}

#[test]
fn retirement_partition_contains_only_native_forge_and_editor_processes() {
    let versions = Path::new("/Artisan/versions");
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let processes = vec![
        process(1, &format!("/Artisan/versions/0.2.11/bin/forge{suffix}")),
        process(2, &format!("/Artisan/versions/0.2.11/bin/editor{suffix}")),
        process(3, &format!("/Artisan/versions/0.2.11/bin/ae{suffix}")),
        process(
            4,
            &format!("/Artisan/versions/0.2.11/editor/display{suffix}"),
        ),
        process(
            5,
            &format!("/Artisan/versions/0.2.11/bin/forge/child{suffix}"),
        ),
    ];
    let (forges, editors) = partition_by_role(&processes, versions);
    assert_eq!(
        forges.iter().map(|process| process.pid).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(
        editors
            .iter()
            .map(|process| process.pid)
            .collect::<Vec<_>>(),
        vec![2]
    );
}

#[test]
fn a_reused_pid_does_not_keep_a_retired_forge_alive() {
    let expected = process(6172, "/Artisan/versions/0.2.27/bin/forge");
    let reused = process(6172, "/Windows/System32/notepad.exe");
    assert!(still_running(&[&expected], &[reused]).is_empty());
}

#[test]
fn the_same_versioned_executable_remains_a_live_identity() {
    let expected = process(6172, "/Artisan/versions/0.2.27/bin/forge");
    let discovered = expected.clone();
    assert!(same_process(&discovered, &expected));
}

#[test]
fn orderly_retirement_targets_the_discovered_forge_pid() {
    assert_eq!(
        exact_stop_arguments(6172, true),
        ["stop", "--pid", "6172", "--if-idle"].map(str::to_owned)
    );
    assert_eq!(
        exact_stop_arguments(6172, false),
        ["stop", "--pid", "6172"].map(str::to_owned)
    );
}

#[test]
fn only_an_authenticated_busy_report_cancels_safe_retirement() {
    assert_eq!(
        forge_stop_disposition(Some(FORGE_BUSY_EXIT_CODE)),
        ForgeStopDisposition::Busy
    );
    assert_eq!(
        forge_stop_disposition(Some(FORGE_ACTIVITY_UNAVAILABLE_EXIT_CODE)),
        ForgeStopDisposition::Unresponsive
    );
    assert_eq!(
        forge_stop_disposition(Some(0)),
        ForgeStopDisposition::Accepted
    );
    assert_eq!(
        forge_stop_disposition(Some(1)),
        ForgeStopDisposition::Failed
    );
}

#[cfg(not(windows))]
#[test]
fn executable_projection_preserves_space_in_root_without_arguments() {
    let versions = Path::new("/opt/Artisan Street/versions");
    let parsed = parse_executable_projection(
        "1234 /opt/Artisan Street/versions/0.2.11/bin/editor\n7 /opt/Artisan Street/versions/0.2.11/bin/forge\n",
        versions,
    );
    assert_eq!(parsed.len(), 2);
    assert_eq!(
        parsed[0].executable,
        PathBuf::from("/opt/Artisan Street/versions/0.2.11/bin/editor")
    );
    assert_eq!(parsed[1].executable, versions.join("0.2.11/bin/forge"));
}

#[cfg(not(windows))]
#[test]
fn ps_args_fallback_recovers_native_path_before_arguments() {
    let versions = Path::new("/opt/Artisan Street/versions");
    let parsed = parse_ps_args_discovery(
        "1234 /opt/Artisan Street/versions/0.2.11/bin/editor --project '/tmp/with spaces'\n7 /opt/Artisan Street/versions/0.2.11/bin/forge --idle\n8 /opt/Artisan Street/versions/0.2.11/bin/editor.exe --wrong-extension\n9 /opt/Artisan Street/versions-old/0.2.11/bin/editor\n",
        versions,
    );
    assert_eq!(parsed.len(), 2);
    assert_eq!(
        parsed[0].executable,
        PathBuf::from("/opt/Artisan Street/versions/0.2.11/bin/editor")
    );
    assert_eq!(
        parsed[1].executable,
        PathBuf::from("/opt/Artisan Street/versions/0.2.11/bin/forge")
    );
    assert!(!parsed[0].executable.to_string_lossy().contains("--project"));
}

#[cfg(windows)]
#[test]
fn windows_discovery_projects_only_native_executable_paths() {
    let script = windows_discovery_script(Path::new("C:/Artisan/versions"));

    assert!(script.contains("ExecutablePath"));
    assert!(script.contains("ProcessId"));
    assert!(!script.contains("CommandLine"));
}
