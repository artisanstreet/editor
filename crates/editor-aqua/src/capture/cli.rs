use super::scenario::Scenario;
#[cfg(windows)]
use super::{
    scenario::Route,
    session::{Options, run_script},
};
use anyhow::{Context, Result};
use std::path::PathBuf;

fn variable(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(windows)]
fn number(name: &str, default: f64) -> Result<f64> {
    variable(name).map_or(Ok(default), |value| {
        value
            .parse()
            .with_context(|| format!("{name} is not a number"))
    })
}

fn serve(fixture: PathBuf, editor_dir: PathBuf) -> Result<()> {
    use std::io::{Read, Write};
    let root = tempfile::tempdir().context("create the served Forge directory")?;
    let mut scenario = Scenario::serve(&fixture, root.path(), &editor_dir)?;
    println!("serving {}", editor_dir.display());
    println!("host {}", super::scenario::MACHINE_ID);
    println!("account {}", super::scenario::ACCOUNT_ID);
    std::io::stdout().flush().context("flush stdout")?;
    let mut sink = Vec::new();
    std::io::stdin().read_to_end(&mut sink).context("wait for stdin to close")?;
    scenario.shutdown();
    Ok(())
}

pub fn run_from_environment() -> Result<bool> {
    if let Some(editor_dir) = variable("ARTISAN_AQUA_SERVE_EDITOR_DIR") {
        let fixture = variable("ARTISAN_AQUA_CAPTURE_FIXTURE")
            .context("ARTISAN_AQUA_CAPTURE_FIXTURE names the Forge fixture to serve")?;
        serve(PathBuf::from(fixture), PathBuf::from(editor_dir))?;
        return Ok(true);
    }
    let Some(script) = variable("ARTISAN_EDITOR_SCRIPT") else {
        return Ok(false);
    };
    capture(&script)
}

#[cfg(not(windows))]
fn capture(_script: &str) -> Result<bool> {
    anyhow::bail!("Editor capture requires Windows D3D11")
}

#[cfg(windows)]
fn capture(script: &str) -> Result<bool> {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
    let fixture = variable("ARTISAN_AQUA_CAPTURE_FIXTURE")
        .context("ARTISAN_AQUA_CAPTURE_FIXTURE names the Forge fixture to capture against")?;
    let route_name = variable("ARTISAN_AQUA_CAPTURE_ROUTE").unwrap_or_else(|| "thread".to_owned());
    let options = Options {
        fixture: PathBuf::from(fixture),
        route: Route::parse(&route_name)?,
        route_name,
        logical: (
            number("ARTISAN_AQUA_WIDTH", 1100.0)?,
            number("ARTISAN_AQUA_HEIGHT", 760.0)?,
        ),
        scale: number("ARTISAN_AQUA_SCALE", 1.0)?,
        focused: variable("ARTISAN_AQUA_FOCUSED").is_some_and(|value| value == "1"),
        out_dir: variable("ARTISAN_AQUA_CAPTURE_DIR")
            .map_or_else(|| PathBuf::from("."), PathBuf::from),
    };
    let text = std::fs::read_to_string(&script).with_context(|| format!("read {script}"))?;
    let records = run_script(options, &text)?;
    tracing::info!(captures = records.len(), "script finished");
    Ok(true)
}
