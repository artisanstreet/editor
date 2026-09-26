//! Bounded subprocess execution for discovery probes.
//!
//! Every probe spawns one finite command, captures bounded stdout/stderr,
//! and enforces a single deadline with kill-and-reap. The program is always a
//! Forge-managed executable spawned with its explicit environment; arguments
//! are never reinterpreted by a command interpreter.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Captured result of one bounded command.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BoundedOutput {
    /// Captured stdout decoded lossily.
    pub(crate) stdout: String,
    /// Whether the process exited successfully.
    pub(crate) success: bool,
}

/// Runs one command with null stdin, bounded output, and a hard deadline.
/// Returns `None` when the command cannot be spawned, exceeds the deadline,
/// or produces undecodable output beyond the bound.
pub(crate) async fn run_bounded(
    program: &super::EngineProgram,
    args: &[&str],
    timeout: Duration,
    max_bytes: usize,
) -> Option<BoundedOutput> {
    let mut child = Command::new(&program.executable)
        .env_clear()
        .envs(program.environment.iter().cloned())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .ok()?;

    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;

    let captured = Box::pin(tokio::time::timeout(timeout, async {
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let (stdout_result, stderr_result) = tokio::join!(
            read_bounded(&mut stdout, &mut stdout_bytes, max_bytes),
            read_bounded(&mut stderr, &mut stderr_bytes, max_bytes),
        );
        let status = child.wait().await.ok()?;
        if stdout_result.is_err() || stderr_result.is_err() {
            return None;
        }
        Some((stdout_bytes, status.success()))
    }))
    .await;

    if let Ok(Some((stdout_bytes, success))) = captured {
        Some(BoundedOutput {
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            success,
        })
    } else {
        let _ = child.kill().await;
        let _ = child.wait().await;
        None
    }
}

async fn read_bounded<R>(reader: &mut R, buffer: &mut Vec<u8>, max_bytes: usize) -> Result<(), ()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut chunk = [0_u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await.map_err(|_| ())?;
        if read == 0 {
            return Ok(());
        }
        if buffer.len() + read > max_bytes {
            return Err(());
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}
