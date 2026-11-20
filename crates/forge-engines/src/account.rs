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
const LOGIN_TIMEOUT: Duration = Duration::from_mins(15);
const LOGIN_OUTPUT_CAP: usize = 64 * 1024;

pub(crate) fn engine_command(
    engine: EngineId,
    binary: &Path,
    home: &Path,
) -> Result<Command, crate::sandbox::GuardError> {
    crate::sandbox::guard::restore(engine, home)?;
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
        && let Err(error) = crate::sandbox::home::make_folder(home, Path::new(directory))
    {
        tracing::warn!(home = %home.display(), %error, "couldn't create engine configuration directory");
    }
    Ok(command)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RunShortError {
    #[error("restore the configuration of {} before running it", engine.display_name())]
    Guard {
        engine: EngineId,
        #[source]
        source: crate::sandbox::GuardError,
    },
    #[error("open the usage lock of {}", engine.display_name())]
    UsageLock {
        engine: EngineId,
        #[source]
        source: crate::install::UsageLockError,
    },
    #[error("share the usage lock of {}", engine.display_name())]
    Share {
        engine: EngineId,
        #[source]
        source: std::io::Error,
    },
    #[error("run {} {args}", engine.display_name())]
    Probe {
        engine: EngineId,
        args: String,
        #[source]
        source: Box<crate::process::ProbeError>,
    },
}

pub(crate) async fn run_short(
    engine: EngineId,
    binary: &Path,
    home: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<(bool, String, String), RunShortError> {
    let usage = crate::install::usage_lock(home)
        .map_err(|source| RunShortError::UsageLock { engine, source })?;
    fs2::FileExt::lock_shared(&usage).map_err(|source| RunShortError::Share { engine, source })?;
    let mut command = engine_command(engine, binary, home)
        .map_err(|source| RunShortError::Guard { engine, source })?;
    command.args(args);
    let output = crate::process::probe(command, timeout, 1024 * 1024)
        .await
        .map_err(|source| RunShortError::Probe {
            engine,
            args: args.join(" "),
            source: Box::new(source),
        })?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

async fn check(engine: EngineId, binary: &Path, home: &Path) -> SignInState {
    let args: &[&str] = match engine {
        EngineId::Claude => &["auth", "status"],
        EngineId::Codex => &["login", "status"],
        EngineId::OpenCode => return opencode_sign_in(binary, home).await,
    };
    match run_short(engine, binary, home, args, CHECK_TIMEOUT).await {
        Ok((success, stdout, stderr)) => parse_status(engine, success, &stdout, &stderr),
        Err(error) => {
            let error = diagnostics::chain(&error);
            tracing::warn!(engine = %engine, home = %home.display(), error, "sign-in check failed");
            SignInState::Failed {
                message: format!(
                    "Couldn't check the {} sign-in: {error}",
                    engine.display_name()
                ),
            }
        }
    }
}

fn parse_status(engine: EngineId, success: bool, stdout: &str, stderr: &str) -> SignInState {
    let text = strip_ansi(&format!("{stdout}\n{stderr}"));
    let lower = text.to_lowercase();
    if engine == EngineId::Claude {
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
            let at = line.to_ascii_lowercase().find("logged in as ")?;
            let (_, account) = line.split_at(at + "logged in as ".len());
            Some(account.trim().to_owned()).filter(|account| !account.is_empty())
        });
        return SignInState::SignedIn { account };
    }
    let detail = last_line(&text);
    SignInState::Failed {
        message: format!(
            "{} sign-in status is unavailable: {detail}",
            engine.display_name()
        ),
    }
}

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
            let error = diagnostics::chain(&error);
            tracing::warn!(home = %home.display(), error, "OpenCode sign-in check failed");
            SignInState::Failed {
                message: format!("Couldn't check the OpenCode sign-in: {error}"),
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Connection {
    integration: String,
    name: String,
    credential: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum OpenCodeConnectionsError {
    #[error("list OpenCode's integrations")]
    Run(#[source] RunShortError),
    #[error("OpenCode could not list its integrations ({stderr_bytes} bytes on stderr)")]
    Refused { stderr_bytes: usize },
    #[error("read OpenCode's integrations")]
    Decode(#[source] serde_json::Error),
}

async fn opencode_connections(
    binary: &Path,
    home: &Path,
) -> Result<Vec<Connection>, OpenCodeConnectionsError> {
    let (success, stdout, stderr) = run_short(
        EngineId::OpenCode,
        binary,
        home,
        &["api", "--standalone", "integration.list"],
        CHECK_TIMEOUT,
    )
    .await
    .map_err(OpenCodeConnectionsError::Run)?;
    if !success {
        return Err(OpenCodeConnectionsError::Refused {
            stderr_bytes: stderr.len(),
        });
    }
    let listing: serde_json::Value =
        serde_json::from_str(&stdout).map_err(OpenCodeConnectionsError::Decode)?;
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

#[derive(Debug, thiserror::Error)]
enum OpenCodeLogoutError {
    #[error("find OpenCode's logins")]
    Connections(#[source] OpenCodeConnectionsError),
    #[error("sign out of {name}")]
    Run {
        name: String,
        #[source]
        source: RunShortError,
    },
    #[error("OpenCode refused to sign out of {name} ({stderr_bytes} bytes on stderr)")]
    Refused { name: String, stderr_bytes: usize },
}

async fn opencode_logout(binary: &Path, home: &Path) -> Result<(), OpenCodeLogoutError> {
    for connection in opencode_connections(binary, home)
        .await
        .map_err(OpenCodeLogoutError::Connections)?
    {
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
        .await
        .map_err(|source| OpenCodeLogoutError::Run {
            name: connection.name.clone(),
            source,
        })?;
        if !success {
            return Err(OpenCodeLogoutError::Refused {
                name: connection.name,
                stderr_bytes: stderr.len(),
            });
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
#[error("{} signs in per provider; use its interactive CLI sign-in", engine.display_name())]
pub(crate) struct NoDeviceLogin {
    pub engine: EngineId,
}

#[derive(Debug, thiserror::Error)]
#[error("{} signs out per provider; use its interactive CLI", engine.display_name())]
pub(crate) struct NoLogout {
    pub engine: EngineId,
}

fn login_args(engine: EngineId) -> Result<&'static [&'static str], NoDeviceLogin> {
    crate::engines::host_spec(engine)
        .device_login
        .ok_or(NoDeviceLogin { engine })
}

fn logout_args(engine: EngineId) -> Result<&'static [&'static str], NoLogout> {
    crate::engines::host_spec(engine)
        .logout
        .ok_or(NoLogout { engine })
}

fn parse_login_output(text: &str) -> (Option<String>, Option<String>) {
    let text = strip_ansi(text);
    let url = text
        .split_whitespace()
        .find(|word| word.starts_with("https://"))
        .map(|word| word.trim_end_matches(['.', ',', ')']).to_owned());
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

fn strip_ansi(text: &str) -> String {
    let mut parser = vte::Parser::new();
    let mut plain = TerminalText::default();
    parser.advance(&mut plain, text.as_bytes());
    plain.text
}

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

#[derive(Debug)]
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

#[derive(Debug)]
struct LiveLogin {
    stdin: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    cancel: CancellationToken,
}

#[derive(Debug)]
struct Inner {
    machine: Machine,
    generation: u64,
    binary: Option<PathBuf>,
    lease: Option<Arc<crate::BinaryLease>>,
    probe: Option<tokio_util::task::AbortOnDropHandle<()>>,
    login: Option<LiveLogin>,
}

#[derive(Debug)]
pub struct SignIn {
    engine: EngineId,
    home: PathBuf,
    state: watch::Sender<SignInState>,
    inner: Mutex<Inner>,
}

impl SignIn {
    #[must_use]
    pub(crate) fn new(engine: EngineId, home: PathBuf) -> Arc<Self> {
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
    pub(crate) fn state(&self) -> SignInState {
        self.state.borrow().clone()
    }

    #[must_use]
    pub(crate) fn subscribe(&self) -> watch::Receiver<SignInState> {
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

    pub(crate) async fn check(self: &Arc<Self>, binary: &Path) {
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

    fn login(self: &Arc<Self>, binary: &Path) {
        self.lock().binary = Some(binary.to_owned());
        self.handle(&Event::Login);
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "`stdin` borrows from the guard and is written until the end"
    )]
    pub(crate) async fn submit_code(&self, code: &str) -> Result<(), SubmitCodeError> {
        let engine = self.engine;
        let stdin = self
            .lock()
            .login
            .as_ref()
            .map(|login| Arc::clone(&login.stdin))
            .ok_or(SubmitCodeError::NoSignIn { engine })?;
        let mut guard = stdin.lock().await;
        let stdin = guard.as_mut().ok_or(SubmitCodeError::Closed { engine })?;
        let write = |source| SubmitCodeError::Write { engine, source };
        stdin
            .write_all(format!("{}\n", code.trim()).as_bytes())
            .await
            .map_err(write)?;
        stdin.flush().await.map_err(write)?;
        Ok(())
    }

    pub(crate) fn cancel_login(self: &Arc<Self>) {
        self.handle(&Event::Cancel);
    }

    fn logout(self: &Arc<Self>, binary: &Path) {
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
                            tracing::warn!(engine = %this.engine, error = diagnostics::chain(&error), "logout failed");
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
                                tracing::warn!(engine = %this.engine, error = diagnostics::chain(&error), "logout failed");
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
                .map(|error| diagnostics::chain(&error));
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
    ) -> Result<(), LoginError> {
        let engine = self.engine;
        if cancel.is_cancelled() {
            return Ok(());
        }
        let usage = crate::install::usage_lock(&self.home)
            .map_err(|source| LoginError::UsageLock { engine, source })?;
        fs2::FileExt::lock_shared(&usage).map_err(|source| LoginError::Share { engine, source })?;
        if cancel.is_cancelled() {
            return Ok(());
        }
        let args = login_args(engine).map_err(LoginError::NoDeviceLogin)?;
        let mut command = engine_command(self.engine, &binary, &self.home)
            .map_err(|source| LoginError::Guard { engine, source })?;
        command
            .args(args)
            .stdin(Stdio::piped())
            .env("NO_OPEN_BROWSER", "1");
        #[cfg(unix)]
        command.env("BROWSER", "true");
        let mut child = crate::process::owned_command(command, crate::sandbox::Sandbox::Open)
            .map_err(|source| LoginError::Start {
                engine,
                args: args.join(" "),
                source,
            })?;
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
                () = &mut deadline => return Err(LoginError::TimedOut { engine }),
                Some(chunk) = received.recv() => {
                    terminal_parser.advance(&mut terminal_text, chunk.as_bytes());
                    let plain = std::mem::take(&mut terminal_text.text);
                    output.push_str(&plain);
                    if output.len() > LOGIN_OUTPUT_CAP {
                        let cut = output.ceil_char_boundary(output.len() - LOGIN_OUTPUT_CAP);
                        output.drain(..cut);
                        scan_start = scan_start.saturating_sub(cut);
                    }
                    let found = parse_login_output(output.split_at(output.floor_char_boundary(scan_start)).1);
                    scan_start = output.rfind('\n').map_or(0, |index| index + 1);
                    let found = (found.0.or_else(|| shown.0.clone()), found.1.or_else(|| shown.1.clone()));
                    if found != shown {
                        shown = found.clone();
                        self.handle_login(&Event::Prompt { url: found.0, code: found.1 }, &cancel);
                    }
                }
                status = child.wait() => break status.map_err(|source| LoginError::Wait { engine, source })?,
            }
        };
        child.end().await;
        if status.success() {
            return Ok(());
        }
        let _drained_or_timed_out = tokio::time::timeout(Duration::from_secs(1), async {
            while let Some(chunk) = received.recv().await {
                terminal_parser.advance(&mut terminal_text, chunk.as_bytes());
                let plain = std::mem::take(&mut terminal_text.text);
                let (kept, _) =
                    plain.split_at(plain.floor_char_boundary(LOGIN_OUTPUT_CAP - output.len()));
                output.push_str(kept);
            }
        })
        .await;
        Err(LoginError::Failed {
            engine,
            detail: last_line(&strip_ansi(&output)).to_owned(),
        })
    }
}

fn last_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("no output")
}

#[derive(Debug, thiserror::Error)]
pub enum SubmitCodeError {
    #[error("no {} sign-in in progress", engine.display_name())]
    NoSignIn { engine: EngineId },
    #[error("the {} sign-in no longer accepts input", engine.display_name())]
    Closed { engine: EngineId },
    #[error("send the code to the {} sign-in", engine.display_name())]
    Write {
        engine: EngineId,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
enum LoginError {
    #[error("restore the configuration of {} before signing in", engine.display_name())]
    Guard {
        engine: EngineId,
        #[source]
        source: crate::sandbox::GuardError,
    },
    #[error("open the usage lock of {}", engine.display_name())]
    UsageLock {
        engine: EngineId,
        #[source]
        source: crate::install::UsageLockError,
    },
    #[error("share the usage lock of {}", engine.display_name())]
    Share {
        engine: EngineId,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    NoDeviceLogin(NoDeviceLogin),
    #[error("start {} {args}", engine.display_name())]
    Start {
        engine: EngineId,
        args: String,
        #[source]
        source: crate::process::SpawnError,
    },
    #[error("{} sign-in timed out", engine.display_name())]
    TimedOut { engine: EngineId },
    #[error("wait for the {} sign-in", engine.display_name())]
    Wait {
        engine: EngineId,
        #[source]
        source: std::io::Error,
    },
    #[error("{} sign-in failed: {detail}", engine.display_name())]
    Failed { engine: EngineId, detail: String },
}

async fn forward(mut stream: impl AsyncRead + Unpin, chunks: mpsc::Sender<String>) {
    let mut buffer = vec![0_u8; 4096];
    loop {
        let read = match stream.read(&mut buffer).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                tracing::warn!(%error, "the sign-in output stopped being read");
                break;
            }
        };
        if chunks
            .send(String::from_utf8_lossy(&buffer[..read]).into_owned())
            .await
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
#[path = "../tests/account/unit.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/account/lifecycle.rs"]
mod lifecycle_tests;
