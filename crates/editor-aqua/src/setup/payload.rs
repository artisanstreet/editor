use artisan_paths::RootError;
use artisan_payload::{Payload, WritePayloadError};
use std::path::PathBuf;

pub(super) static AE: Payload<'static> = artisan_payload::payload!("ae.gz");
static AE_LINUX: Payload<'static> = artisan_payload::payload!("ae-linux.gz");
static NOTICES: Payload<'static> = artisan_payload::payload!("notices.gz");

#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error("create a folder for the installer")]
    Folder(#[source] std::io::Error),
    #[error("write the installer")]
    Installer(#[source] WritePayloadError),
    #[error("write the Linux installer")]
    LinuxInstaller(#[source] WritePayloadError),
}

#[derive(Debug, thiserror::Error)]
pub enum WriteNoticesError {
    #[error("find the Editor's folder")]
    Folder(#[source] RootError),
    #[error("write the notices into the Editor's folder")]
    Write(#[source] WritePayloadError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum OpenNoticesError {
    #[error("write the notices to a temporary file")]
    Write(#[source] WritePayloadError),
    #[error("open the notices")]
    Open(#[source] crate::shell::OpenPathError),
}

pub(super) struct Staged {
    _folder: tempfile::TempDir,
    pub(crate) ae: PathBuf,
    pub(crate) linux_ae: Option<PathBuf>,
}

pub(super) fn stage() -> Result<Staged, StageError> {
    let folder = tempfile::Builder::new()
        .prefix("artisan-setup-")
        .tempdir()
        .map_err(StageError::Folder)?;
    let ae = folder.path().join(artisan_paths::App::Ae.binary_name());
    AE.write_program(&ae).map_err(StageError::Installer)?;
    let linux_ae = AE_LINUX
        .present()
        .then(|| {
            let path = folder.path().join("ae-linux");
            AE_LINUX
                .write_program(&path)
                .map_err(StageError::LinuxInstaller)?;
            Ok(path)
        })
        .transpose()?;
    Ok(Staged {
        _folder: folder,
        ae,
        linux_ae,
    })
}

pub(super) fn write_notices() -> Result<(), WriteNoticesError> {
    let folder = artisan_paths::App::Editor
        .dir()
        .map_err(WriteNoticesError::Folder)?;
    if !folder.is_dir() {
        return Ok(());
    }
    NOTICES
        .write(&folder.join("NOTICES.txt"))
        .map_err(WriteNoticesError::Write)
}

pub(crate) fn open_notices() -> Result<(), OpenNoticesError> {
    let path = std::env::temp_dir().join("Artisan Editor NOTICES.txt");
    NOTICES.write(&path).map_err(OpenNoticesError::Write)?;
    crate::shell::open_path(&path).map_err(OpenNoticesError::Open)
}

#[cfg(test)]
#[path = "../../tests/setup/payload.rs"]
mod tests;
