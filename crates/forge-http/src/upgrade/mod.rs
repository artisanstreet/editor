mod controller;
mod process;

use anyhow::{Context, Result, ensure};
use forge_runs::runtime::{Member, PROTOCOL, Registry};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

pub fn gateway(data: &Path) -> Result<Option<Gateway>> {
    let registry = Registry::open(data)?;
    Ok(read(&registry.root().join("gateway.json"))?)
}

pub async fn replace(data: &Path, binary: &Path) -> Result<Gateway> {
    let gateway=gateway(data)?.context("running Forge does not support online upgrades; bootstrap the upgrade-capable service first")?;
    ensure!(
        gateway.protocol == PROTOCOL,
        "running Forge uses an incompatible upgrade protocol"
    );
    let binary = binary.canonicalize()?;
    let result: Outcome = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(180))
        .build()?
        .post(format!("{}/upgrade", gateway.control))
        .bearer_auth(&gateway.token)
        .json(&Upgrade { binary })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    match result {
        Outcome::Ready(gateway) => Ok(*gateway),
        Outcome::Error(error) => anyhow::bail!("{error}"),
    }
}

pub async fn stop(data: &Path) -> Result<()> {
    let gateway = gateway(data)?.context("Forge gateway is not running")?;
    stop_member(&gateway).await
}

async fn stop_member(gateway: &Gateway) -> Result<()> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()?
        .post(format!("{}/stop", gateway.control))
        .bearer_auth(&gateway.token)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

pub async fn serve(
    config: crate::ServeConfig,
    handoff: Option<PathBuf>,
    log: diagnostics::Destination,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    controller::serve(config, handoff, log, stop).await
}

pub use forge_runs::runtime::{read, write};
