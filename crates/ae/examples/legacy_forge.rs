use artisan_paths::App;
use std::io::Write as _;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::ExitCode;

const RELEASED_SCHEMA: usize = 26;

#[derive(Debug, thiserror::Error)]
enum LegacyError {
    #[error("error: unexpected argument '{0}' found")]
    Argument(String),
    #[error("find Forge's data folder")]
    Data(#[source] artisan_paths::RootError),
    #[error("read where Forge listens")]
    Listen(#[source] forge_http::ConfiguredListenError),
    #[error("open the database")]
    Open(#[source] rusqlite::Error),
    #[error("database schema version {found} is newer than this Forge ({RELEASED_SCHEMA})")]
    Newer { found: usize },
    #[error("write the log {}", path.display())]
    Log {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("listen")]
    Bind(#[source] std::io::Error),
}

fn log_path() -> Result<Option<PathBuf>, LegacyError> {
    let mut args = std::env::args().skip(1);
    let mut log = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--log" => log = args.next().map(PathBuf::from),
            other => return Err(LegacyError::Argument(other.to_owned())),
        }
    }
    Ok(log)
}

fn note(log: Option<&PathBuf>, line: &str) -> Result<(), LegacyError> {
    let Some(path) = log else {
        return Ok(());
    };
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{line}"))
        .map_err(|source| LegacyError::Log {
            path: path.clone(),
            source,
        })
}

fn serve(log: Option<&PathBuf>) -> Result<(), LegacyError> {
    let data = App::Forge.data().map_err(LegacyError::Data)?;
    let listen = forge_http::configured_listen(&data).map_err(LegacyError::Listen)?;
    let database = data.join("forge.sqlite3");
    if database.is_file() {
        let found: usize = rusqlite::Connection::open(&database)
            .and_then(|conn| conn.pragma_query_value(None, "user_version", |row| row.get(0)))
            .map_err(LegacyError::Open)?;
        if found > RELEASED_SCHEMA {
            return Err(LegacyError::Newer { found });
        }
    }
    let listener = TcpListener::bind(listen).map_err(LegacyError::Bind)?;
    note(log, &format!("serving on {listen}"))?;
    for connection in listener.incoming() {
        drop(connection);
    }
    Ok(())
}

fn main() -> ExitCode {
    let log = match log_path() {
        Ok(log) => log,
        Err(error) => {
            drop(writeln!(std::io::stderr(), "{error}"));
            return ExitCode::from(2);
        }
    };
    match serve(log.as_ref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let line = format!("error: {}", diagnostics::chain(&error));
            match note(log.as_ref(), &line) {
                Ok(()) => {}
                Err(unlogged) => drop(writeln!(
                    std::io::stderr(),
                    "{line}; {}",
                    diagnostics::chain(&unlogged)
                )),
            }
            ExitCode::FAILURE
        }
    }
}
