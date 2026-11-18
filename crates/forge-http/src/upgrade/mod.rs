mod controller;
mod health;
mod process;

use forge_runs::runtime::{Member, PROTOCOL, Registry, RegistryFileError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use controller::ServeError;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Gateway {
    pub id: String,
    pub pid: u32,
    pub control: String,
    pub token: String,
    pub address: std::net::SocketAddr,
    pub binary: PathBuf,
    pub protocol: u32,
    pub backend: Member,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ready {
    pub gateway: Gateway,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Upgrade {
    pub binary: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Outcome {
    Ready(Box<Gateway>),
    Error(String),
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayRecordError {
    #[error("open Forge's runtime registry in {}", data.display())]
    Registry {
        data: PathBuf,
        #[source]
        source: RegistryFileError,
    },
    #[error("read Forge's gateway record in {}", data.display())]
    Read {
        data: PathBuf,
        #[source]
        source: RegistryFileError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ReplaceError {
    #[error(transparent)]
    Record(#[from] GatewayRecordError),
    #[error(
        "the running Forge in {} does not support online upgrades; bootstrap the upgrade-capable service first",
        data.display()
    )]
    NoGateway { data: PathBuf },
    #[error("the running Forge gateway {gateway} uses upgrade protocol {protocol}, not {PROTOCOL}")]
    Protocol { gateway: String, protocol: u32 },
    #[error("resolve the replacement executable {}", binary.display())]
    Binary {
        binary: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("build the upgrade client")]
    Client(#[source] reqwest::Error),
    #[error("ask gateway {gateway} to upgrade")]
    Request {
        gateway: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("read gateway {gateway}'s upgrade answer")]
    Decode {
        gateway: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("gateway {gateway} could not upgrade: {reason}")]
    Failed { gateway: String, reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum StopError {
    #[error(transparent)]
    Record(#[from] GatewayRecordError),
    #[error("Forge in {} has no running gateway", data.display())]
    NoGateway { data: PathBuf },
    #[error(transparent)]
    Member(#[from] StopMemberError),
}

#[derive(Debug, thiserror::Error)]
pub enum StopMemberError {
    #[error("build the client that stops gateway {gateway}")]
    Client {
        gateway: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("ask gateway {gateway} to stop")]
    Request {
        gateway: String,
        #[source]
        source: reqwest::Error,
    },
}

pub fn gateway(data: &Path) -> Result<Option<Gateway>, GatewayRecordError> {
    let registry = Registry::open(data).map_err(|source| GatewayRecordError::Registry {
        data: data.to_owned(),
        source,
    })?;
    read(&registry.root().join("gateway.json")).map_err(|source| GatewayRecordError::Read {
        data: data.to_owned(),
        source,
    })
}

pub async fn replace(data: &Path, binary: &Path) -> Result<Gateway, ReplaceError> {
    let Some(gateway) = gateway(data)? else {
        return Err(ReplaceError::NoGateway {
            data: data.to_owned(),
        });
    };
    if gateway.protocol != PROTOCOL {
        return Err(ReplaceError::Protocol {
            gateway: gateway.id,
            protocol: gateway.protocol,
        });
    }
    let binary = binary
        .canonicalize()
        .map_err(|source| ReplaceError::Binary {
            binary: binary.to_owned(),
            source,
        })?;
    let request = |source| ReplaceError::Request {
        gateway: gateway.id.clone(),
        source,
    };
    let result: Outcome = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(ReplaceError::Client)?
        .post(format!("{}/upgrade", gateway.control))
        .bearer_auth(&gateway.token)
        .json(&Upgrade { binary })
        .send()
        .await
        .map_err(request)?
        .error_for_status()
        .map_err(request)?
        .json()
        .await
        .map_err(|source| ReplaceError::Decode {
            gateway: gateway.id.clone(),
            source,
        })?;
    match result {
        Outcome::Ready(replacement) => Ok(*replacement),
        Outcome::Error(reason) => Err(ReplaceError::Failed {
            gateway: gateway.id,
            reason,
        }),
    }
}

pub async fn stop(data: &Path) -> Result<(), StopError> {
    let Some(gateway) = gateway(data)? else {
        return Err(StopError::NoGateway {
            data: data.to_owned(),
        });
    };
    Ok(stop_member(&gateway).await?)
}

async fn stop_member(gateway: &Gateway) -> Result<(), StopMemberError> {
    let request = |source| StopMemberError::Request {
        gateway: gateway.id.clone(),
        source,
    };
    reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|source| StopMemberError::Client {
            gateway: gateway.id.clone(),
            source,
        })?
        .post(format!("{}/stop", gateway.control))
        .bearer_auth(&gateway.token)
        .send()
        .await
        .map_err(request)?
        .error_for_status()
        .map_err(request)?;
    Ok(())
}

pub async fn serve(
    config: crate::ServeConfig,
    binary: PathBuf,
    handoff: Option<PathBuf>,
    log: diagnostics::Destination,
    stop: impl Future<Output = ()>,
) -> Result<(), ServeError> {
    controller::serve(config, binary, handoff, log, stop).await
}

pub use forge_runs::runtime::{read, write};
pub use health::{HealthError, health};
