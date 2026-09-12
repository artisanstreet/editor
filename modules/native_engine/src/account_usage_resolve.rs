//! Installed provider-CLI resolution with Windows shim support.
//!
//! Mirrors `resolve_codex_executable` in
//! `modules/engines/src/codex/executable.ts` (configured override, local
//! installs, winget, then `PATH`, never the `WindowsApps` execution alias)
//! and extends it to the installed reality on this host: `codex` and
//! `claude` resolve to pnpm/npm shims (`codex.ps1`, `claude.ps1`,
//! `*.cmd`), which `CreateProcess` cannot run directly. Resolution
//! therefore returns a launch (program plus interpreter prefix) instead of
//! a bare path: `.exe` runs directly, `.cmd`/`.bat` run through
//! `cmd /D /C`, and `.ps1` runs through PowerShell with an explicit
//! bypass limited to that one invocation. No shim is ever executed to
//! probe it; existence is checked with a bounded metadata read and the
//! first launch attempt is the real non-billable usage read.

use std::path::{Path, PathBuf};

/// Resolved provider CLI launch: program plus interpreter prefix.
///
/// The full spawn argv is `program + prefix_args + caller args`, so a
/// `.cmd` shim becomes `cmd /D /C <shim> <args>` transparently.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliLaunch {
    /// Program to spawn (tool, shim interpreter, or configured override).
    pub program: PathBuf,
    /// Interpreter prefix placed before caller arguments.
    pub prefix_args: Vec<String>,
}

impl CliLaunch {
    /// Returns a direct launch of one program with no prefix.
    #[must_use]
    pub fn direct(program: PathBuf) -> Self {
        Self {
            program,
            prefix_args: Vec::new(),
        }
    }
}

/// Inputs for CLI resolution, explicit for fixture control.
#[derive(Clone, Debug)]
pub struct CliResolveInput {
    /// Tool stem, e.g. `"codex"` or `"claude"`.
    pub tool: &'static str,
    /// Environment override variable, e.g. `"ARTISAN_CODEX_EXECUTABLE"`.
    /// (`ARTISAN_CLAUDE_EXECUTABLE` is the symmetric extension; the
    /// TypeScript reference only defines the Codex override.)
    pub override_var: &'static str,
    /// Override value when set and non-blank.
    pub configured: Option<String>,
    /// `%LOCALAPPDATA%`-style root; `None` outside Windows.
    pub local_app_data: Option<PathBuf>,
    /// Split `PATH` directories in order.
    pub path_dirs: Vec<PathBuf>,
    /// `std::env::consts::ARCH`-style architecture for winget matching.
    pub arch: &'static str,
}

/// Resolves one provider CLI from explicit inputs.
///
/// Order: configured override, local installs, winget (Codex only),
/// `PATH` direct executables and shims (never `WindowsApps`), then a bare
/// tool-name fallback resolved at spawn time.
#[must_use]
pub fn resolve_cli_with(input: &CliResolveInput) -> CliLaunch {
    if let Some(configured) = input
        .configured
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return launch_for(PathBuf::from(configured));
    }
    let windows = input.local_app_data.is_some();
    if !windows {
        return CliLaunch::direct(PathBuf::from(input.tool));
    }
    let local_app_data = input.local_app_data.clone().unwrap_or_default();
    if input.tool == "codex" {
        let bin = local_app_data.join("OpenAI").join("Codex").join("bin");
        let mut versioned: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&bin) {
            let mut names: Vec<String> = entries
                .filter_map(std::result::Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect();
            names.sort();
            names.reverse();
            versioned = names
                .into_iter()
                .map(|name| bin.join(name).join("codex.exe"))
                .collect();
        }
        for candidate in [bin.join("codex.exe")]
            .into_iter()
            .chain(versioned)
            .chain([winget_codex(&local_app_data, input.arch)])
        {
            if is_regular_file(&candidate) {
                return launch_for(candidate);
            }
        }
    }
    for directory in &input.path_dirs {
        if directory.as_os_str().is_empty() || is_windows_apps(directory, &local_app_data) {
            continue;
        }
        for file_name in [
            format!("{}.exe", input.tool),
            format!("{}.cmd", input.tool),
            format!("{}.bat", input.tool),
            format!("{}.ps1", input.tool),
        ] {
            let candidate = directory.join(file_name);
            if is_regular_file(&candidate) {
                return launch_for(candidate);
            }
        }
    }
    CliLaunch::direct(PathBuf::from(input.tool))
}

fn winget_codex(local_app_data: &Path, arch: &str) -> PathBuf {
    let binary = if arch == "aarch64" {
        "codex-aarch64-pc-windows-msvc.exe"
    } else {
        "codex-x86_64-pc-windows-msvc.exe"
    };
    local_app_data
        .join("Microsoft")
        .join("WinGet")
        .join("Packages")
        .join("OpenAI.Codex_Microsoft.Winget.Source_8wekyb3d8bbwe")
        .join(binary)
}

fn is_windows_apps(directory: &Path, local_app_data: &Path) -> bool {
    let marker = local_app_data.join("Microsoft").join("WindowsApps");
    directory.to_str().is_some_and(|directory| {
        marker.to_str().is_some_and(|marker| {
            directory.len() >= marker.len()
                && directory[..marker.len()].eq_ignore_ascii_case(marker)
        })
    })
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// Wraps one resolved path with its interpreter prefix by extension.
fn launch_for(path: PathBuf) -> CliLaunch {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_lowercase();
    match extension.as_str() {
        "cmd" | "bat" => CliLaunch {
            program: PathBuf::from("cmd"),
            prefix_args: vec![
                "/D".to_owned(),
                "/C".to_owned(),
                path.to_string_lossy().into_owned(),
            ],
        },
        "ps1" => CliLaunch {
            program: PathBuf::from("powershell"),
            prefix_args: vec![
                "-NoProfile".to_owned(),
                "-NonInteractive".to_owned(),
                "-ExecutionPolicy".to_owned(),
                "Bypass".to_owned(),
                "-File".to_owned(),
                path.to_string_lossy().into_owned(),
            ],
        },
        _ => CliLaunch::direct(path),
    }
}

/// Collects the live host inputs for CLI resolution.
fn live_input(tool: &'static str, override_var: &'static str) -> CliResolveInput {
    let configured = std::env::var(override_var)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    #[cfg(windows)]
    let local_app_data: Option<PathBuf> = Some(
        std::env::var("LOCALAPPDATA")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map_or_else(
                || {
                    std::env::var("USERPROFILE")
                        .map_or_else(|_| PathBuf::from("C:/"), PathBuf::from)
                        .join("AppData")
                        .join("Local")
                },
                PathBuf::from,
            ),
    );
    #[cfg(not(windows))]
    let local_app_data: Option<PathBuf> = None;
    let path_dirs = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default();
    CliResolveInput {
        tool,
        override_var,
        configured,
        local_app_data,
        path_dirs,
        arch: std::env::consts::ARCH,
    }
}

/// Resolves the installed Codex CLI following the repository convention.
#[must_use]
pub fn resolve_codex_cli() -> CliLaunch {
    resolve_cli_with(&live_input("codex", "ARTISAN_CODEX_EXECUTABLE"))
}

/// Resolves the installed Claude CLI following the repository convention.
#[must_use]
pub fn resolve_claude_cli() -> CliLaunch {
    resolve_cli_with(&live_input("claude", "ARTISAN_CLAUDE_EXECUTABLE"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "artisan-cli-resolve-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("fixture root should be created");
        root
    }

    fn write_file(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture parent should be created");
        }
        std::fs::write(path, b"fixture").expect("fixture file should write");
    }

    fn windows_input(tool: &'static str, root: &Path, path_dirs: Vec<PathBuf>) -> CliResolveInput {
        CliResolveInput {
            tool,
            override_var: "ARTISAN_TEST_OVERRIDE",
            configured: None,
            local_app_data: Some(root.to_path_buf()),
            path_dirs,
            arch: "x86_64",
        }
    }

    #[test]
    fn configured_override_wins_with_shim_prefix() {
        let root = temp_root("override");
        let shim = root.join("tools").join("codex.ps1");
        write_file(&shim);
        let mut input = windows_input("codex", &root, Vec::new());
        input.configured = Some(shim.to_string_lossy().into_owned());
        let launch = resolve_cli_with(&input);
        assert_eq!(launch.program, PathBuf::from("powershell"));
        assert!(
            launch
                .prefix_args
                .contains(&shim.to_string_lossy().into_owned())
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn path_shims_resolve_with_extension_priority_and_alias_exclusion() {
        let root = temp_root("shims");
        let first = root.join("first");
        let second = root.join("second");
        // A bare .ps1 loses to a .cmd in the same directory; a later
        // directory never beats an earlier one.
        write_file(&second.join("codex.cmd"));
        write_file(&first.join("codex.ps1"));
        write_file(&first.join("codex.cmd"));
        let apps = root.join("Microsoft").join("WindowsApps");
        write_file(&apps.join("codex.exe"));
        let input = windows_input("codex", &root, vec![first.clone(), apps, second.clone()]);
        let launch = resolve_cli_with(&input);
        assert_eq!(launch.program, PathBuf::from("cmd"));
        assert!(
            launch
                .prefix_args
                .contains(&first.join("codex.cmd").to_string_lossy().into_owned())
        );

        // Without the .cmd, the same directory's .ps1 wins with PowerShell.
        std::fs::remove_file(first.join("codex.cmd")).expect("fixture removal");
        let launch = resolve_cli_with(&input);
        assert_eq!(launch.program, PathBuf::from("powershell"));

        // A direct .exe beats every shim in the same directory.
        write_file(&second.join("codex.exe"));
        let input = windows_input("codex", &root, vec![second.clone()]);
        let launch = resolve_cli_with(&input);
        assert_eq!(launch.program, second.join("codex.exe"));
        assert!(launch.prefix_args.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn local_and_winget_installs_win_over_path_and_fallback_is_bare() {
        let root = temp_root("local");
        let bin = root.join("OpenAI").join("Codex").join("bin");
        write_file(&bin.join("codex.exe"));
        let elsewhere = root.join("elsewhere");
        write_file(&elsewhere.join("codex.cmd"));
        let input = windows_input("codex", &root, vec![elsewhere]);
        let launch = resolve_cli_with(&input);
        assert_eq!(launch.program, bin.join("codex.exe"));

        let empty = temp_root("empty");
        let input = windows_input("claude", &empty, Vec::new());
        let launch = resolve_cli_with(&input);
        assert_eq!(launch, CliLaunch::direct(PathBuf::from("claude")));
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&empty).ok();
    }
}
