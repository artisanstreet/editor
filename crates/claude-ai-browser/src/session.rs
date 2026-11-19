use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;

use artisan_paths::private::PrivateFileError;
use serde_json::json;

use crate::Mode;
use crate::devtools::{DevTools, DevToolsError};

const LOCK_FILE: &str = "artisan-capture.lock";
const PORT_FILE: &str = "DevToolsActivePort";
const START_TIMEOUT: Duration = Duration::from_secs(30);
const START_POLL: Duration = Duration::from_millis(200);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("creating the claude.ai browser profile")]
    Profile(#[source] PrivateFileError),
    #[error("opening the claude.ai browser profile's lock {path}")]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("waiting for the claude.ai browser profile's lock {path}")]
    LockWait {
        path: PathBuf,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("another claude.ai browser window is open on this device")]
    Busy,
    #[error(
        "no Chrome, Edge, Chromium or Brave on this device; install one to sign in to claude.ai"
    )]
    NoBrowser,
    #[error("opening {browser}")]
    Launch {
        browser: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{browser} exited ({status})")]
    Exited {
        browser: PathBuf,
        status: std::process::ExitStatus,
    },
    #[error("checking whether {browser} is still running")]
    Wait {
        browser: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{browser} did not open DevTools")]
    NoDevTools {
        browser: PathBuf,
        #[source]
        last: DevToolsError,
    },
    #[error("opening claude.ai in the browser that already runs on the profile")]
    Attach(#[source] DevToolsError),
}

pub(crate) struct BrowserSession(Option<Owned>);

struct Owned {
    devtools: DevTools,
    child: Option<tokio::process::Child>,
    runtime: tokio::runtime::Handle,
    _lock: File,
}

impl BrowserSession {
    pub(crate) async fn open(
        profile: &Path,
        url: &str,
        mode: Mode,
        browser: impl FnOnce() -> Option<PathBuf>,
    ) -> Result<Self, OpenError> {
        artisan_paths::private::create_folder(profile).map_err(OpenError::Profile)?;
        let lock = claim(profile, mode).await?;
        let port_file = profile.join(PORT_FILE);
        if let Ok(mut devtools) = DevTools::connect(&port_file).await {
            devtools
                .call("Target.createTarget", json!({ "url": url }))
                .await
                .map_err(OpenError::Attach)?;
            return Ok(Self::owning(devtools, None, lock));
        }
        if let Err(error) = tokio::fs::remove_file(&port_file).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!(path = %port_file.display(), "a stale DevTools port file stays: {error}");
        }
        let browser = browser().ok_or(OpenError::NoBrowser)?;
        let mut child = launch(&browser, profile, url, mode)?;
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            let last = match DevTools::connect(&port_file).await {
                Ok(devtools) => return Ok(Self::owning(devtools, Some(child), lock)),
                Err(error) => error,
            };
            match child.try_wait() {
                Ok(Some(status)) if !status.success() => {
                    return Err(OpenError::Exited { browser, status });
                }
                Ok(Some(_) | None) => {}
                Err(source) => return Err(OpenError::Wait { browser, source }),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(OpenError::NoDevTools { browser, last });
            }
            tokio::time::sleep(START_POLL).await;
        }
    }

    fn owning(devtools: DevTools, child: Option<tokio::process::Child>, lock: File) -> Self {
        Self(Some(Owned {
            devtools,
            child,
            runtime: tokio::runtime::Handle::current(),
            _lock: lock,
        }))
    }

    pub(crate) fn devtools(&mut self) -> &mut DevTools {
        &mut self
            .0
            .as_mut()
            .expect("invariant: a browser session is owned until it closes")
            .devtools
    }

    pub(crate) async fn close(mut self) {
        if let Some(owned) = self.0.take() {
            owned.close().await;
        }
    }
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        if let Some(owned) = self.0.take() {
            let runtime = owned.runtime.clone();
            runtime.spawn(owned.close());
        }
    }
}

impl Owned {
    async fn close(mut self) {
        if let Err(error) = self.devtools.call("Browser.close", json!({})).await {
            tracing::debug!("the claude.ai browser did not close over DevTools: {error}");
        }
        let Some(mut child) = self.child.take() else {
            return;
        };
        if tokio::time::timeout(EXIT_TIMEOUT, child.wait())
            .await
            .is_err()
            && let Err(error) = child.kill().await
        {
            tracing::warn!("the claude.ai browser could not be stopped: {error}");
        }
    }
}

async fn claim(profile: &Path, mode: Mode) -> Result<File, OpenError> {
    let path = profile.join(LOCK_FILE);
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|source| OpenError::Lock {
            path: path.clone(),
            source,
        })?;
    match mode {
        Mode::Renew => match file.try_lock() {
            Ok(()) => Ok(file),
            Err(TryLockError::WouldBlock) => Err(OpenError::Busy),
            Err(TryLockError::Error(source)) => Err(OpenError::Lock { path, source }),
        },
        Mode::SignIn => {
            let waited = tokio::task::spawn_blocking(move || file.lock().map(|()| file))
                .await
                .map_err(|source| OpenError::LockWait {
                    path: path.clone(),
                    source,
                })?;
            waited.map_err(|source| OpenError::Lock { path, source })
        }
    }
}

fn launch(
    browser: &Path,
    profile: &Path,
    url: &str,
    mode: Mode,
) -> Result<tokio::process::Child, OpenError> {
    let mut command = tokio::process::Command::new(browser);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--remote-debugging-port=0")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if mode == Mode::Renew {
        command
            .arg("--window-position=-32000,-32000")
            .arg("--window-size=640,480");
    }
    command.arg(url);
    command.spawn().map_err(|source| OpenError::Launch {
        browser: browser.to_path_buf(),
        source,
    })
}
