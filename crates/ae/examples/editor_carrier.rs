use std::ffi::OsString;
use std::process::{Command, ExitCode};

const CARRIED_AE: &str = "ARTISAN_TEST_CARRIED_AE";
const CARRIED_STAGE: &str = "ARTISAN_TEST_CARRIED_STAGE";

#[derive(Debug, thiserror::Error)]
enum CarrierError {
    #[error("the Editor's installer is started with --install, not {0:?}")]
    NotInstall(Option<OsString>),
    #[error("{0} names no carried program")]
    Unset(&'static str),
    #[error("run the carried ae")]
    Run(#[source] std::io::Error),
}

fn carried(name: &'static str) -> Result<OsString, CarrierError> {
    std::env::var_os(name).ok_or(CarrierError::Unset(name))
}

fn install() -> Result<ExitCode, CarrierError> {
    let mut arguments = std::env::args_os().skip(1);
    match arguments.next() {
        Some(first) if first == "--install" => {}
        other => return Err(CarrierError::NotInstall(other)),
    }
    let status = Command::new(carried(CARRIED_AE)?)
        .args(["install", "--apply"])
        .args(arguments)
        .arg("--from")
        .arg(carried(CARRIED_STAGE)?)
        .status()
        .map_err(CarrierError::Run)?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from))
}

#[expect(
    clippy::print_stderr,
    reason = "a stand-in installer reports its failure on stderr"
)]
fn main() -> ExitCode {
    install().unwrap_or_else(|error| {
        eprintln!("error: {}", diagnostics::chain(&error));
        ExitCode::FAILURE
    })
}
