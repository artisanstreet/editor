//! Standalone parent smoke for the TEST-ONLY engine-owner fixture.
//!
//! One integration test crate independent of `backend/engine_owner`:
//! `std` + pinned `tokio` 1.53.1 + `serde_json` + official `runfiles` only.
//! Six actual children run serially with absolute `15s` hard / `10s` probe +
//! `5s` cleanup deadlines, remaining-capacity reads, and one common
//! `CaseState` + finalizer. No detached task, no raw diagnostic.

use std::net::SocketAddr;
use std::process::Stdio;
use std::time::Duration;

use runfiles::{Runfiles, rlocation};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::time::Instant;

const FIXTURE_ENV: &str = "ARTISAN_ENGINE_OWNER_FIXTURE";
const SCENARIO_ENV: &str = "ARTISAN_ENGINE_OWNER_TEST_SCENARIO";
const AUTH_ENV: &str = "ARTISAN_ENGINE_OWNER_TEST_AUTHORIZATION";

const EXPECTED_VERSION: &str = "0.0.0-fixture";
const INCOMPATIBLE_VERSION: &str = "0.0.0-fixture-incompatible";
const SYNTHETIC_AUTH: &str = "Basic c3ludGhldGljLWF1dGh6LXRlc3QtdG9rZW4=";

const READINESS_LIMIT: usize = 256;
const READINESS_OVERFLOW_PRE: usize = 257;
const READINESS_CAPTURE_MAX: usize = 258;
const HEADER_CAP: usize = 4096;
const BODY_CAP: usize = 512;
const BODY_SENTINEL: usize = BODY_CAP + 1;
const STDERR_CAP: usize = 64;

const HARD_SECS: u64 = 15;
const PROBE_SECS: u64 = 10;
const GRACE_SECS: u64 = 1;

const WATCHDOG_EXIT: i32 = 99;
const LIFELINE_EXIT: i32 = 3;
const ABRUPT_EXIT: i32 = 7;

fn resolved_fixture_program() -> std::path::PathBuf {
    let mapping = std::env::var(FIXTURE_ENV)
        .unwrap_or_else(|_| panic!("test env {FIXTURE_ENV} must be set via rlocationpath"));
    let runfiles = Runfiles::create().expect("official runfiles discovery should succeed");
    rlocation!(runfiles, mapping.as_str())
        .unwrap_or_else(|| panic!("declared fixture artifact must resolve: {mapping}"))
}

fn build_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build")
}

fn find_crlfcrlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

struct CaseState {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    pid: u32,
    stdout_buf: Vec<u8>,
    stderr_buf: Vec<u8>,
    stdout_eof: bool,
    stderr_eof: bool,
    exit_code: Option<i32>,
    killed: bool,
    hard_deadline: Instant,
    probe_deadline: Instant,
}

impl CaseState {
    fn spawn(scenario: &str, hard_deadline: Instant) -> Self {
        let probe_deadline = hard_deadline
            .checked_sub(Duration::from_secs(HARD_SECS - PROBE_SECS))
            .unwrap_or(hard_deadline);
        let program = resolved_fixture_program();
        let stdout_buf = Vec::with_capacity(READINESS_CAPTURE_MAX);
        let stderr_buf = Vec::with_capacity(STDERR_CAP);
        let mut cmd = tokio::process::Command::new(program);
        cmd.env_clear();
        if let Ok(v) = std::env::var("SYSTEMROOT") {
            cmd.env("SYSTEMROOT", v);
        }
        cmd.env(SCENARIO_ENV, scenario);
        cmd.env(AUTH_ENV, SYNTHETIC_AUTH);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().expect("spawn fixture should succeed");
        let pid = child.id().unwrap_or(0);
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        Self {
            child,
            stdin,
            stdout,
            stderr,
            pid,
            stdout_buf,
            stderr_buf,
            stdout_eof: false,
            stderr_eof: false,
            exit_code: None,
            killed: false,
            hard_deadline,
            probe_deadline,
        }
    }

    fn check_still_alive(&mut self, stage: &str) -> Result<(), String> {
        match self.child.try_wait() {
            Ok(None) => Ok(()),
            Ok(Some(s)) => {
                let code = s.code().unwrap_or(-1);
                self.exit_code = Some(code);
                Err(format!(
                    "stage={stage} early exit code={code} pid={}",
                    self.pid
                ))
            }
            Err(e) => Err(format!(
                "stage={stage} try_wait kind={} pid={}",
                e.kind(),
                self.pid
            )),
        }
    }

    async fn read_readiness(&mut self) -> Result<String, String> {
        let deadline = self.probe_deadline;
        let out = self
            .stdout
            .as_mut()
            .ok_or_else(|| format!("stage=readiness stdout missing pid={}", self.pid))?;
        let mut tmp = [0_u8; 64];
        loop {
            if Instant::now() >= deadline {
                return Err(format!("stage=readiness timeout pid={}", self.pid));
            }
            if self.stdout_buf.contains(&b'\n') {
                break;
            }
            let remaining = READINESS_CAPTURE_MAX.saturating_sub(self.stdout_buf.len());
            if remaining == 0 {
                break;
            }
            let want = remaining.min(tmp.len());
            let n = tokio::time::timeout_at(deadline, out.read(&mut tmp[..want]))
                .await
                .map_err(|_| format!("stage=readiness timeout pid={}", self.pid))?
                .map_err(|e| format!("stage=readiness read kind={} pid={}", e.kind(), self.pid))?;
            if n == 0 {
                self.stdout_eof = true;
                break;
            }
            self.stdout_buf.extend_from_slice(&tmp[..n]);
            if self.stdout_buf.len() > READINESS_CAPTURE_MAX {
                return Err(format!("stage=readiness over capture pid={}", self.pid));
            }
        }
        let lf_pos = self.stdout_buf.iter().position(|b| *b == b'\n');
        let has_lf = lf_pos.is_some();
        if !has_lf {
            return Err(format!("stage=readiness missing LF pid={}", self.pid));
        }
        let pos = lf_pos.expect("has lf");
        if self.stdout_buf.len() > pos + 1 {
            return Err(format!("stage=readiness trailing bytes pid={}", self.pid));
        }
        let pre_len = pos;
        if pre_len > READINESS_LIMIT {
            return Err(format!("stage=readiness overflow pid={}", self.pid));
        }
        let text = std::str::from_utf8(&self.stdout_buf[..pre_len])
            .map_err(|_| format!("stage=readiness not utf8 pid={}", self.pid))?;
        Ok(text.to_owned())
    }

    async fn read_stderr_r(&mut self) -> Result<(), String> {
        let deadline = self.probe_deadline;
        let err = self
            .stderr
            .as_mut()
            .ok_or_else(|| format!("stage=hang stderr missing pid={}", self.pid))?;
        let mut tmp = [0_u8; 16];
        loop {
            if Instant::now() >= deadline {
                return Err(format!("stage=hang timeout pid={}", self.pid));
            }
            let remaining = STDERR_CAP.saturating_sub(self.stderr_buf.len());
            if remaining == 0 {
                return Err(format!("stage=hang stderr over cap pid={}", self.pid));
            }
            let want = remaining.min(tmp.len());
            let n = tokio::time::timeout_at(deadline, err.read(&mut tmp[..want]))
                .await
                .map_err(|_| format!("stage=hang timeout pid={}", self.pid))?
                .map_err(|e| format!("stage=hang read kind={} pid={}", e.kind(), self.pid))?;
            if n == 0 {
                self.stderr_eof = true;
                break;
            }
            self.stderr_buf.extend_from_slice(&tmp[..n]);
            if !self.stderr_buf.is_empty() {
                break;
            }
        }
        if self.stderr_buf != b"R" {
            return Err(format!(
                "stage=hang expected R got len={} pid={}",
                self.stderr_buf.len(),
                self.pid
            ));
        }
        Ok(())
    }

    async fn drain_pipes(&mut self) -> Result<(), String> {
        let stdout_err = drain_one(
            &mut self.stdout,
            &mut self.stdout_buf,
            &mut self.stdout_eof,
            READINESS_CAPTURE_MAX,
            self.hard_deadline,
            self.pid,
            "stdout",
        )
        .await;
        let stderr_err = drain_one(
            &mut self.stderr,
            &mut self.stderr_buf,
            &mut self.stderr_eof,
            STDERR_CAP,
            self.hard_deadline,
            self.pid,
            "stderr",
        )
        .await;
        match (stdout_err, stderr_err) {
            (None, None) => Ok(()),
            (Some(e), None) | (None, Some(e)) => Err(e),
            (Some(e1), Some(e2)) => Err(format!("{e1}; {e2}")),
        }
    }
}

fn classify_url(text: &str, pid: u32) -> Result<SocketAddr, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|_| format!("stage=readiness not json pid={pid}"))?;
    let url = v
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("stage=readiness missing url pid={pid}"))?;
    if !url.starts_with("http://127.0.0.1:") {
        return Err(format!("stage=readiness url not loopback pid={pid}"));
    }
    let port_str = url
        .strip_prefix("http://127.0.0.1:")
        .expect("prefix checked");
    if port_str.contains('/')
        || port_str.contains('?')
        || port_str.contains('#')
        || port_str.contains('@')
    {
        return Err(format!("stage=readiness url has path/cred pid={pid}"));
    }
    let port: u16 = port_str
        .parse()
        .map_err(|_| format!("stage=readiness port not u16 pid={pid}"))?;
    if port == 0 {
        return Err(format!("stage=readiness port zero pid={pid}"));
    }
    Ok(SocketAddr::from(([127, 0, 0, 1], port)))
}

async fn read_headers_and_prefix(
    stream: &mut TcpStream,
    deadline: Instant,
    pid: u32,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut buf: Vec<u8> = Vec::with_capacity(HEADER_CAP + 1);
    let mut tmp = [0_u8; 512];
    loop {
        if Instant::now() >= deadline {
            return Err(format!("stage=health header timeout pid={pid}"));
        }
        if let Some(pos) = find_crlfcrlf(&buf) {
            if pos + 4 > HEADER_CAP {
                return Err(format!("stage=health header beyond cap pid={pid}"));
            }
            let header = buf[..pos + 4].to_vec();
            let prefix = buf[pos + 4..].to_vec();
            if prefix.len() > BODY_SENTINEL {
                return Err(format!("stage=health prefix over sentinel pid={pid}"));
            }
            return Ok((header, prefix));
        }
        if buf.len() > HEADER_CAP {
            return Err(format!("stage=health header over cap pid={pid}"));
        }
        let remaining = (HEADER_CAP + 1).saturating_sub(buf.len());
        if remaining == 0 {
            return Err(format!("stage=health header over cap pid={pid}"));
        }
        let want = remaining.min(tmp.len());
        let n = tokio::time::timeout_at(deadline, stream.read(&mut tmp[..want]))
            .await
            .map_err(|_| format!("stage=health header timeout pid={pid}"))?
            .map_err(|e| format!("stage=health header read kind={} pid={}", e.kind(), pid))?;
        if n == 0 {
            return Err(format!("stage=health header EOF pid={pid}"));
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > HEADER_CAP + 1 {
            return Err(format!("stage=health header over cap pid={pid}"));
        }
    }
}

fn parse_headers(header: &[u8], pid: u32) -> Result<(usize, bool, bool), String> {
    let text = std::str::from_utf8(header)
        .map_err(|_| format!("stage=health header not utf8 pid={pid}"))?;
    let mut lines = text.split("\r\n");
    let status = lines
        .next()
        .ok_or_else(|| format!("stage=health missing status pid={pid}"))?;
    if status != "HTTP/1.1 200 OK" {
        return Err(format!("stage=health status not 200 pid={pid}"));
    }
    let mut len: Option<usize> = None;
    let mut ctype = false;
    let mut close = false;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let colon = line
            .find(':')
            .ok_or_else(|| format!("stage=health header missing colon pid={pid}"))?;
        let name = line[..colon].trim().to_ascii_lowercase();
        let value = line[colon + 1..].trim();
        if name == "content-length" {
            if len.is_some() {
                return Err(format!("stage=health duplicate length pid={pid}"));
            }
            let v: usize = value
                .parse()
                .map_err(|_| format!("stage=health length not numeric pid={pid}"))?;
            len = Some(v);
        } else if name == "content-type" && value.eq_ignore_ascii_case("application/json") {
            ctype = true;
        } else if name == "connection" && value.eq_ignore_ascii_case("close") {
            close = true;
        }
    }
    if !ctype {
        return Err(format!("stage=health content-type not json pid={pid}"));
    }
    if !close {
        return Err(format!("stage=health connection not close pid={pid}"));
    }
    let declared = len.ok_or_else(|| format!("stage=health missing length pid={pid}"))?;
    if declared > BODY_CAP {
        return Err(format!(
            "stage=health length over cap {declared}>{BODY_CAP} pid={pid}"
        ));
    }
    Ok((declared, ctype, close))
}

async fn read_body_and_eof(
    stream: &mut TcpStream,
    mut body: Vec<u8>,
    declared: usize,
    deadline: Instant,
    pid: u32,
) -> Result<Vec<u8>, String> {
    if body.len() > declared {
        return Err(format!("stage=health body prefix over declared pid={pid}"));
    }
    let mut tmp = [0_u8; 512];
    while body.len() < declared {
        if Instant::now() >= deadline {
            return Err(format!("stage=health body timeout pid={pid}"));
        }
        let remaining = declared.saturating_sub(body.len());
        let want = remaining.min(tmp.len());
        let n = tokio::time::timeout_at(deadline, stream.read(&mut tmp[..want]))
            .await
            .map_err(|_| format!("stage=health body timeout pid={pid}"))?
            .map_err(|e| format!("stage=health body read kind={} pid={}", e.kind(), pid))?;
        if n == 0 {
            return Err(format!("stage=health body EOF pid={pid}"));
        }
        body.extend_from_slice(&tmp[..n]);
    }
    if body.len() != declared {
        return Err(format!("stage=health body length mismatch pid={pid}"));
    }
    if Instant::now() >= deadline {
        return Err(format!("stage=health eof timeout pid={pid}"));
    }
    let mut eof_tmp = [0_u8; 1];
    match tokio::time::timeout_at(deadline, stream.read(&mut eof_tmp)).await {
        Ok(Ok(0)) => Ok(body),
        Ok(Ok(_)) => Err(format!("stage=health expected EOF got bytes pid={pid}")),
        Ok(Err(e)) => Err(format!("stage=health eof kind={} pid={}", e.kind(), pid)),
        Err(_) => Err(format!("stage=health eof timeout pid={pid}")),
    }
}

async fn probe_health(
    addr: SocketAddr,
    auth: &str,
    pid: u32,
    deadline: Instant,
) -> Result<String, String> {
    let mut stream = tokio::time::timeout_at(deadline, TcpStream::connect(addr))
        .await
        .map_err(|_| format!("stage=health connect timeout pid={pid}"))?
        .map_err(|e| format!("stage=health connect kind={} pid={}", e.kind(), pid))?;
    let req = format!(
        "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: {auth}\r\nConnection: close\r\n\r\n"
    );
    tokio::time::timeout_at(deadline, stream.write_all(req.as_bytes()))
        .await
        .map_err(|_| format!("stage=health write timeout pid={pid}"))?
        .map_err(|e| format!("stage=health write kind={} pid={}", e.kind(), pid))?;
    tokio::time::timeout_at(deadline, stream.flush())
        .await
        .map_err(|_| format!("stage=health flush timeout pid={pid}"))?
        .map_err(|e| format!("stage=health flush kind={} pid={}", e.kind(), pid))?;
    let (header, prefix) = read_headers_and_prefix(&mut stream, deadline, pid).await?;
    let (declared, _, _) = parse_headers(&header, pid)?;
    let body = read_body_and_eof(&mut stream, prefix, declared, deadline, pid).await?;
    let text =
        std::str::from_utf8(&body).map_err(|_| format!("stage=health body not utf8 pid={pid}"))?;
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|_| format!("stage=health body not json pid={pid}"))?;
    let healthy = v
        .get("healthy")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| format!("stage=health missing healthy pid={pid}"))?;
    if !healthy {
        return Err(format!("stage=health healthy false pid={pid}"));
    }
    let got_pid = v
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("stage=health missing pid pid={pid}"))?;
    if got_pid != u64::from(pid) {
        return Err(format!("stage=health pid mismatch pid={pid}"));
    }
    let version = v
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("stage=health missing version pid={pid}"))?
        .to_owned();
    Ok(version)
}

fn join_wait_errors(natural: Option<String>, kill: Option<String>, tail: String) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(e) = natural {
        parts.push(e);
    }
    if let Some(k) = kill {
        parts.push(k);
    }
    parts.push(tail);
    parts.join("; ")
}

async fn drain_one<R>(
    stream: &mut Option<R>,
    buf: &mut Vec<u8>,
    eof: &mut bool,
    cap: usize,
    deadline: Instant,
    pid: u32,
    label: &str,
) -> Option<String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(s) = stream.as_mut() else {
        return Some(format!("stage=drain {label} missing pipe pid={pid}"));
    };
    let mut tmp = [0_u8; 64];
    while !*eof {
        if Instant::now() >= deadline {
            return Some(format!("stage=drain {label} timeout pid={pid}"));
        }
        if buf.len() >= cap {
            let mut probe = [0_u8; 1];
            match tokio::time::timeout_at(deadline, s.read(&mut probe)).await {
                Ok(Ok(0)) => {
                    *eof = true;
                    break;
                }
                Ok(Ok(_)) => return Some(format!("stage=drain {label} overflow pid={pid}")),
                Ok(Err(e)) => {
                    return Some(format!("stage=drain {label} kind={} pid={}", e.kind(), pid));
                }
                Err(_) => return Some(format!("stage=drain {label} timeout pid={pid}")),
            }
        }
        let remaining = cap.saturating_sub(buf.len());
        let want = remaining.min(tmp.len());
        match tokio::time::timeout_at(deadline, s.read(&mut tmp[..want])).await {
            Ok(Ok(0)) => {
                *eof = true;
                break;
            }
            Ok(Ok(n)) => buf.extend_from_slice(&tmp[..n]),
            Ok(Err(e)) => {
                return Some(format!("stage=drain {label} kind={} pid={}", e.kind(), pid));
            }
            Err(_) => return Some(format!("stage=drain {label} timeout pid={pid}")),
        }
    }
    None
}

async fn wait_grace(state: &mut CaseState) -> Result<i32, String> {
    let grace = std::cmp::min(
        state.hard_deadline,
        Instant::now() + Duration::from_secs(GRACE_SECS),
    );
    let natural_err = if Instant::now() < grace {
        match tokio::time::timeout_at(grace, state.child.wait()).await {
            Ok(Ok(s)) => {
                let code = s.code().unwrap_or(-1);
                state.exit_code = Some(code);
                return Ok(code);
            }
            Ok(Err(e)) => Some(format!("stage=wait kind={} pid={}", e.kind(), state.pid)),
            Err(_) => Some(format!("stage=wait grace timeout pid={}", state.pid)),
        }
    } else {
        Some(format!("stage=wait grace expired pid={}", state.pid))
    };
    let kill_res = state.child.start_kill();
    let kill_err = kill_res
        .err()
        .map(|e| format!("stage=kill kind={} pid={}", e.kind(), state.pid));
    state.killed = true;
    if Instant::now() >= state.hard_deadline {
        return Err(join_wait_errors(
            natural_err,
            kill_err,
            format!("stage=wait hard deadline pid={}", state.pid),
        ));
    }
    match tokio::time::timeout_at(state.hard_deadline, state.child.wait()).await {
        Ok(Ok(s)) => {
            let code = s.code().unwrap_or(-1);
            state.exit_code = Some(code);
            Err(join_wait_errors(
                natural_err,
                kill_err,
                format!("stage=wait needed kill pid={}", state.pid),
            ))
        }
        Ok(Err(e)) => Err(join_wait_errors(
            natural_err,
            kill_err,
            format!("stage=wait after kill kind={} pid={}", e.kind(), state.pid),
        )),
        Err(_) => Err(join_wait_errors(
            natural_err,
            kill_err,
            format!("stage=wait hard deadline pid={}", state.pid),
        )),
    }
}

fn validate_outputs(
    state: &CaseState,
    expected_stdout: &[u8],
    expected_stderr: &[u8],
    scenario: &str,
) -> Result<(), String> {
    if !state.stdout_eof {
        return Err(format!("stage={scenario} stdout not EOF pid={}", state.pid));
    }
    if !state.stderr_eof {
        return Err(format!("stage={scenario} stderr not EOF pid={}", state.pid));
    }
    if state.stdout_buf != expected_stdout {
        return Err(format!(
            "stage={scenario} stdout mismatch pid={}",
            state.pid
        ));
    }
    if state.stderr_buf != expected_stderr {
        return Err(format!(
            "stage={scenario} stderr mismatch pid={}",
            state.pid
        ));
    }
    if scenario == "hang_until_lifeline" {
        if state.stderr_buf != b"R" {
            return Err(format!("stage={scenario} stderr not R pid={}", state.pid));
        }
        if !state.stdout_buf.is_empty() {
            return Err(format!(
                "stage={scenario} unexpected stdout pid={}",
                state.pid
            ));
        }
    } else if scenario == "abrupt_child_exit_nonzero" {
        if !state.stdout_buf.is_empty() || !state.stderr_buf.is_empty() {
            return Err(format!(
                "stage={scenario} unexpected output pid={}",
                state.pid
            ));
        }
    } else if !state.stderr_buf.is_empty() {
        return Err(format!(
            "stage={scenario} unexpected stderr pid={}",
            state.pid
        ));
    }
    Ok(())
}

async fn finalize_case(
    state: &mut CaseState,
    scenario: &str,
    expected: i32,
    is_abrupt: bool,
    primary: Result<(), String>,
) -> Result<(), String> {
    let mut primary_err = primary.err();
    if is_abrupt && state.exit_code.is_none() {
        match tokio::time::timeout_at(state.probe_deadline, state.child.wait()).await {
            Ok(Ok(s)) => {
                let code = s.code().unwrap_or(-1);
                state.exit_code = Some(code);
                if code == WATCHDOG_EXIT {
                    let msg = format!("stage={scenario} watchdog pid={}", state.pid);
                    primary_err = Some(match primary_err {
                        Some(e) => format!("{e}; {msg}"),
                        None => msg,
                    });
                } else if code != expected {
                    let msg = format!(
                        "stage={scenario} expected {expected} got {code} pid={}",
                        state.pid
                    );
                    primary_err = Some(match primary_err {
                        Some(e) => format!("{e}; {msg}"),
                        None => msg,
                    });
                }
            }
            Ok(Err(e)) => {
                let msg = format!("stage={scenario} wait kind={} pid={}", e.kind(), state.pid);
                primary_err = Some(match primary_err {
                    Some(pe) => format!("{pe}; {msg}"),
                    None => msg,
                });
            }
            Err(_) => {
                let msg = format!("stage={scenario} wait timeout pid={}", state.pid);
                primary_err = Some(match primary_err {
                    Some(pe) => format!("{pe}; {msg}"),
                    None => msg,
                });
            }
        }
    }
    let expected_stdout = state.stdout_buf.clone();
    let expected_stderr = state.stderr_buf.clone();
    drop(state.stdin.take());
    let mut wait_err: Option<String> = None;
    if state.exit_code.is_none() {
        match wait_grace(state).await {
            Ok(code) => {
                if code == WATCHDOG_EXIT {
                    wait_err = Some(format!("stage={scenario} watchdog pid={}", state.pid));
                } else if code != expected {
                    wait_err = Some(format!(
                        "stage={scenario} expected {expected} got {code} pid={}",
                        state.pid
                    ));
                } else if state.killed {
                    wait_err = Some(format!(
                        "stage={scenario} unexpected kill pid={}",
                        state.pid
                    ));
                }
            }
            Err(e) => wait_err = Some(e),
        }
    } else {
        let code = state.exit_code.unwrap_or(-1);
        if code == WATCHDOG_EXIT {
            wait_err = Some(format!("stage={scenario} watchdog pid={}", state.pid));
        }
        if state.killed {
            let msg = format!("stage={scenario} unexpected kill pid={}", state.pid);
            wait_err = Some(match wait_err {
                Some(e) => format!("{e}; {msg}"),
                None => msg,
            });
        }
    }
    let drain_err = state.drain_pipes().await.err();
    let output_err = validate_outputs(state, &expected_stdout, &expected_stderr, scenario).err();
    let mut combined: Option<String> = None;
    for err in [primary_err, wait_err, drain_err, output_err]
        .into_iter()
        .flatten()
    {
        combined = Some(match combined {
            Some(c) => format!("{c}; {err}"),
            None => err,
        });
    }
    match combined {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

async fn run_ready_ok(state: &mut CaseState) -> Result<(), String> {
    let text = state.read_readiness().await?;
    if text.len() > READINESS_LIMIT {
        return Err(format!("stage=ready_ok overflow pid={}", state.pid));
    }
    let addr = classify_url(&text, state.pid)?;
    let version = probe_health(addr, SYNTHETIC_AUTH, state.pid, state.probe_deadline).await?;
    if version != EXPECTED_VERSION {
        return Err(format!("stage=ready_ok version mismatch pid={}", state.pid));
    }
    state.check_still_alive("ready_ok")?;
    Ok(())
}

async fn run_ready_malformed(state: &mut CaseState) -> Result<(), String> {
    let text = state.read_readiness().await?;
    if text.len() > READINESS_LIMIT {
        return Err(format!("stage=ready_malformed overflow pid={}", state.pid));
    }
    if serde_json::from_str::<serde_json::Value>(&text).is_ok() {
        return Err(format!(
            "stage=ready_malformed expected invalid pid={}",
            state.pid
        ));
    }
    state.check_still_alive("ready_malformed")?;
    Ok(())
}

async fn run_oversized(state: &mut CaseState) -> Result<(), String> {
    // Read raw readiness without classification to check overflow handling.
    let deadline = state.probe_deadline;
    let out = state
        .stdout
        .as_mut()
        .ok_or_else(|| format!("stage=oversized stdout missing pid={}", state.pid))?;
    let mut tmp = [0_u8; 64];
    while !state.stdout_buf.contains(&b'\n') {
        if Instant::now() >= deadline {
            return Err(format!("stage=oversized timeout pid={}", state.pid));
        }
        let remaining = READINESS_CAPTURE_MAX.saturating_sub(state.stdout_buf.len());
        if remaining == 0 {
            break;
        }
        let want = remaining.min(tmp.len());
        let n = tokio::time::timeout_at(deadline, out.read(&mut tmp[..want]))
            .await
            .map_err(|_| format!("stage=oversized timeout pid={}", state.pid))?
            .map_err(|e| format!("stage=oversized read kind={} pid={}", e.kind(), state.pid))?;
        if n == 0 {
            state.stdout_eof = true;
            break;
        }
        state.stdout_buf.extend_from_slice(&tmp[..n]);
    }
    let pos = state
        .stdout_buf
        .iter()
        .position(|b| *b == b'\n')
        .ok_or_else(|| format!("stage=oversized missing LF pid={}", state.pid))?;
    if state.stdout_buf.len() != READINESS_CAPTURE_MAX {
        return Err(format!("stage=oversized capture not 258 pid={}", state.pid));
    }
    if pos != READINESS_OVERFLOW_PRE {
        return Err(format!("stage=oversized pre_len not 257 pid={}", state.pid));
    }
    if state.stdout_buf.len() > pos + 1 {
        return Err(format!("stage=oversized trailing pid={}", state.pid));
    }
    let text = std::str::from_utf8(&state.stdout_buf[..pos])
        .map_err(|_| format!("stage=oversized not utf8 pid={}", state.pid))?;
    let v: serde_json::Value = serde_json::from_str(text.trim())
        .map_err(|_| format!("stage=oversized json invalid pid={}", state.pid))?;
    if v.get("url").is_none() {
        return Err(format!("stage=oversized missing url pid={}", state.pid));
    }
    let base = r#"{"url":"http://127.0.0.1:1"}"#;
    if !text.starts_with(base) {
        return Err(format!("stage=oversized base mismatch pid={}", state.pid));
    }
    if !text[base.len()..].chars().all(|c| c == ' ') {
        return Err(format!(
            "stage=oversized padding not space pid={}",
            state.pid
        ));
    }
    state.check_still_alive("oversized")?;
    Ok(())
}

async fn run_health_version(state: &mut CaseState) -> Result<(), String> {
    let text = state.read_readiness().await?;
    if text.len() > READINESS_LIMIT {
        return Err(format!("stage=health_version overflow pid={}", state.pid));
    }
    let addr = classify_url(&text, state.pid)?;
    let version = probe_health(addr, SYNTHETIC_AUTH, state.pid, state.probe_deadline).await?;
    if version != INCOMPATIBLE_VERSION {
        return Err(format!(
            "stage=health_version expected incompatible pid={}",
            state.pid
        ));
    }
    state.check_still_alive("health_version")?;
    Ok(())
}

async fn run_hang(state: &mut CaseState) -> Result<(), String> {
    state.read_stderr_r().await?;
    // No timed absence check; R is causal. Final drain will prove stdout empty.
    state.check_still_alive("hang")?;
    Ok(())
}

fn run_abrupt(state: &mut CaseState) -> Result<(), String> {
    if state.stdin.is_none() {
        return Err(format!("stage=abrupt stdin not retained pid={}", state.pid));
    }
    // Do not use timed absence as proof; finalizer will verify totals after wait.
    Ok(())
}

fn emit_record(scenario: &str, state: &CaseState) {
    let exit = state.exit_code.unwrap_or(-1);
    let stdout_bytes = state.stdout_buf.len();
    let stderr_bytes = state.stderr_buf.len();
    let reaped = state.exit_code.is_some();
    let killed = state.killed;
    eprintln!(
        "record scenario={scenario} pid={} exit={exit} reaped={reaped} killed={killed} stdout_bytes={stdout_bytes} stderr_bytes={stderr_bytes}",
        state.pid
    );
}

fn ensure_spawned(state: &CaseState) -> Result<(), String> {
    if state.pid == 0 {
        return Err("stage=spawn pid zero".to_string());
    }
    if state.stdin.is_none() {
        return Err(format!("stage=spawn stdin missing pid={}", state.pid));
    }
    if state.stdout.is_none() {
        return Err(format!("stage=spawn stdout missing pid={}", state.pid));
    }
    if state.stderr.is_none() {
        return Err(format!("stage=spawn stderr missing pid={}", state.pid));
    }
    Ok(())
}

#[test]
fn engine_owner_fixture_parent_smoke() {
    let runtime = build_runtime();
    let cases: &[(&str, u8, i32, bool)] = &[
        ("ready_ok", 0, LIFELINE_EXIT, false),
        ("ready_malformed", 1, LIFELINE_EXIT, false),
        ("ready_oversized_bounded_reject", 2, LIFELINE_EXIT, false),
        ("health_version_reject", 3, LIFELINE_EXIT, false),
        ("hang_until_lifeline", 4, LIFELINE_EXIT, false),
        ("abrupt_child_exit_nonzero", 5, ABRUPT_EXIT, true),
    ];
    for (scenario, kind, expected, is_abrupt) in cases.iter().copied() {
        let hard_deadline = Instant::now() + Duration::from_secs(HARD_SECS);
        let mut state = {
            let _enter = runtime.enter();
            CaseState::spawn(scenario, hard_deadline)
        };
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runtime.block_on(async {
                if let Err(e) = ensure_spawned(&state) {
                    return (Err(e), scenario, expected, is_abrupt);
                }
                let res = match kind {
                    0 => run_ready_ok(&mut state).await,
                    1 => run_ready_malformed(&mut state).await,
                    2 => run_oversized(&mut state).await,
                    3 => run_health_version(&mut state).await,
                    4 => run_hang(&mut state).await,
                    5 => run_abrupt(&mut state),
                    _ => panic!("unknown kind"),
                };
                (res, scenario, expected, is_abrupt)
            })
        }));
        let (primary, sc, exp, abrupt, payload) = match caught {
            Ok(v) => (v.0, v.1, v.2, v.3, None),
            Err(p) => (
                Err("panic".to_string()),
                scenario,
                expected,
                is_abrupt,
                Some(p),
            ),
        };
        let fin = runtime.block_on(finalize_case(&mut state, sc, exp, abrupt, primary));
        if let Some(p) = payload {
            if let Err(e) = &fin {
                eprintln!(
                    "stage={scenario} panic cleanup failed pid={} err={e}",
                    state.pid
                );
            }
            std::panic::resume_unwind(p);
        }
        match fin {
            Ok(()) => emit_record(scenario, &state),
            Err(e) => panic!("scenario {scenario} pid={} failed: {e}", state.pid),
        }
        assert!(
            state.exit_code.is_some(),
            "scenario {scenario} pid={} missing exit",
            state.pid
        );
        assert!(
            state.exit_code.unwrap_or(-1) != WATCHDOG_EXIT,
            "scenario {scenario} pid={} watchdog",
            state.pid
        );
        assert!(
            !state.killed,
            "scenario {scenario} pid={} unexpected kill",
            state.pid
        );
        if !abrupt {
            assert!(
                state.stdout_eof,
                "scenario {scenario} pid={} stdout not EOF",
                state.pid
            );
            assert!(
                state.stderr_eof || state.stderr_buf.is_empty(),
                "scenario {scenario} pid={} stderr not EOF",
                state.pid
            );
        }
    }
}
