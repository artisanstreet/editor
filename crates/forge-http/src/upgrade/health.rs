use super::{Gateway, Ready};
use forge_runs::runtime::{RegistryFileError, read};
use reqwest::StatusCode;
use std::path::{Path, PathBuf};
use std::time::Duration;

const HEALTH_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum HealthError {
    #[error("read Forge's gateway record in {data}")]
    Record {
        data: PathBuf,
        #[source]
        source: RegistryFileError,
    },
    #[error(
        "Forge in {data} has no gateway record; it is not running as the upgrade-capable service"
    )]
    NoGateway { data: PathBuf },
    #[error("start the client for Forge's health check")]
    Client(#[source] reqwest::Error),
    #[error("Forge's health endpoint {control} did not answer")]
    Unreachable {
        control: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("Forge's health endpoint {control} refused the gateway token")]
    Unauthorized { control: String },
    #[error("Forge at {control} is unhealthy: {reason}")]
    Unhealthy { control: String, reason: String },
    #[error("Forge's health endpoint {control} answered with status {status}")]
    Unexpected { control: String, status: StatusCode },
    #[error("decode the health reply from {control}")]
    Decode {
        control: String,
        #[source]
        source: reqwest::Error,
    },
}

pub async fn health(data: &Path) -> Result<Gateway, HealthError> {
    let gateway: Gateway = read(&data.join("runtime").join("gateway.json"))
        .map_err(|source| HealthError::Record {
            data: data.to_owned(),
            source,
        })?
        .ok_or_else(|| HealthError::NoGateway {
            data: data.to_owned(),
        })?;
    let control = gateway.control;
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(HEALTH_DEADLINE)
        .build()
        .map_err(HealthError::Client)?
        .get(format!("{control}/health"))
        .bearer_auth(&gateway.token)
        .send()
        .await
        .map_err(|source| HealthError::Unreachable {
            control: control.clone(),
            source,
        })?;
    match response.status() {
        StatusCode::OK => response
            .json::<Ready>()
            .await
            .map(|ready| ready.gateway)
            .map_err(|source| HealthError::Decode { control, source }),
        StatusCode::UNAUTHORIZED => Err(HealthError::Unauthorized { control }),
        StatusCode::SERVICE_UNAVAILABLE => {
            let reason = response
                .text()
                .await
                .map_err(|source| HealthError::Decode {
                    control: control.clone(),
                    source,
                })?;
            Err(HealthError::Unhealthy { control, reason })
        }
        status => Err(HealthError::Unexpected { control, status }),
    }
}
