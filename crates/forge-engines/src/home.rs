use artisan_api::EngineId;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[must_use]
pub(crate) fn forge_records(home: &Path) -> PathBuf {
    let own = home
        .file_name()
        .expect("invariant: an engine home is a named folder");
    let mut sibling = own.to_os_string();
    sibling.push(".artisan");
    home.with_file_name(sibling)
}

#[cfg_attr(
    not(unix),
    expect(unused_variables, reason = "only Unix files carry a permission mode")
)]
pub(crate) fn write(home: &Path, relative: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let named = home.join(relative);
    let path = match fs::symlink_metadata(&named) {
        Ok(metadata) if metadata.is_symlink() => fs::canonicalize(&named)?,
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(&named)?;
            named
        }
        Ok(_) | Err(_) => named,
    };
    let folder = path
        .parent()
        .ok_or_else(|| io::Error::other(format!("{} names no folder", path.display())))?;
    make_private_folder(folder)?;
    let mut staged = tempfile::NamedTempFile::new_in(folder)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staged
            .as_file()
            .set_permissions(fs::Permissions::from_mode(mode))?;
    }
    io::Write::write_all(&mut staged, bytes)?;
    staged.persist(&path).map_err(|failure| failure.error)?;
    Ok(())
}

#[cfg(unix)]
fn make_private_folder(folder: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
}

#[cfg(not(unix))]
fn make_private_folder(folder: &Path) -> io::Result<()> {
    fs::create_dir_all(folder)
}

pub(crate) fn read(home: &Path, relative: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(home.join(relative)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn remove(home: &Path, relative: &Path) -> io::Result<()> {
    let path = home.join(relative);
    let removed = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&path),
        Ok(_) => fs::remove_file(&path),
        Err(error) => Err(error),
    };
    match removed {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("write {engine}'s configuration {}", path.display())]
    Write {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("read {engine}'s configuration {}", path.display())]
    Read {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("remove {engine}'s configuration {}", path.display())]
    Remove {
        engine: EngineId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub(crate) fn write_config(
    engine: EngineId,
    home: &Path,
    relative: &Path,
    bytes: &[u8],
    mode: u32,
) -> Result<(), ConfigError> {
    write(home, relative, bytes, mode).map_err(|source| ConfigError::Write {
        engine,
        path: home.join(relative),
        source,
    })
}

pub(crate) struct Editing {
    engine: EngineId,
    home: PathBuf,
    relative: PathBuf,
    base: Option<Vec<u8>>,
}

pub(crate) fn edit_config(
    engine: EngineId,
    home: &Path,
    relative: &Path,
) -> Result<Editing, ConfigError> {
    let base = read(home, relative).map_err(|source| ConfigError::Read {
        engine,
        path: home.join(relative),
        source,
    })?;
    Ok(Editing {
        engine,
        home: home.to_path_buf(),
        relative: relative.to_path_buf(),
        base,
    })
}

impl Editing {
    pub(crate) fn base(&self) -> Option<&[u8]> {
        self.base.as_deref()
    }

    pub(crate) fn write(self, bytes: &[u8], mode: u32) -> Result<(), ConfigError> {
        write_config(self.engine, &self.home, &self.relative, bytes, mode)
    }
}

pub(crate) fn remove_config(
    engine: EngineId,
    home: &Path,
    relative: &Path,
) -> Result<(), ConfigError> {
    remove(home, relative).map_err(|source| ConfigError::Remove {
        engine,
        path: home.join(relative),
        source,
    })
}
