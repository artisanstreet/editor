use crate::Hub;
use artisan_api::McpServer;
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const EVERY: Duration = Duration::from_secs(10);
const PATIENCE: Duration = Duration::from_secs(3);

pub(crate) type Endpoint = (String, u16);

pub(crate) fn endpoint(server: &McpServer) -> Option<Endpoint> {
    let url = url::Url::parse(server.url.as_deref()?).ok()?;
    Some((url.host_str()?.to_owned(), url.port_or_known_default()?))
}

async fn answers(endpoint: &Endpoint) -> bool {
    let connect = tokio::net::TcpStream::connect((endpoint.0.as_str(), endpoint.1));
    matches!(tokio::time::timeout(PATIENCE, connect).await, Ok(Ok(_)))
}

impl Hub {
    pub(crate) fn mcp_server_reachable(&self, server: &McpServer) -> bool {
        endpoint(server).is_none_or(|endpoint| {
            !self
                .inner
                .mcp_unreachable
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&endpoint)
        })
    }

    pub async fn probe_mcp_servers(&self) -> anyhow::Result<()> {
        let servers: Vec<McpServer> = self
            .store()
            .setting("mcp_servers")
            .await?
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
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
            if let (endpoint, false) = verdict? {
                unreachable.insert(endpoint);
            }
        }
        let mut known = self
            .inner
            .mcp_unreachable
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
        Ok(())
    }

    pub async fn watch_mcp_servers(&self, stop: CancellationToken) {
        loop {
            if let Err(error) = self.probe_mcp_servers().await {
                tracing::warn!("check which MCP servers answer: {error:#}");
            }
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(EVERY) => {}
            }
        }
    }
}
