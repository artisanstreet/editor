//! Spawning an engine and speaking JSON lines over its stdio.

use crate::Turn;
use anyhow::{Context, Result};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// Longest stdout line we accept. Longer lines are dropped with a warning
/// (a truncated JSON value can't be parsed anyway).
pub const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;
const STDERR_TAIL_BYTES: usize = 8 * 1024;

pub struct EngineProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr_tail: Arc<Mutex<String>>,
    line: Vec<u8>,
    oversized: bool,
}

/// Starts `turn.binary` with `args` in `turn.cwd`, `HOME` set to the
/// engine home and the launch environment applied. The process dies with
/// the returned value.
pub fn spawn(turn: &Turn, args: &[String]) -> Result<EngineProcess> {
    let mut command = Command::new(&turn.binary);
    command
        .args(args)
        .args(&turn.launch.args)
        .current_dir(&turn.cwd)
        .env("HOME", &turn.home)
        .envs(turn.launch.env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.env("USERPROFILE", &turn.home);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().with_context(|| {
        format!(
            "starting {} in {}",
            turn.binary.display(),
            turn.cwd.display()
        )
    })?;
    let stdin = child.stdin.take();
    let stdout = BufReader::new(child.stdout.take().context("engine stdout")?);
    let stderr_tail = Arc::new(Mutex::new(String::new()));
    if let Some(mut stderr) = child.stderr.take() {
        let tail = Arc::clone(&stderr_tail);
        tokio::spawn(async move {
            let mut buffer = vec![0_u8; 4096];
            while let Ok(read) = stderr.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&buffer[..read]);
                tracing::debug!(target: "engine_stderr", "{}", text.trim_end());
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
        });
    }
    Ok(EngineProcess {
        child,
        stdin,
        stdout,
        stderr_tail,
        line: Vec::new(),
        oversized: false,
    })
}

impl EngineProcess {
    /// Writes one JSON value and a newline.
    pub async fn send(&mut self, value: &serde_json::Value) -> Result<()> {
        let stdin = self.stdin.as_mut().context("engine stdin is closed")?;
        let mut bytes = serde_json::to_vec(value)?;
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

    /// Closes stdin, which tells line-mode CLIs to finish.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
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
                tracing::warn!("dropped an engine output line over {MAX_LINE_BYTES} bytes");
                continue;
            }
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_end_matches(['\n', '\r']);
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
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            // `Child::kill` only reaches the engine itself; its tool
            // processes share its group, so signal the group too.
            let _ = tokio::process::Command::new("kill")
                .args(["-TERM", &format!("-{pid}")])
                .status()
                .await;
        }
        let _ = self.child.kill().await;
    }

    /// Waits for the process to exit and returns its exit code.
    pub async fn wait(&mut self) -> Option<i32> {
        self.child
            .wait()
            .await
            .ok()
            .and_then(|status| status.code())
    }
}
