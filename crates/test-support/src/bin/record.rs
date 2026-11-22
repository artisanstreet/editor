use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use test_support::transcript::{RecordError, Scrub, Source, Transcript, record};

const USAGE: &str =
    "usage: record <fixture.jsonl> <trace.jsonl>... [--scrub <text>=<replacement>]...";
const HOME_PLACEHOLDER: &str = "/home/user";

#[derive(Debug)]
struct Request {
    fixture: PathBuf,
    traces: Vec<PathBuf>,
    scrubs: Vec<Scrub>,
}

#[derive(Debug, thiserror::Error)]
enum RecordCommandError {
    #[error("{USAGE}")]
    Usage,
    #[error("--scrub takes <text>=<replacement>, not {given:?}")]
    Scrub { given: String },
    #[error("read the trace {}", path.display())]
    ReadTrace {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("turn the trace {} into replay steps", path.display())]
    Record {
        path: PathBuf,
        #[source]
        source: RecordError,
    },
    #[error("write the fixture {}", path.display())]
    WriteFixture {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn main() -> ExitCode {
    match parse(std::env::args().skip(1)).and_then(|request| run(&request)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            let mut stderr = std::io::stderr().lock();
            let mut cause: Option<&dyn std::error::Error> = Some(&failure);
            while let Some(error) = cause {
                writeln!(stderr, "{error}").expect("the recorder writes to stderr");
                cause = error.source();
            }
            ExitCode::FAILURE
        }
    }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Request, RecordCommandError> {
    let fixture = args
        .next()
        .map(PathBuf::from)
        .ok_or(RecordCommandError::Usage)?;
    let mut traces = Vec::new();
    let mut scrubs: Vec<Scrub> = home().into_iter().collect();
    while let Some(arg) = args.next() {
        if arg == "--scrub" {
            let given = args.next().ok_or(RecordCommandError::Usage)?;
            let (from, to) = given
                .split_once('=')
                .filter(|(from, _)| !from.is_empty())
                .ok_or_else(|| RecordCommandError::Scrub {
                    given: given.clone(),
                })?;
            scrubs.push(Scrub::new(from.to_owned(), to.to_owned()));
        } else {
            traces.push(PathBuf::from(arg));
        }
    }
    if traces.is_empty() {
        return Err(RecordCommandError::Usage);
    }
    Ok(Request {
        fixture,
        traces,
        scrubs,
    })
}

fn home() -> Option<Scrub> {
    ["HOME", "USERPROFILE"]
        .into_iter()
        .find_map(std::env::var_os)
        .map(|home| home.to_string_lossy().into_owned())
        .filter(|home| home.len() > 1)
        .map(|home| Scrub::new(home, HOME_PLACEHOLDER.to_owned()))
}

fn run(request: &Request) -> Result<(), RecordCommandError> {
    let launches = request
        .traces
        .iter()
        .map(|path| {
            let trace =
                std::fs::read_to_string(path).map_err(|source| RecordCommandError::ReadTrace {
                    path: path.clone(),
                    source,
                })?;
            record(&trace, &request.scrubs).map_err(|source| RecordCommandError::Record {
                path: path.clone(),
                source,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    std::fs::write(
        &request.fixture,
        Transcript::new(Source::Recorded, launches).render(),
    )
    .map_err(|source| RecordCommandError::WriteFixture {
        path: request.fixture.clone(),
        source,
    })
}
