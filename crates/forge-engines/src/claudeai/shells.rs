//! Background shell commands for the claude.ai transport.
//!
//! Claude Code keeps a background command running between tool rounds and
//! lets the model read its output with `BashOutput` and stop it with
//! `KillShell`. The claude.ai transport has no engine process to hold them,
//! so this module does: each command runs in its own process group, its
//! unread output is buffered, and the run's cancellation ends every shell
//! it started.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use encoding_rs::{DecoderResult, Encoding};
use process_wrap::tokio::ChildWrapper;
use tokio::io::AsyncReadExt as _;
use tokio_util::sync::CancellationToken;

/// The most unread output one shell keeps: past it, the oldest goes.
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
/// How much of a pipe is read at once.
const READ_CHUNK: usize = 16 * 1024;

/// The last shell id handed out, across every run in the process, so an id
/// from an earlier turn never reads another command's output.
static LAST_ID: AtomicUsize = AtomicUsize::new(0);

/// The background shells one run started.
#[derive(Default)]
pub struct Shells {
    shells: Mutex<HashMap<String, Arc<Shell>>>,
}

struct Shell {
    state: Mutex<ShellState>,
    cancel: CancellationToken,
}

#[derive(Default)]
struct ShellState {
    /// Output the model has not been shown yet: the newest of it, bounded.
    unread: String,
    /// Bytes of unread output dropped for the bound since the last read.
    dropped: usize,
    ended: Option<Ended>,
}

/// How a shell stopped.
#[derive(Clone, Copy)]
enum Ended {
    /// It exited with this code.
    Code(i32),
    /// It ended without a code (a signal).
    WithoutCode,
    /// The user or the run stopped it.
    Stopped,
}

impl Shells {
    /// Starts `command` in `cwd` and returns the id the model uses to read
    /// or stop it.
    pub fn start(&self, command: &str, cwd: &Path) -> Result<String, String> {
        let mut shell = if cfg!(windows) {
            let mut shell = tokio::process::Command::new("cmd");
            shell.arg("/C").arg(command);
            shell
        } else {
            let mut shell = tokio::process::Command::new("sh");
            shell.arg("-c").arg(command);
            shell
        };
        shell
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = crate::process::owned_command(shell)
            .map_err(|error| format!("The command could not start: {error:#}"))?;
        let stdout = child.stdout().take();
        let stderr = child.stderr().take();
        let id = format!("bash-{}", LAST_ID.fetch_add(1, Ordering::Relaxed) + 1);
        let shell = Arc::new(Shell {
            state: Mutex::new(ShellState::default()),
            cancel: CancellationToken::new(),
        });
        self.shells
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.clone(), Arc::clone(&shell));
        tokio::spawn(async move {
            let legacy = legacy_encoding().await;
            // The shell's exit is recorded when the shell exits, not when
            // its pipes close: a command it left running (`server &`) holds
            // them open. Its output is read on until then.
            let watch = async {
                let ended = tokio::select! {
                    status = leader(&mut *child).wait() => status.ok().and_then(|status| status.code()).map_or(Ended::WithoutCode, Ended::Code),
                    () = shell.cancel.cancelled() => {
                        if let Err(error) = child.start_kill() {
                            tracing::debug!(%error, "stopping a background shell");
                        }
                        Ended::Stopped
                    }
                };
                shell.lock().ended = Some(ended);
            };
            tokio::join!(
                watch,
                pump(stdout, &shell, legacy),
                pump(stderr, &shell, legacy)
            );
            // Dropping the child ends whatever the command left running in
            // its process group (its job object on Windows).
            drop(child);
        });
        Ok(id)
    }

    /// The shell's new output since the last read, and where it stands.
    pub fn read(&self, id: &str) -> Result<String, String> {
        let shell = self.find(id)?;
        let mut state = shell.lock();
        let mut out = String::new();
        if state.dropped > 0 {
            writeln!(
                out,
                "[{} bytes of output were dropped before they were read; the newest follows]",
                state.dropped
            )
            .ok();
            state.dropped = 0;
        }
        out.push_str(&std::mem::take(&mut state.unread));
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        match state.ended {
            Some(Ended::Code(code)) => {
                write!(out, "[exit code {code}]").ok();
            }
            Some(Ended::Stopped) => out.push_str("[stopped]"),
            Some(Ended::WithoutCode) => out.push_str("[ended without an exit code]"),
            None => out.push_str("[still running]"),
        }
        drop(state);
        Ok(out)
    }

    /// Stops a shell and everything it started.
    pub fn kill(&self, id: &str) -> Result<String, String> {
        let shell = self.find(id)?;
        if shell.cancel.is_cancelled() {
            return Ok(format!("Shell {id} was already stopped."));
        }
        shell.cancel.cancel();
        Ok(format!("Shell {id} stopped."))
    }

    fn find(&self, id: &str) -> Result<Arc<Shell>, String> {
        self.shells
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id.trim())
            .cloned()
            .ok_or_else(|| {
                format!(
                    "There is no background shell called {id} in this turn. Background shells end with the turn that started them; start a new one with Bash and run_in_background."
                )
            })
    }
}

impl Drop for Shells {
    fn drop(&mut self) {
        let shells = self
            .shells
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for shell in shells.values() {
            shell.cancel.cancel();
        }
    }
}

/// The command's own process, under the wrappers that own its group.
/// Waiting on it waits for the shell alone; the group's wait would also
/// wait for everything the shell left running.
fn leader(child: &mut dyn ChildWrapper) -> &mut dyn ChildWrapper {
    let inner: *const dyn ChildWrapper = child.inner();
    if std::ptr::addr_eq(inner, std::ptr::from_ref::<dyn ChildWrapper>(child)) {
        return child;
    }
    leader(child.inner_mut())
}

/// Reads one pipe into the shell's buffer until it ends or the shell is
/// stopped.
async fn pump(
    stream: Option<impl tokio::io::AsyncRead + Unpin>,
    shell: &Shell,
    legacy: Option<&'static Encoding>,
) {
    let Some(mut stream) = stream else {
        return;
    };
    let mut decoder = Decoder::new(legacy);
    let mut buffer = vec![0_u8; READ_CHUNK];
    loop {
        tokio::select! {
            () = shell.cancel.cancelled() => break,
            read = stream.read(&mut buffer) => match read {
                Ok(0) | Err(_) => break,
                Ok(read) => shell.append(&decoder.push(&buffer[..read], false)),
            },
        }
    }
    shell.append(&decoder.push(&[], true));
}

impl Shell {
    fn lock(&self) -> std::sync::MutexGuard<'_, ShellState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Keeps new output, dropping the oldest unread output past the bound.
    fn append(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut state = self.lock();
        state.unread.push_str(text);
        if state.unread.len() > MAX_OUTPUT_BYTES {
            let cut = state
                .unread
                .ceil_char_boundary(state.unread.len() - MAX_OUTPUT_BYTES);
            state.unread.drain(..cut);
            state.dropped += cut;
        }
    }
}

/// A pipe's bytes as text, as they come: a character split across two
/// reads waits for its rest instead of becoming U+FFFD.
///
/// cmd and console programs on Windows write in the OEM code page, while
/// newer tools (git, node, cargo) write UTF-8 whatever it is. Forcing
/// `chcp 65001` would need a console the command may not have, and would
/// change what the user's command runs in; instead a pipe is read as UTF-8
/// until it holds something UTF-8 cannot be, and from there in the OEM
/// code page, when `encoding_rs` knows it.
pub(super) struct Decoder {
    utf8: encoding_rs::Decoder,
    legacy: Option<&'static Encoding>,
    /// The legacy decoder, once the pipe proved not to be UTF-8.
    fallback: Option<encoding_rs::Decoder>,
}

impl Decoder {
    pub(super) fn new(legacy: Option<&'static Encoding>) -> Self {
        Self {
            utf8: encoding_rs::UTF_8.new_decoder_without_bom_handling(),
            legacy: legacy.filter(|legacy| *legacy != encoding_rs::UTF_8),
            fallback: None,
        }
    }

    /// The text `bytes` complete; `last` at the end of the pipe.
    pub(super) fn push(&mut self, mut bytes: &[u8], last: bool) -> String {
        let mut out = String::new();
        loop {
            if let Some(fallback) = &mut self.fallback {
                out.reserve(
                    fallback
                        .max_utf8_buffer_length(bytes.len())
                        .unwrap_or(bytes.len() * 3 + 16),
                );
                drop(fallback.decode_to_string(bytes, &mut out, last));
                return out;
            }
            out.reserve(
                self.utf8
                    .max_utf8_buffer_length_without_replacement(bytes.len())
                    .unwrap_or(bytes.len() * 3 + 16),
            );
            let (result, read) = self
                .utf8
                .decode_to_string_without_replacement(bytes, &mut out, last);
            match result {
                DecoderResult::InputEmpty => return out,
                DecoderResult::OutputFull => bytes = &bytes[read..],
                DecoderResult::Malformed(bad, extra) => {
                    if let Some(legacy) = self.legacy {
                        let from = read.saturating_sub(usize::from(bad) + usize::from(extra));
                        self.fallback = Some(legacy.new_decoder_without_bom_handling());
                        bytes = &bytes[from..];
                    } else {
                        out.push('\u{FFFD}');
                        bytes = &bytes[read..];
                    }
                }
            }
        }
    }
}

/// The code page a command's non-UTF-8 output is read in: the OEM code
/// page on Windows, when `encoding_rs` has it (866, 874, 932, 936, 949,
/// 950 and the 125x pages; not 437 or 850, whose ASCII still reads right).
#[allow(clippy::unused_async, reason = "Windows path awaits spawn_blocking")]
pub(super) async fn legacy_encoding() -> Option<&'static Encoding> {
    #[cfg(windows)]
    {
        static OEM: std::sync::OnceLock<Option<&'static Encoding>> = std::sync::OnceLock::new();
        if let Some(known) = OEM.get() {
            return *known;
        }
        tokio::task::spawn_blocking(|| *OEM.get_or_init(oem_code_page))
            .await
            .ok()
            .flatten()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The system's OEM code page, from the registry (`GetOEMCP` would need
/// unsafe code, which the workspace forbids).
#[cfg(windows)]
fn oem_code_page() -> Option<&'static Encoding> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SYSTEM\CurrentControlSet\Control\Nls\CodePage",
            "/v",
            "OEMCP",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let page: u16 = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .last()?
        .parse()
        .ok()?;
    code_page(page)
}

/// The `encoding_rs` encoding of a Windows code page, when it has one.
#[cfg_attr(not(windows), allow(dead_code, reason = "only used on Windows"))]
fn code_page(page: u16) -> Option<&'static Encoding> {
    Some(match page {
        866 => encoding_rs::IBM866,
        874 => encoding_rs::WINDOWS_874,
        932 => encoding_rs::SHIFT_JIS,
        936 => encoding_rs::GBK,
        949 => encoding_rs::EUC_KR,
        950 => encoding_rs::BIG5,
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1252 => encoding_rs::WINDOWS_1252,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        1257 => encoding_rs::WINDOWS_1257,
        1258 => encoding_rs::WINDOWS_1258,
        65001 => encoding_rs::UTF_8,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Reads the shell until it has ended, or gives up after a while: what
    /// it printed, and how it ended.
    #[cfg(unix)]
    async fn read_to_end(shells: &Shells, id: &str) -> String {
        let mut seen = String::new();
        for _ in 0..250 {
            let read = shells.read(id).unwrap();
            let Some(running) = read.strip_suffix("[still running]") else {
                seen.push_str(&read);
                return seen;
            };
            seen.push_str(running);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        seen
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_background_command_runs_and_reports_its_output_and_exit() {
        let work = tempfile::tempdir().unwrap();
        let shells = Shells::default();
        let id = shells
            .start("echo hello; sleep 0.2; echo done; exit 3", work.path())
            .unwrap();
        let mut first = String::new();
        for _ in 0..100 {
            first = shells.read(&id).unwrap();
            if first.contains("hello") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(first.contains("hello"), "{first}");
        tokio::time::sleep(Duration::from_millis(400)).await;
        let rest = shells.read(&id).unwrap();
        assert!(rest.contains("done"), "{rest}");
        assert!(rest.contains("[exit code 3]"), "{rest}");
        assert!(!rest.contains("hello"), "output is only shown once: {rest}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn killing_a_shell_stops_it_and_says_so() {
        let work = tempfile::tempdir().unwrap();
        let shells = Shells::default();
        let id = shells.start("echo up; sleep 30", work.path()).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(shells.kill(&id).unwrap().contains("stopped"));
        tokio::time::sleep(Duration::from_millis(200)).await;
        let read = shells.read(&id).unwrap();
        assert!(read.contains("[stopped]"), "{read}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_id_from_another_turn_never_reads_this_turns_shell() {
        let work = tempfile::tempdir().unwrap();
        let earlier = Shells::default();
        let old = earlier.start("echo old", work.path()).unwrap();
        drop(earlier);
        let shells = Shells::default();
        let id = shells.start("echo new", work.path()).unwrap();
        assert_ne!(id, old, "ids are not reused across turns");
        let error = shells.read(&old).unwrap_err();
        assert!(error.contains("end with the turn"), "{error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_shell_that_leaves_a_command_running_still_reports_its_exit() {
        let work = tempfile::tempdir().unwrap();
        let shells = Shells::default();
        // The sleep keeps the shell's output pipes open after it exits.
        let id = shells
            .start("sleep 30 & echo started", work.path())
            .unwrap();
        let read = read_to_end(&shells, &id).await;
        assert!(read.contains("started"), "{read}");
        assert!(read.ends_with("[exit code 0]"), "{read}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_newest_output_is_kept_past_the_bound_and_the_loss_said_once() {
        let work = tempfile::tempdir().unwrap();
        let shells = Shells::default();
        // 300 000 bytes, a newline and `tail-marker\n`: 300 013 bytes.
        let id = shells
            .start(
                "head -c 300000 /dev/zero | tr '\\0' a; echo; echo tail-marker",
                work.path(),
            )
            .unwrap();
        // Nothing is read while it prints, so all of it is unread.
        tokio::time::sleep(Duration::from_secs(1)).await;
        let read = read_to_end(&shells, &id).await;
        assert!(
            read.contains("tail-marker\n[exit code 0]"),
            "{}",
            &read[read.len() - 100..]
        );
        assert!(
            read.starts_with(&format!(
                "[{} bytes of output were dropped before they were read",
                300_013 - MAX_OUTPUT_BYTES
            )),
            "{}",
            &read[..200]
        );
        let again = shells.read(&id).unwrap();
        assert_eq!(again, "[exit code 0]", "the loss is reported once");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stopping_the_run_ends_what_a_shell_started() {
        let work = tempfile::tempdir().unwrap();
        let shells = Shells::default();
        let id = shells
            .start("sleep 30 & echo $!; wait", work.path())
            .unwrap();
        let mut pid = String::new();
        for _ in 0..100 {
            pid.push_str(
                shells
                    .read(&id)
                    .unwrap()
                    .trim_end_matches("[still running]"),
            );
            if pid.ends_with('\n') {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let pid = pid.trim().to_owned();
        assert!(pid.parse::<u32>().is_ok(), "the grandchild's pid: {pid:?}");
        let alive = |pid: &str| {
            std::process::Command::new("kill")
                .args(["-0", pid])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        assert!(alive(&pid));
        // The run's end drops its shells.
        drop(shells);
        let mut gone = false;
        for _ in 0..100 {
            if !alive(&pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(gone, "the grandchild {pid} outlived the run");
    }

    #[test]
    fn a_character_split_across_reads_stays_whole() {
        let mut decoder = Decoder::new(None);
        let bytes = "café ✓".as_bytes();
        let (head, tail) = bytes.split_at(bytes.len() - 2);
        let mut text = decoder.push(&bytes[..4], false);
        text.push_str(&decoder.push(&head[4..], false));
        text.push_str(&decoder.push(tail, false));
        text.push_str(&decoder.push(&[], true));
        assert_eq!(text, "café ✓");
    }

    #[test]
    fn output_that_is_not_utf8_is_read_in_the_legacy_code_page() {
        let mut decoder = Decoder::new(Some(encoding_rs::IBM866));
        let mut text = decoder.push(b"ok ", false);
        // "Привет" in code page 866.
        text.push_str(&decoder.push(b"\x8f\xe0\xa8\xa2\xa5\xe2", true));
        assert_eq!(text, "ok Привет");
    }
}
