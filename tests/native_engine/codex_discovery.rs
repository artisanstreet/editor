//! Codex executable discovery precedence fixtures.
//!
//! Registration (controller-owned, not part of this packet):
//! `tests/native_engine/BUILD.bazel` gains a `rust_test` target for
//! `codex_discovery.rs` depending on `//modules/native_engine:native_engine`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use artisan_native_engine::codex::{
    CodexDiscoveryInput, codex_winget_arch, is_windows_apps_path, resolve_codex_executable,
    resolve_codex_home,
};

fn input(
    local_app_data: Option<&str>,
    path_entries: &[&str],
    directory_names: &[&str],
) -> CodexDiscoveryInput {
    CodexDiscoveryInput {
        architecture: "x64".to_owned(),
        configured_executable: None,
        local_app_data: local_app_data.map(PathBuf::from),
        platform_windows: true,
        path_entries: path_entries.iter().map(PathBuf::from).collect(),
        directory_names: directory_names.iter().map(ToString::to_string).collect(),
    }
}

fn existence(existing: &[&str]) -> HashMap<PathBuf, bool> {
    existing
        .iter()
        .map(|path| (PathBuf::from(path), true))
        .collect()
}

fn exists(map: &HashMap<PathBuf, bool>) -> impl Fn(&Path) -> bool + '_ {
    move |path| map.get(path).copied().unwrap_or(false)
}

#[test]
fn configured_override_wins_with_trim_and_spaces() {
    let configured = "C:\\Program Files\\Custom Tools\\codex.exe";
    let mut discovery = input(
        Some("C:\\Users\\runner\\AppData\\Local"),
        &["C:\\Tools"],
        &[],
    );
    discovery.configured_executable = Some(format!("  {configured}  "));
    let map = existence(&["C:\\Users\\runner\\AppData\\Local\\OpenAI\\Codex\\bin\\codex.exe"]);
    assert_eq!(
        resolve_codex_executable(&discovery, &exists(&map)),
        PathBuf::from(configured)
    );
}

#[test]
fn non_windows_resolves_to_bare_command() {
    let mut discovery = input(None, &[], &[]);
    discovery.platform_windows = false;
    let map = existence(&[]);
    assert_eq!(
        resolve_codex_executable(&discovery, &exists(&map)),
        PathBuf::from("codex")
    );
}

#[test]
fn local_plain_binary_beats_versioned_winget_and_path() {
    let root = "C:\\Users\\runner\\AppData\\Local";
    let plain = format!("{root}\\OpenAI\\Codex\\bin\\codex.exe");
    let versioned = format!("{root}\\OpenAI\\Codex\\bin\\0.150.0\\codex.exe");
    let discovery = input(Some(root), &["C:\\Tools"], &["0.150.0"]);
    let map = existence(&[&plain, &versioned]);
    assert_eq!(
        resolve_codex_executable(&discovery, &exists(&map)),
        PathBuf::from(&plain)
    );
}

#[test]
fn versioned_directories_resolve_reverse_numeric_order() {
    let root = "C:\\Users\\runner\\AppData\\Local";
    let discovery = input(Some(root), &[], &["0.142.5", "0.9.10", "0.150.0"]);
    let newest = format!("{root}\\OpenAI\\Codex\\bin\\0.150.0\\codex.exe");
    let map = existence(&[&newest]);
    assert_eq!(
        resolve_codex_executable(&discovery, &exists(&map)),
        PathBuf::from(&newest)
    );
}

#[test]
fn winget_candidate_beats_path_and_uses_arch_mapping() {
    assert_eq!(codex_winget_arch("arm64"), "aarch64");
    assert_eq!(codex_winget_arch("x64"), "x86_64");
    assert_eq!(codex_winget_arch("aarch64"), "x86_64");

    let root = "C:\\Users\\runner\\AppData\\Local";
    let winget = format!(
        "{root}\\Microsoft\\WinGet\\Packages\\OpenAI.Codex_Microsoft.Winget.Source_8wekyb3d8bbwe\\codex-x86_64-pc-windows-msvc.exe"
    );
    let path_candidate = "C:\\Tools\\codex.exe";
    let discovery = input(Some(root), &["C:\\Tools"], &[]);
    let map = existence(&[&winget, path_candidate]);
    assert_eq!(
        resolve_codex_executable(&discovery, &exists(&map)),
        PathBuf::from(&winget)
    );
}

#[test]
fn windows_apps_alias_is_rejected_case_insensitively() {
    let root = "C:\\Users\\runner\\AppData\\Local";
    let alias = format!("{root}\\Microsoft\\WindowsApps");
    assert!(is_windows_apps_path(
        Path::new(&alias),
        Some(Path::new(root))
    ));
    assert!(is_windows_apps_path(
        Path::new(&alias.to_ascii_uppercase()),
        Some(Path::new(root))
    ));
    assert!(!is_windows_apps_path(
        Path::new("C:\\Tools"),
        Some(Path::new(root))
    ));
    assert!(!is_windows_apps_path(Path::new(&alias), None));

    let discovery = input(Some(root), &[&alias, "C:\\Tools"], &[]);
    let path_candidate = "C:\\Tools\\codex.exe";
    let map = existence(&[&format!("{alias}\\codex.exe"), path_candidate]);
    assert_eq!(
        resolve_codex_executable(&discovery, &exists(&map)),
        PathBuf::from(path_candidate)
    );
}

#[test]
fn paths_containing_spaces_resolve_without_quoting() {
    let root = "C:\\Users\\runner name\\AppData\\Local";
    let spaced = "D:\\My Tools\\Codex Bin";
    let candidate = format!("{spaced}\\codex.exe");
    let discovery = input(Some(root), &[spaced], &[]);
    let map = existence(&[&candidate]);
    assert_eq!(
        resolve_codex_executable(&discovery, &exists(&map)),
        PathBuf::from(&candidate)
    );
}

#[test]
fn fallback_is_local_root_when_nothing_exists() {
    let root = "C:\\Users\\runner\\AppData\\Local";
    let discovery = input(Some(root), &["C:\\Tools"], &["0.150.0"]);
    let map = existence(&[]);
    assert_eq!(
        resolve_codex_executable(&discovery, &exists(&map)),
        PathBuf::from(format!("{root}\\OpenAI\\Codex\\bin\\codex.exe"))
    );
}

#[test]
fn codex_home_prefers_override_then_user_profile() {
    assert_eq!(
        resolve_codex_home(Some("D:\\Codex Home"), Some("C:\\Users\\runner")),
        PathBuf::from("D:\\Codex Home")
    );
    assert_eq!(
        resolve_codex_home(None, Some("C:\\Users\\runner")),
        PathBuf::from("C:\\Users\\runner\\.codex")
    );
    assert_eq!(
        resolve_codex_home(Some("   "), Some("C:\\Users\\runner")),
        PathBuf::from("C:\\Users\\runner\\.codex")
    );
}
