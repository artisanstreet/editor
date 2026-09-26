//! Cursor resolution and version-parsing fixtures. The executable is only
//! ever the Forge-managed generation or the absolute developer override;
//! `PATH` is never searched.

use artisan_native_engine::cursor::{CURSOR_EXECUTABLE_ENV, parse_cursor_version, resolve_live};

#[test]
fn without_a_managed_install_cursor_is_not_resolved_from_path() {
    assert_eq!(CURSOR_EXECUTABLE_ENV, "ARTISAN_CURSOR_EXECUTABLE");
    if std::env::var_os(CURSOR_EXECUTABLE_ENV).is_none() {
        assert!(resolve_live().is_none());
    }
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
