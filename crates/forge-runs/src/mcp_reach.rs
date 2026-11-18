use crate::error::ErrorClass;
use crate::mcp_auth::{Mcp, ReadMcpServersError};
use artisan_api::McpServer;
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const EVERY: Duration = Duration::from_secs(10);
const PATIENCE: Duration = Duration::from_secs(3);

pub(crate) type Endpoint = (String, u16);

#[derive(Debug, thiserror::Error)]
pub enum ProbeServersError {
    #[error("read the MCP servers to probe")]
    Servers(#[source] ReadMcpServersError),
    #[error("probe whether an MCP server answers")]
    Probe(#[source] tokio::task::JoinError),
}

impl ProbeServersError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Servers(_) | Self::Probe(_) => ErrorClass::Internal,
        }
    }
}

pub(crate) fn endpoint(server: &McpServer) -> Option<Endpoint> {
    let url = url::Url::parse(server.url.as_deref()?).ok()?;
    Some((url.host_str()?.to_owned(), url.port_or_known_default()?))
}

async fn answers(endpoint: &Endpoint) -> bool {
    let connect = tokio::net::TcpStream::connect((endpoint.0.as_str(), endpoint.1));
    matches!(tokio::time::timeout(PATIENCE, connect).await, Ok(Ok(_)))
}

impl Mcp {
    pub(crate) fn server_reachable(&self, server: &McpServer) -> bool {
        endpoint(server).is_none_or(|endpoint| {
            !self
                .unreachable
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&endpoint)
        })
    }

    pub async fn probe_servers(&self) -> Result<(), ProbeServersError> {
        let servers = crate::mcp_auth::read_mcp_servers(self.store())
            .await
            .map_err(ProbeServersError::Servers)?;
        let named: BTreeMap<Endpoint, String> = servers
            .iter()
            .filter_map(|server| Some((endpoint(server)?, server.name.clone())))
            .collect();
        let mut probes: tokio::task::JoinSet<(Endpoint, bool)> = named
            .keys()
            .cloned()
            .map(|endpoint| async move {
                let up = answers(&endpoint).await;
                (endpoint, up)
            })
            .collect();
        let mut unreachable = HashSet::new();
        while let Some(verdict) = probes.join_next().await {
            if let (endpoint, false) = verdict.map_err(ProbeServersError::Probe)? {
                unreachable.insert(endpoint);
            }
        }
        let mut known = self
            .unreachable
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (endpoint, name) in &named {
            match (known.contains(endpoint), unreachable.contains(endpoint)) {
                (false, true) => tracing::info!(
                    server = %name,
                    address = %format_args!("{}:{}", endpoint.0, endpoint.1),
                    "MCP server does not answer; new turns start without it"
                ),
                (true, false) => {
                    tracing::info!(server = %name, "MCP server answers again; new turns include it");
                }
                (true, true) | (false, false) => {}
            }
        }
        *known = unreachable;
        drop(known);
        Ok(())
    }

    pub async fn watch_servers(&self, stop: CancellationToken) {
        loop {
            if let Err(error) = self.probe_servers().await {
                tracing::warn!(
                    "check which MCP servers answer: {}",
                    diagnostics::chain(&error)
                );
            }
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(EVERY) => {}
            }
        }
    }
}
