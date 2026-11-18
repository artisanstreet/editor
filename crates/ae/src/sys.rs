use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum BoundedRunError {
    #[error("start {program}")]
    Start {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("wait for {program}")]
    Wait {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{program} didn't finish within {} seconds", after.as_secs())]
    TimedOut { program: String, after: Duration },
}

pub fn output_within(command: &mut Command, limit: Duration) -> Result<Output, BoundedRunError> {
    let program = command.get_program().to_string_lossy().into_owned();
    let wait = |source| BoundedRunError::Wait {
        program: program.clone(),
        source,
    };
    let mut child = quiet(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| BoundedRunError::Start {
            program: program.clone(),
            source,
        })?;
    let stdout = child.stdout.take().map(read_all);
    let stderr = child.stderr.take().map(read_all);
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(wait)? {
            break status;
        }
        if started.elapsed() >= limit {
            child.kill().map_err(wait)?;
            child.wait().map_err(wait)?;
            return Err(BoundedRunError::TimedOut {
                program,
                after: limit,
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ok(Output {
        status,
        stdout: joined(stdout).map_err(wait)?,
        stderr: joined(stderr).map_err(wait)?,
    })
}

fn read_all(
    mut pipe: impl std::io::Read + Send + 'static,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).map(|_read| bytes)
    })
}

fn joined(
    reader: Option<std::thread::JoinHandle<std::io::Result<Vec<u8>>>>,
) -> std::io::Result<Vec<u8>> {
    reader.map_or_else(
        || Ok(Vec::new()),
        |reader| {
            reader
                .join()
                .expect("invariant: reading a pipe doesn't panic")
        },
    )
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("run {program}")]
    Start {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{program} {args} failed ({status}): {stderr}")]
    Failed {
        program: String,
        args: String,
        status: std::process::ExitStatus,
        stderr: String,
    },
    #[error("PowerShell failed ({status}): {stderr}")]
    PowerShell {
        status: std::process::ExitStatus,
        stderr: String,
    },
}

pub fn run(program: &str, args: &[&str]) -> Result<(), RunError> {
    output(program, args).map(drop)
}

pub fn output(program: &str, args: &[&str]) -> Result<String, RunError> {
    let output = quiet(Command::new(program).args(args))
        .env_remove("LD_LIBRARY_PATH")
        .stdin(Stdio::null())
        .output()
        .map_err(|source| RunError::Start {
            program: program.to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(RunError::Failed {
            program: program.to_owned(),
            args: args.join(" "),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn powershell(script: &str) -> Result<String, RunError> {
    let output = powershell_command(script)
        .output()
        .map_err(|source| RunError::Start {
            program: "PowerShell".to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(RunError::PowerShell {
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn powershell_command(script: &str) -> Command {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .stdin(Stdio::null());
    quiet(&mut command);
    command
}

pub fn ps_quote(value: &Path) -> String {
    ps_string(&value.display().to_string())
}

pub fn ps_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[allow(
    clippy::missing_const_for_fn,
    reason = "const only off Windows, where the body sets no flags"
)]
pub fn quiet(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

#[derive(Debug, thiserror::Error)]
#[error("start {}", program.display())]
pub struct SpawnError {
    pub program: PathBuf,
    #[source]
    pub source: std::io::Error,
}

pub fn spawn_detached(command: &mut Command) -> Result<u32, SpawnError> {
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn().map_err(|source| SpawnError {
        program: PathBuf::from(command.get_program()),
        source,
    })?;
    Ok(child.id())
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveError {
    #[error("inspect {}", path.display())]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("remove {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn remove(path: &Path) -> Result<(), RemoveError> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(RemoveError::Inspect {
                path: path.to_owned(),
                source,
            });
        }
    };
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .map_err(|source| RemoveError::Remove {
        path: path.to_owned(),
        source,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("create {}", path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn write(path: &Path, contents: &str) -> Result<(), WriteError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| WriteError::CreateDir {
            path: parent.to_owned(),
            source,
        })?;
    }
    std::fs::write(path, contents).map_err(|source| WriteError::Write {
        path: path.to_owned(),
        source,
    })
}
