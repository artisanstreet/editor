use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::sandbox::Sandbox;
use encoding_rs::{DecoderResult, Encoding};
use process_wrap::tokio::ChildWrapper;
use tokio::io::AsyncReadExt as _;
use tokio_util::sync::CancellationToken;

const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const READ_CHUNK: usize = 16 * 1024;
const MAX_RUNNING_SHELLS: usize = 16;
const MAX_KEPT_SHELLS: usize = 64;

static LAST_ID: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Default)]
pub struct Shells {
    shells: Mutex<HashMap<String, Arc<Shell>>>,
    turn: CancellationToken,
}

#[derive(Debug)]
struct Shell {
    number: usize,
    state: Mutex<ShellState>,
    cancel: CancellationToken,
}

#[derive(Debug, Default)]
struct ShellState {
    unread: Unread,
    phase: Phase,
}

#[derive(Debug, Clone, Copy, Default)]
enum Phase {
    #[default]
    Running,
    Exited(Ended),
    Released(Ended),
}

#[derive(Debug, Default)]
struct Unread {
    chunks: VecDeque<String>,
    bytes: usize,
    dropped: usize,
}

impl Unread {
    fn push(&mut self, text: &str) {
        self.chunks.push_back(text.to_owned());
        self.bytes += text.len();
        let mut excess = self.bytes.saturating_sub(MAX_OUTPUT_BYTES);
        while excess > 0
            && let Some(front) = self.chunks.front_mut()
        {
            let cut = if front.len() <= excess {
                let whole = front.len();
                self.chunks.pop_front();
                whole
            } else {
                let cut = front.ceil_char_boundary(excess);
                front.drain(..cut);
                cut
            };
            self.bytes -= cut;
            self.dropped += cut;
            excess = excess.saturating_sub(cut);
        }
    }

    fn take(&mut self) -> (usize, String) {
        let mut text = String::with_capacity(self.bytes);
        for chunk in self.chunks.drain(..) {
            text.push_str(&chunk);
        }
        self.bytes = 0;
        (std::mem::take(&mut self.dropped), text)
    }
}

#[derive(Debug, Clone, Copy)]
enum Ended {
    Code(i32),
    WithoutCode,
    Stopped,
}

impl Shells {
    pub fn start(&self, command: &str, cwd: &Path, sandbox: &Sandbox) -> Result<String, String> {
        let mut shells = self
            .shells
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let running = shells.values().filter(|shell| shell.holds_group()).count();
        if running >= MAX_RUNNING_SHELLS {
            return Err(format!(
                "This turn already runs {MAX_RUNNING_SHELLS} background shells, the most it may. Stop one with KillShell, or wait for one to finish, before starting another."
            ));
        }
        let mut released: Vec<(usize, String)> = shells
            .iter()
            .filter(|(_, shell)| !shell.holds_group())
            .map(|(id, shell)| (shell.number, id.clone()))
            .collect();
        released.sort_unstable();
        for (_, id) in released
            .into_iter()
            .take((shells.len() + 1).saturating_sub(MAX_KEPT_SHELLS))
        {
            shells.remove(&id);
        }
        let mut shell = command_line(command);
        shell
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = crate::process::owned_command(shell, sandbox.clone())
            .map_err(|error| format!("The command could not start: {error:#}"))?;
        let stdout = child.stdout().take();
        let stderr = child.stderr().take();
        let number = LAST_ID.fetch_add(1, Ordering::Relaxed) + 1;
        let id = format!("bash-{number}");
        let shell = Arc::new(Shell {
            number,
            state: Mutex::new(ShellState::default()),
            cancel: self.turn.child_token(),
        });
        shells.insert(id.clone(), Arc::clone(&shell));
        drop(shells);
        tokio::spawn(async move {
            let legacy = legacy_encoding().await;
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
                shell.lock().phase = Phase::Exited(ended);
                ended
            };
            let (ended, (), ()) = tokio::join!(
                watch,
                pump(stdout, &shell, legacy),
                pump(stderr, &shell, legacy)
            );
            drop(child);
            shell.lock().phase = Phase::Released(ended);
        });
        Ok(id)
    }

    pub fn read(&self, id: &str) -> Result<String, String> {
        let shell = self.find(id)?;
        let mut state = shell.lock();
        let mut out = String::new();
        let (dropped, unread) = state.unread.take();
        if dropped > 0 {
            writeln!(
                out,
                "[{dropped} bytes of output were dropped before they were read; the newest follows]"
            )
            .expect("invariant: writing to a String cannot fail");
        }
        out.push_str(&unread);
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        match state.phase {
            Phase::Running => out.push_str("[still running]"),
            Phase::Exited(ended) | Phase::Released(ended) => match ended {
                Ended::Code(code) => {
                    write!(out, "[exit code {code}]")
                        .expect("invariant: writing to a String cannot fail");
                }
                Ended::Stopped => out.push_str("[stopped]"),
                Ended::WithoutCode => out.push_str("[ended without an exit code]"),
            },
        }
        drop(state);
        Ok(out)
    }

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
        self.turn.cancel();
    }
}

fn leader(child: &mut dyn ChildWrapper) -> &mut dyn ChildWrapper {
    let inner: *const dyn ChildWrapper = child.inner();
    if std::ptr::addr_eq(inner, std::ptr::from_ref::<dyn ChildWrapper>(child)) {
        return child;
    }
    leader(child.inner_mut())
}

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

    fn append(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.lock().unread.push(text);
    }

    fn holds_group(&self) -> bool {
        !self.cancel.is_cancelled() && !matches!(self.lock().phase, Phase::Released(_))
    }
}

#[cfg(windows)]
pub(super) fn command_line(command: &str) -> tokio::process::Command {
    let mut shell = tokio::process::Command::new("cmd");
    shell
        .raw_arg("/D /V:OFF /S /C")
        .raw_arg(format!("\"{command}\""));
    shell
}

#[cfg(not(windows))]
pub(super) fn command_line(command: &str) -> tokio::process::Command {
    let mut shell = tokio::process::Command::new("sh");
    shell.arg("-c").arg(command);
    shell
}

pub(super) struct Decoder {
    utf8: encoding_rs::Decoder,
    legacy: Option<&'static Encoding>,
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

#[cfg_attr(
    not(windows),
    expect(
        clippy::unused_async,
        reason = "only the Windows path awaits spawn_blocking"
    )
)]
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

#[cfg_attr(
    not(windows),
    expect(dead_code, reason = "only Windows reads code pages")
)]
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
#[path = "../../tests/claudeai/shells.rs"]
mod tests;
