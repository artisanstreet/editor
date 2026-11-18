use artisan_api::{ApiError, ErrorCode};

#[derive(Debug, thiserror::Error)]
pub enum NewClientError {
    #[error("set up TLS for Forge at {address}")]
    Tls {
        address: String,
        #[source]
        source: artisan_api::tls::TlsConfigError,
    },
    #[error("build the HTTP client for Forge at {address}")]
    Build {
        address: String,
        #[source]
        source: reqwest::Error,
    },
}

#[derive(Debug, thiserror::Error)]
#[error("{method} {path}")]
pub struct CallError {
    pub method: reqwest::Method,
    pub path: String,
    #[source]
    pub failure: Box<CallFailure>,
}

#[cfg(feature = "test-support")]
impl CallError {
    #[must_use]
    pub fn test_inconsistent(path: &str, check: &'static str) -> Self {
        Self {
            method: reqwest::Method::GET,
            path: path.to_owned(),
            failure: Box::new(CallFailure::Inconsistent { check }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CallFailure {
    #[error("build the request")]
    Build(#[source] reqwest::Error),
    #[error("reach Forge")]
    Reach(#[source] reqwest::Error),
    #[error("Forge did not answer")]
    Answer(#[source] ResponseError),
    #[error("read Forge's answer")]
    Read(#[source] ReadBodyError),
    #[error("decode Forge's answer")]
    Decode(#[source] serde_json::Error),
    #[error("Forge's answer is inconsistent: {check}")]
    Inconsistent { check: &'static str },
    #[error("Forge {forge_version} speaks protocol {protocol}; update Forge or the Editor")]
    Incompatible {
        forge_version: String,
        protocol: u32,
    },
    #[error("streaming request bodies cannot travel the Relay")]
    StreamingBody,
    #[error("encode the request body for the Relay")]
    EncodeBody(#[source] serde_json::Error),
    #[error("call through the Relay")]
    Relay(#[source] artisan_account::relay_client::RelayCallError),
    #[error("the request task stopped")]
    Task(#[source] tokio::task::JoinError),
}

#[derive(Debug, thiserror::Error)]
pub enum ResponseError {
    #[error("Forge answered {status}")]
    Refused {
        status: reqwest::StatusCode,
        #[source]
        error: ApiError,
    },
    #[error("Forge answered {status}: {body}")]
    Unexplained {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("Forge answered {status} and its explanation could not be read")]
    Unreadable {
        status: reqwest::StatusCode,
        #[source]
        source: ReadBodyError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ReadBodyError {
    #[error("the response exceeds {limit} bytes")]
    TooLarge { limit: usize },
    #[error("read a response chunk")]
    Chunk(#[source] reqwest::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum RangeError {
    #[error("source {source_id} has no valid identity")]
    InvalidSource { source_id: artisan_api::ContentId },
    #[error("source {source_id} has no range at {offset} of {limit} bytes")]
    InvalidRange {
        source_id: artisan_api::ContentId,
        offset: u64,
        limit: u64,
    },
    #[error("read source {source_id} at {offset}")]
    Call {
        source_id: artisan_api::ContentId,
        offset: u64,
        #[source]
        source: CallError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CopyContentError {
    #[error(
        "source {source_id} has {bytes} bytes, too large for the clipboard; export it to a file instead"
    )]
    TooLarge {
        source_id: artisan_api::ContentId,
        bytes: u64,
    },
    #[error("read source {source_id}")]
    Range {
        source_id: artisan_api::ContentId,
        #[source]
        source: RangeError,
    },
    #[error("Forge returned source {source_id} from {start} where {offset} was next")]
    Noncontiguous {
        source_id: artisan_api::ContentId,
        offset: u64,
        start: u64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ExportContentError {
    #[error("export destination {path} has no parent directory")]
    NoParent { path: std::path::PathBuf },
    #[error("create a temporary file beside {path}")]
    Create {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read source {source_id}")]
    Range {
        source_id: artisan_api::ContentId,
        #[source]
        source: RangeError,
    },
    #[error("Forge returned source {source_id} from {start} where {offset} was next")]
    Noncontiguous {
        source_id: artisan_api::ContentId,
        offset: u64,
        start: u64,
    },
    #[error("write the export of {path}")]
    Write {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("save the export to {path}")]
    Persist {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the export task for {path} stopped")]
    Task {
        path: std::path::PathBuf,
        #[source]
        source: tokio::task::JoinError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RelayClientError {
    #[error("connect to the Relay")]
    Connect(#[source] artisan_account::relay_client::ConnectRelayError),
    #[error("read the Forge's protocol through the Relay")]
    State(#[source] artisan_account::relay_client::RelayCallError),
    #[error("the Forge behind the Relay speaks protocol {protocol:?}; update Forge or the Editor")]
    Incompatible { protocol: Option<u64> },
    #[error("the Relay dial task stopped")]
    Task(#[source] tokio::task::JoinError),
}

impl CallError {
    #[must_use]
    pub fn api_code(&self) -> Option<ErrorCode> {
        self.failure.api_code()
    }

    #[must_use]
    pub fn fault(&self) -> ForgeFault {
        self.failure.fault()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForgeFault {
    Unrecognised,
    Unreachable,
    Unauthorized,
    Incompatible,
    Unreadable,
    Failed,
}

impl ForgeFault {
    const fn of(code: ErrorCode) -> Self {
        match code {
            ErrorCode::NotFound => Self::Unrecognised,
            ErrorCode::Unauthorized => Self::Unauthorized,
            ErrorCode::VersionMismatch => Self::Incompatible,
            ErrorCode::BadRequest
            | ErrorCode::Conflict
            | ErrorCode::EngineUnavailable
            | ErrorCode::Internal => Self::Failed,
        }
    }

    #[must_use]
    pub const fn sentence(self) -> &'static str {
        match self {
            Self::Unrecognised => "Forge didn't recognise it.",
            Self::Unreachable => "Forge couldn't be reached.",
            Self::Unauthorized => "Forge didn't accept this Editor's credential.",
            Self::Incompatible => "Forge and this Editor speak different versions.",
            Self::Unreadable => "Forge's answer couldn't be read.",
            Self::Failed => "Forge ran into a problem.",
        }
    }
}

impl CallFailure {
    #[must_use]
    fn fault(&self) -> ForgeFault {
        use artisan_account::relay_client::CallFailure as Relay;
        match self {
            Self::Answer(error) => ForgeFault::of(error.code()),
            Self::Incompatible { .. } => ForgeFault::Incompatible,
            Self::Relay(error) => match &*error.failure {
                Relay::Refused(error) => ForgeFault::of(error.code),
                Relay::Closed | Relay::Open(_) | Relay::Exchange(_) | Relay::TimedOut => {
                    ForgeFault::Unreachable
                }
                Relay::Decode(_) | Relay::Bytes(_) | Relay::TooLarge { .. } => {
                    ForgeFault::Unreadable
                }
            },
            Self::Reach(_) | Self::Task(_) => ForgeFault::Unreachable,
            Self::Read(_) | Self::Decode(_) | Self::Inconsistent { .. } => ForgeFault::Unreadable,
            Self::Build(_) | Self::StreamingBody | Self::EncodeBody(_) => ForgeFault::Failed,
        }
    }

    #[must_use]
    fn api_code(&self) -> Option<ErrorCode> {
        match self {
            Self::Answer(error) => Some(error.code()),
            Self::Incompatible { .. } => Some(ErrorCode::VersionMismatch),
            Self::Relay(error) => match &*error.failure {
                artisan_account::relay_client::CallFailure::Refused(error) => Some(error.code),
                artisan_account::relay_client::CallFailure::Closed
                | artisan_account::relay_client::CallFailure::Open(_)
                | artisan_account::relay_client::CallFailure::Exchange(_)
                | artisan_account::relay_client::CallFailure::TimedOut
                | artisan_account::relay_client::CallFailure::Decode(_)
                | artisan_account::relay_client::CallFailure::Bytes(_)
                | artisan_account::relay_client::CallFailure::TooLarge { .. } => None,
            },
            Self::Build(_)
            | Self::Reach(_)
            | Self::Read(_)
            | Self::Decode(_)
            | Self::Inconsistent { .. }
            | Self::StreamingBody
            | Self::EncodeBody(_)
            | Self::Task(_) => None,
        }
    }
}

impl ResponseError {
    #[must_use]
    pub(crate) const fn code(&self) -> ErrorCode {
        match self {
            Self::Refused { status, .. }
            | Self::Unexplained { status, .. }
            | Self::Unreadable { status, .. }
                if matches!(status.as_u16(), 401 | 403) =>
            {
                ErrorCode::Unauthorized
            }
            Self::Refused { error, .. } => error.code,
            Self::Unexplained { .. } | Self::Unreadable { .. } => ErrorCode::Internal,
        }
    }
}
