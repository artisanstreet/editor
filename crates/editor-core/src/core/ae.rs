use tokio_util::task::AbortOnDropHandle;

#[derive(Clone)]
pub enum Ae {
    Installed,
    #[cfg(any(test, feature = "test-support"))]
    Scripted(AeScript),
}

#[cfg(any(test, feature = "test-support"))]
pub type AeScript = std::sync::Arc<dyn Fn(&[&str]) -> Result<String, AeError> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum AeError {
    #[error("locate the installed `ae`")]
    Locate(#[source] artisan_paths::RootError),
    #[error("run {path} (install Artisan with `ae install`)")]
    Run {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the `ae {command}` task stopped")]
    Task {
        command: String,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("`ae {command}` failed: {detail}")]
    Failed { command: String, detail: String },
}

impl Ae {
    pub(crate) async fn run(&self, args: &[&str]) -> Result<String, AeError> {
        match self {
            Self::Installed => installed(args.iter().map(|arg| (*arg).to_owned()).collect()).await,
            #[cfg(any(test, feature = "test-support"))]
            Self::Scripted(script) => script(args),
        }
    }
}

async fn installed(args: Vec<String>) -> Result<String, AeError> {
    let path = artisan_paths::App::Ae.binary().map_err(AeError::Locate)?;
    let command_line = args.join(" ");
    let mut command = tokio::process::Command::new(&path);
    command.args(&args).kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = AbortOnDropHandle::new(
        crate::core::runtime::runtime().spawn(async move { command.output().await }),
    )
    .await
    .map_err(|source| AeError::Task {
        command: command_line.clone(),
        source,
    })?
    .map_err(|source| AeError::Run { path, source })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("no message");
        return Err(AeError::Failed {
            command: command_line,
            detail: detail.trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

impl std::fmt::Debug for Ae {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Installed => f.write_str("Installed"),
            #[cfg(any(test, feature = "test-support"))]
            Self::Scripted(_) => f.write_str("Scripted"),
        }
    }
}
