//! Spawning an engine and speaking JSON lines over its stdio.

use crate::Turn;
use crate::trace::{Kind, Trace};
use anyhow::{Context, Result};
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};

/// Longest stdout line we accept. Exceeding this fails the protocol exchange.
pub const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;
const STDERR_TAIL_BYTES: usize = 8 * 1024;

pub struct EngineProcess {
    child: OwnedChild,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr_tail: Arc<Mutex<String>>,
    line: Vec<u8>,
    oversized: bool,
    trace: Trace,
    _stderr_task: Option<tokio_util::task::AbortOnDropHandle<()>>,
}

/// Starts `turn.binary` with `args` in `turn.cwd`, `HOME` set to the
/// engine home and the launch environment applied. The process dies with
/// the returned value.
pub fn spawn(turn: &Turn, args: &[String]) -> Result<EngineProcess> {
    let mut command = crate::account::engine_command(turn.engine, &turn.binary, &turn.home);
    command
        .args(args)
        .args(&turn.launch.args)
        .current_dir(&turn.cwd)
        .envs(turn.launch.env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let trace = Trace::start(
        turn.trace_dir.as_deref(),
        &format!("{}-{}", turn.engine, turn.thread_id),
    );
    spawn_command(command, trace).with_context(|| {
        format!(
            "starting {} in {}",
            turn.binary.display(),
            turn.cwd.display()
        )
    })
}

pub(crate) fn spawn_probe(command: Command) -> Result<EngineProcess> {
    spawn_command(command, Trace::default())
}

pub(crate) struct OwnedChild {
    child: Box<dyn ChildWrapper>,
    armed: bool,
}
impl std::ops::Deref for OwnedChild {
    type Target = dyn ChildWrapper;
    fn deref(&self) -> &Self::Target {
        self.child.as_ref()
    }
}
impl std::ops::DerefMut for OwnedChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.child.as_mut()
    }
}
impl OwnedChild {
    pub(crate) fn shutdown(&mut self) {
        if self.armed {
            let _ = self.child.start_kill();
            self.armed = false;
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) fn owned_command(command: Command) -> Result<OwnedChild> {
    let mut command = CommandWrap::from(command);
    command.wrap(KillOnDrop);
    #[cfg(unix)]
    command.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(process_wrap::tokio::JobObject);
    command
        .spawn()
        .map(|child| OwnedChild { child, armed: true })
        .context("starting engine process")
}

fn spawn_command(command: Command, trace: Trace) -> Result<EngineProcess> {
    let mut child = owned_command(command)?;
    trace.record(Kind::Note, &format!("started (pid {:?})", child.id()));
    let stdin = child.stdin().take();
    let stdout = BufReader::new(child.stdout().take().context("engine stdout")?);
    let stderr_tail = Arc::new(Mutex::new(String::new()));
    let stderr_task = if let Some(mut stderr) = child.stderr().take() {
        let tail = Arc::clone(&stderr_tail);
        let stderr_trace = trace.clone();
        Some(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
            async move {
                let mut buffer = vec![0_u8; 4096];
                while let Ok(read) = stderr.read(&mut buffer).await {
                    if read == 0 {
                        break;
                    }
                    let text = String::from_utf8_lossy(&buffer[..read]);
                    tracing::debug!(target: "engine_stderr", "{}", text.trim_end());
                    stderr_trace.record(Kind::Stderr, &text);
                    let mut tail = tail
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    tail.push_str(&text);
                    if tail.len() > STDERR_TAIL_BYTES {
                        let mut cut = tail.len() - STDERR_TAIL_BYTES;
                        while !tail.is_char_boundary(cut) {
                            cut += 1;
                        }
                        tail.drain(..cut);
                    }
                }
            },
        )))
    } else {
        None
    };
    Ok(EngineProcess {
        child,
        stdin,
        stdout,
        stderr_tail,
        line: Vec::new(),
        oversized: false,
        trace,
        _stderr_task: stderr_task,
    })
}

impl EngineProcess {
    /// Writes one JSON value and a newline.
    pub async fn send(&mut self, value: &serde_json::Value) -> Result<()> {
        let stdin = self.stdin.as_mut().context("engine stdin is closed")?;
        let mut bytes = serde_json::to_vec(value)?;
        self.trace
            .record(Kind::In, &String::from_utf8_lossy(&bytes));
        bytes.push(b'\n');
        stdin
            .write_all(&bytes)
            .await
            .context("writing to the engine")?;
        stdin.flush().await.context("writing to the engine")?;
        Ok(())
    }

    #[must_use]
    pub fn stdin_open(&self) -> bool {
        self.stdin.is_some()
    }

    /// Closes stdin, which tells line-mode CLIs to finish. `why` goes to
    /// the trace: closing it early is what ends background agents mid-work.
    pub fn close_stdin(&mut self, why: &str) {
        self.trace
            .record(Kind::Note, &format!("closing stdin: {why}"));
        self.stdin = None;
    }

    /// Records a decision about this process in its trace.
    pub fn note(&self, text: &str) {
        self.trace.record(Kind::Note, text);
    }

    /// The next stdout line, or `None` at end of output.
    ///
    /// Cancel-safe: a partly read line stays buffered, so this can sit in a
    /// `select!` beside the run's inbox.
    pub async fn next_line(&mut self) -> Result<Option<String>> {
        loop {
            loop {
                let available = self
                    .stdout
                    .fill_buf()
                    .await
                    .context("reading engine output")?;
                if available.is_empty() {
                    if self.line.is_empty() && !self.oversized {
                        return Ok(None);
                    }
                    break;
                }
                let (chunk, found) = match available.iter().position(|byte| *byte == b'\n') {
                    Some(index) => (&available[..=index], true),
                    None => (available, false),
                };
                let used = chunk.len();
                if !self.oversized && self.line.len() + used <= MAX_LINE_BYTES {
                    self.line.extend_from_slice(chunk);
                } else {
                    self.oversized = true;
                    self.line.clear();
                }
                self.stdout.consume(used);
                if found {
                    break;
                }
            }
            let line = std::mem::take(&mut self.line);
            if std::mem::take(&mut self.oversized) {
                anyhow::bail!("engine output line exceeds {MAX_LINE_BYTES} bytes");
            }
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_end_matches(['\n', '\r']);
            self.trace.record(Kind::Out, text);
            if text.trim().is_empty() {
                continue;
            }
            return Ok(Some(text.to_owned()));
        }
    }

    /// The last few KB the engine wrote to stderr, for error messages.
    #[must_use]
    pub fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .trim()
            .to_owned()
    }

    /// Ends the process (and its group on Unix).
    pub async fn kill(&mut self) {
        self.trace.record(Kind::Note, "killing the process");
        self.child.shutdown();
        let _ = self.child.wait().await;
    }

    /// Waits for the process to exit and returns its exit code.
    pub async fn wait(&mut self) -> Option<i32> {
        let status = self.child.wait().await;
        self.child.shutdown();
        self.trace
            .record(Kind::Note, &format!("exited: {status:?}"));
        status.ok().and_then(|status| status.code())
    }
}

/// Short CLI probe with bounded output and one timeout. Overflow ends the
/// process rather than leaving it blocked on an unread pipe.
pub async fn probe(
    mut command: Command,
    timeout: std::time::Duration,
    limit: usize,
) -> Result<std::process::Output> {
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut process = spawn_probe(command)?;
    let result = tokio::time::timeout(timeout, async {
        let mut stdout = Vec::new();
        (&mut process.stdout)
            .take(limit as u64 + 1)
            .read_to_end(&mut stdout)
            .await?;
        anyhow::ensure!(
            stdout.len() <= limit,
            "engine probe output exceeds {limit} bytes"
        );
        let status = process.child.wait().await?;
        process.child.shutdown();
        if let Some(stderr) = process._stderr_task.take() {
            let _ = stderr.await;
        }
        Ok::<_, anyhow::Error>(std::process::Output {
            stdout,
            stderr: process.stderr_tail().into_bytes(),
            status,
        })
    })
    .await
    .context("engine probe timed out")?;
    if result.is_err() {
        process.kill().await;
    }
    result
}

#[cfg(all(test, unix))]
mod reform_tests {
    use super::*;
    #[tokio::test]
    async fn dropping_the_owner_closes_pipes_held_by_descendants() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 30 & echo ready; wait"])
            .stdout(Stdio::piped());
        let mut child = owned_command(command).unwrap();
        let mut pipe = BufReader::new(child.stdout().take().unwrap());
        let mut ready = String::new();
        pipe.read_line(&mut ready).await.unwrap();
        assert_eq!(ready.trim(), "ready");
        drop(child);
        let mut rest = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            pipe.read_to_end(&mut rest),
        )
        .await
        .expect("descendant inherited stdout must close with the owner")
        .unwrap();
    }

    #[tokio::test]
    async fn oversized_probe_fails_without_waiting_for_a_blocked_pipe() {
        let mut command = Command::new("sh");
        command.args(["-c", "head -c 1048576 /dev/zero; sleep 30"]);
        let result = probe(command, std::time::Duration::from_secs(2), 4096).await;
        assert!(result.unwrap_err().to_string().contains("exceeds"));
    }

    #[tokio::test]
    async fn oversized_engine_line_is_a_protocol_error() {
        let home = tempfile::tempdir().unwrap();
        let turn = Turn {
            thread_id: artisan_api::ThreadId::from("bounded-output"),
            engine: artisan_api::EngineId::Codex,
            binary: "/bin/sh".into(),
            home: home.path().into(),
            cwd: home.path().into(),
            config: artisan_api::EngineConfig::new(artisan_api::EngineId::Codex),
            session_id: None,
            input: crate::UserInput::default(),
            launch: crate::Launch::default(),
            trace_dir: None,
        };
        let mut process = spawn(
            &turn,
            &[
                "-c".into(),
                format!("head -c {} /dev/zero; printf '\\n'", MAX_LINE_BYTES + 1),
            ],
        )
        .unwrap();
        let error = tokio::time::timeout(std::time::Duration::from_secs(5), process.next_line())
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("exceeds"));
    }
}

#[cfg(all(test, windows))]
mod windows_owner_tests {
    use super::*;
    #[tokio::test]
    async fn native_job_drop_closes_pipes_held_by_its_own_descendants() {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", "$child = Start-Process -FilePath $env:ComSpec -ArgumentList '/c ping -n 30 127.0.0.1 >NUL' -NoNewWindow -PassThru; Write-Output 'ready'; Wait-Process -Id $child.Id"])
            .stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = owned_command(command).unwrap();
        let mut pipe = BufReader::new(child.stdout().take().unwrap());
        let mut ready = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            pipe.read_line(&mut ready),
        )
        .await
        .expect("owned native parent must start its descendant")
        .unwrap();
        assert_eq!(ready.trim(), "ready");
        drop(child);
        let mut rest = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            pipe.read_to_end(&mut rest),
        )
        .await
        .expect("native JobObject drop must close descendant stdout")
        .unwrap();
    }
}
