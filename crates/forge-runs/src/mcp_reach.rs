use artisan_api::{Event, McpServer, ThreadId};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

const PATIENCE: Duration = Duration::from_secs(3);
const FRESH: Duration = Duration::from_secs(60);

pub(crate) type Endpoint = (String, u16);

fn endpoint(server: &McpServer) -> Option<Endpoint> {
    let url = url::Url::parse(server.url.as_deref()?).ok()?;
    Some((url.host_str()?.to_owned(), url.port_or_known_default()?))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    Answers,
    Silent,
}

#[derive(Clone, Copy)]
enum Entry {
    Probing,
    Probed { reach: Reach, at: Instant },
}

impl Entry {
    fn fresh(self, now: Instant) -> Option<Reach> {
        match self {
            Self::Probed { reach, at } if now.duration_since(at) < FRESH => Some(reach),
            Self::Probed { .. } | Self::Probing => None,
        }
    }

    fn due(self, now: Instant) -> bool {
        match self {
            Self::Probing => false,
            Self::Probed { at, .. } => now.duration_since(at) >= FRESH,
        }
    }
}

#[derive(Clone, Copy)]
enum Refresh {
    Stale,
    All,
}

#[derive(Default)]
pub(crate) struct Reachability {
    entries: Mutex<HashMap<Endpoint, Entry>>,
}

async fn reach(endpoint: Endpoint) -> (Endpoint, Reach) {
    let connect = tokio::net::TcpStream::connect((endpoint.0.as_str(), endpoint.1));
    let reach = match tokio::time::timeout(PATIENCE, connect).await {
        Ok(Ok(_)) => Reach::Answers,
        Ok(Err(_)) | Err(_) => Reach::Silent,
    };
    (endpoint, reach)
}

impl Reachability {
    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<Endpoint, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn claim(&self, servers: &[McpServer], refresh: Refresh) -> BTreeSet<Endpoint> {
        let now = Instant::now();
        let mut entries = self.entries();
        let claimed: BTreeSet<Endpoint> = servers
            .iter()
            .filter_map(endpoint)
            .filter(|endpoint| match (refresh, entries.get(endpoint)) {
                (_, None) | (Refresh::All, Some(Entry::Probed { .. })) => true,
                (Refresh::Stale, Some(entry)) => entry.due(now),
                (Refresh::All, Some(Entry::Probing)) => false,
            })
            .collect();
        for endpoint in &claimed {
            entries.insert(endpoint.clone(), Entry::Probing);
        }
        claimed
    }

    async fn probe(&self, endpoints: BTreeSet<Endpoint>) -> HashMap<Endpoint, Reach> {
        let mut probes: tokio::task::JoinSet<(Endpoint, Reach)> =
            endpoints.into_iter().map(reach).collect();
        let mut found = HashMap::new();
        while let Some(probed) = probes.join_next().await {
            let (endpoint, reach) = probed
                .expect("invariant: a reachability probe is never cancelled and never panics");
            self.entries().insert(
                endpoint.clone(),
                Entry::Probed {
                    reach,
                    at: Instant::now(),
                },
            );
            found.insert(endpoint, reach);
        }
        found
    }

    #[cfg(test)]
    pub(crate) fn probed(&self, endpoints: usize) -> bool {
        let entries = self.entries();
        entries.len() == endpoints
            && entries
                .values()
                .all(|entry| matches!(entry, Entry::Probed { .. }))
    }

    pub(crate) fn warm(self: &Arc<Self>, servers: &[McpServer]) {
        self.spawn_probe(servers, Refresh::Stale);
    }

    fn spawn_probe(self: &Arc<Self>, servers: &[McpServer], refresh: Refresh) {
        let claimed = self.claim(servers, refresh);
        if claimed.is_empty() {
            return;
        }
        let reachability = Arc::clone(self);
        drop(tokio::spawn(async move {
            reachability.probe(claimed).await;
        }));
    }

    pub(crate) async fn watch_config(self: Arc<Self>, store: forge_store::Store) {
        let mut changes = store.subscribe();
        loop {
            match changes.recv().await {
                Ok(envelope) => {
                    if let Event::Setting { key, value } = envelope.event
                        && key == "mcp_servers"
                    {
                        match serde_json::from_value::<Vec<McpServer>>(value) {
                            Ok(servers) => self.spawn_probe(&servers, Refresh::All),
                            Err(error) => tracing::warn!(
                                %error,
                                "decode the changed MCP server list to probe it"
                            ),
                        }
                    }
                }
                Err(RecvError::Lagged(_)) => {
                    match crate::mcp_auth::read_mcp_servers(&store).await {
                        Ok(servers) => self.spawn_probe(&servers, Refresh::All),
                        Err(error) => tracing::warn!(
                            error = diagnostics::chain(&error),
                            "read the MCP servers after missing their changes"
                        ),
                    }
                }
                Err(RecvError::Closed) => return,
            }
        }
    }

    pub(crate) async fn answering(
        &self,
        servers: Vec<McpServer>,
        thread: &ThreadId,
    ) -> Vec<McpServer> {
        let now = Instant::now();
        let (mut known, cold): (HashMap<Endpoint, Reach>, BTreeSet<Endpoint>) = {
            let entries = self.entries();
            let mut known = HashMap::new();
            let mut cold = BTreeSet::new();
            for endpoint in servers.iter().filter_map(endpoint) {
                match entries.get(&endpoint).and_then(|entry| entry.fresh(now)) {
                    Some(reach) => {
                        known.insert(endpoint, reach);
                    }
                    None => {
                        cold.insert(endpoint);
                    }
                }
            }
            (known, cold)
        };
        known.extend(self.probe(cold).await);
        servers
            .into_iter()
            .filter(|server| match endpoint(server) {
                Some(address) if known.get(&address) == Some(&Reach::Silent) => {
                    tracing::info!(
                        %thread,
                        server = %server.name,
                        address = %format_args!("{}:{}", address.0, address.1),
                        "MCP server does not answer; the turn starts without it"
                    );
                    false
                }
                Some(_) | None => true,
            })
            .collect()
    }
}
