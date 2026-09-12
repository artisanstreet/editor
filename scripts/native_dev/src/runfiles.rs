//! Build-output discovery for the four payload binaries.
//!
//! The launcher finds Bazel-built binaries without a wrapper script:
//! explicit `--bin-dir` first, then the Bazel runfiles directory (Bzlmod
//! `_main`, legacy workspace, and flat layouts), then the runfiles manifest
//! with the same prefixes, then the `bazel-bin` sibling layout relative to
//! the current executable.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::{error::DevError, paths::exe_name};

/// Runfiles directory marker present under `bazel run`.
pub const RUNFILES_DIR_ENV: &str = "RUNFILES_DIR";

/// Runfiles manifest marker present under `bazel run` on Windows.
pub const RUNFILES_MANIFEST_ENV: &str = "RUNFILES_MANIFEST_FILE";

/// Located build outputs for the four payload binaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinarySet {
    /// Staged `ae` build output.
    pub ae: PathBuf,
    /// Staged `editor` build output.
    pub editor: PathBuf,
    /// Staged `forge` build output.
    pub forge: PathBuf,
    /// Staged `installer` build output.
    pub installer: PathBuf,
}

impl BinarySet {
    /// Iterates `(payload-relative-name, source-path)` pairs.
    #[must_use]
    pub fn entries(&self) -> [(String, PathBuf); 4] {
        [
            (format!("bin/{}", exe_name("ae")), self.ae.clone()),
            (
                format!("bin/{}", exe_name("installer")),
                self.installer.clone(),
            ),
            (format!("bin/{}", exe_name("editor")), self.editor.clone()),
            (format!("bin/{}", exe_name("forge")), self.forge.clone()),
        ]
    }
}

/// Runfile-relative locations of the four binaries inside the workspace.
fn runfile_names() -> [(&'static str, &'static str); 4] {
    [
        ("ae", "modules/cli/ae"),
        ("installer", "modules/installer/installer"),
        ("editor", "modules/frontend/editor"),
        ("forge", "modules/backend/forge"),
    ]
}

/// Repository prefixes tried for one runfile, in search order.
///
/// Bzlmod publishes the main repository under `_main`; older layouts use
/// the workspace name or no prefix at all.
fn runfile_prefixes(runfile: &str) -> [String; 3] {
    [
        format!("_main/{runfile}"),
        format!("artisan_editor/{runfile}"),
        runfile.to_owned(),
    ]
}

/// Finds one runfile inside runfiles manifest text.
///
/// The manifest format is `<runfile> <local-path>` per line; the first
/// ASCII space separates the name from the path, so Windows paths with
/// spaces survive. The name must match exactly — prefix impostors never
/// match.
#[must_use]
pub fn find_in_manifest(manifest: &str, runfile: &str) -> Option<PathBuf> {
    manifest.lines().find_map(|line| {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let (name, path) = line.split_once(' ')?;
        if name.trim() == runfile {
            let trimmed = path.trim();
            (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
        } else {
            None
        }
    })
}

/// Finds one runfile trying every repository prefix.
///
/// Manifest keys carry the repository prefix (`_main/...` under Bzlmod),
/// so each prefix is probed with and without the platform binary suffix.
#[must_use]
pub fn find_prefixed_in_manifest(manifest: &str, runfile: &str) -> Option<PathBuf> {
    let suffixed = if cfg!(windows) {
        format!("{runfile}.exe")
    } else {
        runfile.to_owned()
    };
    for prefix in runfile_prefixes(&suffixed) {
        if let Some(path) = find_in_manifest(manifest, &prefix) {
            return Some(path);
        }
    }
    for prefix in runfile_prefixes(runfile) {
        if let Some(path) = find_in_manifest(manifest, &prefix) {
            return Some(path);
        }
    }
    None
}

/// Candidate runfiles locations for one binary, in search order.
#[must_use]
pub fn runfiles_candidates(runfile: &str, runfiles_dir: &str) -> Vec<PathBuf> {
    let suffixed = if cfg!(windows) {
        format!("{runfile}.exe")
    } else {
        runfile.to_owned()
    };
    let base = PathBuf::from(runfiles_dir);
    vec![
        base.join("_main").join(&suffixed),
        base.join("artisan_editor").join(&suffixed),
        base.join(&suffixed),
    ]
}

/// Locates the four build outputs.
///
/// # Errors
///
/// Returns [`DevError::BinaryMissing`] naming every binary that could not
/// be located.
pub fn locate_binaries(explicit: Option<&Path>) -> Result<BinarySet, DevError> {
    if let Some(directory) = explicit {
        return locate_in_dir(directory);
    }
    let mut found: BTreeMap<&str, PathBuf> = BTreeMap::new();
    if let Some(runfiles_dir) = std::env::var_os(RUNFILES_DIR_ENV) {
        let runfiles_dir = runfiles_dir.to_string_lossy().into_owned();
        for (stem, runfile) in runfile_names() {
            if found.contains_key(stem) {
                continue;
            }
            for candidate in runfiles_candidates(runfile, &runfiles_dir) {
                if candidate.is_file() {
                    found.insert(stem, candidate);
                    break;
                }
            }
        }
    }
    if found.len() < 4
        && let Some(manifest_path) = std::env::var_os(RUNFILES_MANIFEST_ENV)
        && let Ok(manifest) = std::fs::read_to_string(&manifest_path)
    {
        for (stem, runfile) in runfile_names() {
            if found.contains_key(stem) {
                continue;
            }
            if let Some(path) =
                find_prefixed_in_manifest(&manifest, runfile).filter(|path| path.is_file())
            {
                found.insert(stem, path);
            }
        }
    }
    if found.len() < 4
        && let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        for (stem, runfile) in runfile_names() {
            if found.contains_key(stem) {
                continue;
            }
            let candidate = exe_dir
                .join("../../")
                .join(runfile)
                .with_extension(exe_extension());
            if candidate.is_file() {
                found.insert(stem, candidate);
            }
        }
    }
    let mut missing = Vec::new();
    for (stem, _) in runfile_names() {
        if !found.contains_key(stem) {
            missing.push(exe_name(stem));
        }
    }
    if !missing.is_empty() {
        return Err(DevError::BinaryMissing {
            name: missing.join(", "),
            hint: "expected bazel run runfiles or --bin-dir".to_owned(),
        });
    }
    let mut take = |stem: &str| {
        found
            .remove(stem)
            .unwrap_or_else(|| PathBuf::from(exe_name(stem)))
    };
    Ok(BinarySet {
        ae: take("ae"),
        editor: take("editor"),
        forge: take("forge"),
        installer: take("installer"),
    })
}

/// Extension used for sibling-layout probing.
fn exe_extension() -> &'static str {
    if cfg!(windows) { "exe" } else { "" }
}

/// Locates the four binaries inside one explicit directory.
///
/// # Errors
///
/// Returns [`DevError::BinaryMissing`] when any binary is absent.
pub fn locate_in_dir(directory: &Path) -> Result<BinarySet, DevError> {
    let mut missing = Vec::new();
    let mut get = |stem: &str| {
        let path = directory.join(exe_name(stem));
        if path.is_file() {
            Some(path)
        } else {
            missing.push(exe_name(stem));
            None
        }
    };
    let set = BinarySet {
        ae: get("ae").unwrap_or_default(),
        editor: get("editor").unwrap_or_default(),
        forge: get("forge").unwrap_or_default(),
        installer: get("installer").unwrap_or_default(),
    };
    if missing.is_empty() {
        Ok(set)
    } else {
        Err(DevError::BinaryMissing {
            name: missing.join(", "),
            hint: format!("--bin-dir {}", directory.display()),
        })
    }
}
