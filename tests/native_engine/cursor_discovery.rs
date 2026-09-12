//! Cursor executable discovery precedence and version-parsing fixtures.
//!
//! Registration (controller-owned, not part of this packet):
//! `tests/native_engine/BUILD.bazel` gains a `rust_test` target for
//! `cursor_discovery.rs` depending on `//modules/native_engine:native_engine`,
//! plus a Cargo `[[test]] cursor_discovery` entry in
//! `modules/native_engine/Cargo.toml`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use artisan_native_engine::cursor::{
    CursorResolveSource, explicit_override, find_cursor_on_path, parse_cursor_version,
    resolve_cursor_binary,
};

fn existence(existing: &[&str]) -> HashMap<PathBuf, bool> {
    existing
        .iter()
        .map(|path| (PathBuf::from(path), true))
        .collect()
}

fn exists(map: &HashMap<PathBuf, bool>) -> impl Fn(&Path) -> bool + '_ {
    move |path| map.get(path).copied().unwrap_or(false)
}

#[cfg(windows)]
fn path_var(directories: &[&str]) -> String {
    directories.join(";")
}

#[cfg(not(windows))]
fn path_var(directories: &[&str]) -> String {
    directories.join(":")
}

#[test]
fn explicit_override_wins_verbatim_with_spaces() {
    let configured = "C:\\Program Files\\Custom Tools\\cursor-agent.exe";
    let map = existence(&["C:\\Tools\\cursor-agent.exe"]);
    let resolved = resolve_cursor_binary(
        Some(&format!("  {configured}  ")),
        Some(&path_var(&["C:\\Tools"])),
        exists(&map),
    )
    .unwrap();
    assert_eq!(resolved.path(), Path::new(configured));
    assert_eq!(resolved.source(), CursorResolveSource::ExplicitOverride);
}

#[test]
fn blank_override_falls_through_to_path_lookup() {
    let candidate = if cfg!(windows) {
        "C:\\Tools\\cursor-agent.exe"
    } else {
        "/opt/cursor/cursor-agent"
    };
    let map = existence(&[candidate]);
    let resolved = resolve_cursor_binary(
        Some("   "),
        Some(&path_var(&[if cfg!(windows) {
            "C:\\Tools"
        } else {
            "/opt/cursor"
        }])),
        exists(&map),
    )
    .unwrap();
    assert_eq!(resolved.path(), Path::new(candidate));
    assert_eq!(resolved.source(), CursorResolveSource::PathLookup);
    assert_eq!(explicit_override(Some("  ")), None);
    assert_eq!(explicit_override(None), None);
}

#[test]
fn cursor_agent_is_preferred_over_agent_names() {
    let (directory, preferred, fallback) = if cfg!(windows) {
        (
            "C:\\Tools",
            "C:\\Tools\\cursor-agent.exe",
            "C:\\Tools\\agent.cmd",
        )
    } else {
        (
            "/opt/cursor",
            "/opt/cursor/cursor-agent",
            "/opt/cursor/agent",
        )
    };
    let map = existence(&[preferred, fallback]);
    let resolved =
        resolve_cursor_binary(None, Some(&path_var(&[directory])), exists(&map)).unwrap();
    assert_eq!(resolved.path(), Path::new(preferred));
}

#[test]
fn typescript_default_agent_names_resolve_as_fallback() {
    let (directory, fallback) = if cfg!(windows) {
        ("C:\\Tools", "C:\\Tools\\agent.cmd")
    } else {
        ("/opt/cursor", "/opt/cursor/agent")
    };
    let map = existence(&[fallback]);
    let resolved =
        resolve_cursor_binary(None, Some(&path_var(&[directory])), exists(&map)).unwrap();
    assert_eq!(resolved.path(), Path::new(fallback));
    assert_eq!(resolved.source(), CursorResolveSource::PathLookup);
}

#[test]
fn paths_containing_spaces_resolve_without_quoting() {
    let (directory, candidate) = if cfg!(windows) {
        (
            "D:\\My Tools\\Cursor Bin",
            "D:\\My Tools\\Cursor Bin\\cursor-agent.exe",
        )
    } else {
        ("/opt/cursor code", "/opt/cursor code/cursor-agent")
    };
    let map = existence(&[candidate]);
    let resolved =
        resolve_cursor_binary(None, Some(&path_var(&[directory])), exists(&map)).unwrap();
    assert_eq!(resolved.path(), Path::new(candidate));
}

#[test]
fn missing_everything_reports_no_binary() {
    let map = existence(&[]);
    assert!(resolve_cursor_binary(None, Some(&path_var(&["C:\\Tools"])), exists(&map)).is_none());
    assert!(resolve_cursor_binary(None, None, exists(&map)).is_none());
    assert!(resolve_cursor_binary(None, Some("   "), exists(&map)).is_none());
    assert!(find_cursor_on_path(Some(&path_var(&["C:\\Tools"])), exists(&map)).is_none());
}

#[test]
fn version_parsing_matches_typescript_regex() {
    assert_eq!(
        parse_cursor_version("cursor-agent 2025.09.06-abc123"),
        Some("2025.09.06-abc123".to_owned())
    );
    assert_eq!(
        parse_cursor_version("2025.9.6-rc.1_extra-build ok"),
        Some("2025.9.6-rc.1_extra-build".to_owned())
    );
    assert_eq!(
        parse_cursor_version("prefix 2025.12.31-x.y_z-9! suffix"),
        Some("2025.12.31-x.y_z-9".to_owned())
    );
    // Leading word character defeats the `\b` anchor at that position.
    assert_eq!(
        parse_cursor_version("x2025.09.06-abc 2025.09.07-def"),
        Some("2025.09.07-def".to_owned())
    );
}

#[test]
fn malformed_versions_stay_unparseable() {
    for output in [
        "",
        "no version here",
        "cursor-agent 1.2.3",
        "2025.09.06",
        "2025.09.06-",
        "2025.9.123-abc",
        "2025.100.15-abc",
        "25.09.06-abc",
        "2025.09.06 abc",
        "20250.09.06-abc",
    ] {
        assert_eq!(parse_cursor_version(output), None, "input: {output}");
    }
}
