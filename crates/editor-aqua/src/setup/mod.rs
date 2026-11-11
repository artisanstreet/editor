mod install;
pub mod model;
mod payload;
pub mod view;
mod window;

use anyhow::{Context as _, Result};
use artisan_paths::App;
use model::{Entry, Existing, Setup};

pub enum Outcome {
    OpenEditor,
    Finished,
}

fn installed_version() -> Option<String> {
    App::Editor
        .binary()
        .ok()
        .filter(|binary| binary.is_file())?;
    let version = std::fs::read_to_string(App::Editor.dir().ok()?.join("version")).ok()?;
    Some(version.trim().to_owned())
}

fn open_installed(arguments: &[String]) -> Result<()> {
    let installed = App::Editor.binary()?;
    std::process::Command::new(&installed)
        .args(arguments)
        .spawn()
        .with_context(|| format!("open {}", installed.display()))?;
    Ok(())
}

pub fn begin() -> Result<Outcome> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let running = std::env::current_exe().context("find this program")?;
    let runs_installed = App::Editor
        .binary()
        .is_ok_and(|installed| model::same_program(&running, &installed));
    let version = env!("CARGO_PKG_VERSION");
    match model::entry(payload::AE.present(), runs_installed, &arguments) {
        Entry::Editor => Ok(Outcome::OpenEditor),
        Entry::Install { arguments } => {
            install::headless(&arguments)?;
            Ok(Outcome::Finished)
        }
        Entry::Setup => {
            match model::existing(installed_version().as_deref(), version) {
                Existing::Current => open_installed(&arguments)?,
                Existing::Older(from) => {
                    window::run(Setup::update(version, from, artisan_paths::root()?))?;
                }
                Existing::Absent => {
                    window::run(Setup::fresh(version, artisan_paths::default_root()?))?;
                }
            }
            Ok(Outcome::Finished)
        }
    }
}
