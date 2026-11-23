use artisan_api::{SignInState, Timestamp, UsageCadence, UsageWindow};
use std::path::{Path, PathBuf};
use std::time::Duration;

const QUOTA_SUMMARY: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
const USER_AGENT: &str = "antigravity";
const TIMEOUT: Duration = Duration::from_secs(15);
const EXPIRY_MARGIN_MS: i64 = 60_000;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadAntigravityUsageError {
    #[error("read the Antigravity token file {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse the Antigravity token file {}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("the Antigravity sign-in in {} expired and a sign-in check did not renew it", path.display())]
    Expired { path: PathBuf },
    #[error("build the HTTP client for Antigravity's quota")]
    Client(#[source] reqwest::Error),
    #[error("ask Antigravity for its quota")]
    Request(#[source] reqwest::Error),
    #[error("Antigravity answered its quota with {status}")]
    Status { status: reqwest::StatusCode },
    #[error("read Antigravity's quota")]
    Body(#[source] reqwest::Error),
    #[error("decode Antigravity's quota")]
    Decode(#[source] serde_json::Error),
}

#[derive(serde::Deserialize)]
struct TokenFile {
    token: Token,
}

#[derive(serde::Deserialize)]
struct Token {
    access_token: String,
    #[serde(default, deserialize_with = "rfc3339")]
    expiry: Option<Timestamp>,
}

enum Access {
    Live(String),
    Expired,
}

fn access(path: &Path, now: Timestamp) -> Result<Access, ReadAntigravityUsageError> {
    let text = std::fs::read(path).map_err(|source| ReadAntigravityUsageError::Read {
        path: path.to_owned(),
        source,
    })?;
    let file: TokenFile =
        serde_json::from_slice(&text).map_err(|source| ReadAntigravityUsageError::Parse {
            path: path.to_owned(),
            source,
        })?;
    let live = file
        .token
        .expiry
        .is_none_or(|expiry| expiry > now + EXPIRY_MARGIN_MS);
    Ok(if live {
        Access::Live(file.token.access_token)
    } else {
        Access::Expired
    })
}

pub(crate) async fn read(
    binary: &Path,
    home: &Path,
) -> Result<Vec<UsageWindow>, ReadAntigravityUsageError> {
    let path = crate::antigravity::token_path(home);
    let token = match access(&path, artisan_api::now())? {
        Access::Live(token) => token,
        Access::Expired => {
            let renewed = matches!(
                crate::account::antigravity_sign_in(binary, home).await,
                SignInState::SignedIn { .. }
            );
            match (renewed, access(&path, artisan_api::now())?) {
                (true, Access::Live(token)) => token,
                (_, Access::Live(_) | Access::Expired) => {
                    return Err(ReadAntigravityUsageError::Expired { path });
                }
            }
        }
    };
    let response = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(ReadAntigravityUsageError::Client)?
        .post(QUOTA_SUMMARY)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body("{}")
        .send()
        .await
        .map_err(ReadAntigravityUsageError::Request)?;
    let status = response.status();
    if !status.is_success() {
        return Err(ReadAntigravityUsageError::Status { status });
    }
    let bytes = response
        .bytes()
        .await
        .map_err(ReadAntigravityUsageError::Body)?;
    parse_quota_summary(&bytes).map_err(ReadAntigravityUsageError::Decode)
}

fn rfc3339<'de, D: serde::Deserializer<'de>>(at: D) -> Result<Option<Timestamp>, D::Error> {
    let Some(text) = <Option<String> as serde::Deserialize>::deserialize(at)? else {
        return Ok(None);
    };
    chrono::DateTime::parse_from_rfc3339(&text)
        .map(|at| Some(at.timestamp_millis()))
        .map_err(serde::de::Error::custom)
}

#[derive(serde::Deserialize)]
struct Summary {
    #[serde(default)]
    groups: Vec<Group>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Group {
    display_name: Option<String>,
    #[serde(default)]
    buckets: Vec<Bucket>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bucket {
    window: Option<String>,
    remaining_fraction: Option<f64>,
    #[serde(default, deserialize_with = "rfc3339")]
    reset_time: Option<Timestamp>,
}

pub(crate) fn parse_quota_summary(body: &[u8]) -> Result<Vec<UsageWindow>, serde_json::Error> {
    let summary: Summary = serde_json::from_slice(body)?;
    Ok(summary
        .groups
        .into_iter()
        .flat_map(|group| {
            let scope = group.display_name.filter(|name| !name.is_empty());
            group.buckets.into_iter().map(move |bucket| {
                let (label, cadence) = match bucket.window.as_deref() {
                    Some("5h") => ("5 hours".to_owned(), UsageCadence::Session),
                    Some("weekly") => ("Weekly".to_owned(), UsageCadence::Week),
                    Some("monthly") => ("Monthly".to_owned(), UsageCadence::Month),
                    Some(other) => (other.to_owned(), UsageCadence::Other),
                    None => ("Usage".to_owned(), UsageCadence::Other),
                };
                UsageWindow {
                    label,
                    cadence,
                    scope: scope.clone(),
                    used_percent: ((1.0 - bucket.remaining_fraction.unwrap_or(0.0)) * 100.0)
                        .clamp(0.0, 100.0),
                    resets_at: bucket.reset_time,
                }
            })
        })
        .collect())
}
