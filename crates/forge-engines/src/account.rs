//! Engine sign-in: a cheap status probe, the engine's own login flow, logout.
//!
//! Probes never call a model: `claude auth status`, `codex login status`,
//! `grok models`, `cursor-agent status`, and `OpenCode`'s `auth.json`. Login
//! runs the engine's CLI flow with browser opening disabled and surfaces the
//! URL and device code it prints, so the Editor (possibly on another machine)
//! opens the link.

use anyhow::{Context, Result, bail};
use artisan_api::{EngineId, SignInState};
use statig::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// Device codes expire after 15 minutes.
const LOGIN_TIMEOUT: Duration = Duration::from_mins(15);
const LOGIN_OUTPUT_CAP: usize = 64 * 1024;

/// A command for an engine's own CLI in the engine home, with the config
/// directory pinned there (as `Engines::launch_env` does) so nothing
/// inherited points the engine at another account.
pub(crate) fn engine_command(engine: EngineId, binary: &Path, home: &Path) -> Command {
    if let Err(error) = std::fs::create_dir_all(home) {
        tracing::warn!(home = %home.display(), error = %error, "couldn't create the engine home");
    }
    let mut command = Command::new(binary);
    command
        .env("HOME", home)
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.env("USERPROFILE", home);
    match engine {
        EngineId::Claude => {
            command
                .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
                .env("DISABLE_UPDATES", "1");
        }
        EngineId::Codex => {
            // Codex refuses a CODEX_HOME that doesn't exist yet.
            let codex_home = home.join(".codex");
            if let Err(error) = std::fs::create_dir_all(&codex_home) {
                tracing::warn!(dir = %codex_home.display(), error = %error, "couldn't create CODEX_HOME");
            }
            command.env("CODEX_HOME", codex_home);
        }
        EngineId::OpenCode | EngineId::Grok | EngineId::Cursor => {}
    }
    command
}

/// Runs a short engine command; returns (success, stdout, stderr).
pub(crate) async fn run_short(
    engine: EngineId,
    binary: &Path,
    home: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<(bool, String, String)> {
    let mut command = engine_command(engine, binary, home);
    command.args(args);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .with_context(|| format!("{} {} timed out", binary.display(), args.join(" ")))?
        .with_context(|| format!("running {} {}", binary.display(), args.join(" ")))?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// Probes whether the engine in `home` is signed in. Never calls a model.
pub async fn check(engine: EngineId, binary: &Path, home: &Path) -> SignInState {
    let args: &[&str] = match engine {
        EngineId::Claude => &["auth", "status"],
        EngineId::Codex => &["login", "status"],
        EngineId::Grok => &["--no-auto-update", "models"],
        EngineId::Cursor => &["status"],
        EngineId::OpenCode => return opencode_sign_in(home),
    };
    match run_short(engine, binary, home, args, CHECK_TIMEOUT).await {
        Ok((success, stdout, stderr)) => parse_status(engine, success, &stdout, &stderr),
        Err(error) => {
            tracing::warn!(engine = %engine, home = %home.display(), error = %format!("{error:#}"), "sign-in check failed");
            SignInState::Failed {
                message: format!(
                    "Couldn't check the {} sign-in: {error:#}",
                    engine.display_name()
                ),
            }
        }
    }
}

/// Classifies a status command's output.
fn parse_status(engine: EngineId, success: bool, stdout: &str, stderr: &str) -> SignInState {
    let text = strip_ansi(&format!("{stdout}\n{stderr}"));
    let lower = text.to_lowercase();
    if engine == EngineId::Claude {
        // `auth status` prints JSON and exits 1 when signed out.
        let json = stdout
            .find('{')
            .zip(stdout.rfind('}'))
            .and_then(|(start, end)| stdout.get(start..=end))
            .and_then(|span| serde_json::from_str::<serde_json::Value>(span).ok());
        if let Some(json) = json {
            return if json["loggedIn"].as_bool() == Some(true) {
                let account = json["email"].as_str().map(str::to_owned);
                SignInState::SignedIn { account }
            } else {
                SignInState::SignedOut
            };
        }
    } else if lower.contains("not logged in") || lower.contains("not authenticated") {
        return SignInState::SignedOut;
    } else if success {
        let account = text.lines().find_map(|line| {
            let line = line.trim();
            let lower = line.to_lowercase();
            let at = lower.find("logged in as ")?;
            Some(line[at + "logged in as ".len()..].trim().to_owned())
                .filter(|account| !account.is_empty())
        });
        return SignInState::SignedIn { account };
    }
    let detail = text
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("no output");
    SignInState::Failed {
        message: format!(
            "{} sign-in status is unavailable: {detail}",
            engine.display_name()
        ),
    }
}

/// `OpenCode` runs its free models without an account, so it is always
/// usable; the providers it holds credentials for name the account.
fn opencode_sign_in(home: &Path) -> SignInState {
    let path = home.join("data/opencode/auth.json");
    let providers = std::fs::read(&path)
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes).ok()
        })
        .map(|map| map.keys().cloned().collect::<Vec<_>>().join(", "))
        .filter(|providers| !providers.is_empty());
    SignInState::SignedIn { account: providers }
}

fn login_args(engine: EngineId) -> Result<&'static [&'static str]> {
    Ok(match engine {
        EngineId::Claude => &["auth", "login"],
        EngineId::Codex | EngineId::Grok => &["login", "--device-auth"],
        EngineId::Cursor => &["login"],
        EngineId::OpenCode => {
            bail!("OpenCode signs in per provider; run `opencode auth login` in its home")
        }
    })
}

fn logout_args(engine: EngineId) -> Result<&'static [&'static str]> {
    Ok(match engine {
        EngineId::Claude => &["auth", "logout"],
        EngineId::Codex | EngineId::Grok | EngineId::Cursor => &["logout"],
        EngineId::OpenCode => {
            bail!("OpenCode signs out per provider; run `opencode auth logout` in its home")
        }
    })
}

/// Finds the sign-in link and the device code in login output so far.
fn parse_login_output(text: &str) -> (Option<String>, Option<String>) {
    let text = strip_ansi(text);
    let url = text
        .split_whitespace()
        .find(|word| word.starts_with("https://"))
        .map(|word| word.trim_end_matches(['.', ',', ')']).to_owned());
    // Device codes stand on their own line: `QES3-C7POE`, `3XW5-WYV2`.
    let code = text
        .lines()
        .map(str::trim)
        .find(|line| is_device_code(line))
        .map(str::to_owned);
    (url, code)
}

fn is_device_code(line: &str) -> bool {
    let Some((left, right)) = line.split_once('-') else {
        return false;
    };
    let part = |part: &str| {
        (4..=8).contains(&part.len())
            && part
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    };
    part(left) && part(right)
}

pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

// --- the sign-in lifecycle -------------------------------------------------

#[derive(Debug)]
enum Event {
    Checked(SignInState),
    Login,
    Prompt {
        url: Option<String>,
        code: Option<String>,
    },
    LoginEnded {
        error: Option<String>,
    },
    Cancel,
    Logout,
}

#[derive(Debug, PartialEq, Eq)]
enum Effect {
    Check,
    StartLogin,
    StopLogin,
    Logout,
}

#[derive(Default)]
struct Machine;

#[state_machine(initial = "State::unknown()", state(derive(Debug, Clone)))]
impl Machine {
    #[superstate]
    fn settled(context: &mut Vec<Effect>, event: &Event) -> Outcome<State> {
        match event {
            Event::Checked(probe) => Transition(match probe {
                SignInState::SignedIn { account } => State::signed_in(account.clone()),
                SignInState::SignedOut => State::signed_out(),
                SignInState::Failed { message } => State::failed(message.clone()),
                SignInState::Unknown | SignInState::LoggingIn { .. } => State::unknown(),
            }),
            Event::Login => {
                context.push(Effect::StartLogin);
                Transition(State::logging_in(None, None))
            }
            _ => Handled,
        }
    }

    #[state(superstate = "settled")]
    fn unknown(event: &Event) -> Outcome<State> {
        let _ = event;
        Super
    }

    #[state(superstate = "settled")]
    fn signed_out(event: &Event) -> Outcome<State> {
        let _ = event;
        Super
    }

    #[state(superstate = "settled")]
    fn signed_in(
        account: &mut Option<String>,
        context: &mut Vec<Effect>,
        event: &Event,
    ) -> Outcome<State> {
        let _ = account;
        match event {
            Event::Logout => {
                context.push(Effect::Logout);
                Transition(State::unknown())
            }
            _ => Super,
        }
    }

    #[state(superstate = "settled")]
    fn failed(message: &mut String, event: &Event) -> Outcome<State> {
        let _ = (message, event);
        Super
    }

    #[state]
    fn logging_in(
        url: &mut Option<String>,
        code: &mut Option<String>,
        context: &mut Vec<Effect>,
        event: &Event,
    ) -> Outcome<State> {
        match event {
            Event::Prompt {
                url: new_url,
                code: new_code,
            } => {
                if new_url.is_some() {
                    url.clone_from(new_url);
                }
                if new_code.is_some() {
                    code.clone_from(new_code);
                }
                Handled
            }
            Event::LoginEnded { error: None } => {
                context.push(Effect::Check);
                Transition(State::unknown())
            }
            Event::LoginEnded {
                error: Some(message),
            } => Transition(State::failed(message.clone())),
            Event::Checked(SignInState::SignedIn { account }) => {
                context.push(Effect::StopLogin);
                Transition(State::signed_in(account.clone()))
            }
            Event::Cancel => {
                context.push(Effect::StopLogin);
                context.push(Effect::Check);
                Transition(State::unknown())
            }
            _ => Handled,
        }
    }
}

fn public_state(state: &State) -> SignInState {
    match state {
        State::Unknown {} => SignInState::Unknown,
        State::SignedOut {} => SignInState::SignedOut,
        State::SignedIn { account } => SignInState::SignedIn {
            account: account.clone(),
        },
        State::Failed { message } => SignInState::Failed {
            message: message.clone(),
        },
        State::LoggingIn { url, code } => SignInState::LoggingIn {
            url: url.clone(),
            code: code.clone(),
        },
    }
}

struct LiveLogin {
    stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    cancel: CancellationToken,
}

struct Inner {
    machine: StateMachine<Machine>,
    /// The binary of the last call; follow-up probes use it.
    binary: Option<PathBuf>,
    login: Option<LiveLogin>,
}

/// One engine's sign-in. The owner (the engine table) keeps one per engine
/// and forwards `state()` changes to clients via [`SignIn::subscribe`].
pub struct SignIn {
    engine: EngineId,
    home: PathBuf,
    state: watch::Sender<SignInState>,
    inner: Mutex<Inner>,
}

impl SignIn {
    #[must_use]
    pub fn new(engine: EngineId, home: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            engine,
            home,
            state: watch::Sender::new(SignInState::Unknown),
            inner: Mutex::new(Inner {
                machine: Machine.state_machine(),
                binary: None,
                login: None,
            }),
        })
    }

    #[must_use]
    pub fn state(&self) -> SignInState {
        self.state.borrow().clone()
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<SignInState> {
        self.state.subscribe()
    }

    /// Probes the engine now and settles the state on the result.
    pub async fn check(self: &Arc<Self>, binary: &Path) {
        self.lock().binary = Some(binary.to_owned());
        self.recheck().await;
    }

    async fn recheck(self: &Arc<Self>) {
        let Some(binary) = self.lock().binary.clone() else {
            return;
        };
        let probe = check(self.engine, &binary, &self.home).await;
        self.handle(&Event::Checked(probe));
    }

    /// Starts the engine's login flow. The state moves to `LoggingIn` and
    /// fills in the URL and code as the CLI prints them.
    pub fn login(self: &Arc<Self>, binary: &Path) {
        self.lock().binary = Some(binary.to_owned());
        self.handle(&Event::Login);
    }

    /// Sends what the user pasted (Claude asks for the code shown after
    /// signing in on the web).
    ///
    /// # Errors
    ///
    /// When no sign-in is running or it stopped reading input.
    pub async fn submit_code(&self, code: &str) -> Result<()> {
        let stdin = self
            .lock()
            .login
            .as_ref()
            .map(|login| Arc::clone(&login.stdin))
            .context("no sign-in in progress")?;
        let mut stdin = stdin.lock().await;
        let stdin = stdin
            .as_mut()
            .context("the sign-in no longer accepts input")?;
        stdin
            .write_all(format!("{}\n", code.trim()).as_bytes())
            .await
            .context("sending the code to the sign-in")?;
        stdin
            .flush()
            .await
            .context("sending the code to the sign-in")?;
        Ok(())
    }

    pub fn cancel_login(self: &Arc<Self>) {
        self.handle(&Event::Cancel);
    }

    /// Signs the engine out, then probes again.
    pub fn logout(self: &Arc<Self>, binary: &Path) {
        self.lock().binary = Some(binary.to_owned());
        self.handle(&Event::Logout);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn handle(self: &Arc<Self>, event: &Event) {
        let mut effects = Vec::new();
        let state = {
            let mut inner = self.lock();
            inner.machine.handle_with_context(event, &mut effects);
            public_state(inner.machine.state())
        };
        self.state.send_if_modified(|current| {
            let changed = *current != state;
            *current = state;
            changed
        });
        for effect in &effects {
            self.perform(effect);
        }
    }

    fn perform(self: &Arc<Self>, effect: &Effect) {
        let this = Arc::clone(self);
        match effect {
            Effect::Check => {
                tokio::spawn(async move { this.recheck().await });
            }
            Effect::StartLogin => self.start_login(),
            Effect::StopLogin => {
                if let Some(login) = self.lock().login.take() {
                    login.cancel.cancel();
                }
            }
            Effect::Logout => {
                tokio::spawn(async move {
                    let binary = this.lock().binary.clone().unwrap_or_default();
                    match logout_args(this.engine) {
                        Ok(args) => {
                            if let Err(error) =
                                run_short(this.engine, &binary, &this.home, args, CHECK_TIMEOUT)
                                    .await
                            {
                                tracing::warn!(engine = %this.engine, error = %format!("{error:#}"), "logout failed");
                            }
                        }
                        Err(error) => {
                            tracing::warn!(engine = %this.engine, error = %error, "logout unavailable")
                        }
                    }
                    this.recheck().await;
                });
            }
        }
    }

    fn start_login(self: &Arc<Self>) {
        let cancel = CancellationToken::new();
        let stdin = Arc::new(tokio::sync::Mutex::new(None));
        let previous = self.lock().login.replace(LiveLogin {
            stdin: Arc::clone(&stdin),
            cancel: cancel.clone(),
        });
        if let Some(previous) = previous {
            previous.cancel.cancel();
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let error = this
                .run_login(stdin, cancel.clone())
                .await
                .err()
                .map(|error| format!("{error:#}"));
            let current = this
                .lock()
                .login
                .as_ref()
                .is_some_and(|login| login.cancel == cancel);
            if current && !cancel.is_cancelled() {
                this.lock().login = None;
                if let Some(error) = &error {
                    tracing::warn!(engine = %this.engine, error = %error, "sign-in failed");
                }
                this.handle(&Event::LoginEnded { error });
            }
        });
    }

    async fn run_login(
        self: &Arc<Self>,
        stdin_slot: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let args = login_args(self.engine)?;
        let binary = self
            .lock()
            .binary
            .clone()
            .context("the engine is not installed")?;
        let mut command = engine_command(self.engine, &binary, &self.home);
        command
            .args(args)
            .stdin(Stdio::piped())
            .env("NO_OPEN_BROWSER", "1");
        // The Editor opens the link, possibly on another machine.
        #[cfg(unix)]
        command.env("BROWSER", "true");
        let mut child = command
            .spawn()
            .with_context(|| format!("starting {} {}", binary.display(), args.join(" ")))?;
        *stdin_slot.lock().await = child.stdin.take();
        let (chunks, mut received) = mpsc::channel::<String>(16);
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(forward(stdout, chunks.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(forward(stderr, chunks));
        }
        let mut output = String::new();
        let mut shown = (None, None);
        let deadline = tokio::time::sleep(LOGIN_TIMEOUT);
        tokio::pin!(deadline);
        let status = loop {
            tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                () = &mut deadline => bail!("{} sign-in timed out", self.engine.display_name()),
                Some(chunk) = received.recv() => {
                    if output.len() < LOGIN_OUTPUT_CAP {
                        output.push_str(&chunk);
                    }
                    let found = parse_login_output(&output);
                    if found != shown {
                        shown = found.clone();
                        self.handle(&Event::Prompt { url: found.0, code: found.1 });
                    }
                }
                status = child.wait() => break status.context("waiting for the sign-in")?,
            }
        };
        if status.success() {
            return Ok(());
        }
        // The pipes close with the process; keep what it printed last.
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            while let Some(chunk) = received.recv().await {
                output.push_str(&chunk);
            }
        })
        .await;
        let text = strip_ansi(&output);
        let detail = text
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .unwrap_or("no output");
        bail!("{} sign-in failed: {detail}", self.engine.display_name())
    }
}

async fn forward(mut stream: impl AsyncRead + Unpin, chunks: mpsc::Sender<String>) {
    let mut buffer = vec![0_u8; 4096];
    while let Ok(read) = stream.read(&mut buffer).await {
        if read == 0
            || chunks
                .send(String::from_utf8_lossy(&buffer[..read]).into_owned())
                .await
                .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Status output recorded from the installed engines (account details
    /// replaced).
    #[test]
    fn status_probes_classify_recorded_output() {
        let claude_in = r#"{
  "loggedIn": true,
  "authMethod": "claude.ai",
  "apiProvider": "firstParty",
  "email": "user@example.com",
  "subscriptionType": "max"
}"#;
        let claude_out = r#"{
  "loggedIn": false,
  "authMethod": "none",
  "apiProvider": "firstParty"
}"#;
        let cases = [
            (EngineId::Claude, true, claude_in, ""),
            (EngineId::Claude, false, claude_out, ""),
            (EngineId::Claude, false, "", "error: unknown command 'auth'"),
            (EngineId::Codex, true, "", "Logged in using ChatGPT\n"),
            (
                EngineId::Codex,
                false,
                "",
                "WARNING: proceeding, even though we could not create PATH aliases\nNot logged in\n",
            ),
            (
                EngineId::Grok,
                true,
                "You are not authenticated.\n\nDefault model: grok-4.6\n",
                "",
            ),
            (
                EngineId::Grok,
                true,
                "Default model: grok-4.6\n\nAvailable models:\n  * grok-4.6 (default)\n",
                "",
            ),
            (
                EngineId::Cursor,
                true,
                "\u{1b}[2K\u{1b}[GNot logged in\n",
                "",
            ),
            (
                EngineId::Cursor,
                true,
                "\u{1b}[32m✓\u{1b}[0m Logged in as user@example.com\n",
                "",
            ),
        ];
        let results: Vec<_> = cases
            .iter()
            .map(|(engine, success, stdout, stderr)| {
                (
                    engine.as_str(),
                    parse_status(*engine, *success, stdout, stderr),
                )
            })
            .collect();
        insta::assert_debug_snapshot!(results);
    }

    /// Login output recorded from each engine in a throwaway home.
    #[test]
    fn login_output_yields_link_and_code() {
        let claude = "Opening browser to sign in…\nIf the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&client_id=x&state=y\nPaste code here if prompted > ";
        let codex = "\nWelcome to Codex [v\u{1b}[90m0.162.1\u{1b}[0m]\n\nFollow these steps to sign in with ChatGPT using device code authorization:\n\n1. Open this link in your browser and sign in to your account\n   \u{1b}[94mhttps://auth.openai.com/codex/device\u{1b}[0m\n\n2. Enter this one-time code \u{1b}[90m(expires in 15 minutes)\u{1b}[0m\n   \u{1b}[94mQES3-C7POE\u{1b}[0m\n";
        let grok = "\nTo sign in, open this URL in your browser:\n\n  https://accounts.x.ai/oauth2/device?user_code=3XW5-WYV2\n\nConfirm this code in your browser:\n\n  3XW5-WYV2\n\nWaiting for authorization...\n";
        let cursor = "Starting login process...\nAuthenticating with Cursor...\nWaiting for browser authentication...\nOpen a browser and navigate to this link: https://cursor.com/loginDeepControl?challenge=abc&uuid=def&mode=login\n";
        let results: Vec<_> = [
            ("claude", claude),
            ("codex", codex),
            ("grok", grok),
            ("cursor", cursor),
        ]
        .into_iter()
        .map(|(engine, text)| (engine, parse_login_output(text)))
        .collect();
        insta::assert_debug_snapshot!(results);
    }

    /// The sign-in lifecycle as the Editor sees it: signed out, logging in
    /// with the link, then signed in once the probe confirms.
    #[test]
    fn login_walks_from_signed_out_to_signed_in() {
        let mut machine = Machine.state_machine();
        let mut effects = Vec::new();
        let mut seen = Vec::new();
        for event in [
            Event::Checked(SignInState::SignedOut),
            Event::Login,
            Event::Prompt {
                url: Some("https://auth.example/device".into()),
                code: None,
            },
            Event::Prompt {
                url: None,
                code: Some("ABCD-1234".into()),
            },
            Event::Checked(SignInState::SignedOut),
            Event::LoginEnded { error: None },
            Event::Checked(SignInState::SignedIn {
                account: Some("user@example.com".into()),
            }),
            Event::Logout,
            Event::Checked(SignInState::SignedOut),
            Event::Login,
            Event::LoginEnded {
                error: Some("Codex sign-in failed: denied".into()),
            },
        ] {
            machine.handle_with_context(&event, &mut effects);
            seen.push((public_state(machine.state()), std::mem::take(&mut effects)));
        }
        insta::assert_debug_snapshot!(seen);
    }
}
