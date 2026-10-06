//! Skill links inside engine skill folders.
//!
//! Artisan places a skill in an engine by linking `<skills>/<name>` to the
//! skill's folder in the content store (or, in development builds, to the
//! repository's first-party skill folder). A link pointing into one of those
//! roots is Artisan's; anything else in a skills folder belongs to the user or
//! the engine and is only ever read, or moved into the backups when the user
//! adopts it.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::skill::hex;

/// What occupies one skill name in an engine's skills folder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SkillEntry {
    /// Nothing.
    Missing,
    /// Artisan's link to `target`.
    Ours {
        /// Where the link points.
        target: PathBuf,
    },
    /// The user's or the engine's own skill (a folder, a file, or a link
    /// elsewhere).
    Foreign,
}

/// Classifies `<skills_dir>/<name>`. `owned_roots` are the folders Artisan's
/// links point into.
#[must_use]
pub fn inspect_entry(skills_dir: &Path, name: &str, owned_roots: &[PathBuf]) -> SkillEntry {
    let path = skills_dir.join(name);
    match fs::symlink_metadata(&path) {
        Err(_) => SkillEntry::Missing,
        Ok(metadata) if metadata.file_type().is_symlink() => match fs::read_link(&path) {
            Ok(target) if owned_roots.iter().any(|root| target.starts_with(root)) => {
                SkillEntry::Ours { target }
            }
            _ => SkillEntry::Foreign,
        },
        Ok(_) => SkillEntry::Foreign,
    }
}

/// Points `<skills_dir>/<name>` at `target`, creating the skills folder when
/// needed. The link is built beside its destination and renamed over it, so
/// an engine never sees the name missing while a link is retargeted.
///
/// # Errors
///
/// Returns [`LinkError`] when the folder cannot be created, the operating
/// system refuses the link, or the name is occupied by something not
/// Artisan's.
pub fn place_link(
    skills_dir: &Path,
    name: &str,
    target: &Path,
    owned_roots: &[PathBuf],
) -> Result<(), LinkError> {
    match inspect_entry(skills_dir, name, owned_roots) {
        SkillEntry::Foreign => return Err(LinkError::Occupied),
        SkillEntry::Ours { target: current } if current == target => return Ok(()),
        SkillEntry::Ours { .. } | SkillEntry::Missing => {}
    }
    fs::create_dir_all(skills_dir).map_err(|_| LinkError::Io)?;
    let mut nonce = [0_u8; 6];
    getrandom::fill(&mut nonce).map_err(|_| LinkError::Io)?;
    let staged = skills_dir.join(format!(".artisan-{name}-{}", hex(&nonce)));
    create_dir_link(target, &staged)?;
    let destination = skills_dir.join(name);
    if fs::rename(&staged, &destination).is_err() {
        // Windows cannot rename over a directory link: remove ours, then move.
        let replaced = matches!(
            inspect_entry(skills_dir, name, owned_roots),
            SkillEntry::Ours { .. }
        ) && remove_link_file(&destination).is_ok()
            && fs::rename(&staged, &destination).is_ok();
        if !replaced {
            let _ = remove_link_file(&staged);
            return Err(LinkError::Io);
        }
    }
    Ok(())
}

/// Removes `<skills_dir>/<name>` when it is Artisan's link; anything else is
/// left untouched. Returns whether a link was removed.
///
/// # Errors
///
/// Returns [`LinkError::Io`] when Artisan's link cannot be removed.
pub fn remove_link(
    skills_dir: &Path,
    name: &str,
    owned_roots: &[PathBuf],
) -> Result<bool, LinkError> {
    match inspect_entry(skills_dir, name, owned_roots) {
        SkillEntry::Ours { .. } => remove_link_file(&skills_dir.join(name))
            .map(|()| true)
            .map_err(|_| LinkError::Io),
        SkillEntry::Missing | SkillEntry::Foreign => Ok(false),
    }
}

/// Lists Artisan's links in a skills folder by name.
#[must_use]
pub fn owned_links(skills_dir: &Path, owned_roots: &[PathBuf]) -> Vec<String> {
    entry_names(skills_dir)
        .into_iter()
        .filter(|name| {
            matches!(
                inspect_entry(skills_dir, name, owned_roots),
                SkillEntry::Ours { .. }
            )
        })
        .collect()
}

/// Lists the folders in a skills folder that are not Artisan's, by name.
/// Links to elsewhere are skipped: Artisan never follows them.
#[must_use]
pub fn native_skill_dirs(skills_dir: &Path) -> Vec<(String, PathBuf)> {
    entry_names(skills_dir)
        .into_iter()
        .filter_map(|name| {
            let path = skills_dir.join(&name);
            fs::symlink_metadata(&path)
                .is_ok_and(|metadata| metadata.is_dir())
                .then_some((name, path))
        })
        .collect()
}

fn entry_names(skills_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(skills_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// Moves `path` into `backups_dir` under a unique, labelled name and returns
/// the new location.
///
/// # Errors
///
/// Returns [`LinkError::Io`] when the backup cannot be made.
pub fn move_to_backup(path: &Path, backups_dir: &Path, label: &str) -> Result<PathBuf, LinkError> {
    fs::create_dir_all(backups_dir).map_err(|_| LinkError::Io)?;
    let mut nonce = [0_u8; 4];
    getrandom::fill(&mut nonce).map_err(|_| LinkError::Io)?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let destination = backups_dir.join(format!("{millis}-{label}-{}", hex(&nonce)));
    fs::rename(path, &destination).map_err(|_| LinkError::Io)?;
    Ok(destination)
}

/// Whether this Forge may create directory links: on Windows that needs
/// Developer Mode (or administrator rights). Probes by creating and removing
/// one link inside `scratch`.
#[must_use]
pub fn links_supported(scratch: &Path) -> bool {
    if fs::create_dir_all(scratch).is_err() {
        return false;
    }
    let mut nonce = [0_u8; 6];
    if getrandom::fill(&mut nonce).is_err() {
        return false;
    }
    let link = scratch.join(format!(".artisan-link-probe-{}", hex(&nonce)));
    let created = create_dir_link(scratch, &link).is_ok();
    let _ = remove_link_file(&link);
    created
}

fn create_dir_link(target: &Path, link: &Path) -> Result<(), LinkError> {
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_dir(target, link);
    #[cfg(not(any(unix, windows)))]
    let created: std::io::Result<()> = Err(std::io::Error::from(std::io::ErrorKind::Unsupported));
    created.map_err(|error| {
        // ERROR_PRIVILEGE_NOT_HELD: Windows without Developer Mode.
        if cfg!(windows) && error.raw_os_error() == Some(1314) {
            LinkError::LinksUnavailable
        } else {
            LinkError::Io
        }
    })
}

/// Removes a link itself, never its target.
fn remove_link_file(link: &Path) -> std::io::Result<()> {
    // A directory link is a directory entry on Windows and a file on Unix.
    fs::remove_file(link).or_else(|_| fs::remove_dir(link))
}

/// Why a skill link could not be placed or removed.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum LinkError {
    #[error("the engine already has its own skill under this name")]
    Occupied,
    #[error("the operating system refuses symbolic links")]
    LinksUnavailable,
    #[error("the skill link could not be written")]
    Io,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn links_are_placed_retargeted_and_removed_only_when_ours() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        let first = store.join("aaa");
        let second = store.join("bbb");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let owned = vec![store.clone()];
        let skills = root.path().join("home/.claude/skills");

        place_link(&skills, "demo", &first, &owned).unwrap();
        assert_eq!(
            inspect_entry(&skills, "demo", &owned),
            SkillEntry::Ours {
                target: first.clone()
            }
        );
        place_link(&skills, "demo", &second, &owned).unwrap();
        assert_eq!(fs::read_link(skills.join("demo")).unwrap(), second);
        assert_eq!(owned_links(&skills, &owned), vec!["demo".to_owned()]);
        assert!(native_skill_dirs(&skills).is_empty());

        fs::create_dir_all(skills.join("mine")).unwrap();
        assert_eq!(
            place_link(&skills, "mine", &first, &owned),
            Err(LinkError::Occupied)
        );
        assert!(!remove_link(&skills, "mine", &owned).unwrap());
        assert!(skills.join("mine").is_dir());
        assert_eq!(
            native_skill_dirs(&skills),
            vec![("mine".to_owned(), skills.join("mine"))]
        );

        assert!(remove_link(&skills, "demo", &owned).unwrap());
        assert_eq!(inspect_entry(&skills, "demo", &owned), SkillEntry::Missing);
        assert!(second.is_dir(), "removing a link never removes its target");
    }

    #[test]
    fn backups_move_folders_aside_and_probe_reports_links() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("skills/mine");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "x").unwrap();
        let backup = move_to_backup(&skill, &root.path().join("backups"), "claude-mine").unwrap();
        assert!(!skill.exists());
        assert_eq!(fs::read(backup.join("SKILL.md")).unwrap(), b"x");
        assert!(links_supported(&root.path().join("probe")));
    }
}
