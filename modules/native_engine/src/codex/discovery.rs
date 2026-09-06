//! Finite native Codex executable discovery.
//!
//! Pure Rust port of the precedence in
//! `modules/engines/src/codex/executable.ts` (`resolve_codex_executable`):
//! explicit `ARTISAN_CODEX_EXECUTABLE` override, then on Windows the
//! `<LOCALAPPDATA>/OpenAI/Codex/bin` scan (plain binary first, then
//! versioned subdirectories in reverse numeric order), then the WinGet
//! package path, then `PATH` entries with the Windows App Execution Alias
//! directory removed. Non-Windows hosts resolve to `codex` unless an
//! explicit override is set. No filesystem or process work happens here;
//! callers supply directory listings and an existence predicate so the
//! precedence stays testable with fixture inputs.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// Environment variable selecting an explicit Codex executable override.
pub const CODEX_EXECUTABLE_OVERRIDE_ENV: &str = "ARTISAN_CODEX_EXECUTABLE";

/// Fallback command used on non-Windows hosts and when nothing is installed.
pub const CODEX_FALLBACK_COMMAND: &str = "codex";

/// WinGet package directory suffix used by the TypeScript resolver.
pub const CODEX_WINGET_PACKAGE_DIR: &str = "OpenAI.Codex_Microsoft.Winget.Source_8wekyb3d8bbwe";

/// Input for the pure Codex executable resolver.
///
/// `path_entries` must already be split on the host path delimiter in order;
/// empty entries are ignored and alias entries are filtered during
/// resolution. `directory_names` are the raw child names of the local Codex
/// bin root; they are sorted numerically by the resolver.
#[derive(Clone, Debug)]
pub struct CodexDiscoveryInput {
    /// Host architecture string (for example `x64` or `arm64`).
    pub architecture: String,
    /// Trimmed `ARTISAN_CODEX_EXECUTABLE` value when configured.
    pub configured_executable: Option<String>,
    /// Trimmed `LOCALAPPDATA` (or caller fallback) on Windows.
    pub local_app_data: Option<PathBuf>,
    /// Whether the host is Windows. Non-Windows resolves to `codex`.
    pub platform_windows: bool,
    /// Ordered `PATH` directories to consider after local candidates.
    pub path_entries: Vec<PathBuf>,
    /// Raw child directory names of the local Codex bin root.
    pub directory_names: Vec<String>,
}

impl CodexDiscoveryInput {
    /// Builds the local Codex bin root for the configured app-data dir.
    #[must_use]
    pub fn local_codex_root(&self) -> PathBuf {
        codex_local_root(self.local_app_data.as_deref())
    }

    /// Builds the WinGet executable candidate for the configured app-data dir.
    #[must_use]
    pub fn winget_candidate(&self) -> Option<PathBuf> {
        codex_winget_executable(self.local_app_data.as_deref(), &self.architecture)
    }
}

/// Returns the local Codex bin root for an optional app-data directory.
///
/// Mirrors `join(local_app_data ?? "", "OpenAI", "Codex", "bin")`: a missing
/// app-data dir yields the same relative fallback the TypeScript resolver
/// uses as its final default.
#[must_use]
pub fn codex_local_root(local_app_data: Option<&Path>) -> PathBuf {
    match local_app_data {
        Some(root) => root.join("OpenAI").join("Codex").join("bin"),
        None => PathBuf::from("OpenAI").join("Codex").join("bin"),
    }
}

/// Maps the host architecture to the WinGet binary name.
///
/// Mirrors the TypeScript resolver exactly: only `arm64` selects the
/// `aarch64` binary; every other value (including `aarch64` itself) selects
/// `x86_64`.
#[must_use]
pub fn codex_winget_arch(architecture: &str) -> &'static str {
    if architecture == "arm64" {
        "aarch64"
    } else {
        "x86_64"
    }
}

/// Builds the WinGet executable candidate for an app-data directory.
#[must_use]
pub fn codex_winget_executable(
    local_app_data: Option<&Path>,
    architecture: &str,
) -> Option<PathBuf> {
    let root = local_app_data?;
    Some(
        root.join("Microsoft")
            .join("WinGet")
            .join("Packages")
            .join(CODEX_WINGET_PACKAGE_DIR)
            .join(format!(
                "codex-{}-pc-windows-msvc.exe",
                codex_winget_arch(architecture)
            )),
    )
}

/// Returns the default executable when no installed candidate exists.
#[must_use]
pub fn codex_fallback_executable(local_app_data: Option<&Path>) -> PathBuf {
    codex_local_root(local_app_data).join("codex.exe")
}

/// Reports whether a `PATH` directory is the Windows App Execution Alias dir.
///
/// The comparison is ASCII case-insensitive, matching the TypeScript
/// `toLocaleLowerCase` prefix check. A missing app-data dir never matches.
#[must_use]
pub fn is_windows_apps_path(dir: &Path, local_app_data: Option<&Path>) -> bool {
    let Some(root) = local_app_data else {
        return false;
    };
    let alias = root.join("Microsoft").join("WindowsApps");
    let dir_text = dir.as_os_str().to_string_lossy();
    let alias_text = alias.as_os_str().to_string_lossy();
    if alias_text.is_empty() {
        return false;
    }
    let dir_lower = dir_text.to_ascii_lowercase();
    let alias_lower = alias_text.to_ascii_lowercase();
    dir_lower.starts_with(&alias_lower)
}

/// Compares two Codex bin child names with numeric-aware ordering.
///
/// Digit runs compare by numeric value (leading zeros ignored); other runs
/// compare case-insensitively. This mirrors `localeCompare` with
/// `{ numeric: true, sensitivity: "base" }` for the version-like directory
/// names produced by the Codex installer.
#[must_use]
pub fn compare_codex_directory_names(left: &str, right: &str) -> Ordering {
    let mut left_chunks = ChunkIter::new(left);
    let mut right_chunks = ChunkIter::new(right);
    loop {
        match (left_chunks.next(), right_chunks.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left), Some(right)) => {
                let order = compare_chunks(&left, &right);
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// Sorts Codex bin child names in ascending numeric-aware order.
pub fn sort_codex_directory_names(names: &mut [String]) {
    names.sort_by(|left, right| compare_codex_directory_names(left, right));
}

/// Resolves the Codex executable with TypeScript precedence.
///
/// Order: explicit override, then (Windows only) local `codex.exe`, local
/// versioned `*/codex.exe` in reverse numeric order, WinGet binary, eligible
/// `PATH` `codex.exe` entries; otherwise the local fallback. `PATH` entries
/// that are empty or inside the Windows App Execution Alias directory are
/// skipped. `PathBuf` joins preserve paths containing spaces without
/// quoting.
#[must_use]
pub fn resolve_codex_executable(
    input: &CodexDiscoveryInput,
    exists: &dyn Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(configured) = input.configured_executable.as_ref() {
        let trimmed = configured.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    if !input.platform_windows {
        return PathBuf::from(CODEX_FALLBACK_COMMAND);
    }

    let local_app_data = input.local_app_data.as_deref();
    let root = codex_local_root(local_app_data);
    let mut candidates = Vec::new();
    candidates.push(root.join("codex.exe"));
    let mut directories = input.directory_names.clone();
    sort_codex_directory_names(&mut directories);
    for directory in directories.iter().rev() {
        candidates.push(root.join(directory).join("codex.exe"));
    }
    if let Some(winget) = codex_winget_executable(local_app_data, &input.architecture) {
        candidates.push(winget);
    }
    for entry in &input.path_entries {
        if entry.as_os_str().is_empty() {
            continue;
        }
        if is_windows_apps_path(entry, local_app_data) {
            continue;
        }
        candidates.push(entry.join("codex.exe"));
    }

    candidates
        .into_iter()
        .find(|candidate| exists(candidate))
        .unwrap_or_else(|| codex_fallback_executable(local_app_data))
}

/// Resolves the `CODEX_HOME` directory for a Codex child process.
///
/// A non-empty override is used as-is; otherwise the home is
/// `<user-profile>/.codex`. Mirrors `make_codex_process_environment` without
/// copying the ambient environment. The function is total over fixture
/// inputs and never fails.
#[must_use]
pub fn resolve_codex_home(configured_home: Option<&str>, user_profile: Option<&str>) -> PathBuf {
    if let Some(home) = configured_home {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    match user_profile
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(profile) => Path::new(profile).join(".codex"),
        None => PathBuf::from(".codex"),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Chunk {
    Digits(String),
    Other(String),
}

struct ChunkIter<'a> {
    text: &'a str,
    byte_index: usize,
}

impl<'a> ChunkIter<'a> {
    const fn new(text: &'a str) -> Self {
        Self {
            text,
            byte_index: 0,
        }
    }
}

impl Iterator for ChunkIter<'_> {
    type Item = Chunk;

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.text.as_bytes();
        if self.byte_index >= bytes.len() {
            return None;
        }
        let is_digit = bytes[self.byte_index].is_ascii_digit();
        let start = self.byte_index;
        while self.byte_index < bytes.len() && bytes[self.byte_index].is_ascii_digit() == is_digit {
            self.byte_index += 1;
        }
        let chunk = &self.text[start..self.byte_index];
        if is_digit {
            let stripped = chunk.trim_start_matches('0');
            Some(Chunk::Digits(stripped.to_owned()))
        } else {
            Some(Chunk::Other(chunk.to_ascii_lowercase()))
        }
    }
}

fn compare_chunks(left: &Chunk, right: &Chunk) -> Ordering {
    match (left, right) {
        (Chunk::Digits(left_value), Chunk::Digits(right_value)) => left_value
            .len()
            .cmp(&right_value.len())
            .then_with(|| left_value.cmp(right_value)),
        (Chunk::Digits { .. }, Chunk::Other(_)) => Ordering::Less,
        (Chunk::Other(_), Chunk::Digits { .. }) => Ordering::Greater,
        (Chunk::Other(left), Chunk::Other(right)) => left.cmp(right),
    }
}
