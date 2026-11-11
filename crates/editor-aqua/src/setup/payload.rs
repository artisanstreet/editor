use anyhow::{Context as _, Result};
use artisan_payload::Payload;
use std::path::PathBuf;

pub static AE: Payload<'static> = artisan_payload::payload!("ae.gz");
static AE_LINUX: Payload<'static> = artisan_payload::payload!("ae-linux.gz");
static NOTICES: Payload<'static> = artisan_payload::payload!("notices.gz");

pub struct Staged {
    _folder: tempfile::TempDir,
    pub ae: PathBuf,
    pub linux_ae: Option<PathBuf>,
}

pub fn stage() -> Result<Staged> {
    let folder = tempfile::Builder::new()
        .prefix("artisan-setup-")
        .tempdir()
        .context("create a folder for the installer")?;
    let ae = folder.path().join(artisan_paths::App::Ae.binary_name());
    AE.write_program(&ae)
        .with_context(|| format!("write {}", ae.display()))?;
    let linux_ae = AE_LINUX
        .present()
        .then(|| -> Result<PathBuf> {
            let path = folder.path().join("ae-linux");
            AE_LINUX
                .write_program(&path)
                .with_context(|| format!("write {}", path.display()))?;
            Ok(path)
        })
        .transpose()?;
    Ok(Staged {
        _folder: folder,
        ae,
        linux_ae,
    })
}

pub fn write_notices() -> Result<()> {
    let folder = artisan_paths::App::Editor.dir()?;
    if !folder.is_dir() {
        return Ok(());
    }
    let path = folder.join("NOTICES.txt");
    NOTICES
        .write(&path)
        .with_context(|| format!("write {}", path.display()))
}
