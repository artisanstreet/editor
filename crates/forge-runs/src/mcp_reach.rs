use artisan_api::{McpServer, ThreadId};
use std::collections::BTreeSet;
use std::time::Duration;

const PATIENCE: Duration = Duration::from_secs(3);

type Endpoint = (String, u16);

fn endpoint(server: &McpServer) -> Option<Endpoint> {
    let url = url::Url::parse(server.url.as_deref()?).ok()?;
    Some((url.host_str()?.to_owned(), url.port_or_known_default()?))
}

async fn answers(endpoint: Endpoint) -> Option<Endpoint> {
    let connect = tokio::net::TcpStream::connect((endpoint.0.as_str(), endpoint.1));
    match tokio::time::timeout(PATIENCE, connect).await {
        Ok(Ok(_)) => None,
        Ok(Err(_)) | Err(_) => Some(endpoint),
    }
}

pub(crate) async fn answering(servers: Vec<McpServer>, thread: &ThreadId) -> Vec<McpServer> {
    let endpoints: BTreeSet<Endpoint> = servers.iter().filter_map(endpoint).collect();
    let mut probes: tokio::task::JoinSet<Option<Endpoint>> =
        endpoints.into_iter().map(answers).collect();
    let mut silent = BTreeSet::new();
    while let Some(probed) = probes.join_next().await {
        silent.extend(
            probed.expect("invariant: a reachability probe is never cancelled and never panics"),
        );
    }
    servers
        .into_iter()
        .filter(|server| match endpoint(server) {
            Some(address) if silent.contains(&address) => {
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
