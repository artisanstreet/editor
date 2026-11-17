use crate::RecentLines;
use tracing_error::ErrorLayer;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::writer::MakeWriterExt as _;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

#[derive(Debug, thiserror::Error)]
pub enum SubscribeError {
    #[error("install the tracing subscriber")]
    Install {
        #[source]
        source: tracing_subscriber::util::TryInitError,
    },
}

#[derive(Debug, thiserror::Error)]
enum FilterError {
    #[error("RUST_LOG is not Unicode")]
    NotUnicode,
    #[error("parse RUST_LOG {directives:?}")]
    Parse {
        directives: String,
        #[source]
        source: tracing_subscriber::filter::ParseError,
    },
}

fn filter(default: &str) -> (EnvFilter, Option<FilterError>) {
    match std::env::var("RUST_LOG") {
        Err(std::env::VarError::NotPresent) => (EnvFilter::new(default), None),
        Err(std::env::VarError::NotUnicode(_)) => {
            (EnvFilter::new(default), Some(FilterError::NotUnicode))
        }
        Ok(directives) => match EnvFilter::try_new(&directives) {
            Ok(filter) => (filter, None),
            Err(source) => (
                EnvFilter::new(default),
                Some(FilterError::Parse { directives, source }),
            ),
        },
    }
}

pub fn subscribe<W>(default_filter: &str, writer: W) -> Result<RecentLines, SubscribeError>
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let recent = RecentLines::default();
    let (filter, problem) = filter(default_filter);
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(writer.and(recent.clone()))
        .finish()
        .with(ErrorLayer::default())
        .try_init()
        .map_err(|source| SubscribeError::Install { source })?;
    if let Some(problem) = problem {
        tracing::warn!(
            error = &problem as &dyn std::error::Error,
            default = default_filter,
            "using the default log filter"
        );
    }
    Ok(recent)
}
