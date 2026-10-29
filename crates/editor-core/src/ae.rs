//! The installed `ae`. It owns this machine's Forge enrollment and pairing,
//! and forwards Forge commands into WSL when the Forge runs there.

use anyhow::{Context as _, Result, bail};
use tokio_util::task::AbortOnDropHandle;

#[derive(Clone)]
pub enum Ae {
    Installed,
    #[cfg(any(test, feature = "test-support"))]
    Scripted(AeScript),
}

/// Answers `ae <args>` with what it would print, or its failure.
#[cfg(any(test, feature = "test-support"))]
pub type AeScript = std::sync::Arc<dyn Fn(&[&str]) -> Result<String> + Send + Sync>;

impl Ae {
    /// Runs `ae <args>` and returns what it printed.
    pub async fn run(&self, args: &[&str]) -> Result<String> {
        match self {
            Self::Installed => installed(args.iter().map(|arg| (*arg).to_owned()).collect()).await,
            #[cfg(any(test, feature = "test-support"))]
            Self::Scripted(script) => script(args),
        }
    }
}

async fn installed(args: Vec<String>) -> Result<String> {
    let path = artisan_paths::App::Ae
        .binary()
        .context("locate the installed `ae`")?;
    let mut command = tokio::process::Command::new(&path);
    command.args(&args).kill_on_drop(true);
    #[cfg(windows)]
    {
        // The Editor is a GUI app: a console child would flash a window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = AbortOnDropHandle::new(
        crate::runtime::runtime().spawn(async move { command.output().await }),
    )
    .await
    .context("`ae` task")?
    .with_context(|| format!("run {} (install Artisan with `ae install`)", path.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("no message");
        bail!("`ae {}` failed: {}", args.join(" "), detail.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
