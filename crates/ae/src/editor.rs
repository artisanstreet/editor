//! `ae editor open [<path>]`.

use crate::install::Root;
use crate::install::platform::spawn_detached;
use anyhow::{Context, Result};
use std::path::Path;

/// Starts the installed Editor, detached from this terminal. The path (or
/// an `artisan://` link) is passed on as its only argument.
pub fn open(path: Option<&Path>, dry_run: bool) -> Result<()> {
    let root = Root::locate()?;
    let editor = root
        .current_editor()
        .context("the Editor isn't installed here; run `ae install editor`")?;
    let mut command = std::process::Command::new(&editor);
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
        .env("ARTISAN_AE_COMMAND", root.ae())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if dry_run {
        println!("would start {command:?}");
        return Ok(());
    }
    spawn_detached(&mut command)?;
    Ok(())
}
