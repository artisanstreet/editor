use anyhow::{Context, Result, bail};
use std::path::Path;
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

/// Runs `program` and fails with its stderr when it fails.
pub fn run(program: &str, args: &[&str]) -> Result<()> {
    output(program, args).map(drop)
}

/// Runs `program` and returns its stdout. System tools run without
/// `LD_LIBRARY_PATH`: a Nix shell's libraries in it break the system's glibc
/// programs, systemctl among them.
pub fn output(program: &str, args: &[&str]) -> Result<String> {
    let output = quiet(Command::new(program).args(args))
        .env_remove("LD_LIBRARY_PATH")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs a PowerShell script and fails with its error output.
pub fn powershell(script: &str) -> Result<String> {
    let output = powershell_command(script)
        .output()
        .context("run PowerShell")?;
    if !output.status.success() {
        bail!(
            "PowerShell failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
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

/// A PowerShell single-quoted string.
pub fn ps_quote(value: &Path) -> String {
    ps_string(&value.display().to_string())
}

pub fn ps_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// No console window flashes up for helper processes on Windows.
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

/// Starts `command` detached from this terminal: its own process group,
/// no console window on Windows.
pub fn spawn_detached(command: &mut Command) -> Result<u32> {
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
    let child = command
        .spawn()
        .with_context(|| format!("start {}", command.get_program().display()))?;
    Ok(child.id())
}

pub fn remove(path: &Path) -> Result<()> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("inspect {}", path.display())),
    };
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .with_context(|| format!("remove {}", path.display()))
}

/// Writes a file, creating its folder.
pub fn write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}
