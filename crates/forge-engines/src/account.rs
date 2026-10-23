//! Engine sign-in: a cheap status probe, the engine's own login flow, logout.
//!
//! Probes never call a model: `claude auth status`, `codex login status`
//! and `OpenCode`'s stored logins (`integration.list`). Login
//! runs the engine's CLI flow with browser opening disabled and surfaces the
//! URL and device code it prints, so the Editor (possibly on another machine)
//! opens the link.

use anyhow::{Context, Result, bail};
use artisan_api::{EngineId, SignInState};
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
pub fn engine_command(engine: EngineId, binary: &Path, home: &Path) -> Command {
    if let Err(error) = std::fs::create_dir_all(home) {
        tracing::warn!(home = %home.display(), error = %error, "couldn't create the engine home");
    }
    let mut command = Command::new(binary);
    command
        .envs(crate::engines::environment(engine, Some(binary), home))
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some((directory, _)) = crate::engines::host_spec(engine).config
        && let Err(error) = std::fs::create_dir_all(home.join(directory))
    {
        tracing::warn!(home = %home.display(), %error, "couldn't create engine configuration directory");
    }
    command
}

/// Runs a short engine command; returns (success, stdout, stderr).
pub async fn run_short(
    engine: EngineId,
    binary: &Path,
    home: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<(bool, String, String)> {
    let usage = crate::install::usage_lock(home)?;
    fs2::FileExt::lock_shared(&usage)?;
    let mut command = engine_command(engine, binary, home);
    command.args(args);
    let output = crate::process::probe(command, timeout, 1024 * 1024).await?;
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
        EngineId::OpenCode => return opencode_sign_in(binary, home).await,
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

/// `OpenCode` signs in per gateway (OpenCode Console, OpenCode Go); the
/// gateways holding a login name the account. Signed out it still runs
/// its free models.
async fn opencode_sign_in(binary: &Path, home: &Path) -> SignInState {
    match opencode_connections(binary, home).await {
        Ok(connections) => {
            let mut names: Vec<&str> = Vec::new();
            for connection in &connections {
                if !names.contains(&connection.name.as_str()) {
                    names.push(&connection.name);
                }
            }
            if names.is_empty() {
                SignInState::SignedOut
            } else {
                SignInState::SignedIn {
                    account: Some(names.join(", ")),
                }
            }
        }
        Err(error) => {
            tracing::warn!(home = %home.display(), error = %format!("{error:#}"), "OpenCode sign-in check failed");
            SignInState::Failed {
                message: format!("Couldn't check the OpenCode sign-in: {error:#}"),
            }
        }
    }
}

/// One stored `OpenCode` login.
#[derive(Debug, PartialEq, Eq)]
struct Connection {
    integration: String,
    name: String,
    /// Absent for logins `OpenCode` reads from the environment.
    credential: Option<String>,
}

/// The logins `OpenCode` holds, through a private server: its background
/// service sits on a port shared with the user's own `OpenCode`.
async fn opencode_connections(binary: &Path, home: &Path) -> Result<Vec<Connection>> {
    let (success, stdout, stderr) = run_short(
        EngineId::OpenCode,
        binary,
        home,
        &["api", "--standalone", "integration.list"],
        CHECK_TIMEOUT,
    )
    .await?;
    if !success {
        bail!("{}", strip_ansi(stderr.trim()));
    }
    let listing: serde_json::Value =
        serde_json::from_str(&stdout).context("reading OpenCode's integrations")?;
    Ok(connections(&listing))
}

fn connections(listing: &serde_json::Value) -> Vec<Connection> {
    let text = |value: &serde_json::Value, key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let mut found = Vec::new();
    for integration in listing
        .get("data")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = text(integration, "id") else {
            continue;
        };
        let name = text(integration, "name").unwrap_or_else(|| id.clone());
        for connection in integration
            .get("connections")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            found.push(Connection {
                integration: id.clone(),
                name: name.clone(),
                credential: text(connection, "id"),
            });
        }
    }
    found
}

/// Signs `OpenCode` out of every stored login.
async fn opencode_logout(binary: &Path, home: &Path) -> Result<()> {
    for connection in opencode_connections(binary, home).await? {
        let Some(credential) = &connection.credential else {
            continue;
        };
        let (success, _, stderr) = run_short(
            EngineId::OpenCode,
            binary,
            home,
            &[
                "auth",
                "logout",
                "--standalone",
                &connection.integration,
                credential,
            ],
            CHECK_TIMEOUT,
        )
        .await?;
        if !success {
            bail!(
                "signing out of {}: {}",
                connection.name,
                strip_ansi(stderr.trim())
            );
        }
    }
    Ok(())
}

fn login_args(engine: EngineId) -> Result<&'static [&'static str]> {
    crate::engines::host_spec(engine)
        .device_login
        .with_context(|| {
            format!(
                "{} signs in per provider; use its interactive CLI sign-in",
                engine.display_name()
            )
        })
}

fn logout_args(engine: EngineId) -> Result<&'static [&'static str]> {
    crate::engines::host_spec(engine).logout.with_context(|| {
        format!(
            "{} signs out per provider; use its interactive CLI",
            engine.display_name()
        )
    })
}

/// Finds the sign-in link and the device code in login output so far.
fn parse_login_output(text: &str) -> (Option<String>, Option<String>) {
    let text = strip_ansi(text);
    let url = text
        .split_whitespace()
        .find(|word| word.starts_with("https://"))
        .map(|word| word.trim_end_matches(['.', ',', ')']).to_owned());
    // Device codes stand on their own line (`QES3-C7POE`) or end it
    // (`Enter code: HNCP-VWCS`).
    let code = text
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .find(|word| is_device_code(word))
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

#[derive(Default)]
struct TerminalText {
    text: String,
}
impl vte::Perform for TerminalText {
    fn print(&mut self, character: char) {
        self.text.push(character);
    }
    fn execute(&mut self, byte: u8) {
        if matches!(byte, b'\n' | b'\r' | b'\t') {
            self.text.push(char::from(byte));
        }
    }
}

pub fn strip_ansi(text: &str) -> String {
    let mut parser = vte::Parser::new();
    let mut plain = TerminalText::default();
    parser.advance(&mut plain, text.as_bytes());
    plain.text
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

#[derive(Debug, Clone)]
enum State {
    Unknown {},
    SignedOut {},
    SignedIn {
        account: Option<String>,
    },
    Failed {
        message: String,
    },
    LoggingIn {
        url: Option<String>,
        code: Option<String>,
    },
}

struct Machine {
    state: State,
}

impl Default for Machine {
    fn default() -> Self {
        Self {
            state: State::Unknown {},
        }
    }
}

impl Machine {
    const fn state(&self) -> &State {
        &self.state
    }

    fn handle_with_context(&mut self, event: &Event, effects: &mut Vec<Effect>) {
        let next = match (&mut self.state, event) {
            (
                State::LoggingIn { url, code },
                Event::Prompt {
                    url: new_url,
                    code: new_code,
                },
            ) => {
                if new_url.is_some() {
                    url.clone_from(new_url);
                }
                if new_code.is_some() {
                    code.clone_from(new_code);
                }
                None
            }
            (State::LoggingIn { .. }, Event::LoginEnded { error: None }) => {
                effects.push(Effect::Check);
                Some(State::Unknown {})
            }
            (
                State::LoggingIn { .. },
                Event::LoginEnded {
                    error: Some(message),
                },
            ) => Some(State::Failed {
                message: message.clone(),
            }),
            (State::LoggingIn { .. }, Event::Checked(SignInState::SignedIn { account })) => {
                effects.push(Effect::StopLogin);
                Some(State::SignedIn {
                    account: account.clone(),
                })
            }
            (State::LoggingIn { .. }, Event::Cancel) => {
                effects.push(Effect::StopLogin);
                effects.push(Effect::Check);
                Some(State::Unknown {})
            }
            (State::LoggingIn { .. }, _) => None,
            (State::SignedIn { .. }, Event::Logout) => {
                effects.push(Effect::Logout);
                Some(State::Unknown {})
            }
            (_, Event::Checked(probe)) => Some(match probe {
                SignInState::SignedIn { account } => State::SignedIn {
                    account: account.clone(),
                },
                SignInState::SignedOut => State::SignedOut {},
                SignInState::Failed { message } => State::Failed {
                    message: message.clone(),
                },
                SignInState::Unknown | SignInState::LoggingIn { .. } => State::Unknown {},
            }),
            (_, Event::Login) => {
                effects.push(Effect::StartLogin);
                Some(State::LoggingIn {
                    url: None,
                    code: None,
                })
            }
            _ => None,
        };
        if let Some(next) = next {
            self.state = next;
        }
    }
}

fn public_state(state: &State, engine: EngineId) -> SignInState {
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
            accepts_code: crate::engines::host_spec(engine).accepts_login_code,
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
    machine: Machine,
    generation: u64,
    /// The binary of the last call; follow-up probes use it.
    binary: Option<PathBuf>,
    lease: Option<Arc<crate::BinaryLease>>,
    probe: Option<tokio_util::task::AbortOnDropHandle<()>>,
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
                machine: Machine::default(),
                generation: 0,
                binary: None,
                lease: None,
                probe: None,
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

    pub(crate) fn context_generation(&self) -> u64 {
        self.lock().generation
    }

    pub(crate) fn with_context<R>(&self, generation: u64, commit: impl FnOnce() -> R) -> Option<R> {
        let inner = self.lock();
        (inner.generation == generation).then(commit)
    }

    pub(crate) fn login_leased(self: &Arc<Self>, lease: crate::BinaryLease) {
        let binary = lease.path().to_owned();
        self.lock().lease = Some(Arc::new(lease));
        self.login(&binary);
    }

    pub(crate) fn logout_leased(self: &Arc<Self>, lease: crate::BinaryLease) {
        let binary = lease.path().to_owned();
        self.lock().lease = Some(Arc::new(lease));
        self.logout(&binary);
    }

    pub(crate) fn stop_owner(&self) {
        let mut inner = self.lock();
        inner.generation = inner.generation.wrapping_add(1);
        inner.lease = None;
        inner.probe = None;
        if let Some(login) = inner.login.take() {
            login.cancel.cancel();
        }
    }

    /// Probes the engine now and settles the state on the result.
    pub async fn check(self: &Arc<Self>, binary: &Path) {
        {
            let mut inner = self.lock();
            inner.binary = Some(binary.to_owned());
            inner.generation = inner.generation.wrapping_add(1);
        }
        self.recheck().await;
    }

    async fn recheck(self: &Arc<Self>) {
        let (binary, generation, lease) = {
            let mut inner = self.lock();
            let Some(binary) = inner.binary.clone() else {
                return;
            };
            inner.generation = inner.generation.wrapping_add(1);
            (binary, inner.generation, inner.lease.clone())
        };
        self.probe_snapshot(binary, generation, lease).await;
    }

    async fn probe_snapshot(
        self: &Arc<Self>,
        binary: PathBuf,
        generation: u64,
        _lease: Option<Arc<crate::BinaryLease>>,
    ) {
        if self.context_generation() != generation {
            return;
        }
        let probe = check(self.engine, &binary, &self.home).await;
        self.handle_if_generation(&Event::Checked(probe), Some(generation));
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
    #[expect(
        clippy::significant_drop_tightening,
        reason = "`stdin` borrows from the guard and is written until the end"
    )]
    pub async fn submit_code(&self, code: &str) -> Result<()> {
        let stdin = self
            .lock()
            .login
            .as_ref()
            .map(|login| Arc::clone(&login.stdin))
            .context("no sign-in in progress")?;
        let mut guard = stdin.lock().await;
        let stdin = guard
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
        self.handle_if_generation(event, None);
    }

    fn handle_if_generation(self: &Arc<Self>, event: &Event, generation: Option<u64>) {
        self.handle_guarded(event, generation, None);
    }

    fn handle_login(self: &Arc<Self>, event: &Event, token: &CancellationToken) {
        self.handle_guarded(event, None, Some(token));
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "the state is published under the lock so concurrent events publish in order"
    )]
    fn handle_guarded(
        self: &Arc<Self>,
        event: &Event,
        generation: Option<u64>,
        token: Option<&CancellationToken>,
    ) {
        let mut effects = Vec::new();
        let (binary, lease, current_generation);
        {
            let mut inner = self.lock();
            if token.is_some_and(|token| {
                inner
                    .login
                    .as_ref()
                    .is_none_or(|login| &login.cancel != token)
            }) {
                return;
            }
            if generation.is_some_and(|generation| generation != inner.generation) {
                return;
            }
            if matches!(event, Event::Login | Event::Logout | Event::Cancel) {
                inner.generation = inner.generation.wrapping_add(1);
            }
            if matches!(event, Event::LoginEnded { .. }) {
                inner.login = None;
            }
            inner.machine.handle_with_context(event, &mut effects);
            let state = public_state(inner.machine.state(), self.engine);
            binary = inner.binary.clone();
            lease = inner.lease.clone();
            current_generation = inner.generation;
            if !matches!(state, SignInState::LoggingIn { .. })
                && !effects
                    .iter()
                    .any(|effect| matches!(effect, Effect::Check | Effect::Logout))
            {
                inner.lease = None;
            }
            self.state.send_if_modified(|current| {
                let changed = *current != state;
                *current = state;
                changed
            });
        }
        for effect in &effects {
            self.perform(effect, binary.clone(), lease.clone(), current_generation);
        }
    }

    fn perform(
        self: &Arc<Self>,
        effect: &Effect,
        binary: Option<PathBuf>,
        lease: Option<Arc<crate::BinaryLease>>,
        generation: u64,
    ) {
        let this = Arc::clone(self);
        match effect {
            Effect::Check => {
                if let Some(binary) = binary {
                    let task = tokio::spawn(async move {
                        this.probe_snapshot(binary, generation, lease).await;
                    });
                    self.lock().probe = Some(tokio_util::task::AbortOnDropHandle::new(task));
                }
            }
            Effect::StartLogin => {
                if let Some(binary) = binary {
                    self.start_login(binary, lease, generation);
                }
            }
            Effect::StopLogin => {
                let login = self.lock().login.take();
                if let Some(login) = login {
                    login.cancel.cancel();
                }
            }
            Effect::Logout => {
                let task = tokio::spawn(async move {
                    let Some(binary) = binary else {
                        return;
                    };
                    if this.context_generation() != generation {
                        return;
                    }
                    if this.engine == EngineId::OpenCode {
                        if let Err(error) = opencode_logout(&binary, &this.home).await {
                            tracing::warn!(engine = %this.engine, error = %format!("{error:#}"), "logout failed");
                        }
                        this.probe_snapshot(binary, generation, lease).await;
                        return;
                    }
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
                            tracing::warn!(engine = %this.engine, error = %error, "logout unavailable");
                        }
                    }
                    this.probe_snapshot(binary, generation, lease).await;
                });
                self.lock().probe = Some(tokio_util::task::AbortOnDropHandle::new(task));
            }
        }
    }

    fn start_login(
        self: &Arc<Self>,
        binary: PathBuf,
        lease: Option<Arc<crate::BinaryLease>>,
        _generation: u64,
    ) {
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
                .run_login(binary, lease, stdin, cancel.clone())
                .await
                .err()
                .map(|error| format!("{error:#}"));
            if !cancel.is_cancelled() {
                if let Some(error) = &error {
                    tracing::warn!(engine = %this.engine, error, "sign-in failed");
                }
                this.handle_login(&Event::LoginEnded { error }, &cancel);
            }
        });
    }

    async fn run_login(
        self: &Arc<Self>,
        binary: PathBuf,
        _lease: Option<Arc<crate::BinaryLease>>,
        stdin_slot: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
        cancel: CancellationToken,
    ) -> Result<()> {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let usage = crate::install::usage_lock(&self.home)?;
        fs2::FileExt::lock_shared(&usage)?;
        if cancel.is_cancelled() {
            return Ok(());
        }
        let args = login_args(self.engine)?;
        let mut command = engine_command(self.engine, &binary, &self.home);
        command
            .args(args)
            .stdin(Stdio::piped())
            .env("NO_OPEN_BROWSER", "1");
        // The Editor opens the link, possibly on another machine.
        #[cfg(unix)]
        command.env("BROWSER", "true");
        let mut child = crate::process::owned_command(command)
            .with_context(|| format!("starting {} {}", binary.display(), args.join(" ")))?;
        *stdin_slot.lock().await = child.stdin().take();
        let (chunks, mut received) = mpsc::channel::<String>(16);
        #[expect(
            clippy::collection_is_never_read,
            reason = "holds the pump tasks; dropping it aborts them when the sign-in ends"
        )]
        let mut pumps = Vec::new();
        if let Some(stdout) = child.stdout().take() {
            pumps.push(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
                forward(stdout, chunks.clone()),
            )));
        }
        if let Some(stderr) = child.stderr().take() {
            pumps.push(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
                forward(stderr, chunks),
            )));
        }
        let mut output = String::new();
        let mut terminal_parser = vte::Parser::new();
        let mut terminal_text = TerminalText::default();
        let mut scan_start: usize = 0;
        let mut shown = (None, None);
        let deadline = tokio::time::sleep(LOGIN_TIMEOUT);
        tokio::pin!(deadline);
        let status = loop {
            tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                () = &mut deadline => bail!("{} sign-in timed out", self.engine.display_name()),
                Some(chunk) = received.recv() => {
                    terminal_parser.advance(&mut terminal_text, chunk.as_bytes());
                    let plain = std::mem::take(&mut terminal_text.text);
                    output.push_str(&plain);
                    if output.len() > LOGIN_OUTPUT_CAP {
                        let mut cut = output.len() - LOGIN_OUTPUT_CAP;
                        while !output.is_char_boundary(cut) { cut += 1; }
                        output.drain(..cut);
                        scan_start = scan_start.saturating_sub(cut);
                    }
                    let found = parse_login_output(&output[scan_start..]);
                    scan_start = output.rfind('\n').map_or(0, |index| index + 1);
                    let found = (found.0.or_else(|| shown.0.clone()), found.1.or_else(|| shown.1.clone()));
                    if found != shown {
                        shown = found.clone();
                        self.handle_login(&Event::Prompt { url: found.0, code: found.1 }, &cancel);
                    }
                }
                status = child.wait() => break status.context("waiting for the sign-in")?,
            }
        };
        child.shutdown();
        if status.success() {
            return Ok(());
        }
        // The pipes close with the process; keep what it printed last. Pipes
        // still open after a second are left behind.
        tokio::time::timeout(Duration::from_secs(1), async {
            while let Some(chunk) = received.recv().await {
                terminal_parser.advance(&mut terminal_text, chunk.as_bytes());
                let plain = std::mem::take(&mut terminal_text.text);
                let mut take = (LOGIN_OUTPUT_CAP - output.len()).min(plain.len());
                while !plain.is_char_boundary(take) {
                    take -= 1;
                }
                output.push_str(&plain[..take]);
            }
        })
        .await
        .ok();
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
        let results: Vec<_> = [("claude", claude), ("codex", codex)]
            .into_iter()
            .map(|(engine, text)| (engine, parse_login_output(text)))
            .collect();
        insta::assert_debug_snapshot!(results);
    }

    /// OpenCode's device sign-in, as 2.1.0 prints it: the code ends a line.
    #[test]
    fn opencode_device_login_yields_link_and_code() {
        let opencode = "\u{250c}  Connect an integration\n\u{25cf}  Enter code: HNCP-VWCS\n\u{2502}\n\u{25cf}  https://opencode.ai/console/device?user_code=HNCP-VWCS&client_id=opencode-cli\n\u{25c7}  Waiting for authorization...\n";
        assert_eq!(
            parse_login_output(opencode),
            (
                Some(
                    "https://opencode.ai/console/device?user_code=HNCP-VWCS&client_id=opencode-cli"
                        .to_owned()
                ),
                Some("HNCP-VWCS".to_owned())
            )
        );
    }

    /// OpenCode is signed in only through a stored login (its Console
    /// account, which also serves Go); signing out removes every login.
    #[cfg(unix)]
    #[tokio::test]
    async fn opencode_is_signed_in_by_its_logins_and_signs_out_of_all() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        let home = fixture.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let binary = fixture.path().join("opencode");
        std::fs::write(
            &binary,
            r#"#!/bin/sh
case "$*" in
 "api --standalone integration.list")
  if [ -e "$PWD/console" ]; then login='[{"type":"credential","id":"cred_x","label":"Default","method":"oauth"}]'; else login='[]'; fi
  printf '{"data":[{"id":"opencode","name":"OpenCode Console","connections":%s},{"id":"opencode-go","name":"OpenCode Go","connections":[]}]}' "$login" ;;
 "auth logout --standalone opencode cred_x") rm "$PWD/console" ;;
 *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let sign_in = SignIn::new(EngineId::OpenCode, home.clone());
        sign_in.check(&binary).await;
        assert_eq!(sign_in.state(), SignInState::SignedOut);
        std::fs::write(home.join("console"), "").unwrap();
        sign_in.check(&binary).await;
        assert_eq!(
            sign_in.state(),
            SignInState::SignedIn {
                account: Some("OpenCode Console".to_owned())
            }
        );
        sign_in.logout(&binary);
        tokio::time::timeout(Duration::from_secs(5), async {
            while sign_in.state() != SignInState::SignedOut {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("signed out of every login");
        assert!(!home.join("console").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_status_probes_cannot_restore_an_account_after_logout() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        let home = fixture.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let binary = fixture.path().join("claude");
        std::fs::write(
            &binary,
            r#"#!/bin/sh
if [ "$2" = "logout" ]; then touch "$PWD/out"; exit 0; fi
if [ -e "$PWD/out" ]; then
  echo '{"loggedIn":false}'
else
  if [ -e "$PWD/slow" ]; then touch "$PWD/check-started"; sleep 0.2; fi
  echo '{"loggedIn":true,"email":"old"}'
fi
"#,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let sign_in = SignIn::new(EngineId::Claude, home.clone());
        sign_in.check(&binary).await;
        assert!(matches!(sign_in.state(), SignInState::SignedIn { .. }));
        std::fs::write(home.join("slow"), "").unwrap();
        let probe_owner = Arc::clone(&sign_in);
        let probe_binary = binary.clone();
        let old_probe = tokio::spawn(async move { probe_owner.check(&probe_binary).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !home.join("check-started").exists() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        sign_in.logout(&binary);
        tokio::time::timeout(Duration::from_secs(2), async {
            while sign_in.state() != SignInState::SignedOut {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        old_probe.await.unwrap();
        assert_eq!(sign_in.state(), SignInState::SignedOut);
    }

    /// The sign-in lifecycle as the Editor sees it: signed out, logging in
    /// with the link, then signed in once the probe confirms.
    #[test]
    fn login_walks_from_signed_out_to_signed_in() {
        let mut machine = Machine::default();
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
            seen.push((
                public_state(machine.state(), EngineId::Codex),
                std::mem::take(&mut effects),
            ));
        }
        insta::assert_debug_snapshot!(seen);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::{Effect, Event, Machine};
    use artisan_api::SignInState;

    #[test]
    fn a_stale_signed_out_probe_does_not_interrupt_login_and_cancel_stops_before_checking() {
        let mut machine = Machine::default();
        let mut effects = Vec::new();
        machine.handle_with_context(&Event::Login, &mut effects);
        assert_eq!(std::mem::take(&mut effects), [Effect::StartLogin]);
        machine.handle_with_context(
            &Event::Prompt {
                url: Some("https://auth.example".into()),
                code: None,
            },
            &mut effects,
        );
        machine.handle_with_context(&Event::Checked(SignInState::SignedOut), &mut effects);
        machine.handle_with_context(&Event::Login, &mut effects);
        assert!(effects.is_empty());
        assert!(
            matches!(machine.state(), super::State::LoggingIn { url: Some(url), code: None }
            if url == "https://auth.example")
        );
        machine.handle_with_context(&Event::Cancel, &mut effects);
        assert_eq!(effects, [Effect::StopLogin, Effect::Check]);
        assert!(matches!(machine.state(), super::State::Unknown {}));
    }

    #[test]
    fn a_successful_probe_stops_an_active_login_before_publishing_the_account() {
        let mut machine = Machine::default();
        let mut effects = Vec::new();
        machine.handle_with_context(&Event::Login, &mut effects);
        effects.clear();
        machine.handle_with_context(
            &Event::Checked(SignInState::SignedIn {
                account: Some("alice".into()),
            }),
            &mut effects,
        );
        assert_eq!(effects, [Effect::StopLogin]);
        assert!(
            matches!(machine.state(), super::State::SignedIn { account: Some(account) } if account == "alice")
        );
    }
}
