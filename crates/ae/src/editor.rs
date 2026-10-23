//! `ae open [<path>]`.

use crate::{install, sys, ui};
use anyhow::{Context, Result};
use artisan_paths::App;
use std::path::Path;

/// Starts the installed Editor, detached from this terminal, on a folder
/// or an `artisan://` link. Offers to install it when it isn't.
pub fn open(path: Option<&Path>) -> Result<()> {
    if install::installed(App::Editor).is_none() {
        if !ui::interactive() {
            anyhow::bail!("the Editor isn't installed here; run `ae install --editor here`");
        }
        let go = cliclack::confirm("The Editor isn't installed here. Install it?")
            .initial_value(true)
            .interact()
            .map_err(ui::prompt_error)?;
        if !go {
            return Ok(());
        }
        return install::install_editor();
    }
    let mut command = std::process::Command::new(App::Editor.binary()?);
    if let Some(path) = path {
        let is_link = path
            .to_str()
            .is_some_and(|text| text.starts_with("artisan://"));
        command.arg(if is_link {
            path.to_owned()
        } else {
            std::path::absolute(path).with_context(|| format!("resolve {}", path.display()))?
        });
    }
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    sys::spawn_detached(&mut command)?;
    Ok(())
}
