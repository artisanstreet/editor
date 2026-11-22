use super::payload::{self, StageError, Staged, WriteNoticesError};
use std::io::{BufRead as _, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("find this program")]
    CurrentExe(#[source] std::io::Error),
    #[error("stage the installer")]
    Stage(#[source] StageError),
    #[error("start the installer {}", path.display())]
    Start {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("wait for the installer")]
    Wait(#[source] std::io::Error),
    #[error("{reason}")]
    Refused { code: Option<i32>, reason: String },
    #[error("write the third-party notices")]
    Notices(#[source] WriteNoticesError),
    #[error("setup was closed before the installer finished")]
    Cancelled,
}

#[derive(Clone, Default)]
pub(super) struct Cancel {
    requested: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Cancel {
    pub(super) fn cancel(&self) {
        self.requested.store(true, Ordering::SeqCst);
        self.kill();
    }

    fn requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    fn kill(&self) {
        if let Some(child) = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
            && let Err(error) = child.kill()
        {
            tracing::warn!(
                pid = child.id(),
                %error,
                "setup could not stop the installer"
            );
        }
    }

    fn adopt(&self, child: Child) {
        *self.child.lock().unwrap_or_else(PoisonError::into_inner) = Some(child);
        if self.requested() {
            self.kill();
        }
    }

    fn release(&self) -> Child {
        self.child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .expect("invariant: the installer child stays adopted until its run releases it")
    }
}

fn command(staged: &Staged, arguments: &[String]) -> Result<Command, InstallError> {
    let mut command = Command::new(&staged.ae);
    command
        .args(arguments)
        .arg("--bundled-editor")
        .arg(std::env::current_exe().map_err(InstallError::CurrentExe)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(linux_ae) = &staged.linux_ae {
        command.arg("--bundled-linux-ae").arg(linux_ae);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(command)
}

fn failure(lines: &[String]) -> String {
    lines
        .iter()
        .rev()
        .find_map(|line| line.strip_prefix("error: "))
        .or_else(|| lines.last().map(String::as_str))
        .unwrap_or("the installer stopped without saying why")
        .to_owned()
}

pub(super) fn run(
    arguments: &[String],
    cancel: &Cancel,
    mut progress: impl FnMut(&str),
) -> Result<(), InstallError> {
    let staged = payload::stage().map_err(InstallError::Stage)?;
    if cancel.requested() {
        return Err(InstallError::Cancelled);
    }
    let mut child = command(&staged, arguments)?
        .spawn()
        .map_err(|source| InstallError::Start {
            path: staged.ae.clone(),
            source,
        })?;
    let stderr = child.stderr.take();
    cancel.adopt(child);
    let mut lines = Vec::new();
    if let Some(stderr) = stderr {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let line = line.trim().to_owned();
            if !line.is_empty() {
                progress(&line);
                lines.push(line);
            }
        }
    }
    let status = cancel.release().wait().map_err(InstallError::Wait)?;
    if cancel.requested() {
        return Err(InstallError::Cancelled);
    }
    if !status.success() {
        return Err(InstallError::Refused {
            code: status.code(),
            reason: failure(&lines),
        });
    }
    payload::write_notices().map_err(InstallError::Notices)
}

#[expect(
    clippy::print_stderr,
    reason = "a headless install reports its progress lines on stderr"
)]
pub(super) fn headless(arguments: &[String]) -> Result<(), InstallError> {
    let mut all = vec!["install".to_owned(), "--apply".to_owned()];
    all.extend_from_slice(arguments);
    run(&all, &Cancel::default(), |line| eprintln!("{line}"))
}
