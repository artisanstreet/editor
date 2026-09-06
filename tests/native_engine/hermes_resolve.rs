//! Hermes executable precedence: override, installed, `PATH`, absent.
//!
//! Fixture inputs flow through [`resolve_hermes_executable_from_parts`], the
//! pure mirror of TypeScript `HermesExecutable`; no environment or filesystem
//! is touched here.

use std::path::PathBuf;

use artisan_native_engine::hermes::resolve::{
    HERMES_EXECUTABLE_ENV, HermesExecutableSource, installed_hermes_path,
    resolve_hermes_executable_from_parts,
};

fn fixture_path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

#[test]
fn explicit_override_wins_over_installed_and_path() {
    let resolved = resolve_hermes_executable_from_parts(
        Some("C:\\tools\\hermes.exe"),
        Some(fixture_path("C:\\installed\\hermes.exe")),
        Some(fixture_path("C:\\path\\hermes.exe")),
    )
    .expect("override resolves");
    assert_eq!(resolved.source(), HermesExecutableSource::ExplicitOverride);
    assert_eq!(
        resolved.path(),
        PathBuf::from("C:\\tools\\hermes.exe").as_path()
    );
}

#[test]
fn explicit_override_is_trimmed() {
    let resolved = resolve_hermes_executable_from_parts(
        Some("  C:\\tools\\hermes.exe  "),
        Some(fixture_path("C:\\installed\\hermes.exe")),
        Some(fixture_path("C:\\path\\hermes.exe")),
    )
    .expect("trimmed override resolves");
    assert_eq!(resolved.source(), HermesExecutableSource::ExplicitOverride);
    assert_eq!(
        resolved.path(),
        PathBuf::from("C:\\tools\\hermes.exe").as_path()
    );
}

#[test]
fn spaced_override_path_is_preserved_verbatim() {
    let spaced = "C:\\Program Files\\hermes\\hermes.exe";
    let resolved = resolve_hermes_executable_from_parts(
        Some(spaced),
        Some(fixture_path("C:\\installed\\hermes.exe")),
        Some(fixture_path("C:\\path\\hermes.exe")),
    )
    .expect("spaced override resolves");
    assert_eq!(resolved.source(), HermesExecutableSource::ExplicitOverride);
    assert_eq!(resolved.path(), PathBuf::from(spaced).as_path());
}

#[test]
fn whitespace_override_falls_through_to_installed() {
    let resolved = resolve_hermes_executable_from_parts(
        Some("   "),
        Some(fixture_path("C:\\installed\\hermes.exe")),
        Some(fixture_path("C:\\path\\hermes.exe")),
    )
    .expect("installed resolves");
    assert_eq!(
        resolved.source(),
        HermesExecutableSource::InstalledLocalAppData
    );
    assert_eq!(
        resolved.path(),
        fixture_path("C:\\installed\\hermes.exe").as_path()
    );
}

#[test]
fn env_missing_falls_through_to_installed() {
    let resolved = resolve_hermes_executable_from_parts(
        None,
        Some(fixture_path("C:\\installed\\hermes.exe")),
        Some(fixture_path("C:\\path\\hermes.exe")),
    )
    .expect("installed resolves");
    assert_eq!(
        resolved.source(),
        HermesExecutableSource::InstalledLocalAppData
    );
}

#[test]
fn path_lookup_is_last_resort() {
    let resolved = resolve_hermes_executable_from_parts(
        None,
        None,
        Some(fixture_path("C:\\path\\hermes.exe")),
    )
    .expect("path lookup resolves");
    assert_eq!(resolved.source(), HermesExecutableSource::PathLookup);
    assert_eq!(
        resolved.path(),
        fixture_path("C:\\path\\hermes.exe").as_path()
    );
}

#[test]
fn none_found_stays_absent() {
    assert_eq!(resolve_hermes_executable_from_parts(None, None, None), None);
    assert_eq!(
        resolve_hermes_executable_from_parts(Some(""), None, None),
        None
    );
}

#[test]
fn installed_path_joins_typescript_layout() {
    let joined = installed_hermes_path(PathBuf::from("C:\\Users\\suite\\AppData\\Local").as_path());
    assert_eq!(
        joined,
        fixture_path("C:\\Users\\suite\\AppData\\Local\\hermes\\hermes-agent\\bin\\hermes.exe")
    );
}

#[test]
fn override_env_name_matches_typescript() {
    assert_eq!(HERMES_EXECUTABLE_ENV, "HERMES_EXECUTABLE");
}

#[test]
fn source_spellings_are_stable() {
    assert_eq!(
        HermesExecutableSource::ExplicitOverride.as_str(),
        "explicit-override"
    );
    assert_eq!(
        HermesExecutableSource::InstalledLocalAppData.as_str(),
        "installed-local-appdata"
    );
    assert_eq!(HermesExecutableSource::PathLookup.as_str(), "path-lookup");
}
