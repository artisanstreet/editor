mod install;
pub mod model;
mod payload;
pub mod view;
mod window;

use artisan_paths::{App, RootError};
pub use install::InstallError;
use model::{Entry, Existing, Setup};
pub(crate) use payload::open_notices;
pub use payload::{StageError, WriteNoticesError};
use std::path::PathBuf;
pub use window::SetupWindowError;

pub(crate) enum Outcome {
    OpenEditor,
    Finished,
}

#[derive(Debug, thiserror::Error)]
pub enum BeginError {
    #[error("find this program")]
    CurrentExe(#[source] std::io::Error),
    #[error("find where Artisan Street is installed")]
    InstallFolder(#[source] RootError),
    #[error("read the installed Editor's version from {}", path.display())]
    ReadVersion {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("open the installed Editor {}", path.display())]
    OpenInstalled {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("install from the command line")]
    Install(#[source] InstallError),
    #[error("run setup")]
    Window(#[source] SetupWindowError),
}

fn installed_version() -> Result<Option<String>, BeginError> {
    let binary = App::Editor.binary().map_err(BeginError::InstallFolder)?;
    if !binary.is_file() {
        return Ok(None);
    }
    let path = App::Editor
        .dir()
        .map_err(BeginError::InstallFolder)?
        .join("version");
    match std::fs::read_to_string(&path) {
        Ok(version) => Ok(Some(version.trim().to_owned())),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(BeginError::ReadVersion { path, source }),
    }
}

fn open_installed(arguments: &[String]) -> Result<(), BeginError> {
    let path = App::Editor.binary().map_err(BeginError::InstallFolder)?;
    std::process::Command::new(&path)
        .args(arguments)
        .spawn()
        .map_err(|source| BeginError::OpenInstalled {
            path: path.clone(),
            source,
        })?;
    Ok(())
}

pub(crate) fn begin() -> Result<Outcome, BeginError> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let running = std::env::current_exe().map_err(BeginError::CurrentExe)?;
    let runs_installed = App::Editor
        .binary()
        .is_ok_and(|installed| model::same_program(&running, &installed));
    let version = env!("CARGO_PKG_VERSION");
    match model::entry(payload::AE.present(), runs_installed, &arguments) {
        Entry::Editor => Ok(Outcome::OpenEditor),
        Entry::Install { arguments } => {
            install::headless(&arguments).map_err(BeginError::Install)?;
            Ok(Outcome::Finished)
        }
        Entry::Setup => {
            match model::existing(installed_version()?.as_deref(), version) {
                Existing::Current => open_installed(&arguments)?,
                Existing::Older(from) => {
                    window::run(Setup::update(version, from)).map_err(BeginError::Window)?;
                }
                Existing::Absent => {
                    let nothing_installed = App::ALL
                        .iter()
                        .all(|app| !app.binary().is_ok_and(|binary| binary.is_file()));
                    let root = artisan_paths::root().map_err(BeginError::InstallFolder)?;
                    window::run(Setup::fresh(version, root, nothing_installed))
                        .map_err(BeginError::Window)?;
                }
            }
            Ok(Outcome::Finished)
        }
    }
}
