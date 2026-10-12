//! `ae engine …`: Forge's engine manager on the data directory. Installs
//! take the engine install lock, so they are safe beside a running Forge
//! (which notices the new version on its next status read).

use crate::EngineCommand;
use anyhow::{Context, Result};
use artisan_api::{EngineId, EngineStatus, InstallState, SignInState};
use forge_engines::Engines;
use std::io::IsTerminal;
use std::path::Path;
use tokio::io::AsyncBufReadExt;
use tokio::sync::broadcast::error::RecvError;

pub fn run(command: EngineCommand, data_dir: &Path) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new().context("start the async runtime")?;
    let engines = Engines::new(data_dir);
    runtime.block_on(async move {
        match command {
            EngineCommand::List => {
                for engine in EngineId::ALL {
                    println!(
                        "{:<10} {}",
                        engine.as_str(),
                        install_line(&engines.status(engine).install)
                    );
                }
                Ok(())
            }
            EngineCommand::Status { engine } => {
                for engine in engine.map_or_else(|| EngineId::ALL.to_vec(), |engine| vec![engine]) {
                    engines.check_sign_in(engine).await;
                    print_status(&engines.status(engine));
                }
                Ok(())
            }
            EngineCommand::Install { engine } => {
                let version = with_progress(&engines, engines.install(engine)).await?;
                println!("{} {version} is installed.", engine.display_name());
                Ok(())
            }
            EngineCommand::Update { engine } => {
                let version = with_progress(&engines, engines.update(engine)).await?;
                println!("{} is at {version}.", engine.display_name());
                Ok(())
            }
            EngineCommand::Use { engine, version } => {
                with_progress(&engines, engines.use_version(engine, &version)).await?;
                println!("{} now uses {version}.", engine.display_name());
                Ok(())
            }
            EngineCommand::Rollback { engine } => {
                let version = engines.rollback(engine).await?;
                println!("{} rolled back to {version}.", engine.display_name());
                Ok(())
            }
            EngineCommand::Login { engine } => login(&engines, engine).await,
        }
    })
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

fn print_status(status: &EngineStatus) {
    let sign_in = match &status.sign_in {
        SignInState::Unknown => "unknown".to_owned(),
        SignInState::SignedOut => "signed out".to_owned(),
        SignInState::LoggingIn { .. } => "signing in".to_owned(),
        SignInState::SignedIn {
            account: Some(account),
        } => format!("signed in as {account}"),
        SignInState::SignedIn { account: None } => "signed in".to_owned(),
        SignInState::Failed { message } => format!("sign-in check failed: {message}"),
    };
    println!("{}", status.engine.display_name());
    println!("  install:  {}", install_line(&status.install));
    println!("  account:  {sign_in}");
}

/// Prints download progress while `work` runs.
async fn with_progress<T>(engines: &Engines, work: impl Future<Output = Result<T>>) -> Result<T> {
    let mut statuses = engines.subscribe();
    let progress = async {
        let mut last = String::new();
        loop {
            let status = match statuses.recv().await {
                Ok(status) => status,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return std::future::pending::<()>().await,
            };
            let line = match &status.install {
                InstallState::Downloading {
                    version,
                    received,
                    total: Some(total),
                } if *total > 0 => {
                    format!(
                        "Downloading {} {version}: {}%",
                        status.engine.display_name(),
                        received * 100 / total
                    )
                }
                InstallState::Downloading { version, .. } => {
                    format!("Downloading {} {version}…", status.engine.display_name())
                }
                InstallState::Verifying { version } => {
                    format!("Verifying {} {version}…", status.engine.display_name())
                }
                _ => continue,
            };
            if line != last {
                eprintln!("{line}");
                last = line;
            }
        }
    };
    tokio::select! {
        result = work => result,
        () = progress => unreachable!("progress never ends"),
    }
}

/// Runs the engine's own sign-in and shows its link and code here.
async fn login(engines: &Engines, engine: EngineId) -> Result<()> {
    let mut statuses = engines.subscribe();
    engines.login(engine)?;
    eprintln!(
        "Signing in to {}… (Ctrl+C to cancel)",
        engine.display_name()
    );
    // Claude asks for a code shown in the browser after signing in.
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let reads_code = engine == EngineId::Claude && std::io::stdin().is_terminal();
    let (mut shown_url, mut shown_code) = (None, None);
    loop {
        tokio::select! {
            status = statuses.recv() => {
                let status = match status {
                    Ok(status) => status,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => anyhow::bail!("the sign-in stopped reporting"),
                };
                if status.engine != engine {
                    continue;
                }
                match status.sign_in {
                    SignInState::LoggingIn { url, code } => {
                        if url.is_some() && url != shown_url {
                            println!("Open this link to sign in: {}", url.as_deref().unwrap_or_default());
                            if reads_code {
                                println!("Then paste the code it shows here and press Enter.");
                            }
                            shown_url = url;
                        }
                        if code.is_some() && code != shown_code {
                            println!("Enter this code when asked: {}", code.as_deref().unwrap_or_default());
                            shown_code = code;
                        }
                    }
                    SignInState::SignedIn { account } => {
                        println!("Signed in{}.", account.map(|account| format!(" as {account}")).unwrap_or_default());
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
