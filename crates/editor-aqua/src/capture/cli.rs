use super::scenario::{Scenario, ScenarioError};
#[cfg(windows)]
use super::{
    scenario::{Route, RouteError},
    session::{Options, SessionError, run_script},
};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{name} names the Forge fixture to {purpose}")]
    MissingFixture {
        name: &'static str,
        purpose: &'static str,
    },
    #[error("create the served Forge directory")]
    ServeDirectory(#[source] std::io::Error),
    #[error("serve the capture scenario")]
    Serve(#[source] ScenarioError),
    #[error("tell the caller what is served")]
    Stdout(#[source] std::io::Error),
    #[error("wait for stdin to close")]
    Stdin(#[source] std::io::Error),
    #[error("{name} is not valid Unicode")]
    Variable {
        name: &'static str,
        #[source]
        source: std::env::VarError,
    },
    #[cfg(not(windows))]
    #[error("Editor capture requires Windows D3D11")]
    Unsupported,
    #[cfg(windows)]
    #[error("{name}={value} is not a number")]
    Number {
        name: &'static str,
        value: String,
        #[source]
        source: std::num::ParseFloatError,
    },
    #[cfg(windows)]
    #[error("ARTISAN_AQUA_CAPTURE_LATENCY_MS={value} is not a whole number of milliseconds")]
    Latency {
        value: String,
        #[source]
        source: std::num::ParseIntError,
    },
    #[cfg(windows)]
    #[error("choose the capture route")]
    Route(#[source] RouteError),
    #[cfg(windows)]
    #[error("read the capture script {}", path.display())]
    ReadScript {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[cfg(windows)]
    #[error("run the capture script")]
    Run(#[source] SessionError),
}

fn variable(name: &'static str) -> Result<Option<String>, CliError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value).filter(|value| !value.is_empty())),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(source) => Err(CliError::Variable { name, source }),
    }
}

fn fixture(purpose: &'static str) -> Result<PathBuf, CliError> {
    const NAME: &str = "ARTISAN_AQUA_CAPTURE_FIXTURE";
    variable(NAME)?
        .map(PathBuf::from)
        .ok_or(CliError::MissingFixture {
            name: NAME,
            purpose,
        })
}

#[cfg(windows)]
fn number(name: &'static str, default: f64) -> Result<f64, CliError> {
    variable(name)?.map_or(Ok(default), |value| {
        value.parse().map_err(|source| CliError::Number {
            name,
            value,
            source,
        })
    })
}

#[expect(
    clippy::print_stdout,
    reason = "the capture CLI tells its caller which host and account it serves on stdout"
)]
fn serve(fixture: PathBuf, editor_dir: PathBuf) -> Result<(), CliError> {
    use std::io::{Read, Write};
    let root = tempfile::tempdir().map_err(CliError::ServeDirectory)?;
    let mut scenario =
        Scenario::serve(&fixture, root.path(), &editor_dir).map_err(CliError::Serve)?;
    println!("serving {}", editor_dir.display());
    println!("host {}", crate::qualification::sign_in::MACHINE_ID);
    println!("account {}", crate::qualification::sign_in::ACCOUNT_ID);
    std::io::stdout().flush().map_err(CliError::Stdout)?;
    let mut sink = Vec::new();
    std::io::stdin()
        .read_to_end(&mut sink)
        .map_err(CliError::Stdin)?;
    scenario.shutdown();
    Ok(())
}

pub(crate) fn run_from_environment() -> Result<bool, CliError> {
    if let Some(editor_dir) = variable("ARTISAN_AQUA_SERVE_EDITOR_DIR")? {
        serve(fixture("serve")?, PathBuf::from(editor_dir))?;
        return Ok(true);
    }
    let Some(script) = variable("ARTISAN_EDITOR_SCRIPT")? else {
        return Ok(false);
    };
    capture(&script)
}

#[cfg(not(windows))]
fn capture(_script: &str) -> Result<bool, CliError> {
    Err(CliError::Unsupported)
}

#[cfg(windows)]
fn capture(script: &str) -> Result<bool, CliError> {
    let fixture = fixture("capture against")?;
    let route_name = variable("ARTISAN_AQUA_CAPTURE_ROUTE")?.unwrap_or_else(|| "thread".to_owned());
    let latency = match variable("ARTISAN_AQUA_CAPTURE_LATENCY_MS")? {
        None => None,
        Some(value) => match value.parse() {
            Ok(milliseconds) => Some(std::time::Duration::from_millis(milliseconds)),
            Err(source) => return Err(CliError::Latency { value, source }),
        },
    };
    let options = Options {
        fixture,
        route: Route::parse(&route_name).map_err(CliError::Route)?,
        route_name,
        logical: (
            number("ARTISAN_AQUA_WIDTH", 1100.0)?,
            number("ARTISAN_AQUA_HEIGHT", 760.0)?,
        ),
        scale: number("ARTISAN_AQUA_SCALE", 1.0)?,
        focused: variable("ARTISAN_AQUA_FOCUSED")?.is_some_and(|value| value == "1"),
        out_dir: variable("ARTISAN_AQUA_CAPTURE_DIR")?
            .map_or_else(|| PathBuf::from("."), PathBuf::from),
        latency: latency.filter(|delay| !delay.is_zero()),
    };
    let text = std::fs::read_to_string(script).map_err(|source| CliError::ReadScript {
        path: PathBuf::from(script),
        source,
    })?;
    let records = run_script(options, &text).map_err(CliError::Run)?;
    tracing::info!(captures = records.len(), "script finished");
    Ok(true)
}
