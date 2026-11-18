use crate::install::{self, InstallEditorError};
use crate::paths::{self, LocateError};
use crate::sys::{self, SpawnError};
use crate::ui::{self, AskError};
use artisan_paths::App;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("the Editor isn't installed here; run `ae install --editor here`")]
    NotInstalled,
    #[error(transparent)]
    Ask(#[from] AskError),
    #[error(transparent)]
    Install(#[from] InstallEditorError),
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("resolve {}", path.display())]
    Resolve {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Spawn(#[from] SpawnError),
}

pub fn open(path: Option<&Path>) -> Result<(), OpenError> {
    if install::installed(App::Editor).is_none() {
        if !ui::interactive() {
            return Err(OpenError::NotInstalled);
        }
        let go = cliclack::confirm("The Editor isn't installed here. Install it?")
            .initial_value(true)
            .interact()
            .map_err(ui::prompt_error)?;
        if !go {
            return Ok(());
        }
        return Ok(install::install_editor()?);
    }
    let mut command = std::process::Command::new(paths::binary(App::Editor)?);
    if let Some(path) = path {
        let is_link = path
            .to_str()
            .is_some_and(|text| text.starts_with("artisan://"));
        command.arg(if is_link {
            path.to_owned()
        } else {
            std::path::absolute(path).map_err(|source| OpenError::Resolve {
                path: path.to_owned(),
                source,
            })?
        });
    }
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    sys::spawn_detached(&mut command)?;
    Ok(())
}
