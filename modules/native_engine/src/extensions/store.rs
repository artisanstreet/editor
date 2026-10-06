//! The Forge's content-addressed extension store.
//!
//! `<state>/extensions/store/<digest>/` holds one skill's files, written once
//! through a staging folder and an atomic rename and never changed after.
//! Engines link to these folders, so the files are made read-only: an engine
//! that tried to edit an Artisan skill in place would otherwise corrupt the
//! store entry every engine shares. `<state>/extensions/backups/` keeps the
//! engine files Artisan replaced, so nothing the user had is ever lost.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::skill::{SkillPackage, hex};

/// The Forge's extension directories below its state root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionStore {
    root: PathBuf,
}

impl ExtensionStore {
    /// Locates the store below the Forge state root (the database's parent).
    #[must_use]
    pub fn new(state_root: &Path) -> Self {
        Self {
            root: state_root.join("extensions"),
        }
    }

    /// Returns `<state>/extensions`.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the folder holding every stored skill.
    #[must_use]
    pub fn store_dir(&self) -> PathBuf {
        self.root.join("store")
    }

    /// Returns the folder one stored skill lives in.
    #[must_use]
    pub fn skill_dir(&self, digest: &str) -> PathBuf {
        self.store_dir().join(digest)
    }

    /// Returns the folder replaced engine files are kept in.
    #[must_use]
    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    /// Creates the private store directories.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Unavailable`] when a directory cannot be created
    /// or is unsafe.
    pub fn prepare(&self) -> Result<(), StoreError> {
        for directory in [
            self.root.clone(),
            self.store_dir(),
            self.root.join("stage"),
            self.backups_dir(),
        ] {
            crate::io::ensure_private_directory(&directory).map_err(|_| StoreError::Unavailable)?;
        }
        Ok(())
    }

    /// Whether `target` lies inside the store, so a link to it is Artisan's.
    #[must_use]
    pub fn owns(&self, target: &Path) -> bool {
        target.starts_with(self.store_dir())
    }

    /// Stores a skill and returns its folder. Storing a skill already present
    /// is a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when writing or publishing the folder fails.
    pub fn put(&self, package: &SkillPackage) -> Result<PathBuf, StoreError> {
        let target = self.skill_dir(package.digest());
        if fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.is_dir()) {
            return Ok(target);
        }
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce).map_err(|_| StoreError::Write)?;
        let stage = self
            .root
            .join("stage")
            .join(format!("{}-{}", package.digest(), hex(&nonce)));
        let written = write_package(&stage, package);
        if written.is_err() {
            let _ = remove_tree(&stage);
            return written.map(|()| target);
        }
        match fs::rename(&stage, &target) {
            Ok(()) => Ok(target),
            Err(_) if target.is_dir() => {
                // Another reconcile stored the same digest first.
                let _ = remove_tree(&stage);
                Ok(target)
            }
            Err(_) => {
                let _ = remove_tree(&stage);
                Err(StoreError::Write)
            }
        }
    }

    /// Removes stored skills whose digest is not in `keep`, and leftover
    /// staging folders. Returns how many entries were removed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Unavailable`] when the store cannot be listed.
    pub fn collect_garbage(&self, keep: &HashSet<String>) -> Result<usize, StoreError> {
        let mut removed = 0;
        for directory in [self.store_dir(), self.root.join("stage")] {
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err(StoreError::Unavailable),
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let staged = directory.ends_with("stage");
                if (staged || !keep.contains(name.to_string_lossy().as_ref()))
                    && remove_tree(&entry.path()).is_ok()
                {
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }
}

fn write_package(stage: &Path, package: &SkillPackage) -> Result<(), StoreError> {
    fs::create_dir_all(stage).map_err(|_| StoreError::Write)?;
    for file in package.files() {
        let path = file
            .path
            .split('/')
            .fold(stage.to_path_buf(), |path, part| path.join(part));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| StoreError::Write)?;
        }
        fs::write(&path, &file.bytes).map_err(|_| StoreError::Write)?;
        make_read_only(&path)?;
    }
    Ok(())
}

fn make_read_only(path: &Path) -> Result<(), StoreError> {
    let mut permissions = fs::metadata(path)
        .map_err(|_| StoreError::Write)?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).map_err(|_| StoreError::Write)
}

/// Removes a folder tree that may hold read-only files. Links inside are
/// removed, never followed.
pub(crate) fn remove_tree(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        let mut permissions = metadata.permissions();
        if permissions.readonly() && !metadata.file_type().is_symlink() {
            #[allow(
                clippy::permissions_set_readonly_false,
                reason = "the store only clears read-only files it wrote itself before deleting them"
            )]
            permissions.set_readonly(false);
            let _ = fs::set_permissions(path, permissions);
        }
        return fs::remove_file(path);
    }
    for entry in fs::read_dir(path)? {
        remove_tree(&entry?.path())?;
    }
    fs::remove_dir(path)
}

/// Why the store could not be used.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum StoreError {
    #[error("the extension store directory is unavailable")]
    Unavailable,
    #[error("a skill could not be written to the extension store")]
    Write,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::skill::{SKILL_FILE, SkillFile};

    fn package(body: &str) -> SkillPackage {
        SkillPackage::from_files(vec![
            SkillFile {
                path: SKILL_FILE.to_owned(),
                bytes: format!("---\nname: demo\ndescription: Demo.\n---\n{body}").into_bytes(),
            },
            SkillFile {
                path: "references/notes.md".to_owned(),
                bytes: b"notes".to_vec(),
            },
        ])
        .unwrap()
    }

    #[test]
    fn put_is_idempotent_and_files_are_read_only() {
        let state = tempfile::tempdir().unwrap();
        let store = ExtensionStore::new(state.path());
        store.prepare().unwrap();
        let skill = package("body");
        let first = store.put(&skill).unwrap();
        let second = store.put(&skill).unwrap();
        assert_eq!(first, second);
        assert!(store.owns(&first));
        let notes = first.join("references").join("notes.md");
        assert_eq!(fs::read(&notes).unwrap(), b"notes");
        assert!(fs::metadata(&notes).unwrap().permissions().readonly());
        assert_eq!(
            crate::extensions::skill::read_skill_dir(&first)
                .unwrap()
                .digest(),
            skill.digest()
        );
    }

    #[test]
    fn garbage_collection_keeps_only_referenced_digests() {
        let state = tempfile::tempdir().unwrap();
        let store = ExtensionStore::new(state.path());
        store.prepare().unwrap();
        let kept = store.put(&package("kept")).unwrap();
        let dropped = store.put(&package("dropped")).unwrap();
        let keep: HashSet<String> = [package("kept").digest().to_owned()].into();
        assert_eq!(store.collect_garbage(&keep).unwrap(), 1);
        assert!(kept.is_dir());
        assert!(!dropped.exists());
    }
}
