use crate::EngineCommand;
use crate::paths::{self, LocateError};
use crate::ui::{self, AskError};
use artisan_api::{ClaudeAiState, EngineId, EngineStatus, InstallState, SignInState};
use artisan_paths::App;
use claude_ai_browser::SignInError;
use forge_engines::{ClaudeAiSignInError, EngineChange, Engines, NotInstalled, SubmitCodeError};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitStatus;
use tokio::io::AsyncBufReadExt;
use tokio::sync::broadcast::error::RecvError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum EngineError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("Forge isn't installed here; run `ae install`")]
    ForgeNotInstalled,
    #[error("start the async runtime")]
    Runtime(#[source] std::io::Error),
    #[error(transparent)]
    Ask(#[from] AskError),
    #[error("install {}", engine.display_name())]
    Install {
        engine: EngineId,
        #[source]
        source: forge_engines::InstallError,
    },
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error("read the claude.ai session")]
    ReadSession(#[source] std::io::Error),
    #[error("the claude.ai session is not JSON")]
    ParseSession(#[source] serde_json::Error),
    #[error("--claude-ai is for Claude")]
    OnlyClaude,
    #[error("check the session with claude.ai")]
    KeepSession(#[source] ClaudeAiSignInError),
    #[error("sign out of claude.ai")]
    SignOutClaudeAi(#[source] forge_engines::claudeai::RemoveSessionError),
    #[error(transparent)]
    NotInstalled(#[from] NotInstalled),
    #[error(transparent)]
    Login(#[from] LoginError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CaptureError {
    #[error("sign in to claude.ai in a browser")]
    Browser(#[source] SignInError),
    #[error("ask Windows where its apps live")]
    LocalAppData(#[source] std::io::Error),
    #[error("run wslpath")]
    Wslpath(#[source] std::io::Error),
    #[error(
        "no browser in WSL, and no Windows `ae` to open Windows' browser; run `ae engine login claude --claude-ai` in Windows instead"
    )]
    NoWindowsAe,
    #[error("run {}", ae.display())]
    RunWindowsAe {
        ae: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Windows' ae could not capture the session ({status})")]
    WindowsAeFailed { status: ExitStatus },
    #[error("Windows' ae printed no session")]
    NoSession(#[source] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LoginError {
    #[error(transparent)]
    NotInstalled(#[from] NotInstalled),
    #[error("the sign-in to {} stopped reporting", engine.display_name())]
    StoppedReporting { engine: EngineId },
    #[error("{} is still signed out", engine.display_name())]
    SignedOut { engine: EngineId },
    #[error("sign-in to {} failed: {message}", engine.display_name())]
    Failed { engine: EngineId, message: String },
    #[error("read the code")]
    ReadCode(#[source] std::io::Error),
    #[error("hand the code to {}", engine.display_name())]
    SubmitCode {
        engine: EngineId,
        #[source]
        source: SubmitCodeError,
    },
    #[error("sign-in cancelled")]
    Cancelled,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum IntoWslError {
    #[error("start the async runtime")]
    Runtime(#[source] std::io::Error),
    #[error("sign in to claude.ai in a browser")]
    Capture(#[source] SignInError),
    #[error("run ae in {distro} (WSL)")]
    Run {
        distro: String,
        #[source]
        source: std::io::Error,
    },
    #[error("hand the claude.ai session to ae in {distro} (WSL)")]
    Hand {
        distro: String,
        #[source]
        source: std::io::Error,
    },
}

pub(crate) fn run(command: EngineCommand) -> Result<(), EngineError> {
    let data = paths::data(App::Forge)?;
    if !paths::binary(App::Forge)?.is_file() && !data.exists() {
        return Err(EngineError::ForgeNotInstalled);
    }
    let runtime = tokio::runtime::Runtime::new().map_err(EngineError::Runtime)?;
    let engines = Engines::new(&data);
    runtime.block_on(async move {
        match command {
            EngineCommand::List { json } => list(&engines, json).await,
            EngineCommand::Install { engines: chosen } => {
                let chosen = if chosen.is_empty() {
                    pick_engines(&engines)?
                } else {
                    chosen
                };
                for engine in chosen {
                    let version = with_progress(&engines, engine, engines.update(engine))
                        .await
                        .map_err(|source| EngineError::Install { engine, source })?;
                    ui::success(format!("{} {version} is installed", engine.display_name()));
                }
                Ok(())
            }
            EngineCommand::Login {
                capture_only: true,
                ..
            } => {
                let session = capture_in_browser().await?;
                ui::print(format_args!("{}", session_json(&session)));
                Ok(())
            }
            EngineCommand::Login {
                engine,
                claude_ai: true,
                session_stdin,
                ..
            } => {
                only_claude(engine)?;
                let session = if session_stdin {
                    let mut text = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
                        .map_err(EngineError::ReadSession)?;
                    serde_json::from_str(&text).map_err(EngineError::ParseSession)?
                } else {
                    ui::intro("Sign in to claude.ai");
                    capture_here().await?
                };
                keep_claude_ai(&engines, session).await
            }
            EngineCommand::Login { engine, .. } => {
                let engine = pick_engine(&engines, engine, "Sign in to", "which engine to sign in to")?;
                Ok(login(&engines, engine).await?)
            }
            EngineCommand::Logout {
                engine,
                claude_ai: true,
            } => {
                only_claude(engine)?;
                engines.claude_ai_sign_out().map_err(EngineError::SignOutClaudeAi)?;
                ui::success("Signed out of claude.ai. claude.ai mode runs on Claude Code until you sign in again.");
                Ok(())
            }
            EngineCommand::Logout { engine, .. } => {
                let engine = pick_engine(&engines, engine, "Sign out of", "which engine to sign out of")?;
                engines.logout(engine)?;
                ui::success(format!("Signing {} out.", engine.display_name()));
                Ok(())
            }
        }
    })
}

fn session_json(session: &artisan_api::ClaudeAiSession) -> String {
    serde_json::to_string(session)
        .expect("invariant: a claude.ai session always serializes as JSON")
}

fn pick_engine(
    engines: &Engines,
    engine: Option<EngineId>,
    prompt: &str,
    question: &str,
) -> Result<EngineId, AskError> {
    match engine {
        Some(engine) => Ok(engine),
        None if ui::interactive() => {
            let mut select = cliclack::select(prompt);
            for engine in EngineId::ALL {
                select = select.item(
                    engine,
                    engine.display_name(),
                    install_line(&engines.status(engine).install),
                );
            }
            select.interact().map_err(ui::prompt_error)
        }
        None => Err(ui::needs_flag(question, "an engine name")),
    }
}

fn only_claude(engine: Option<EngineId>) -> Result<(), EngineError> {
    if engine.is_none_or(|engine| engine == EngineId::Claude) {
        Ok(())
    } else {
        Err(EngineError::OnlyClaude)
    }
}

async fn keep_claude_ai(
    engines: &Engines,
    session: artisan_api::ClaudeAiSession,
) -> Result<(), EngineError> {
    let progress = ui::Progress::spinner("Checking the session with claude.ai");
    let state = engines.claude_ai_sign_in(session).await;
    progress.clear();
    if let ClaudeAiState::SignedIn {
        account: Some(account),
    } = state.map_err(EngineError::KeepSession)?
    {
        ui::outro(format!(
            "Signed in to claude.ai as {account}. Turn on claude.ai mode in the Editor's settings to use it."
        ));
    } else {
        ui::outro(
            "Signed in to claude.ai. Turn on claude.ai mode in the Editor's settings to use it.",
        );
    }
    Ok(())
}

async fn capture_here() -> Result<artisan_api::ClaudeAiSession, CaptureError> {
    if crate::platform::in_wsl() && claude_ai_browser::find_browser().is_none() {
        return capture_through_windows().await;
    }
    capture_in_browser().await
}

async fn capture_in_browser() -> Result<artisan_api::ClaudeAiSession, CaptureError> {
    sign_in_here().await.map_err(CaptureError::Browser)
}

async fn sign_in_here() -> Result<artisan_api::ClaudeAiSession, SignInError> {
    if !claude_ai_browser::has_profile() {
        ui::note(
            "A browser window opens on claude.ai",
            "Sign in there as you normally would. The window closes by itself once you're signed in.\nArtisan keeps this browser profile so it can renew the session later without asking.",
        );
    }
    let progress = ui::Progress::spinner("Waiting for you to sign in to claude.ai");
    let session = claude_ai_browser::sign_in().await;
    progress.clear();
    session
}

async fn capture_through_windows() -> Result<artisan_api::ClaudeAiSession, CaptureError> {
    let local = tokio::process::Command::new("cmd.exe")
        .args(["/c", "echo %LOCALAPPDATA%"])
        .current_dir("/mnt/c")
        .output()
        .await
        .map_err(CaptureError::LocalAppData)?;
    let local = String::from_utf8_lossy(&local.stdout).trim().to_owned();
    let converted = tokio::process::Command::new("wslpath")
        .args(["-u", &local])
        .output()
        .await
        .map_err(CaptureError::Wslpath)?;
    let ae = PathBuf::from(String::from_utf8_lossy(&converted.stdout).trim())
        .join("Artisan Street")
        .join("ae")
        .join("ae.exe");
    if !ae.is_file() {
        return Err(CaptureError::NoWindowsAe);
    }
    let output = tokio::process::Command::new(&ae)
        .args(["engine", "login", "claude", "--claude-ai", "--capture-only"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .output()
        .await
        .map_err(|source| CaptureError::RunWindowsAe {
            ae: ae.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(CaptureError::WindowsAeFailed {
            status: output.status,
        });
    }
    serde_json::from_slice(&output.stdout).map_err(CaptureError::NoSession)
}

pub(crate) fn claude_ai_into_wsl(distro: &str) -> Result<std::process::ExitCode, IntoWslError> {
    let runtime = tokio::runtime::Runtime::new().map_err(IntoWslError::Runtime)?;
    ui::intro("Sign in to claude.ai");
    let session = runtime
        .block_on(sign_in_here())
        .map_err(IntoWslError::Capture)?;
    let mut child = crate::wsl::ae(distro)
        .args([
            "engine",
            "login",
            "claude",
            "--claude-ai",
            "--session-stdin",
        ])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|source| IntoWslError::Run {
            distro: distro.to_owned(),
            source,
        })?;
    {
        let mut stdin = child
            .stdin
            .take()
            .expect("invariant: the WSL ae was started with a piped stdin");
        std::io::Write::write_all(&mut stdin, session_json(&session).as_bytes()).map_err(
            |source| IntoWslError::Hand {
                distro: distro.to_owned(),
                source,
            },
        )?;
    }
    let status = child.wait().map_err(|source| IntoWslError::Run {
        distro: distro.to_owned(),
        source,
    })?;
    Ok(crate::wsl::exit_code(status))
}

fn claude_ai_line(state: &ClaudeAiState) -> String {
    match state {
        ClaudeAiState::SignedOut => "not signed in".to_owned(),
        ClaudeAiState::SignedIn {
            account: Some(account),
        } => format!("signed in as {account}"),
        ClaudeAiState::SignedIn { account: None } => "signed in".to_owned(),
        ClaudeAiState::NeedsRenewal { .. } => "needs renewing in a browser".to_owned(),
        ClaudeAiState::Expired { reason } => format!("expired: {reason}"),
    }
}

async fn list(engines: &Engines, json: bool) -> Result<(), EngineError> {
    let progress = if json {
        None
    } else {
        Some(ui::Progress::spinner("Checking sign-in"))
    };
    for engine in EngineId::ALL {
        engines.check_sign_in(engine).await;
    }
    if let Some(progress) = progress {
        progress.clear();
    }
    let statuses: Vec<EngineStatus> = EngineId::ALL
        .iter()
        .map(|engine| engines.status(*engine))
        .collect();
    if json {
        let list: Vec<_> = statuses
            .iter()
            .map(|status| {
                serde_json::json!({
                    "engine": status.engine.as_str(),
                    "install": install_line(&status.install),
                    "account": sign_in_line(&status.sign_in),
                    "claude_ai": status.claude_ai.as_ref().map(claude_ai_line),
                })
            })
            .collect();
        ui::print(format_args!("{}", serde_json::Value::Array(list)));
        return Ok(());
    }
    let rows: Vec<Vec<String>> = statuses
        .iter()
        .map(|status| {
            vec![
                status.engine.display_name().to_owned(),
                install_line(&status.install),
                sign_in_line(&status.sign_in),
            ]
        })
        .collect();
    ui::print(format_args!(
        "{}",
        ui::table(&["ENGINE", "VERSION", "ACCOUNT"], &rows)
    ));
    if let Some(state) = statuses.iter().find_map(|status| status.claude_ai.as_ref()) {
        ui::note("claude.ai mode", claude_ai_line(state));
    }
    Ok(())
}

fn pick_engines(engines: &Engines) -> Result<Vec<EngineId>, AskError> {
    if !ui::interactive() {
        return Err(ui::needs_flag(
            "which engines to install",
            "engine names, e.g. `claude codex`",
        ));
    }
    let mut select = cliclack::multiselect("Install or update").required(true);
    let mut initial = Vec::new();
    for engine in EngineId::ALL {
        let install = engines.status(engine).install;
        if !matches!(install, InstallState::Installed { update: None, .. }) {
            initial.push(engine);
        }
        select = select.item(engine, engine.display_name(), install_line(&install));
    }
    select
        .initial_values(initial)
        .interact()
        .map_err(ui::prompt_error)
}

fn install_line(install: &InstallState) -> String {
    match install {
        InstallState::Absent => "not installed".to_owned(),
        InstallState::Downloading { version, .. } => format!("downloading {version}"),
        InstallState::Verifying { version } => format!("verifying {version}"),
        InstallState::Installed {
            version,
            update: None,
        } => version.clone(),
        InstallState::Installed {
            version,
            update: Some(update),
        } => format!("{version} (update {update} available)"),
        InstallState::Failed { message } => format!("failed: {message}"),
    }
}

fn sign_in_line(sign_in: &SignInState) -> String {
    match sign_in {
        SignInState::Unknown => "unknown".to_owned(),
        SignInState::SignedOut => "signed out".to_owned(),
        SignInState::LoggingIn { .. } => "signing in".to_owned(),
        SignInState::SignedIn {
            account: Some(account),
        } => format!("signed in as {account}"),
        SignInState::SignedIn { account: None } => "signed in".to_owned(),
        SignInState::Failed { message } => format!("check failed: {message}"),
    }
}

async fn with_progress<T, E: std::error::Error>(
    engines: &Engines,
    engine: EngineId,
    work: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let mut statuses = engines.subscribe();
    let bar = ui::Progress::spinner(format!("Installing {}", engine.display_name()));
    let progress = async {
        loop {
            let install = match statuses.recv().await {
                Ok(EngineChange::Full(status)) if status.engine == engine => status.install,
                Ok(EngineChange::Install {
                    engine: changed,
                    install,
                }) if changed == engine => install,
                Ok(_) | Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return std::future::pending::<()>().await,
            };
            match &install {
                InstallState::Downloading {
                    version,
                    received,
                    total: Some(total),
                } if *total > 0 => bar.message(format!(
                    "Downloading {} {version}: {}%",
                    engine.display_name(),
                    received * 100 / total
                )),
                InstallState::Downloading { version, .. } => {
                    bar.message(format!("Downloading {} {version}", engine.display_name()));
                }
                InstallState::Verifying { version } => {
                    bar.message(format!("Verifying {} {version}", engine.display_name()));
                }
                InstallState::Absent
                | InstallState::Installed { .. }
                | InstallState::Failed { .. } => {}
            }
        }
    };
    let result = tokio::select! {
        result = work => result,
        () = progress => unreachable!("progress never ends"),
    };
    match &result {
        Ok(_) => bar.clear(),
        Err(error) => bar.fail(diagnostics::chain(error)),
    }
    result
}

async fn login(engines: &Engines, engine: EngineId) -> Result<(), LoginError> {
    let mut statuses = engines.subscribe();
    engines.login(engine)?;
    ui::intro(format!("Sign in to {}", engine.display_name()));
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let interactive = std::io::stdin().is_terminal();
    let mut reads_code = false;
    let (mut shown_url, mut shown_code) = (None, None);
    loop {
        tokio::select! {
            status = statuses.recv() => {
                let status = match status {
                    Ok(EngineChange::Full(status)) => status,
                    Ok(EngineChange::Install {..}) | Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return Err(LoginError::StoppedReporting { engine }),
                };
                if status.engine != engine {
                    continue;
                }
                match status.sign_in {
                    SignInState::LoggingIn { url, code, accepts_code, .. } => {
                        reads_code=accepts_code && interactive;
                        if url.is_some() && url != shown_url {
                            let link = url.as_deref().unwrap_or_default();
                            let then = if reads_code {
                                "\n\nThen paste the code it shows here and press Enter."
                            } else {
                                ""
                            };
                            ui::note("Open this link to sign in", format!("{link}{then}"));
                            open_browser(link);
                            shown_url = url;
                        }
                        if code.is_some() && code != shown_code {
                            ui::note("Enter this code when asked", code.as_deref().unwrap_or_default());
                            shown_code = code;
                        }
                    }
                    SignInState::SignedIn { account } => {
                        ui::outro(format!("Signed in{}", account.map(|account| format!(" as {account}")).unwrap_or_default()));
                        return Ok(());
                    }
                    SignInState::SignedOut => return Err(LoginError::SignedOut { engine }),
                    SignInState::Failed { message } => return Err(LoginError::Failed { engine, message }),
                    SignInState::Unknown => {}
                }
            }
            line = stdin.next_line(), if reads_code => {
                match line.map_err(LoginError::ReadCode)? {
                    Some(code) if !code.trim().is_empty() => engines
                        .submit_login_code(engine, &code)
                        .await
                        .map_err(|source| LoginError::SubmitCode { engine, source })?,
                    Some(_) => {}
                    None => {
                        engines.cancel_login(engine);
                        return Err(LoginError::Cancelled);
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                engines.cancel_login(engine);
                return Err(LoginError::Cancelled);
            }
        }
    }
}

fn open_browser(link: &str) {
    if !ui::interactive() {
        return;
    }
    let mut command = if cfg!(windows) || crate::platform::in_wsl() {
        let mut command = std::process::Command::new("rundll32.exe");
        command.args(["url.dll,FileProtocolHandler", link]);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("open");
        command.arg(link);
        command
    } else {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(link);
        command
    };
    if let Err(error) = crate::sys::spawn_detached(
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    ) {
        tracing::warn!("open {link} in the browser: {}", diagnostics::chain(&error));
    }
}
