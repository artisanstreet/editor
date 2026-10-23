//! `ae engine …`: Forge's engines (Claude, Codex, …) in its data folder.
//! Installs take the engine install lock, so they are safe beside a running
//! Forge (which notices the new version at its next refresh).

use crate::{EngineCommand, ui};
use anyhow::{Context, Result};
use artisan_api::{ClaudeAiState, EngineId, EngineStatus, InstallState, SignInState};
use artisan_paths::App;
use forge_engines::{EngineChange, Engines};
use std::io::IsTerminal;
use tokio::io::AsyncBufReadExt;
use tokio::sync::broadcast::error::RecvError;

pub fn run(command: EngineCommand) -> Result<()> {
    let data = App::Forge.data()?;
    anyhow::ensure!(
        App::Forge.binary()?.is_file() || data.exists(),
        "Forge isn't installed here; run `ae install`"
    );
    let runtime = tokio::runtime::Runtime::new().context("start the async runtime")?;
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
                    let version = with_progress(&engines, engine, engines.update(engine)).await?;
                    ui::success(format!("{} {version} is installed", engine.display_name()));
                }
                Ok(())
            }
            EngineCommand::Login {
                capture_only: true,
                ..
            } => {
                // Another `ae` keeps it; only the session goes to stdout.
                let session = capture_in_browser().await?;
                println!("{}", serde_json::to_string(&session)?);
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
                        .context("read the claude.ai session")?;
                    serde_json::from_str(&text).context("the claude.ai session is not JSON")?
                } else {
                    ui::intro("Sign in to claude.ai");
                    capture_here().await?
                };
                keep_claude_ai(&engines, session).await
            }
            EngineCommand::Login { engine, .. } => {
                let engine = pick_engine(&engines, engine, "Sign in to", "which engine to sign in to")?;
                login(&engines, engine).await
            }
            EngineCommand::Logout {
                engine,
                claude_ai: true,
            } => {
                only_claude(engine)?;
                engines.claude_ai_sign_out()?;
                ui::success("Signed out of claude.ai. Native Claude Mode runs on Claude Code until you sign in again.");
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

/// The engine named, or the one the user picks.
fn pick_engine(
    engines: &Engines,
    engine: Option<EngineId>,
    prompt: &str,
    question: &str,
) -> Result<EngineId> {
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

/// claude.ai is Claude's alone.
fn only_claude(engine: Option<EngineId>) -> Result<()> {
    anyhow::ensure!(
        engine.is_none_or(|engine| engine == EngineId::Claude),
        "--claude-ai is for Claude"
    );
    Ok(())
}

/// Checks a captured claude.ai session and keeps it in the Forge's data.
async fn keep_claude_ai(engines: &Engines, session: artisan_api::ClaudeAiSession) -> Result<()> {
    let progress = ui::Progress::spinner("Checking the session with claude.ai");
    let state = engines.claude_ai_sign_in(session).await;
    progress.clear();
    if let ClaudeAiState::SignedIn {
        account: Some(account),
    } = state?
    {
        ui::outro(format!(
            "Signed in to claude.ai as {account}. Turn on Native Claude Mode in the Editor's settings to use it."
        ));
    } else {
        ui::outro(
            "Signed in to claude.ai. Turn on Native Claude Mode in the Editor's settings to use it.",
        );
    }
    Ok(())
}

/// Captures a claude.ai session in a browser on this device. In WSL
/// without a Linux browser, Windows' `ae` opens Windows' browser.
async fn capture_here() -> Result<artisan_api::ClaudeAiSession> {
    if crate::platform::in_wsl() && claude_ai_browser::find_browser().is_none() {
        return capture_through_windows().await;
    }
    capture_in_browser().await
}

async fn capture_in_browser() -> Result<artisan_api::ClaudeAiSession> {
    if !claude_ai_browser::can_renew() {
        ui::note(
            "A browser window opens on claude.ai",
            "Sign in there as you normally would. The window closes by itself once you're signed in.\nArtisan keeps this browser profile so it can renew the session later without asking.",
        );
    }
    let progress = ui::Progress::spinner("Waiting for you to sign in to claude.ai");
    let session = claude_ai_browser::capture(claude_ai_browser::Mode::SignIn).await;
    progress.clear();
    session
}

/// Runs Windows' `ae` to capture the session, from WSL.
async fn capture_through_windows() -> Result<artisan_api::ClaudeAiSession> {
    let local = tokio::process::Command::new("cmd.exe")
        .args(["/c", "echo %LOCALAPPDATA%"])
        .current_dir("/mnt/c")
        .output()
        .await
        .context("ask Windows where its apps live")?;
    let local = String::from_utf8_lossy(&local.stdout).trim().to_owned();
    let converted = tokio::process::Command::new("wslpath")
        .args(["-u", &local])
        .output()
        .await
        .context("run wslpath")?;
    let ae = std::path::PathBuf::from(String::from_utf8_lossy(&converted.stdout).trim())
        .join("Artisan Street")
        .join("ae")
        .join("ae.exe");
    anyhow::ensure!(
        ae.is_file(),
        "no browser in WSL, and no Windows `ae` to open Windows' browser; run `ae engine login claude --claude-ai` in Windows instead"
    );
    let output = tokio::process::Command::new(&ae)
        .args(["engine", "login", "claude", "--claude-ai", "--capture-only"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .output()
        .await
        .with_context(|| format!("run {}", ae.display()))?;
    anyhow::ensure!(
        output.status.success(),
        "Windows' ae could not capture the session"
    );
    serde_json::from_slice(&output.stdout).context("Windows' ae printed no session")
}

/// From Windows, for a Forge in WSL: signs in with Windows' browser and
/// hands the session to the `ae` beside the Forge.
pub fn claude_ai_into_wsl(distro: &str) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new().context("start the async runtime")?;
    ui::intro("Sign in to claude.ai");
    let session = runtime.block_on(capture_in_browser())?;
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
        .context("run wsl.exe")?;
    {
        let mut stdin = child.stdin.take().context("the WSL ae has no stdin")?;
        std::io::Write::write_all(&mut stdin, serde_json::to_string(&session)?.as_bytes())?;
    }
    let status = child.wait().context("wait for the WSL ae")?;
    std::process::exit(status.code().unwrap_or(1));
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

async fn list(engines: &Engines, json: bool) -> Result<()> {
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
        println!("{}", serde_json::Value::Array(list));
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
    println!("{}", ui::table(&["ENGINE", "VERSION", "ACCOUNT"], &rows));
    if let Some(state) = statuses.iter().find_map(|status| status.claude_ai.as_ref()) {
        ui::note("claude.ai (Native Claude Mode)", claude_ai_line(state));
    }
    Ok(())
}

fn pick_engines(engines: &Engines) -> Result<Vec<EngineId>> {
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

/// Shows download progress while `work` runs.
async fn with_progress<T>(
    engines: &Engines,
    engine: EngineId,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
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
        Err(error) => bar.fail(format!("{error:#}")),
    }
    result
}

/// Runs the engine's own sign-in and shows its link and code here.
async fn login(engines: &Engines, engine: EngineId) -> Result<()> {
    let mut statuses = engines.subscribe();
    engines.login(engine)?;
    ui::intro(format!("Sign in to {}", engine.display_name()));
    // Claude asks for a code shown in the browser after signing in.
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
                    Err(RecvError::Closed) => anyhow::bail!("the sign-in stopped reporting"),
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
                    SignInState::SignedOut => anyhow::bail!("{} is still signed out", engine.display_name()),
                    SignInState::Failed { message } => anyhow::bail!("sign-in failed: {message}"),
                    SignInState::Unknown => {}
                }
            }
            line = stdin.next_line(), if reads_code => {
                match line.context("read the code")? {
                    Some(code) if !code.trim().is_empty() => engines.submit_login_code(engine, &code).await?,
                    Some(_) => {}
                    None => {
                        engines.cancel_login(engine);
                        anyhow::bail!("sign-in cancelled");
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                engines.cancel_login(engine);
                anyhow::bail!("sign-in cancelled");
            }
        }
    }
}

/// Opens the sign-in link where a browser is at hand (best effort).
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
    crate::sys::spawn_detached(
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    )
    .ok();
}
