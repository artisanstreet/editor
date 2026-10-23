//! Moves earlier layouts into Artisan Street, once: Forge's data and the
//! Editor's settings from where earlier `ae` versions and the old dev loop
//! kept them, then removes the old programs. Runs before an install of
//! Forge or the Editor on this machine, for those apps; it does nothing when
//! there is nothing old left.

use crate::{platform, service, sys, ui};
use anyhow::{Context, Result};
use artisan_paths::App;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub fn migrate(apps: &[App]) -> Result<()> {
    let forge = apps.contains(&App::Forge);
    let editor = apps.contains(&App::Editor);
    if !forge && !editor {
        return Ok(());
    }
    let old = Old::here()?;
    let forge_data = App::Forge.data()?;
    let editor_data = App::Editor.data()?;
    let move_forge = forge && !forge_data.join("forge.sqlite3").exists();
    let move_editor = editor && !editor_data.exists();

    let forge_from = newest(&old.forge_data, |dir| mtime(&dir.join("forge.sqlite3")));
    let editor_from = newest(&old.editor_data, |dir| {
        ["settings.json", "hosts.json", "identity.json"]
            .iter()
            .filter_map(|name| mtime(&dir.join(name)))
            .max()
    });
    let anything = (move_forge && forge_from.is_some())
        || (move_editor && editor_from.is_some())
        || old.programs.iter().any(|path| path.exists());
    if !anything {
        return Ok(());
    }

    let progress = ui::Progress::spinner("Moving the earlier install into Artisan Street");
    let result = (|| -> Result<Vec<String>> {
        let mut notes = Vec::new();
        stop_old_forge(&old)?;
        platform::close_processes(&old.executables())?;
        if move_forge && let Some(from) = &forge_from {
            move_dir(from, &forge_data)?;
            for name in ["forge.pid", "forge.lock"] {
                std::fs::remove_file(forge_data.join(name)).ok();
            }
            retarget_codex_rollouts(from, &forge_data)?;
            notes.push(format!("Forge's data from {}", from.display()));
        }
        if move_editor && let Some(from) = &editor_from {
            move_dir(from, &editor_data)?;
            // The old dev Forge listened on 47901; there is one Forge now.
            let hosts = editor_data.join("hosts.json");
            if let Ok(text) = std::fs::read_to_string(&hosts) {
                let text = text
                    .replace("127.0.0.1:47901", "127.0.0.1:47900")
                    .replace("localhost:47901", "localhost:47900");
                sys::write(&hosts, &text)?;
            }
            notes.push(format!("the Editor's settings from {}", from.display()));
        }
        // Older copies than the one that moved.
        if editor {
            for dir in &old.editor_data {
                sys::remove(dir).ok();
            }
        }
        for path in &old.programs {
            sys::remove(path)?;
        }
        for path in &old.path_entries {
            platform::remove_path_entry(path)?;
        }
        for dir in &old.parents {
            // Only once nothing (such as unmoved Forge data) is left.
            std::fs::remove_dir(dir).ok();
        }
        Ok(notes)
    })();
    match result {
        Ok(notes) if notes.is_empty() => progress.done("Removed the earlier install"),
        Ok(notes) => progress.done(format!(
            "Moved {} and removed the earlier install",
            notes.join(" and ")
        )),
        Err(error) => {
            progress.fail(format!("{error:#}"));
            return Err(error);
        }
    }
    Ok(())
}

/// Where earlier versions kept things on this OS.
struct Old {
    forge_data: Vec<PathBuf>,
    editor_data: Vec<PathBuf>,
    /// Removed outright.
    programs: Vec<PathBuf>,
    /// Removed when empty.
    parents: Vec<PathBuf>,
    /// User `Path` entries (Windows).
    path_entries: Vec<PathBuf>,
    /// Old `ae` binaries; a Forge running from one is stopped.
    ae: Vec<PathBuf>,
}

impl Old {
    fn here() -> Result<Self> {
        if cfg!(windows) {
            let local =
                PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?);
            let roaming = PathBuf::from(std::env::var_os("APPDATA").context("APPDATA is not set")?);
            let release = local.join("Artisan");
            let dev = local.join("Artisan Dev");
            Ok(Self {
                forge_data: vec![release.join("forge")],
                editor_data: vec![roaming.join("Artisan").join("editor"), dev.join("editor")],
                programs: vec![
                    release.join("versions"),
                    release.join("bin"),
                    release.join("current"),
                    release.join("previous"),
                    dev.join("artisan-editor.exe"),
                    dev.join("launch.cmd"),
                    // Before the Editor was `Artisan Editor.exe`.
                    App::Editor.dir()?.join("artisan-editor.exe"),
                    artisan_paths::home()?
                        .join("Desktop")
                        .join("Artisan Editor.lnk"),
                ],
                parents: vec![release, dev, roaming.join("Artisan")],
                path_entries: vec![local.join("Artisan").join("bin")],
                ae: vec![local.join("Artisan").join("bin").join("ae.exe")],
            })
        } else {
            let data = artisan_paths::xdg_data()?;
            let config = artisan_paths::xdg_config()?;
            let release = data.join("artisan");
            let dev = data.join("artisan-dev");
            Ok(Self {
                forge_data: vec![release.join("forge"), dev.join("forge")],
                editor_data: vec![config.join("Artisan").join("editor"), dev.join("editor")],
                programs: vec![
                    release.join("versions"),
                    release.join("bin"),
                    release.join("current"),
                    release.join("previous"),
                    dev.join("bin"),
                    dev.join("wsl-env.sh"),
                    dev.join("editor.log"),
                ],
                parents: vec![release, dev, config.join("Artisan")],
                path_entries: Vec::new(),
                ae: vec![data.join("artisan/bin/ae"), data.join("artisan-dev/bin/ae")],
            })
        }
    }

    /// Old binaries whose processes must stop before their files go.
    fn executables(&self) -> Vec<PathBuf> {
        let mut executables = self.ae.clone();
        if cfg!(windows)
            && let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        {
            executables.push(local.join("Artisan Dev").join("artisan-editor.exe"));
            if let Ok(dir) = App::Editor.dir() {
                executables.push(dir.join("artisan-editor.exe"));
            }
            if let Ok(versions) = std::fs::read_dir(local.join("Artisan").join("versions")) {
                for version in versions.flatten() {
                    executables.push(version.path().join("artisan-editor.exe"));
                    executables.push(version.path().join("ae.exe"));
                }
            }
        } else if let Ok(data) = artisan_paths::xdg_data() {
            executables.push(data.join("artisan-dev/bin/artisan-editor"));
            if let Ok(versions) = std::fs::read_dir(data.join("artisan/versions")) {
                for version in versions.flatten() {
                    executables.push(version.path().join("artisan-editor"));
                    executables.push(version.path().join("ae"));
                }
            }
        }
        executables
    }
}

/// Stops a Forge an earlier `ae` runs: its systemd unit (`ae forge serve`),
/// or the detached process in its pid file.
fn stop_old_forge(old: &Old) -> Result<()> {
    if cfg!(target_os = "linux")
        && let Ok(unit) = service::unit_path()
        && let Ok(text) = std::fs::read_to_string(&unit)
        && text.contains("forge serve")
    {
        sys::run("systemctl", &["--user", "disable", "--now", service::UNIT]).ok();
        sys::remove(&unit)?;
        sys::run("systemctl", &["--user", "daemon-reload"]).ok();
    }
    for dir in &old.forge_data {
        let Some(pid) = std::fs::read_to_string(dir.join("forge.pid"))
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
        else {
            continue;
        };
        if cfg!(windows) {
            sys::run("taskkill.exe", &["/PID", &pid.to_string(), "/T", "/F"]).ok();
        } else {
            sys::run("kill", &["-TERM", &pid.to_string()]).ok();
        }
    }
    // Old Forges run as `ae forge serve` from an old `ae`.
    platform::close_processes(&old.ae)?;
    Ok(())
}

fn newest(dirs: &[PathBuf], time: impl Fn(&Path) -> Option<SystemTime>) -> Option<PathBuf> {
    dirs.iter()
        .filter_map(|dir| Some((time(dir)?, dir)))
        .max_by_key(|(time, _)| *time)
        .map(|(_, dir)| dir.clone())
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Renames `from` to `to`; copies and deletes across volumes.
fn move_dir(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::remove_dir(to).ok();
    if std::fs::rename(from, to).is_err() {
        copy_dir(from, to)
            .with_context(|| format!("copy {} to {}", from.display(), to.display()))?;
        std::fs::remove_dir_all(from).with_context(|| format!("remove {}", from.display()))?;
    }
    retarget_links(to, from, to);
    Ok(())
}

/// Points absolute symlinks into the old folder (engine homes have some,
/// such as a Nix profile) at the same place in the new one.
fn retarget_links(dir: &Path, from: &Path, to: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            #[cfg(unix)]
            if let Ok(target) = std::fs::read_link(&path)
                && let Ok(rest) = target.strip_prefix(from)
            {
                std::fs::remove_file(&path).ok();
                std::os::unix::fs::symlink(to.join(rest), &path).ok();
            }
        } else if kind.is_dir() {
            retarget_links(&path, from, to);
        }
    }
}

/// Codex resumes a thread from the absolute rollout path its state
/// database recorded, and doesn't look again when the file isn't there:
/// after a move every earlier thread fails with "no rollout found". Points
/// those paths into the moved folder.
fn retarget_codex_rollouts(from: &Path, to: &Path) -> Result<()> {
    // Codex's home inside Forge's data (forge-engines' `Layout`).
    let codex = to.join("engines").join("codex").join("home").join(".codex");
    let Ok(entries) = std::fs::read_dir(&codex) else {
        return Ok(());
    };
    let separator = std::path::MAIN_SEPARATOR_STR;
    let old = format!("{}{separator}", from.display());
    let new = format!("{}{separator}", to.display());
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("state_") && name.ends_with(".sqlite")) {
            continue;
        }
        let retarget = || -> Result<usize> {
            let db = rusqlite::Connection::open(&path)?;
            db.busy_timeout(std::time::Duration::from_secs(5))?;
            Ok(db.execute(
                "UPDATE threads SET rollout_path = ?2 || substr(rollout_path, length(?1) + 1)
                 WHERE substr(rollout_path, 1, length(?1)) = ?1",
                rusqlite::params![old, new],
            )?)
        };
        retarget().with_context(|| {
            format!(
                "point Codex's rollouts in {} from {} to {}",
                path.display(),
                from.display(),
                to.display()
            )
        })?;
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Codex thread started before the move resumes after it: its
    /// recorded rollout path follows the data, other paths stay put.
    #[test]
    fn codex_threads_from_before_the_move_find_their_rollouts() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("artisan-dev").join("forge");
        let to = dir.path().join("artisan-street").join("forge").join("data");
        let codex = to.join("engines").join("codex").join("home").join(".codex");
        std::fs::create_dir_all(&codex).unwrap();
        let rollout = |root: &Path| {
            root.join("engines/codex/home/.codex/sessions/2026/10/17/rollout-a.jsonl")
                .display()
                .to_string()
        };
        let elsewhere = dir
            .path()
            .join("other/rollout-b.jsonl")
            .display()
            .to_string();
        let db = rusqlite::Connection::open(codex.join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL)")
            .unwrap();
        db.execute(
            "INSERT INTO threads VALUES ('a', ?1), ('b', ?2)",
            rusqlite::params![rollout(&from), elsewhere],
        )
        .unwrap();
        drop(db);

        retarget_codex_rollouts(&from, &to).unwrap();

        let db = rusqlite::Connection::open(codex.join("state_5.sqlite")).unwrap();
        let path = |id: &str| -> String {
            db.query_row(
                "SELECT rollout_path FROM threads WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(path("a"), rollout(&to));
        assert_eq!(path("b"), elsewhere);
    }
}
